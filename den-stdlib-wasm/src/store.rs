//! The single wasm store of a JS context.

use std::{cell::RefCell, rc::Rc};

use den_util::Probe;
use rquickjs::{Class, Ctx, Exception, JsLifetime, Object, Result, Value, class::Trace};

use crate::{
    backend,
    error::{throw_link_error, throw_runtime_error},
};

/// Handle to the one wasmtime [`backend::Store`] a JS context owns.
///
/// Every `Instance`, `Memory`, `Table` and `Global` of a context lives in this
/// store, which is what makes them interchangeable as imports.
#[derive(Clone, JsLifetime)]
pub struct Store {
    /// Prefer [`Store::with_mut`]: a bare `borrow_mut` panics on re-entry.
    pub(crate) inner: Rc<RefCell<backend::Store>>,
}

impl Store {
    /// Why a re-entrant use was refused, naming both what is holding the store
    /// and what the caller can do instead.
    const REENTRY_REFUSED: &'static str =
        "a WebAssembly export is still running and has called back into JS: this build cannot \
         re-enter its wasm store, so calling another export — or creating a Memory, Table, Global \
         or Tag — is unsupported until that call returns";

    pub fn new(engine: &wasmtime::Engine, ctx: &Ctx<'_>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(backend::Store::new(
                engine,
                backend::StoreData::new(ctx),
            ))),
        }
    }

    /// The store `den:wasm` installed in this context.
    pub fn from_ctx(ctx: &Ctx<'_>) -> Result<Self> {
        ctx.userdata::<Self>()
            .map(|store| store.clone())
            .ok_or_else(|| {
                Exception::throw_internal(ctx, "the WebAssembly store is missing from this context")
            })
    }

    /// Run `f` with the store mutably borrowed, refusing re-entrant use.
    ///
    /// The refusal is a `WebAssembly.RuntimeError`, never a panic: the only way
    /// to get here twice is from JS — a host function called by a running
    /// export — so a `borrow_mut` would be a JS-reachable abort.
    ///
    /// ponytail: one `RefCell` for the whole store, so *every* wasm → JS → wasm
    /// re-entry is refused, including the legitimate ones (a host callback
    /// touching an unrelated `Memory`, or calling an export of a different
    /// instance). Lifting it needs the borrow scoped per call frame rather than
    /// per outermost call — either by handing host callbacks the `Caller`'s own
    /// store context instead of reaching back into this `RefCell`, or by
    /// splitting the store per instance and giving up on imports being
    /// interchangeable between them.
    pub fn with_mut<R>(
        &self, ctx: &Ctx<'_>, f: impl FnOnce(&mut backend::Store) -> Result<R>,
    ) -> Result<R> {
        match self.inner.try_borrow_mut() {
            Ok(mut store) => f(&mut store),
            Err(_) => Err(throw_runtime_error(ctx, Self::REENTRY_REFUSED)),
        }
    }
}

/// The opaque object `den:wasm`'s `wasiImports()` hands back.
///
/// WASI preview1 is implemented by the *engine*: every call reads and writes
/// the calling instance's own linear memory, which nothing reachable from JS
/// can stand in for, so the namespace cannot be an ordinary bag of functions.
/// It is this marker instead, and `Instance::read_imports` recognising it in
/// the place of the `wasi_snapshot_preview1` namespace is the one and only way
/// WASI ever reaches a linker:
///
/// ```js
/// import { wasiImports } from "den:wasm";
/// await WebAssembly.instantiate(bytes, { wasi_snapshot_preview1: wasiImports() });
/// ```
///
/// It lives on `den:wasm` rather than on `WebAssembly`, which is exactly the
/// namespace the spec says it is and nothing more.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct WasiImports {}

impl WasiImports {
    /// `wasiImports()`: the marker, or a `TypeError` if this build has no
    /// WASI to give.
    ///
    /// Nothing is built here — the host's stdio and environment are inherited
    /// by [`backend::link_wasi`], at instantiation — so holding the marker
    /// grants nothing on its own.
    pub fn namespace<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
        if !backend::SUPPORTS_WASI {
            return Err(Exception::throw_type(
                ctx,
                "WASI is not available in this build",
            ));
        }
        Ok(Class::instance(ctx.clone(), Self {})?.into_value())
    }

    /// Whether `namespace` is the marker rather than an import namespace to
    /// read names out of.
    ///
    /// Probed, because a failed `from_object` throws (see `Probe`) and every
    /// import object that is *not* WASI passes through here.
    pub fn is_marker(ctx: &Ctx<'_>, namespace: &Object<'_>) -> bool {
        ctx.probe(|| Class::<Self>::from_object(namespace))
            .is_some()
    }

    /// Link the engine's preview1 implementation, once the caller has asked for
    /// it under the namespace it actually implements.
    pub fn link(ctx: &Ctx<'_>, linker: &mut backend::Linker, namespace: &str) -> Result<()> {
        if namespace != backend::WASI_NAMESPACE {
            return Err(throw_link_error(
                ctx,
                format_args!(
                    "wasiImports() implements the \"{}\" namespace, not \"{namespace}\"",
                    backend::WASI_NAMESPACE
                ),
            ));
        }
        backend::link_wasi(linker).map_err(|err| throw_link_error(ctx, err))
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{Context, Module, Runtime};

    use super::*;
    use crate::memory::testing::with_wasm_context;

    /// The `name` and `message` of the pending JS error.
    fn pending_error(ctx: &Ctx<'_>) -> (String, String) {
        let thrown = ctx.catch();
        let exception = thrown.as_exception().expect("a JS error was thrown");
        (
            exception.get("name").expect("name"),
            exception.get("message").expect("message"),
        )
    }

    #[test]
    fn a_second_borrow_of_the_store_is_a_runtime_error_rather_than_a_panic() {
        with_wasm_context(|ctx| {
            let store = Store::from_ctx(ctx).expect("store");
            let refused = store.with_mut(ctx, |_| {
                // Exactly what a host callback reaching any wasm object does.
                store.with_mut(ctx, |_| Ok(()))
            });
            assert!(refused.is_err());

            let (name, message) = pending_error(ctx);
            assert_eq!(name, "RuntimeError");
            assert!(
                message.contains("called back into JS") && message.contains("another export"),
                "the refusal does not say what re-entered: {message}"
            );
        })
    }

    #[test]
    fn wasi_imports_hands_out_a_marker_object() {
        with_wasm_context(|ctx| {
            let marker = WasiImports::namespace(ctx).expect("WASI is supported");
            let marker = marker.into_object().expect("wasiImports() is an object");
            assert!(WasiImports::is_marker(ctx, &marker));
            // The probe must recognise an ordinary import namespace as *not*
            // the marker, and leave no `TypeError` pending when it does.
            let ordinary = Object::new(ctx.clone()).expect("an object");
            assert!(!WasiImports::is_marker(ctx, &ordinary));
            assert!(!ctx.has_exception(), "the probe left an exception pending");
        })
    }

    /// The namespace is part of what `wasiImports()` implements, so handing it
    /// to some other one is a `LinkError` that says so rather than a module
    /// whose real imports silently go missing.
    #[test]
    fn wasi_imports_refuses_to_stand_in_for_another_namespace() {
        with_wasm_context(|ctx| {
            let engine = backend::new_engine().expect("engine");
            let mut linker = backend::Linker::new(&engine);
            let refused = WasiImports::link(ctx, &mut linker, "env");
            assert!(refused.is_err());

            let (name, message) = pending_error(ctx);
            assert_eq!(name, "LinkError");
            assert!(
                message.contains("wasi_snapshot_preview1") && message.contains("\"env\""),
                "the refusal does not name both namespaces: {message}"
            );
        })
    }

    /// A module that asks WASI for something only the engine can answer: the
    /// environment count is written into the *caller's* linear memory, which is
    /// why preview1 cannot be a bag of JS functions in the first place. The
    /// return value is the errno, `0` for success.
    const CALLS_WASI: &str = r#"
      (module
        (import "wasi_snapshot_preview1" "environ_sizes_get" (func $sizes (param i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "run") (result i32) (call $sizes (i32.const 0) (i32.const 8))))
    "#;

    /// End to end through wasmtime, one layer below the JS path that
    /// `instance.rs` covers. Links twice on purpose: `read_imports` reaches the
    /// namespace once per WASI import, and `add_to_linker_sync` defines the
    /// whole namespace each time.
    #[test]
    fn wasi_imports_links_a_preview1_module_that_writes_its_own_memory() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let (_, evaluation) =
                Module::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")
                    .expect("den:wasm evaluates");
            evaluation.finish::<()>().expect("den:wasm finishes");

            let engine = crate::engine::Engine::from_ctx(&ctx).expect("engine");
            let mut linker = backend::Linker::new(&engine);
            for _ in 0..2 {
                WasiImports::link(&ctx, &mut linker, backend::WASI_NAMESPACE)
                    .expect("wasi links, idempotently");
            }

            let bytes = wat::parse_str(CALLS_WASI).expect("the fixture assembles");
            let module = backend::compile_module(&engine, &bytes).expect("the fixture compiles");
            let errno = Store::from_ctx(&ctx)
                .expect("store")
                .with_mut(&ctx, |backend_store| {
                    let instance = linker
                        .instantiate(&mut *backend_store, &module)
                        .expect("the module instantiates against WASI");
                    let run = instance
                        .get_export(&mut *backend_store, "run")
                        .and_then(wasmtime::Extern::into_func)
                        .expect("the run export");
                    let mut results = [wasmtime::Val::I32(-1)];
                    run.call(&mut *backend_store, &[], &mut results)
                        .expect("the export calls WASI");
                    Ok(results[0].i32())
                })
                .expect("the call completes");
            assert_eq!(errno, Some(0), "environ_sizes_get did not report success");
        })
    }

    /// The documented ceiling, pinned end to end: a host function called by a
    /// running export cannot reach a *different* export either, even though
    /// nothing about that is unsound. Flip this test when the store learns to
    /// hand out per-frame borrows.
    const REENTERS_FROM_A_HOST_CALL: &str = r#"
      const bytes = denWasm.wat2wasm(`(module
        (import "env" "reenter" (func $reenter))
        (func (export "run") (call $reenter))
        (func (export "other") (result i32) (i32.const 1)))`);

      let instance;
      let caught = null;
      instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {
        env: {
          reenter: () => {
            try {
              instance.exports.other();
            } catch (error) {
              caught = error;
            }
          },
        },
      });
      instance.exports.run();
      [caught === null ? "no error" : caught.name, caught === null ? "" : caught.message]
    "#;

    #[test]
    fn a_host_callback_cannot_reach_another_export_of_the_same_store() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let (module, evaluation) =
                Module::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")
                    .expect("den:wasm evaluates");
            evaluation.finish::<()>().expect("den:wasm finishes");
            ctx.globals()
                .set("denWasm", module.namespace().expect("den:wasm namespace"))
                .expect("bind den:wasm exports");

            let caught: Vec<String> = ctx
                .eval(REENTERS_FROM_A_HOST_CALL)
                .expect("the module runs");
            let [name, message] = <[String; 2]>::try_from(caught).expect("name and message");
            assert_eq!(name, "RuntimeError");
            assert!(
                message.contains("called back into JS"),
                "the refusal does not say what re-entered: {message}"
            );
        })
    }
}
