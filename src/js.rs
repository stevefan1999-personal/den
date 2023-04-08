use den_stdlib_core::WORLD_END;
use rquickjs::{BuiltinLoader, BuiltinResolver, Context, FileResolver, ModuleLoader, Runtime};
use tokio::{
    signal,
    sync::mpsc,
    task::{AbortHandle, JoinSet},
};

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::http::HttpResolver,
};

pub fn create_qjs_context(join_set: &mut JoinSet<()>) -> (Runtime, Context, AbortHandle) {
    let rt = Runtime::new().unwrap();

    {
        let resolver = (
            BuiltinResolver::default(),
            HttpResolver::default(),
            FileResolver::default()
                .with_path("./")
                .with_pattern("{}.js")
                .with_pattern("{}.jsx")
                .with_pattern("{}.ts")
                .with_pattern("{}.tsx")
                .with_pattern("{}.mjs"),
        );
        let loader = (
            BuiltinLoader::default(),
            HttpLoader::default(),
            MmapScriptLoader::default()
                .with_extension("js")
                .with_extension("jsx")
                .with_extension("ts")
                .with_extension("tsx")
                .with_extension("mjs"),
            ModuleLoader::default(),
        );
        rt.set_loader(resolver, loader);
    }

    rt.set_interrupt_handler({
        let world_end = WORLD_END.get().unwrap().child_token();
        let (ctrlc_tx, mut ctrlc_rx) = mpsc::unbounded_channel();
        tokio::spawn({
            async move {
                loop {
                    let _ = signal::ctrl_c().await;
                    let _ = ctrlc_tx.send(());
                }
            }
        });
        Some(Box::new(move || {
            ctrlc_rx
                .try_recv()
                .map_or_else(|_| world_end.is_cancelled(), |_| true)
        }))
    });
    let handle = join_set.spawn(rt.run_executor());

    let ctx = Context::full(&rt).unwrap();
    ctx.enable_big_num_ext(true);
    ctx.with(|ctx| {
        let global = ctx.globals();
        global.init_def::<den_stdlib_console::Console>().unwrap();
        global.init_def::<den_stdlib_timer::Timer>().unwrap();
        global.init_def::<den_stdlib_socket::Socket>().unwrap();
        global.init_def::<den_stdlib_socket::IpAddr>().unwrap();
        global.init_def::<den_stdlib_socket::SocketAddr>().unwrap();
    });

    (rt, ctx, handle)
}
