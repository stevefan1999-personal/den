use color_eyre::eyre::eyre;

async fn wasm() -> color_eyre::Result<()> {
    let mut config = wasmtime::Config::default();
    config.consume_fuel(true);

    let engine = wasmtime::Engine::new(&config).map_err(|_x| eyre!("a"))?;
    let linking2 = wasmtime::Module::new(&engine, vec![]).map_err(|_x| eyre!("c"))?;

    let linker = wasmtime::Linker::new(&engine);

    // wasmtime_wasi::add_to_linker(&mut linker, |s| s).map_err(|x| eyre!("b"))?;
    // let wasi = WasiCtxBuilder::new()
    //     .inherit_stdio()
    //     .build();
    let mut store = wasmtime::Store::new(&engine, ());

    let instance = linker
        .instantiate(&mut store, &linking2)
        .map_err(|_x| eyre!("d"))?;
    // let instance = instance.start(&mut store).map_err(|x| eyre!("f"))?;

    instance
        .get_typed_func::<(), ()>(&mut store, "test")
        .map_err(|_x| eyre!("f"))?;

    Ok(())
}
