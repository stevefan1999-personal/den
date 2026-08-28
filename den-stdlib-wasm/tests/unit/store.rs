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
#[cfg(feature = "wasi")]
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
#[cfg(feature = "wasi")]
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
#[cfg(feature = "wasi")]
const CALLS_WASI: &str = include_str!("../fixtures/unit/store_calls_wasi.wat");

/// End to end through wasmtime, one layer below the JS path that
/// `instance.rs` covers. Links twice on purpose: `read_imports` reaches the
/// namespace once per WASI import, and `add_to_linker_sync` defines the
/// whole namespace each time.
#[test]
#[cfg(feature = "wasi")]
fn wasi_imports_links_a_preview1_module_that_writes_its_own_memory() {
    with_wasm_context(|ctx| {
        let engine = Store::from_ctx(ctx)
            .expect("store")
            .inner
            .borrow()
            .engine()
            .clone();
        let mut linker = backend::Linker::new(&engine);
        for _ in 0..2 {
            WasiImports::link(ctx, &mut linker, backend::WASI_NAMESPACE)
                .expect("wasi links, idempotently");
        }

        let bytes = wat::parse_str(CALLS_WASI).expect("the fixture assembles");
        let module = backend::compile_module(&engine, &bytes).expect("the fixture compiles");
        let errno = Store::from_ctx(ctx)
            .expect("store")
            .with_mut(ctx, |backend_store| {
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

/// A host callback can reach another export through the parked `Caller`.
/// Creating a Memory / Table / Global / Tag still goes through `with_mut`
/// and is refused; that ceiling is what `errors.js` used to pin via a
/// recursive `run()`, which would livelock once invoke was allowed.
const REENTERS_FROM_A_HOST_CALL: &str = include_str!("../fixtures/unit/store_reentrancy.js");

#[test]
fn a_host_callback_can_reach_another_export_of_the_same_store() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let engine = den_core::engine::Engine::new().await;
            engine
                .context
                .with(crate::install_test_wat2wasm)
                .await
                .expect("install WAT assembler");
            let seen: Vec<String> = engine
                .eval(REENTERS_FROM_A_HOST_CALL)
                .await
                .expect("the module runs");
            let [result, status] = <[String; 2]>::try_from(seen).expect("result and status");
            assert_eq!((result.as_str(), status.as_str()), ("1", "ok"));
        })
}
