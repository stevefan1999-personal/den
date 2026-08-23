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

/// Speculative conversions that leave no pending exception behind.
///
/// `Class::from_object` is `JS_GetOpaque2` (quickjs.c:11681), which *throws* a
/// `TypeError` when the object belongs to some other class, and a failed
/// `FromJs` probe throws just the same. Code that reads such a failure as "not
/// this shape, try the next one" has to take that exception back out of the
/// context: it stays pending otherwise, and the next place den inspects the
/// pending exception — `utils::ExportedFunction::call` and
/// `instance::Instance::throw_instantiation_failure` above all — reports it as
/// its own.
pub(crate) trait Probe {
    /// Run `attempt`, discarding whatever exception it leaves pending when it
    /// yields `None`.
    fn probe<T>(&self, attempt: impl FnOnce() -> Option<T>) -> Option<T>;
}

impl Probe for rquickjs::Ctx<'_> {
    fn probe<T>(&self, attempt: impl FnOnce() -> Option<T>) -> Option<T> {
        let outcome = attempt();
        if outcome.is_none() && self.has_exception() {
            // `catch` is `JS_GetException`, which is what clears the slot; the value
            // itself is of no interest, the caller reports its own error.
            drop(self.catch());
        }
        outcome
    }
}

/// `compileStreaming` / `instantiateStreaming`.
///
/// Written in JS: the algorithm is "await a response, check it, await its
/// body", which needs no Rust future plumbing, and duck-typing the `Response`
/// keeps this crate independent of den's fetch implementation (any polyfill
/// with `ok`, `headers.get` and `arrayBuffer` works).
const DEFINE_STREAMING: &str = r#"
(namespace) => {
  const wasmBytes = async (source) => {
    const response = await source;
    const contentType = String(response.headers.get("Content-Type") ?? "").trim().toLowerCase();
    if (contentType !== "application/wasm") {
      throw new TypeError(`expected a Content-Type of application/wasm, got "${contentType}"`);
    }
    if (!response.ok) {
      throw new TypeError(`the response status ${response.status} is not an ok status`);
    }
    return await response.arrayBuffer();
  };
  const define = (name, value) =>
    Object.defineProperty(namespace, name, { value, writable: true, configurable: true });
  define("compileStreaming", async (source) => namespace.compile(await wasmBytes(source)));
  define("instantiateStreaming", async (source, importObject) =>
    namespace.instantiate(await wasmBytes(source), importObject));
}
"#;

/// The WebIDL shape of the namespace object, applied once everything is on it.
///
/// Two rules, neither of which rquickjs' `Object::set` can express: a namespace
/// member and an interface object are "the property attributes { [[Writable]]:
/// true, [[Enumerable]]: false, [[Configurable]]: true }" (WebIDL §
/// *Namespaces* and § *Interfaces*), and every namespace object and interface
/// prototype carries an `@@toStringTag` naming it, so `String(WebAssembly)` is
/// `[object WebAssembly]` rather than `[object Object]`.
/// `JSTag` comes along for the ride because it is the one namespace member that
/// is a *value* rather than a function or a class: the tag `(param externref)`
/// that a JS exception crossing into wasm carries. WebIDL exposes a namespace's
/// `readonly attribute` as an accessor, so a getter it is.
const DEFINE_NAMESPACE_SHAPE: &str = r#"
(namespace, interfaces, supportsTags) => {
  const tag = (target, value) =>
    target && Object.defineProperty(target, Symbol.toStringTag, { value, configurable: true });
  // Only the enumerable members need fixing; the error classes and the
  // streaming functions were defined with the right attributes already.
  // Every attribute is spelled out: redefining an existing property leaves the
  // ones the descriptor omits exactly as they were, so `enumerable: false` is
  // the whole point of this loop and cannot be left implicit.
  for (const name of Object.keys(namespace)) {
    Object.defineProperty(namespace, name,
      { value: namespace[name], writable: true, enumerable: false, configurable: true });
  }
  for (const name of interfaces) {
    tag(namespace[name]?.prototype, `WebAssembly.${name}`);
  }
  tag(namespace, "WebAssembly");
  if (supportsTags) {
    const jsTag = new namespace.Tag({ parameters: ["externref"] });
    Object.defineProperty(namespace, "JSTag", { get: () => jsTag, configurable: true });
  }
}
"#;

#[rquickjs::module]
pub mod wasm {
    use rquickjs::{
        Class, Ctx, Exception, Function, Object, Result, TypedArray, Value, module::Exports,
        prelude::Opt,
    };

    use crate::{
        Probe, backend,
        engine::Engine,
        error::WebAssemblyErrors,
        instance::ImportedFunctions,
        memory::MemoryBuffers,
        module::BufferSource,
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

    /// Asynchronous, so a decode failure *rejects* with `CompileError` rather
    /// than throwing.
    #[rquickjs::function]
    pub async fn compile<'js>(bytes: BufferSource, ctx: Ctx<'js>) -> Result<Module> {
        Module::compile(&ctx, bytes.into_bytes())
    }

    /// Both overloads: a `Module` resolves with a bare `Instance`, a
    /// `BufferSource` with `{ module, instance }`.
    #[rquickjs::function]
    pub async fn instantiate<'js>(
        source: Value<'js>,
        import_object: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        // A `Module` argument and a `BufferSource` argument are told apart by
        // probing, so the probe must not leave its `TypeError` pending.
        if let Some(module) =
            ctx.probe(|| source.as_object().and_then(Class::<Module>::from_object))
        {
            let module = module.try_borrow().map(|module| Module::clone(&module));
            let module = module.map_err(|_| {
                Exception::throw_type(&ctx, "the WebAssembly.Module is already in use")
            })?;
            let instance = Instance::instantiate(&ctx, &module, import_object.0)?;
            return Ok(Class::instance(ctx.clone(), instance)?.into_value());
        }

        let bytes = <BufferSource as rquickjs::FromJs>::from_js(&ctx, source)?;
        let module = Module::compile(&ctx, bytes.into_bytes())?;
        let instance = Instance::instantiate(&ctx, &module, import_object.0)?;
        let result = Object::new(ctx.clone())?;
        result.set("module", Class::instance(ctx.clone(), module)?)?;
        result.set("instance", Class::instance(ctx.clone(), instance)?)?;
        Ok(result.into_value())
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
    pub fn wasi_imports<'js>(ctx: Ctx<'js>) -> Result<Value<'js>> {
        WasiImports::namespace(&ctx)
    }

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
        namespace.set("wat2wasm", js_wat2wasm)?;

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
        ctx.eval::<Function, _>(crate::DEFINE_STREAMING)?
            .call::<_, ()>((namespace.clone(),))?;
        // Last, so that every member is already on the namespace when its
        // property attributes are corrected.
        ctx.eval::<Function, _>(crate::DEFINE_NAMESPACE_SHAPE)?
            .call::<_, ()>((namespace.clone(), interface_names, backend::SUPPORTS_TAGS))?;
        ctx.globals().set("WebAssembly", namespace)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Object, Promise, TypedArray,
        context::EvalOptions,
    };

    use crate::backend;

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
    #[cfg(feature = "wasmtime")]
    const CALLS_WASI: &str = r#"
        (module
          (import "wasi_snapshot_preview1" "environ_sizes_get"
                  (func $sizes (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "run") (result i32) (call $sizes (i32.const 0) (i32.const 8))))
    "#;

    const EXPORTS_A_TAG: &str = r#"(module (tag (export "t") (param i32)))"#;

    /// Imports and exports whose declaration order is neither alphabetical nor
    /// grouped by kind, so the two backends disagree unless the order is read
    /// back out of the binary.
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
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .async_with(async |ctx| {
                let run = async {
                    let (module, evaluated) =
                        Module::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")?;
                    evaluated.into_future::<()>().await?;
                    // The snippets are evaluated as global scripts, which cannot
                    // `import`, so `den:wasm`'s own exports are hoisted onto the
                    // global object under the name a script would bind them to.
                    ctx.globals().set("denWasm", module.namespace()?)?;
                    if let Some(bytes) = bytes {
                        // `new_copy` for the same reason as `wat2wasm`: the
                        // fixture must behave like a buffer script made.
                        ctx.globals()
                            .set("WASM", TypedArray::new_copy(ctx.clone(), bytes)?)?;
                    }
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    ctx.eval_with_options::<Promise, _>(source, options)?
                        .into_future::<Object>()
                        .await?
                        .get::<_, T>("value")
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
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

    /// `memory.grow` executed as a wasm instruction moves the linear memory —
    /// on wasmi it is a `Vec`, so the old pages are freed outright — and the
    /// buffer JS is holding has to be detached on the way back out.
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
        let asked = if backend::SUPPORTS_WASI {
            "object"
        } else {
            "TypeError: WASI is not supported by the wasmi backend of this build"
        };
        assert_eq!(outcome, format!("false,{asked}"));
    }

    /// The whole opt-in path, end to end through JS: the marker stands in for
    /// the `wasi_snapshot_preview1` namespace, `read_imports` recognises it,
    /// and the module then calls a preview1 function that writes the caller's
    /// own linear memory. Without the hook in `read_imports` the marker is just
    /// an object with no `environ_sizes_get` on it and this is a `LinkError`.
    #[cfg(feature = "wasmtime")]
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
        if backend::SUPPORTS_TAGS {
            assert_eq!(outcome, "true,i32");
        } else {
            // wasmi implements no part of exception handling, so the fixture does
            // not even compile there.
            assert_eq!(outcome, "CompileError");
        }
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
              const wrong = Object.getOwnPropertyNames(WebAssembly).filter((name) => {
                const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, name);
                // `JSTag` is a readonly attribute, hence an accessor with no `writable`.
                return descriptor.enumerable || !descriptor.configurable
                  || ("value" in descriptor && !descriptor.writable);
              });
              [wrong.join("|"),
               Object.keys(WebAssembly).length,
               String(WebAssembly),
               Object.prototype.toString.call(new WebAssembly.Memory({ initial: 1 }))].join(";")
            "#,
        )
        .await
        .expect("the snippet evaluates");
        assert_eq!(shape, ";0;[object WebAssembly];[object WebAssembly.Memory]");

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
        if backend::SUPPORTS_TAGS {
            assert_eq!(js_tag, "true,true,externref,true,true");
        } else {
            assert_eq!(js_tag, "absent");
        }
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

    /// Both backends must present imports and exports in *module declaration*
    /// order: wasmtime does already, wasmi groups imports by kind and sorts
    /// exports, so the order is recovered from the module binary.
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
