use std::{path::PathBuf, sync::Arc};

use bytes::Bytes;
use color_eyre::eyre;
use den_stdlib_console::Console;
use den_stdlib_core::{js_core, WORLD_END};
use rquickjs::{
    async_with,
    context::EvalOptions,
    loader::{BuiltinLoader, BuiltinResolver, FileResolver, ModuleLoader},
    AsyncContext, AsyncRuntime, FromJs, Module,
};
use swc_core::{
    base::{config::IsModule, sourcemap::SourceMap},
    ecma::parser::Syntax,
};
use tokio::{fs, signal, sync::mpsc, task::yield_now};
use tokio_util::sync::CancellationToken;

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::http::HttpResolver,
    transpile::EasySwcTranspiler,
};

#[derive(Clone)]
pub struct Engine {
    pub(crate) transpiler: Arc<EasySwcTranspiler>,
    pub(crate) runtime:    AsyncRuntime,
    pub(crate) context:    AsyncContext,
    pub(crate) stop_token: CancellationToken,
}

impl Engine {
    pub async fn new() -> Engine {
        let runtime = AsyncRuntime::new().unwrap();

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
            runtime.set_loader(resolver, loader).await;
        }

        runtime
            .set_interrupt_handler({
                let world_end = WORLD_END.child_token();
                let (ctrlc_tx, mut ctrlc_rx) = mpsc::unbounded_channel();
                tokio::spawn({
                    async move {
                        loop {
                            let _ = signal::ctrl_c().await;
                            let _ = ctrlc_tx.send(());
                            yield_now().await;
                        }
                    }
                });
                Some(Box::new(move || {
                    ctrlc_rx
                        .try_recv()
                        .map_or_else(|_| world_end.is_cancelled(), |_| true)
                }))
            })
            .await;

        let context = AsyncContext::full(&runtime).await.unwrap();

        async_with!(context => |ctx| {
            let global = ctx.globals();
            global.set("console", Console {})?;
            Module::declare_def::<js_core, _>(ctx.clone(), "den:core")?;
            Ok::<_, rquickjs::Error>(())
        })
        .await
        .unwrap();

        Self {
            transpiler: Arc::new(Default::default()),
            runtime,
            context,
            stop_token: CancellationToken::new(),
        }
    }

    pub async fn run_file(&self, filename: PathBuf) -> eyre::Result<()> {
        let file = fs::read_to_string(filename.clone()).await?;
        let (src, _) = self.transpile(
            &file,
            Syntax::Typescript(Default::default()),
            IsModule::Bool(true),
        )?;

        self.context
            .with(|ctx| {
                ctx.compile(filename.to_str().unwrap_or("<unknown>"), src)
                    .map(|_| ())
            })
            .await?;
        Ok(())
    }

    pub async fn run_immediate<U: for<'a> FromJs<'a> + Sync + Send>(
        &self,
        src: &str,
    ) -> eyre::Result<U> {
        let (src, _) = self.transpile(
            src,
            Syntax::Typescript(Default::default()),
            IsModule::Bool(true),
        )?;

        Ok(self
            .context
            .with(|ctx| {
                ctx.eval_with_options(
                    src,
                    EvalOptions {
                        global: false,
                        ..Default::default()
                    },
                )
            })
            .await?)
    }

    pub fn transpile(
        &self,
        src: &str,
        syntax: Syntax,
        module: IsModule,
    ) -> eyre::Result<(Bytes, Option<SourceMap>)> {
        self.transpiler.transpile(src, syntax, module, false)
    }

    pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(
        &self,
        src: &str,
    ) -> eyre::Result<U> {
        let (src, _) = self.transpile(
            src,
            Syntax::Typescript(Default::default()),
            IsModule::Unknown,
        )?;

        Ok(async_with!(self.context => |ctx| {
            ctx.eval_with_options(
                src,
                EvalOptions {
                    global: true,
                    ..Default::default()
                },
            )
        })
        .await?)
    }

    pub async fn stop(&mut self) {
        self.stop_token.cancel()
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;

    use crate::js::Engine;

    #[tokio::test(flavor = "multi_thread")]
    async fn my_test() -> eyre::Result<()> {
        let engine = Engine::new().await;
        let _: usize = engine
            .run_immediate(
                r#"
            console.log("hello world")
        "#,
            )
            .await?;
        engine
            .run_immediate(
                r#"
            import { hello } from 'builtin'
            console.log(hello)
        "#,
            )
            .await?;
        Ok(())
    }
}
