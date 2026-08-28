use super::*;

/// A context with the whole of `den:wasm` evaluated into it: these two
/// cross wrappers — `Instance` into `Table`, and `Instance` against
/// `Module` — so they are written the way JS reaches them.
fn with_wasm_namespace<R: Send>(f: impl FnOnce(&Ctx<'_>) -> R + Send) -> R {
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
            engine.context.with(|ctx| f(&ctx)).await
        })
}

/// An export read off `instance.exports` is an Exported Function, so it has
/// a `[[FunctionAddress]]` a funcref table accepts — and reading it back
/// hands out that very object rather than a second callable wrapping the
/// same wasm function. Before the exports object went through
/// `HostReferences::exported_function`, the `set` threw a `TypeError`.
const FUNCREF_ROUND_TRIP: &str = include_str!("../fixtures/unit/instance_funcref_round_trip.js");

#[test]
fn an_export_round_trips_through_a_funcref_table_as_one_callable_object() {
    with_wasm_namespace(|ctx| {
        let outcome: String = ctx.eval(FUNCREF_ROUND_TRIP).expect("the snippet runs");
        assert_eq!(outcome, "true,true,42,0,2");
    })
}

/// `Module.exports()` promises a name for every declared export, so the
/// exports object has to carry all of them: an export kind den could not
/// wrap used to be dropped silently, leaving `instance.exports.x`
/// `undefined` with no diagnostic anywhere.
const BOTH_LISTINGS_AGREE: &str = include_str!("../fixtures/unit/instance_export_listings.js");

#[test]
fn the_exports_object_carries_every_export_the_module_declares() {
    with_wasm_namespace(|ctx| {
        let outcome: String = ctx.eval(BOTH_LISTINGS_AGREE).expect("the snippet runs");
        assert_eq!(outcome, "f,m,t,g|f,m,t,g");
    })
}
