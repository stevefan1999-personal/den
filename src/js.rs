use std::{path::PathBuf, sync::Arc};

use bytes::Bytes;
use color_eyre::eyre;
use den_stdlib_core::WORLD_END;
use rquickjs::{
    intrinsic::All, BuiltinLoader, BuiltinResolver, Context, EvalOptions, FileResolver, FromJs,
    ModuleLoader, Runtime,
};
use swc_core::{
    base::{config::IsModule, sourcemap::SourceMap},
    ecma::parser::Syntax,
};
use tokio::{fs, signal, sync::mpsc, task::JoinHandle};

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::http::HttpResolver,
    transpile::EasySwcTranspiler,
};

#[derive(Clone)]
pub struct Engine {
    pub(crate) transpiler:      Arc<EasySwcTranspiler>,
    pub(crate) ctx:             Context,
    pub(crate) executor_handle: Arc<JoinHandle<()>>,
}

impl Engine {
    pub fn new() -> Engine {
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
            let world_end = WORLD_END.child_token();
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
        let handle = tokio::spawn(rt.run_executor());

        let ctx = Context::builder().with::<All>().build(&rt).unwrap();
        ctx.with(|ctx| {
            let global = ctx.globals();
            global.init_def::<den_stdlib_console::Console>().unwrap();
            global.init_def::<den_stdlib_timer::Timer>().unwrap();
            global.init_def::<den_stdlib_socket::Socket>().unwrap();
            global.init_def::<den_stdlib_socket::IpAddr>().unwrap();
            global.init_def::<den_stdlib_socket::SocketAddr>().unwrap();
        });

        Self {
            transpiler: Arc::new(Default::default()),
            ctx,
            executor_handle: handle.into(),
        }
    }

    pub async fn run_file(&self, filename: PathBuf) -> eyre::Result<()> {
        let file = fs::read_to_string(filename.clone()).await?;
        let (src, _) = self.transpile(
            &file,
            Syntax::Typescript(Default::default()),
            IsModule::Bool(true),
        )?;

        self.ctx.with(|ctx| {
            ctx.compile(filename.to_str().unwrap_or("<unknown>"), src)
                .map(|_| ())
        })?;
        Ok(())
    }

    pub fn run_immediate<U: for<'a> FromJs<'a>>(&self, src: &str) -> eyre::Result<U> {
        let (src, _) = self.transpile(
            src,
            Syntax::Typescript(Default::default()),
            IsModule::Bool(true),
        )?;

        Ok(self.ctx.with(|ctx| {
            ctx.eval_with_options(
                src,
                EvalOptions {
                    global: false,
                    ..Default::default()
                },
            )
        })?)
    }

    pub fn transpile(
        &self,
        src: &str,
        syntax: Syntax,
        module: IsModule,
    ) -> eyre::Result<(Bytes, Option<SourceMap>)> {
        self.transpiler.transpile(src, syntax, module, false)
    }

    pub fn eval<U: for<'a> FromJs<'a>>(&self, src: &str) -> eyre::Result<U> {
        let (src, _) = self.transpile(
            src,
            Syntax::Typescript(Default::default()),
            IsModule::Unknown,
        )?;

        Ok(self.ctx.with(|ctx| {
            ctx.eval_with_options(
                src,
                EvalOptions {
                    global: true,
                    ..Default::default()
                },
            )
        })?)
    }

    pub fn stop(&self) {
        self.executor_handle.abort()
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;

    use crate::js::Engine;

    #[tokio::test(flavor = "multi_thread")]
    async fn my_test() -> eyre::Result<()> {
        let engine = Engine::new();
        let _: usize = engine.run_immediate(
            r#"
            console.log("hello world")
        "#,
        )?;
        engine.run_immediate(
            r#"
            import { hello } from 'builtin'
            console.log(hello)
        "#,
        )?;
        Ok(())
    }
}
