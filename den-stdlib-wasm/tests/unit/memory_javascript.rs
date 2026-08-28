const EXERCISE: &str = include_str!("../fixtures/unit/memory_objects.js");

#[test]
fn the_wasm_objects_behave_the_way_scripts_expect() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let held: bool = den_core::engine::Engine::new()
                .await
                .eval(EXERCISE)
                .await
                .expect("every assertion holds");
            assert!(held);
        })
}
