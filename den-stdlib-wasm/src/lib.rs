pub mod backend;
pub mod engine;
pub mod error;
pub mod exception;
pub mod global;
pub mod instance;
pub mod memory;
pub mod module;
pub mod store;
pub mod table;
pub mod tag;
pub mod utils;

use rquickjs::{
    Coerced, Constructor, Ctx, Exception, Filter, FromJs as _, Function, IntoAtom, IntoJs,
    JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    function::{Async, Opt, This},
    object::{Accessor, AsProperty, Property},
    promise::MaybePromise,
    qjs,
};

/// The one `JSTag` instance, looked up from userdata so the getter captures
/// no JS value the cycle GC cannot trace.
#[derive(JsLifetime)]
struct WasmJsTag<'js>(Value<'js>);

/// `defineProperty` flags rquickjs' `Property` builder cannot express (HAS_*
/// without the corresponding attribute bit), which is how we turn enumerable
/// off on an existing slot.
struct Spec<'js> {
    flags: qjs::c_int,
    value: Value<'js>,
    get:   Value<'js>,
    set:   Value<'js>,
}

impl<'js> AsProperty<'js, ()> for Spec<'js> {
    fn config(self, _ctx: &Ctx<'js>) -> Result<(qjs::c_int, Value<'js>, Value<'js>, Value<'js>)> {
        Ok((self.flags, self.value, self.get, self.set))
    }
}

const fn data_flags(writable: bool, enumerable: bool, configurable: bool) -> qjs::c_int {
    let mut flags = qjs::JS_PROP_HAS_VALUE
        | qjs::JS_PROP_HAS_WRITABLE
        | qjs::JS_PROP_HAS_ENUMERABLE
        | qjs::JS_PROP_HAS_CONFIGURABLE;
    if writable {
        flags |= qjs::JS_PROP_WRITABLE;
    }
    if enumerable {
        flags |= qjs::JS_PROP_ENUMERABLE;
    }
    if configurable {
        flags |= qjs::JS_PROP_CONFIGURABLE;
    }
    flags as qjs::c_int
}

fn spec_data<'js>(
    ctx: &Ctx<'js>, value: impl IntoJs<'js>, writable: bool, enumerable: bool, configurable: bool,
) -> Result<Spec<'js>> {
    let undef = Value::new_undefined(ctx.clone());
    Ok(Spec {
        flags: data_flags(writable, enumerable, configurable),
        value: value.into_js(ctx)?,
        get:   undef.clone(),
        set:   undef,
    })
}

fn try_define<'js>(object: &Object<'js>, key: impl IntoAtom<'js>, spec: Spec<'js>) {
    let _ = object.prop(key, spec);
}

fn is_operation(name: &str) -> bool {
    matches!(
        name,
        "validate" | "compile" | "instantiate" | "compileStreaming" | "instantiateStreaming"
    )
}

fn own_property<'js>(object: &Object<'js>, key: &str) -> Result<Option<Object<'js>>> {
    let ctor: Object = object.ctx().globals().get("Object")?;
    let get: Function = ctor.get("getOwnPropertyDescriptor")?;
    Ok(get.call::<_, Value>((object.clone(), key))?.into_object())
}

fn try_accessor<'js>(
    object: &Object<'js>, key: &str, get: Function<'js>, set: Option<Function<'js>>,
) {
    let ctx = object.ctx();
    let mut flags = qjs::JS_PROP_HAS_GET
        | qjs::JS_PROP_ENUMERABLE
        | qjs::JS_PROP_HAS_ENUMERABLE
        | qjs::JS_PROP_CONFIGURABLE
        | qjs::JS_PROP_HAS_CONFIGURABLE;
    let set = set.map_or_else(
        || Value::new_undefined(ctx.clone()),
        |set| {
            flags |= qjs::JS_PROP_HAS_SET;
            set.into_value()
        },
    );
    try_define(object, key, Spec {
        flags: flags as qjs::c_int,
        value: Value::new_undefined(ctx.clone()),
        get: get.into_value(),
        set,
    });
}

fn try_tag(target: &Object<'_>, value: &str) {
    let _ = target.prop(
        PredefinedAtom::SymbolToStringTag,
        Property::from(value).configurable(),
    );
}

fn lock_prototype(constructor: &Function<'_>) {
    let ctx = constructor.ctx();
    let proto: Value = constructor
        .get(PredefinedAtom::Prototype)
        .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
    if constructor
        .prop(PredefinedAtom::Prototype, Spec {
            flags: data_flags(false, false, false),
            value: proto,
            get:   Value::new_undefined(ctx.clone()),
            set:   Value::new_undefined(ctx.clone()),
        })
        .is_err()
    {
        let _ = constructor.prop(PredefinedAtom::Prototype, Spec {
            flags: qjs::JS_PROP_HAS_WRITABLE as qjs::c_int,
            value: Value::new_undefined(ctx.clone()),
            get:   Value::new_undefined(ctx.clone()),
            set:   Value::new_undefined(ctx.clone()),
        });
    }
}

fn shape_interface<'js>(ctx: &Ctx<'js>, namespace: &Object<'js>, name: &str) -> Result<()> {
    let Ok(constructor) = namespace.get::<_, Function>(name) else {
        return Ok(());
    };
    let proto: Object = constructor.get(PredefinedAtom::Prototype)?;
    let configurable = own_property(&proto, "constructor")?
        .and_then(|desc| desc.get::<_, bool>("configurable").ok())
        .unwrap_or(true);
    if configurable {
        try_define(
            &proto,
            PredefinedAtom::Constructor,
            spec_data(ctx, constructor.clone(), true, false, true)?,
        );
    }
    try_tag(&proto, &format!("WebAssembly.{name}"));
    let keys: Vec<String> = proto
        .own_keys(Filter::new().string())
        .collect::<Result<_>>()?;
    for key in keys {
        if key == "constructor" {
            continue;
        }
        let Some(desc) = own_property(&proto, &key)? else {
            continue;
        };
        let configurable = desc.get::<_, bool>("configurable").unwrap_or(false);
        if let Ok(value) = desc.get::<_, Function>("value") {
            let _ = value.set_name(&key);
            if configurable {
                try_define(
                    &proto,
                    key.as_str(),
                    spec_data(ctx, value, true, true, true)?,
                );
            }
        } else if let Ok(get) = desc.get::<_, Function>("get") {
            let _ = get.set_name(format!("get {key}"));
            let _ = get.set_length(0);
            let set = desc.get::<_, Function>("set").ok().inspect(|set| {
                let _ = set.set_name(format!("set {key}"));
                let _ = set.set_length(1);
            });
            if configurable {
                try_accessor(&proto, &key, get, set);
            }
        }
    }
    lock_prototype(&constructor);
    Ok(())
}

/// WebIDL property attributes on the namespace and its interfaces.
///
/// A namespace member and an interface object are "the property attributes {
/// [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: true }"
/// (WebIDL § *Namespaces* and § *Interfaces*), and every namespace object and
/// interface prototype carries an `@@toStringTag` naming it, so
/// `String(WebAssembly)` is `[object WebAssembly]` rather than `[object
/// Object]`. `JSTag` is the one namespace member that is a *value* rather than
/// a function or a class: the tag `(param externref)` that a JS exception
/// crossing into wasm carries. WebIDL exposes a namespace's `readonly
/// attribute` as an accessor, so a getter it is.
///
/// Every `defineProperty` is fallible: rquickjs class prototypes lock some
/// slots, and a throw here would keep `den:wasm` from evaluating at all (every
/// spec_core file shares that evaluate hook).
fn shape_namespace<'js>(
    ctx: &Ctx<'js>, namespace: &Object<'js>, interfaces: &[&str], supports_tags: bool,
) -> Result<()> {
    let names: Vec<String> = namespace
        .own_keys(Filter::new().string())
        .collect::<Result<_>>()?;
    for name in names {
        let value: Value = namespace.get(name.as_str())?;
        let enumerable = is_operation(&name);
        if enumerable && let Some(func) = value.as_function() {
            let _ = func.set_name(&name);
        }
        try_define(
            namespace,
            name.as_str(),
            spec_data(ctx, value, true, enumerable, true)?,
        );
    }
    for (name, length) in [("validate", 1_usize), ("compile", 1), ("instantiate", 1)] {
        if let Ok(func) = namespace.get::<_, Function>(name) {
            let _ = func.set_length(length);
        }
    }

    for name in interfaces
        .iter()
        .copied()
        .chain(["CompileError", "LinkError", "RuntimeError"])
    {
        shape_interface(ctx, namespace, name)?;
    }

    if let Ok(module) = namespace.get::<_, Object>("Module") {
        for (name, length) in [("exports", 1_usize), ("imports", 1), ("customSections", 2)] {
            let Ok(func) = module.get::<_, Function>(name) else {
                continue;
            };
            let _ = func.set_name(name);
            let _ = func.set_length(length);
            try_define(&module, name, spec_data(ctx, func, true, true, true)?);
        }
    }

    try_tag(namespace, "WebAssembly");
    try_define(
        &ctx.globals(),
        "WebAssembly",
        spec_data(ctx, namespace.clone(), true, false, true)?,
    );
    if supports_tags {
        let tag_ctor: Constructor = namespace.get("Tag")?;
        let descriptor = Object::new(ctx.clone())?;
        descriptor.set("parameters", vec!["externref"])?;
        let js_tag: Value = tag_ctor.construct((descriptor,))?;
        ctx.store_userdata(WasmJsTag(js_tag)).map_err(|_error| {
            Exception::throw_internal(ctx, "WebAssembly.JSTag is already in use")
        })?;
        let _ = namespace.prop(
            "JSTag",
            Accessor::from(|ctx: Ctx<'js>| -> Result<Value<'js>> {
                ctx.userdata::<WasmJsTag>()
                    .map(|tag| tag.0.clone())
                    .ok_or_else(|| Exception::throw_internal(&ctx, "WebAssembly.JSTag is missing"))
            })
            .configurable(),
        );
    }
    Ok(())
}

/// Await a duck-typed `Response` (ok, headers.get, arrayBuffer, status) and
/// yield its body. Independent of den-stdlib-whatwg-fetch.
async fn wasm_bytes<'js>(ctx: Ctx<'js>, source: Value<'js>) -> Result<Value<'js>> {
    let response = MaybePromise::from_value(source)
        .into_future::<Value<'js>>()
        .await?;
    let response = response
        .into_object()
        .ok_or_else(|| Exception::throw_type(&ctx, "expected a Response"))?;
    let headers: Object = response.get("headers")?;
    let get: Function = headers.get("get")?;
    let content_type: Value = get.call((This(headers), "Content-Type"))?;
    let content_type = if content_type.is_null() || content_type.is_undefined() {
        String::new()
    } else {
        Coerced::<String>::from_js(&ctx, content_type)?.0
    };
    let content_type = content_type.trim().to_lowercase();
    if content_type != "application/wasm" {
        return Err(Exception::throw_type(
            &ctx,
            &format!("expected a Content-Type of application/wasm, got \"{content_type}\""),
        ));
    }
    if !Coerced::<bool>::from_js(&ctx, response.get("ok")?)?.0 {
        let status = Coerced::<String>::from_js(&ctx, response.get("status")?)?;
        return Err(Exception::throw_type(
            &ctx,
            &format!("the response status {} is not an ok status", status.0),
        ));
    }
    let array_buffer: Function = response.get("arrayBuffer")?;
    let produced: Value = array_buffer.call((This(response),))?;
    MaybePromise::from_value(produced).into_future().await
}

fn wasm_namespace<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> { ctx.globals().get("WebAssembly") }

/// `compileStreaming` / `instantiateStreaming` as async functions on the
/// namespace: await a Response, check Content-Type / ok, then
/// `namespace.compile` / `namespace.instantiate`. Looked up on the global at
/// call time so the closures hold no JS values the GC cannot trace.
fn install_streaming<'js>(ctx: &Ctx<'js>, namespace: &Object<'js>) -> Result<()> {
    let compile_streaming = Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, source: Opt<Value<'js>>| {
            async move {
                let bytes = wasm_bytes(
                    ctx.clone(),
                    source
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                )
                .await?;
                wasm_namespace(&ctx)?
                    .get::<_, Function>("compile")?
                    .call::<_, Value>((bytes,))
            }
        }),
    )?
    .with_name("compileStreaming")?
    .with_length(1)?;
    let instantiate_streaming = Function::new(
        ctx.clone(),
        Async(
            move |ctx: Ctx<'js>, source: Opt<Value<'js>>, import_object: Opt<Value<'js>>| {
                async move {
                    let bytes = wasm_bytes(
                        ctx.clone(),
                        source
                            .0
                            .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                    )
                    .await?;
                    wasm_namespace(&ctx)?
                        .get::<_, Function>("instantiate")?
                        .call::<_, Value>((
                            bytes,
                            import_object
                                .0
                                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                        ))
                }
            },
        ),
    )?
    .with_name("instantiateStreaming")?
    .with_length(2)?;
    namespace.prop(
        "compileStreaming",
        Property::from(compile_streaming).writable().configurable(),
    )?;
    namespace.prop(
        "instantiateStreaming",
        Property::from(instantiate_streaming)
            .writable()
            .configurable(),
    )?;
    Ok(())
}

#[rquickjs::module]
pub mod wasm {
    use den_util::{BufferSource, Probe};
    use rquickjs::{
        Class, Ctx, Exception, IntoJs, Object, Promise, Result, TypedArray, Value, module::Exports,
        prelude::Opt, promise::Promised,
    };

    use crate::{
        backend,
        engine::Engine,
        error::WebAssemblyErrors,
        instance::ImportedFunctions,
        memory::MemoryBuffers,
        store::{Store, WasiImports},
    };
    pub use crate::{
        exception::Exception as WasmException, global::Global, instance::Instance, memory::Memory,
        module::Module, table::Table, tag::Tag,
    };

    /// Decode and validate, never throwing for a bad module — only for an
    /// argument that is not a `BufferSource`, which the conversion above
    /// already rejected.
    #[rquickjs::function]
    pub fn validate(bytes: BufferSource, ctx: Ctx<'_>) -> Result<bool> {
        Ok(backend::compile_module(&Engine::from_ctx(&ctx)?, bytes.bytes()).is_ok())
    }

    /// Promise-returning: argument conversion and the byte copy are
    /// synchronous, a decode failure *rejects* with `CompileError` rather than
    /// throwing. Missing or invalid arguments reject with `TypeError` — they
    /// must not throw, because `promise_rejects_js` only accepts a thenable.
    #[rquickjs::function]
    pub fn compile<'js>(bytes: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let Some(bytes) = bytes.0 else {
            return reject_type(&ctx, "expected a BufferSource");
        };
        match <BufferSource as rquickjs::FromJs>::from_js(&ctx, bytes) {
            Ok(source) => {
                let bytes = source.into_bytes();
                let compile_ctx = ctx.clone();
                Promised(async move { Module::compile(&compile_ctx, bytes) }).into_js(&ctx)
            }
            Err(_) => reject_pending(&ctx),
        }
    }

    /// Both overloads: a `Module` instantiates *now* and the promise resolves
    /// with a bare `Instance` (import getters have already run); a
    /// `BufferSource` is copied now and compiled/instantiated later, so import
    /// getters run after the promise is returned.
    #[rquickjs::function]
    pub fn instantiate<'js>(
        source: Opt<Value<'js>>, import_object: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        let Some(source) = source.0 else {
            return reject_type(&ctx, "expected a Module or a BufferSource");
        };
        // A `Module` argument and a `BufferSource` argument are told apart by
        // probing, so the probe must not leave its `TypeError` pending.
        if let Some(module) =
            ctx.probe(|| source.as_object().and_then(Class::<Module>::from_object))
        {
            let module = match module.try_borrow().map(|module| Module::clone(&module)) {
                Ok(module) => module,
                Err(_) => return reject_type(&ctx, "the WebAssembly.Module is already in use"),
            };
            return match Instance::instantiate(&ctx, &module, import_object.0) {
                Ok(instance) => {
                    resolve_value(&ctx, Class::instance(ctx.clone(), instance)?.into_value())
                }
                Err(_) => reject_pending(&ctx),
            };
        }

        match <BufferSource as rquickjs::FromJs>::from_js(&ctx, source) {
            Ok(bytes) => {
                let bytes = bytes.into_bytes();
                let imports = import_object.0;
                let instantiate_ctx = ctx.clone();
                Promised(async move {
                    let module = Module::compile(&instantiate_ctx, bytes)?;
                    let instance = Instance::instantiate(&instantiate_ctx, &module, imports)?;
                    indexmap::indexmap! {
                        "module" => Class::instance(instantiate_ctx.clone(), module)?.into_js(&instantiate_ctx)?,
                        "instance" => Class::instance(instantiate_ctx.clone(), instance)?.into_js(&instantiate_ctx)?,
                    }
                    .into_js(&instantiate_ctx)
                })
                .into_js(&ctx)
            }
            Err(_) => reject_pending(&ctx),
        }
    }

    fn reject_type<'js>(ctx: &Ctx<'js>, message: &str) -> Result<Value<'js>> {
        let _ = Exception::throw_type(ctx, message);
        reject_pending(ctx)
    }

    fn reject_pending<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let error = if ctx.has_exception() {
            ctx.catch()
        } else {
            return Err(Exception::throw_internal(
                ctx,
                "rejected a WebAssembly promise with no pending exception",
            ));
        };
        let (promise, _resolve, reject) = Promise::new(ctx)?;
        reject.call::<_, ()>((error,))?;
        Ok(promise.into_value())
    }

    fn resolve_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
        let (promise, resolve, _reject) = Promise::new(ctx)?;
        resolve.call::<_, ()>((value,))?;
        Ok(promise.into_value())
    }

    /// den extension: the WASI preview1 import namespace, for the caller who
    /// wants it.
    ///
    /// ```js
    /// import { wasiImports } from "den:wasm";
    /// await WebAssembly.instantiate(bytes, { wasi_snapshot_preview1: wasiImports() });
    /// ```
    ///
    /// A module-level export rather than a `WebAssembly` member on purpose:
    /// `WebAssembly` is the spec's namespace and gains nothing of den's, and
    /// spelling the namespace out at the call site is what makes granting the
    /// host's stdio and environment a decision somebody took. See
    /// [`WasiImports`] for what the returned object is, and for the one hook in
    /// `Instance::read_imports` that still has to honour it.
    #[rquickjs::function(rename = "wasiImports")]
    #[qjs(rename = "wasiImports")]
    pub fn wasi_imports<'js>(ctx: Ctx<'js>) -> Result<Value<'js>> { WasiImports::namespace(&ctx) }

    /// den extension: assemble WebAssembly Text, so tests and scripts need no
    /// checked-in binaries.
    #[rquickjs::function]
    pub fn wat2wasm(source: String, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        match wat::parse_str(&source) {
            // `new_copy`, never `new`: `new` lends QuickJS a Rust `Vec` plus a
            // free hook that QuickJS calls twice on detach (quickjs.c:58037
            // and :57935), and `transfer` reallocs a pointer its allocator
            // never produced — `wat2wasm("(module)").buffer.transfer(4)` alone
            // aborted the process. One copy of a wat blob buys a buffer script
            // can detach and transfer like any other.
            Ok(bytes) => TypedArray::new_copy(ctx.clone(), bytes),
            Err(err) => {
                Err(Exception::throw_type(
                    &ctx,
                    &format!("wat2wasm error: {err}"),
                ))
            }
        }
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let engine = Engine::new().map_err(|err| {
            Exception::throw_internal(ctx, &format!("cannot create a WebAssembly engine: {err}"))
        })?;
        let in_use = |what: &'static str| {
            Exception::throw_internal(ctx, &format!("the WebAssembly {what} is already in use"))
        };
        ctx.store_userdata(engine).map_err(|_| in_use("engine"))?;
        let store = Store::new(&Engine::from_ctx(ctx)?, ctx);
        ctx.store_userdata(store).map_err(|_| in_use("store"))?;
        ctx.store_userdata(ImportedFunctions::default())
            .map_err(|_| in_use("import registry"))?;
        ctx.store_userdata(MemoryBuffers::default())
            .map_err(|_| in_use("memory buffer registry"))?;

        let namespace = Object::new(ctx.clone())?;
        namespace.set("validate", js_validate)?;
        namespace.set("compile", js_compile)?;
        namespace.set("instantiate", js_instantiate)?;

        let interfaces = [
            ("Module", Class::<Module>::create_constructor(ctx)?),
            ("Instance", Class::<Instance>::create_constructor(ctx)?),
            ("Memory", Class::<Memory>::create_constructor(ctx)?),
            ("Table", Class::<Table>::create_constructor(ctx)?),
            ("Global", Class::<Global>::create_constructor(ctx)?),
            ("Tag", Class::<Tag>::create_constructor(ctx)?),
            (
                "Exception",
                Class::<WasmException>::create_constructor(ctx)?,
            ),
        ];
        let interface_names = Vec::from_iter(interfaces.iter().map(|(name, _)| *name));
        for (name, constructor) in interfaces {
            let constructor = constructor.ok_or_else(|| {
                Exception::throw_internal(ctx, &format!("WebAssembly.{name} has no constructor"))
            })?;
            namespace.set(name, constructor)?;
        }

        WebAssemblyErrors::install(ctx, &namespace)?;
        crate::install_streaming(ctx, &namespace)?;
        ctx.globals().set("WebAssembly", namespace.clone())?;
        // After the global is installed, so shape can correct the property
        // attributes of `globalThis.WebAssembly` itself.
        crate::shape_namespace(ctx, &namespace, &interface_names, backend::SUPPORTS_TAGS)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::FromJs;

    const ADD: &str = r#"
        (module
          (@custom "hello" "world")
          (func (export "nothing"))
          (func (export "one") (result i32) i32.const 7)
          (func (export "pair") (result i32 i32) i32.const 1 i32.const 2)
          (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))
    "#;

    const CALLS_IMPORT: &str = r#"
        (module
          (import "env" "log" (func $log (param i32 i32) (result i32)))
          (func (export "run") (param i32 i32) (result i32)
            local.get 0 local.get 1 call $log))
    "#;

    const NEEDS_TWO_PAGES: &str = r#"(module (import "env" "mem" (memory 2)))"#;

    /// A module that grows its own memory from *inside* wasm, where there is no
    /// JS call for den to hook.
    const GROWS_ITS_OWN_MEMORY: &str = r#"
        (module
          (memory (export "mem") 1)
          (func (export "boom") unreachable)
          (func (export "grow") (drop (memory.grow (i32.const 1)))))
    "#;

    /// Re-exports the memory it imports, so one linear memory ends up behind
    /// two `WebAssembly.Memory` wrappers.
    const REEXPORTS_ITS_MEMORY: &str = r#"
        (module
          (import "env" "mem" (memory 1))
          (export "mem" (memory 0)))
    "#;

    /// Traps from its `start` function, which is neither a link failure nor a
    /// call the caller made.
    const TRAPS_ON_START: &str = r#"(module (func $boom unreachable) (start $boom))"#;

    const WANTS_AN_I64_GLOBAL: &str = r#"(module (import "env" "g" (global i64)))"#;
    const WANTS_AN_I32_GLOBAL: &str = r#"(module (import "env" "g" (global i32)))"#;

    /// den links nothing implicitly, WASI included.
    const WANTS_WASI: &str = r#"
        (module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32))))
    "#;

    /// A module that asks WASI for something only the engine can answer: the
    /// environment count is written into the *caller's* linear memory. The
    /// result is the errno, `0` for success.
    const CALLS_WASI: &str = r#"
        (module
          (import "wasi_snapshot_preview1" "environ_sizes_get"
                  (func $sizes (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "run") (result i32) (call $sizes (i32.const 0) (i32.const 8))))
    "#;

    const EXPORTS_A_TAG: &str = r#"(module (tag (export "t") (param i32)))"#;

    /// Imports and exports whose declaration order is neither alphabetical nor
    /// grouped by kind, so a sorted or kind-grouped listing would fail.
    const DECLARATION_ORDER: &str = r#"
        (module
          (import "env" "zebra" (func $zebra))
          (import "env" "apple" (global i32))
          (import "env" "middle" (memory 1))
          (import "env" "banana" (table 1 funcref))
          (export "zebra" (func $zebra))
          (export "apple" (memory 0))
          (export "middle" (table 0)))
    "#;

    /// Fixtures are assembled at test time; no `.wasm` is checked in.
    fn wat(source: &str) -> Vec<u8> {
        wat::parse_str(source).expect("the fixture is valid WebAssembly Text")
    }

    /// Evaluate `source` in a fresh context with `den:wasm` installed.
    ///
    /// One runtime per call: `den:wasm` keeps a single store in the context
    /// userdata, so tests must not share a context. `bytes` shows up as the
    /// `Uint8Array` named `WASM`, and the snippet may use top-level `await`.
    async fn eval<T>(bytes: Option<Vec<u8>>, source: &str) -> Result<T, String>
    where
        T: for<'js> FromJs<'js> + Send + Sync + 'static,
    {
        let prelude = match bytes {
            Some(bytes) => {
                format!(
                    "const denWasm = await import('den:wasm');\nconst WASM = new \
                     Uint8Array([{}]);\n",
                    bytes
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            None => "const denWasm = await import('den:wasm');\n".into(),
        };
        den_core::engine::Engine::new()
            .await
            .eval(&format!("{prelude}{source}"))
            .await
            .map_err(|error| error.to_string())
    }

    #[tokio::test]
    async fn the_namespace_exposes_every_spec_member() {
        let members: String = eval(
            None,
            r#"
              ["validate", "compile", "instantiate", "compileStreaming", "instantiateStreaming",
               "Module", "Instance", "Memory", "Table", "Global", "Tag", "Exception",
               "CompileError", "LinkError", "RuntimeError"]
                .filter((name) => typeof WebAssembly[name] !== "function").join(",")
            "#,
        )
        .await
        .expect("the namespace evaluates");
        assert_eq!(members, "", "these members are missing or not callable");
    }

    #[tokio::test]
    async fn module_and_instance_are_real_constructors() {
        let shape: String = eval(
            Some(wat(ADD)),
            r#"
              const module = new WebAssembly.Module(WASM);
              const instance = new WebAssembly.Instance(module);
              [module instanceof WebAssembly.Module,
               instance instanceof WebAssembly.Instance,
               instance.exports.add(2, 2) === 4].join(",")
            "#,
        )
        .await
        .expect("the constructors are callable with `new`");
        assert_eq!(shape, "true,true,true");
    }

    #[tokio::test]
    async fn validate_answers_true_for_a_module_and_false_for_garbage() {
        let answers: String = eval(
            Some(wat(ADD)),
            r#"[WebAssembly.validate(WASM), WebAssembly.validate(new Uint8Array([1, 2, 3]))].join(",")"#,
        )
        .await
        .expect("validate never throws for a bad module");
        assert_eq!(answers, "true,false");
    }

    #[tokio::test]
    async fn compile_rejects_garbage_with_a_compile_error() {
        let name: String = eval(
            None,
            r#"
              let name = "no rejection";
              try { await WebAssembly.compile(new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9])); }
              catch (error) { name = error.name; }
              name
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(name, "CompileError");
    }

    #[tokio::test]
    async fn instantiate_has_a_buffer_overload_and_a_module_overload() {
        let shapes: String = eval(
            Some(wat(ADD)),
            r#"
              const source = await WebAssembly.instantiate(WASM);
              const compiled = await WebAssembly.compile(WASM);
              const instance = await WebAssembly.instantiate(compiled);
              [source.module instanceof WebAssembly.Module,
               source.instance instanceof WebAssembly.Instance,
               Object.getPrototypeOf(source) === Object.prototype,
               instance instanceof WebAssembly.Instance,
               instance.exports.add(20, 22) === 42].join(",")
            "#,
        )
        .await
        .expect("both overloads resolve");
        assert_eq!(shapes, "true,true,true,true,true");
    }

    #[tokio::test]
    async fn the_exports_object_is_frozen_null_prototype_and_computed_once() {
        let shape: String = eval(
            Some(wat(ADD)),
            r#"
              const { instance } = await WebAssembly.instantiate(WASM);
              [Object.isFrozen(instance.exports),
               Object.getPrototypeOf(instance.exports) === null,
               instance.exports === instance.exports,
               instance.exports.add === instance.exports.add].join(",")
            "#,
        )
        .await
        .expect("the exports object is reachable");
        assert_eq!(shape, "true,true,true,true");
    }

    #[tokio::test]
    async fn an_exported_function_adapts_its_arguments_and_results() {
        let behaviour: String = eval(
            Some(wat(ADD)),
            r#"
              const { nothing, one, pair, add } = (await WebAssembly.instantiate(WASM)).instance.exports;
              [nothing() === undefined,
               one() === 7,
               Array.isArray(pair()),
               pair().join("-") === "1-2",
               add(20, 22) === 42,
               add(20) === 20,
               add(1, 2, 3) === 3,
               add.length === 2,
               add.name === "3"].join(",")
            "#,
        )
        .await
        .expect("the exports are callable");
        assert_eq!(behaviour, "true,true,true,true,true,true,true,true,true");
    }

    #[tokio::test]
    async fn an_imported_js_function_sees_the_wasm_arguments() {
        let outcome: String = eval(
            Some(wat(CALLS_IMPORT)),
            r#"
              let seen = null;
              const { instance } = await WebAssembly.instantiate(WASM, {
                env: { log: (left, right) => { seen = [left, right]; return left + right; } },
              });
              `${instance.exports.run(2, 3)}|${seen.join(",")}`
            "#,
        )
        .await
        .expect("the import is callable");
        assert_eq!(outcome, "5|2,3");
    }

    #[tokio::test]
    async fn a_missing_import_namespace_is_a_type_error() {
        let name: String = eval(
            Some(wat(CALLS_IMPORT)),
            r#"
              let name = "no rejection";
              try { await WebAssembly.instantiate(WASM, {}); } catch (error) { name = error.name; }
              name
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(name, "TypeError");
    }

    #[tokio::test]
    async fn an_imported_memory_that_is_too_small_is_a_link_error() {
        let name: String = eval(
            Some(wat(NEEDS_TWO_PAGES)),
            r#"
              let name = "no rejection";
              try {
                await WebAssembly.instantiate(WASM, {
                  env: { mem: new WebAssembly.Memory({ initial: 1 }) },
                });
              } catch (error) { name = error.name; }
              name
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(name, "LinkError");
    }

    #[tokio::test]
    async fn custom_sections_returns_the_payload_of_each_matching_section() {
        let payloads: String = eval(
            Some(wat(ADD)),
            r#"
              const module = await WebAssembly.compile(WASM);
              const found = WebAssembly.Module.customSections(module, "hello");
              [found.length,
               found[0] instanceof ArrayBuffer,
               String.fromCharCode(...new Uint8Array(found[0])),
               WebAssembly.Module.customSections(module, "absent").length].join(",")
            "#,
        )
        .await
        .expect("customSections is implemented");
        assert_eq!(payloads, "1,true,world,0");
    }

    #[tokio::test]
    async fn module_imports_and_exports_describe_every_entry() {
        let descriptors: String = eval(
            Some(wat(CALLS_IMPORT)),
            r#"
              const module = await WebAssembly.compile(WASM);
              JSON.stringify([WebAssembly.Module.imports(module), WebAssembly.Module.exports(module)])
            "#,
        )
        .await
        .expect("the statics are implemented");
        assert_eq!(
            descriptors,
            r#"[[{"module":"env","name":"log","kind":"function"}],[{"name":"run","kind":"function"}]]"#
        );
    }

    #[tokio::test]
    async fn instantiate_streaming_accepts_a_promise_of_a_duck_typed_response() {
        let outcome: String = eval(
            Some(wat(ADD)),
            r#"
              const response = (contentType) => ({
                ok: true,
                status: 200,
                headers: { get: (name) => name.toLowerCase() === "content-type" ? contentType : null },
                arrayBuffer: async () => WASM.buffer,
              });
              const { instance } = await WebAssembly.instantiateStreaming(
                Promise.resolve(response("application/wasm")));
              let rejected = "no rejection";
              try { await WebAssembly.compileStreaming(response("text/html")); }
              catch (error) { rejected = error.name; }
              `${instance.exports.add(1, 2)}|${rejected}`
            "#,
        )
        .await
        .expect("streaming is implemented");
        assert_eq!(outcome, "3|TypeError");
    }

    /// A trap must arrive as `RuntimeError`, and it must do so after an
    /// *ordinary* `new WebAssembly.Module(uint8Array)`, whose `ArrayBuffer`
    /// probe used to leave a `TypeError` pending on the context forever.
    ///
    /// Before the fix this reported that stale `TypeError`; with nothing
    /// pending it re-threw `JS_UNINITIALIZED` and reading `.name` off the
    /// result segfaulted the process.
    #[tokio::test]
    async fn a_trap_is_a_runtime_error_even_after_a_typed_array_module() {
        let thrown: String = eval(
            Some(wat(GROWS_ITS_OWN_MEMORY)),
            r#"
              const { boom } = new WebAssembly.Instance(new WebAssembly.Module(WASM)).exports;
              let thrown = "nothing thrown";
              try { boom(); } catch (error) { thrown = `${error.name}/${error instanceof Error}`; }
              thrown
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(thrown, "RuntimeError/true");
    }

    /// The other half of the same guard: an imported function that throws must
    /// still reach the caller as *its own* error object, not as a
    /// `RuntimeError` describing the trap that unwound it.
    #[tokio::test]
    async fn an_error_thrown_by_an_import_reaches_the_caller_unchanged() {
        let thrown: String = eval(
            Some(wat(CALLS_IMPORT)),
            r#"
              const sentinel = new RangeError("from the import");
              const { instance } = await WebAssembly.instantiate(WASM, {
                env: { log: () => { throw sentinel; } },
              });
              let thrown = "nothing thrown";
              try { instance.exports.run(1, 2); } catch (error) { thrown = error === sentinel ? "same" : error.name; }
              thrown
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(thrown, "same");
    }

    /// `memory.grow` executed as a wasm instruction moves the linear memory,
    /// and the buffer JS is holding has to be detached on the way back out.
    #[tokio::test]
    async fn a_grow_inside_wasm_detaches_the_buffer_js_is_holding() {
        let outcome: String = eval(
            Some(wat(GROWS_ITS_OWN_MEMORY)),
            r#"
              const { mem, grow } = (await WebAssembly.instantiate(WASM)).instance.exports;
              const stale = mem.buffer;
              const view = new Uint8Array(stale);
              grow();
              [stale.byteLength === 0,
               view.length === 0,
               mem.buffer !== stale,
               mem.buffer.byteLength === 131072].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "true,true,true,true");
    }

    /// One linear memory behind two wrappers is still one `[[BufferObject]]`:
    /// growing through either wrapper has to detach the buffer the other handed
    /// out, or that buffer keeps aliasing freed pages.
    #[tokio::test]
    async fn two_wrappers_over_one_memory_share_and_detach_the_same_buffer() {
        let outcome: String = eval(
            Some(wat(REEXPORTS_ITS_MEMORY)),
            r#"
              const imported = new WebAssembly.Memory({ initial: 1, maximum: 4 });
              const { mem } = (await WebAssembly.instantiate(WASM, { env: { mem: imported } }))
                .instance.exports;
              const stale = mem.buffer;
              imported.grow(1);
              [mem !== imported,
               stale === imported.buffer || stale.byteLength === 0,
               stale.byteLength === 0,
               mem.buffer.byteLength === 131072,
               mem.buffer === imported.buffer].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "true,true,true,true,true");
    }

    /// A `valueOf` hook passed to `grow` re-enters instantiation with the very
    /// memory being grown. That used to reach `Class::borrow` on an already
    /// borrowed cell and panic Rust from pure JS input.
    #[tokio::test]
    async fn a_value_of_hook_may_re_enter_instantiation_with_the_memory_it_grows() {
        let outcome: String = eval(
            Some(wat(REEXPORTS_ITS_MEMORY)),
            r#"
              const memory = new WebAssembly.Memory({ initial: 1, maximum: 4 });
              const module = new WebAssembly.Module(WASM);
              let outcome = "not reached";
              try {
                memory.grow({
                  valueOf: () => {
                    new WebAssembly.Instance(module, { env: { mem: memory } });
                    return 1;
                  },
                });
                outcome = `grew/${memory.buffer.byteLength}`;
              } catch (error) {
                outcome = `${error.name}: ${error.message}`;
              }
              outcome
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "grew/131072");
    }

    /// "Instantiate a WebAssembly module" splits instantiation failures in two:
    /// an import that cannot be linked is a `LinkError`, but by the time the
    /// `start` function runs, real wasm code is executing and its trap is a
    /// `RuntimeError`.
    #[tokio::test]
    async fn a_trap_in_the_start_function_is_a_runtime_error_not_a_link_error() {
        let name: String = eval(
            Some(wat(TRAPS_ON_START)),
            r#"
              let name = "no rejection";
              try { new WebAssembly.Instance(new WebAssembly.Module(WASM)); }
              catch (error) { name = error.name; }
              name
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(name, "RuntimeError");
    }

    /// The read-the-imports steps reject a Number for an `i64` global and a
    /// BigInt for anything else as a *link* failure, before the value coercion
    /// that would otherwise report it as a `TypeError`.
    #[tokio::test]
    async fn a_global_import_of_the_wrong_numeric_flavour_is_a_link_error() {
        for (fixture, value) in [(WANTS_AN_I64_GLOBAL, "1"), (WANTS_AN_I32_GLOBAL, "1n")] {
            let name: String = eval(
                Some(wat(fixture)),
                &format!(
                    r#"
                      let name = "no rejection";
                      try {{ new WebAssembly.Instance(new WebAssembly.Module(WASM),
                                                      {{ env: {{ g: {value} }} }}); }}
                      catch (error) {{ name = error.name; }}
                      name
                    "#
                ),
            )
            .await
            .expect("the snippet evaluates");
            assert_eq!(name, "LinkError", "{fixture} given {value}");
        }
    }

    /// Limits are matched against the memory's *current* size, so a memory
    /// grown to two pages satisfies an import that its descriptor alone would
    /// not.
    #[tokio::test]
    async fn a_memory_grown_to_the_declared_minimum_satisfies_the_import() {
        let outcome: String = eval(
            Some(wat(NEEDS_TWO_PAGES)),
            r#"
              const mem = new WebAssembly.Memory({ initial: 1, maximum: 4 });
              mem.grow(1);
              let outcome = "not reached";
              try {
                await WebAssembly.instantiate(WASM, { env: { mem } });
                outcome = "instantiated";
              } catch (error) { outcome = error.name; }
              outcome
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "instantiated");
    }

    /// An unsatisfied import is a `TypeError`, `wasi_snapshot_preview1`
    /// included: den hands out no host stdio or environment to a module that
    /// merely asked for the namespace.
    #[tokio::test]
    async fn a_wasi_import_is_not_linked_behind_the_callers_back() {
        let names: String = eval(
            Some(wat(WANTS_WASI)),
            r#"
              const module = new WebAssembly.Module(WASM);
              const attempt = (importObject) => {
                try { new WebAssembly.Instance(module, importObject); return "instantiated"; }
                catch (error) { return error.name; }
              };
              [attempt(undefined), attempt(5)].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(names, "TypeError,TypeError");
    }

    /// The opt-in half of the same rule. `wasiImports()` is a `den:wasm`
    /// export, never a `WebAssembly` member: the spec namespace is exactly what
    /// the spec says it is, and asking for WASI is a thing a script has to
    /// spell out.
    #[tokio::test]
    async fn wasi_imports_is_a_den_wasm_export_rather_than_a_webassembly_member() {
        let outcome: String = eval(
            None,
            r#"
              let asked;
              try { asked = typeof denWasm.wasiImports(); }
              catch (error) { asked = `${error.name}: ${error.message}`; }
              [("wasiImports" in WebAssembly), asked].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "false,object");
    }

    /// The den-extension half of the same rule: `wat2wasm` is a `den:wasm`
    /// export, never a `WebAssembly` member.
    #[tokio::test]
    async fn wat2wasm_is_a_den_wasm_export_rather_than_a_webassembly_member() {
        let outcome: String = eval(
            None,
            r#"
              [("wat2wasm" in WebAssembly), typeof denWasm.wat2wasm].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "false,function");
    }

    /// The whole opt-in path, end to end through JS: the marker stands in for
    /// the `wasi_snapshot_preview1` namespace, `read_imports` recognises it,
    /// and the module then calls a preview1 function that writes the caller's
    /// own linear memory. Without the hook in `read_imports` the marker is just
    /// an object with no `environ_sizes_get` on it and this is a `LinkError`.
    #[tokio::test]
    async fn wasi_imports_satisfies_the_preview1_namespace_of_a_module_that_asks_for_it() {
        let outcome: String = eval(
            Some(wat(CALLS_WASI)),
            r#"
              let outcome = "not reached";
              try {
                const { instance } = await WebAssembly.instantiate(WASM, {
                  wasi_snapshot_preview1: denWasm.wasiImports(),
                });
                outcome = `errno ${instance.exports.run()}`;
              } catch (error) { outcome = `${error.name}: ${error.message}`; }
              outcome
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "errno 0");
    }

    /// A tag export is a `WebAssembly.Tag` on the exports object, not a hole
    /// that `WebAssembly.Module.exports` lists and `Object.keys` does not.
    #[tokio::test]
    async fn a_tag_export_reaches_the_exports_object() {
        let outcome: String = eval(
            Some(wat(EXPORTS_A_TAG)),
            r#"
              try {
                const { instance } = await WebAssembly.instantiate(WASM);
                const tag = instance.exports.t;
                `${tag instanceof WebAssembly.Tag},${tag.type().parameters.join()}`;
              } catch (error) { error.name; }
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "true,i32");
    }

    /// WebIDL: a namespace member and an interface object are writable,
    /// non-enumerable and configurable, the namespace object and every
    /// interface prototype carry an `@@toStringTag`, and `JSTag` is a readonly
    /// attribute.
    #[tokio::test]
    async fn the_namespace_has_the_property_shape_webidl_requires() {
        let shape: String = eval(
            None,
            r#"
              const operations = new Set([
                "validate", "compile", "instantiate", "compileStreaming", "instantiateStreaming",
              ]);
              const wrong = Object.getOwnPropertyNames(WebAssembly).filter((name) => {
                const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, name);
                // `JSTag` is a readonly attribute, hence an accessor with no `writable`.
                if (name === "JSTag") {
                  return descriptor.enumerable || !descriptor.configurable || descriptor.set;
                }
                const shouldEnumerate = operations.has(name);
                return descriptor.enumerable !== shouldEnumerate || !descriptor.configurable
                  || ("value" in descriptor && !descriptor.writable);
              });
              const leaked = Object.keys(WebAssembly).filter((name) => !operations.has(name));
              [wrong.join("|"),
               leaked.join("|"),
               String(WebAssembly),
               Object.prototype.toString.call(new WebAssembly.Memory({ initial: 1 }))].join(";")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        // The `"0"` that older quickjs builds leaked onto the namespace is
        // gone; WebIDL wants nothing enumerable beyond the operations.
        assert_eq!(shape, ";;[object WebAssembly];[object WebAssembly.Memory]");

        let js_tag: String = eval(
            None,
            r#"
              const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, "JSTag");
              descriptor === undefined
                ? "absent"
                : [WebAssembly.JSTag instanceof WebAssembly.Tag,
                   WebAssembly.JSTag === WebAssembly.JSTag,
                   WebAssembly.JSTag.type().parameters.join(),
                   descriptor.set === undefined,
                   !descriptor.enumerable && descriptor.configurable].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(js_tag, "true,true,externref,true,true");
    }

    /// `BufferSource` is `ArrayBuffer or ArrayBufferView` and nothing else, so
    /// an object merely wearing a view's property names is a `TypeError`.
    #[tokio::test]
    async fn a_duck_typed_buffer_source_is_a_type_error() {
        let names: String = eval(
            Some(wat(ADD)),
            r#"
              const attempt = (source) => {
                try { new WebAssembly.Module(source); return "compiled"; }
                catch (error) { return error.name; }
              };
              [attempt(WASM),
               attempt(WASM.buffer),
               attempt(new DataView(WASM.buffer)),
               attempt({ buffer: WASM.buffer, byteOffset: 0, byteLength: WASM.byteLength }),
               attempt("not a buffer")].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(names, "compiled,compiled,compiled,TypeError,TypeError");
    }

    /// Imports and exports must be observed in *module declaration* order.
    #[tokio::test]
    async fn imports_and_exports_are_observed_in_module_declaration_order() {
        let orders: String = eval(
            Some(wat(DECLARATION_ORDER)),
            r#"
              const values = {
                zebra: () => {},
                apple: 0,
                middle: new WebAssembly.Memory({ initial: 1 }),
                banana: new WebAssembly.Table({ initial: 1, element: "anyfunc" }),
              };
              const read = [];
              const env = {};
              for (const name of Object.keys(values)) {
                Object.defineProperty(env, name, {
                  get: () => { read.push(name); return values[name]; },
                  enumerable: true,
                });
              }
              const { module, instance } = await WebAssembly.instantiate(WASM, { env });
              [read.join(","),
               WebAssembly.Module.imports(module).map((entry) => entry.name).join(","),
               WebAssembly.Module.exports(module).map((entry) => entry.name).join(","),
               Object.keys(instance.exports).join(",")].join(";")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(
            orders,
            "zebra,apple,middle,banana;zebra,apple,middle,banana;zebra,apple,middle;zebra,apple,\
             middle"
        );
    }

    /// Every buffer den hands to script has to be one QuickJS itself
    /// allocated. `ArrayBuffer::new`/`TypedArray::new` only *lend* a Rust
    /// `Vec`, registering a free hook that quickjs-ng runs twice — once in
    /// `JS_DetachArrayBuffer` (quickjs.c:58037), once in the finalizer
    /// (:57935) — and `transfer` additionally reallocs that foreign pointer
    /// through `js_realloc`. Three lines of script (`wat2wasm("(module)")
    /// .buffer.transfer(4)`) were enough to abort the process, so the check is
    /// simply that the snippet returns at all: an abort takes the test binary
    /// with it.
    #[tokio::test]
    async fn every_buffer_handed_to_script_survives_transfer_and_detach() {
        let outcome: String = eval(
            Some(wat(ADD)),
            r#"
              const module = await WebAssembly.compile(WASM);
              const section = WebAssembly.Module.customSections(module, "hello")[0];
              const assembled = denWasm.wat2wasm("(module)");
              // A length argument makes `transfer` realloc the store, which is
              // what turns a foreign pointer into heap corruption; grow and
              // shrink both go that way.
              const grown = assembled.buffer.transfer(assembled.byteLength + 8);
              const shrunk = grown.transfer(4);
              // `transfer(0)` is a plain `JS_DetachArrayBuffer` — the path a
              // `postMessage` transfer takes — which calls the free hook once
              // and leaves the finalizer to call it a second time.
              denWasm.wat2wasm("(module)").buffer.transfer(0);
              const moved = section.transfer();
              const fixture = WASM.buffer.transfer();
              [new Uint8Array(shrunk).join("-"),
               String.fromCharCode(...new Uint8Array(moved)),
               assembled.buffer.detached, grown.detached, section.detached,
               fixture.byteLength > 0, WASM.byteLength].join(",")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(outcome, "0-97-115-109,world,true,true,true,true,0");
    }
}
