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

#[cfg(test)]
pub(crate) fn install_test_wat2wasm(ctx: Ctx<'_>) -> Result<()> {
    fn assemble(source: String, ctx: Ctx<'_>) -> Result<rquickjs::TypedArray<'_, u8>> {
        let bytes = wat::parse_str(&source)
            .map_err(|error| Exception::throw_type(&ctx, &format!("wat2wasm error: {error}")))?;
        rquickjs::TypedArray::new_copy(ctx, bytes)
    }

    ctx.globals().set(
        "wat2wasm",
        Function::new(ctx.clone(), assemble)?.with_name("wat2wasm")?,
    )
}

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
    ctx: &Ctx<'js>, namespace: &Object<'js>, interfaces: &[&str],
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
    let tag_ctor: Constructor = namespace.get("Tag")?;
    let descriptor = Object::new(ctx.clone())?;
    descriptor.set("parameters", vec!["externref"])?;
    let js_tag: Value = tag_ctor.construct((descriptor,))?;
    ctx.store_userdata(WasmJsTag(js_tag))
        .map_err(|_error| Exception::throw_internal(ctx, "WebAssembly.JSTag is already in use"))?;
    let _ = namespace.prop(
        "JSTag",
        Accessor::from(|ctx: Ctx<'js>| -> Result<Value<'js>> {
            ctx.userdata::<WasmJsTag>()
                .map(|tag| tag.0.clone())
                .ok_or_else(|| Exception::throw_internal(&ctx, "WebAssembly.JSTag is missing"))
        })
        .configurable(),
    );
    Ok(())
}

/// Await a duck-typed `Response` (ok, headers.get, arrayBuffer, status) and
/// yield its body. Independent of Fetch globals.
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
    use den_util::{BufferSource, ConstructorInstaller as _, Probe as _};
    #[cfg(feature = "wasi")] use rquickjs::Function;
    use rquickjs::{
        Class, Ctx, Exception, IntoJs as _, Object, Promise, Result, Value,
        module::{Declarations, Exports},
        prelude::Opt,
        promise::Promised,
    };

    #[cfg(feature = "wasi")]
    use crate::store::WasiImports;
    use crate::{
        backend,
        engine::Engine,
        error::WebAssemblyErrors,
        instance::ImportedFunctions,
        memory::MemoryBuffers,
        store::{ActiveHostCall, Store},
    };
    pub use crate::{
        exception::Exception as ExceptionClass, global::Global, instance::Instance, memory::Memory,
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
        <BufferSource as rquickjs::FromJs>::from_js(&ctx, bytes).map_or_else(
            |_error| reject_pending(&ctx),
            |source| {
                let bytes = source.into_bytes();
                let compile_ctx = ctx.clone();
                Promised(async move { Module::compile(&compile_ctx, bytes) }).into_js(&ctx)
            },
        )
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
            let Ok(module) = module.try_borrow().map(|module| Module::clone(&module)) else {
                return reject_type(&ctx, "the WebAssembly.Module is already in use");
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
    #[cfg(feature = "wasi")]
    fn wasi_imports(ctx: Ctx<'_>) -> Result<Value<'_>> { WasiImports::namespace(&ctx) }

    #[qjs(declare)]
    pub fn declare(declarations: &Declarations<'_>) -> Result<()> {
        #[cfg(not(feature = "wasi"))]
        let _ = declarations;
        #[cfg(feature = "wasi")]
        declarations.declare("wasiImports")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        #[cfg(not(feature = "wasi"))]
        let _ = exports;
        #[cfg(feature = "wasi")]
        exports.export(
            "wasiImports",
            Function::new(ctx.clone(), wasi_imports)?.with_name("wasiImports")?,
        )?;
        let engine = Engine::new().map_err(|err| {
            Exception::throw_internal(ctx, &format!("cannot create a WebAssembly engine: {err}"))
        })?;
        let in_use = |what: &'static str| {
            Exception::throw_internal(ctx, &format!("the WebAssembly {what} is already in use"))
        };
        ctx.store_userdata(engine)
            .map_err(|_error| in_use("engine"))?;
        let store = Store::new(&Engine::from_ctx(ctx)?, ctx);
        ctx.store_userdata(store)
            .map_err(|_error| in_use("store"))?;
        ctx.store_userdata(ActiveHostCall::default())
            .map_err(|_error| in_use("host-call stack"))?;
        ctx.store_userdata(ImportedFunctions::default())
            .map_err(|_error| in_use("import registry"))?;
        ctx.store_userdata(MemoryBuffers::default())
            .map_err(|_error| in_use("memory buffer registry"))?;

        let namespace = Object::new(ctx.clone())?;
        namespace.set("validate", js_validate)?;
        namespace.set("compile", js_compile)?;
        namespace.set("instantiate", js_instantiate)?;

        namespace.install_constructor::<Module>(1)?;
        namespace.install_constructor::<Instance>(1)?;
        namespace.install_constructor::<Memory>(1)?;
        namespace.install_constructor::<Table>(1)?;
        namespace.install_constructor::<Global>(1)?;
        namespace.install_constructor::<Tag>(1)?;
        namespace.install_constructor::<ExceptionClass>(2)?;
        let interface_names = vec![
            "Module",
            "Instance",
            "Memory",
            "Table",
            "Global",
            "Tag",
            "Exception",
        ];

        WebAssemblyErrors::install(ctx, &namespace)?;
        crate::install_streaming(ctx, &namespace)?;
        ctx.globals().set("WebAssembly", namespace.clone())?;
        // After the global is installed, so shape can correct the property
        // attributes of `globalThis.WebAssembly` itself.
        crate::shape_namespace(ctx, &namespace, &interface_names)?;
        Ok(())
    }
}
