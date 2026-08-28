/// Each entry is `"<what>: <outcome>"`, where the outcome is the value the
/// operation produced or the `name` of the error it threw.
const OBSERVE: &str = include_str!("../fixtures/unit/backend_capabilities.js");

#[test]
fn engine_capabilities_match_the_snapshot() {
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
            let observed: Vec<String> = engine.eval(OBSERVE).await.expect("the program runs");
            insta::assert_snapshot!("engine_capabilities", observed.join("\n"));
        })
}
