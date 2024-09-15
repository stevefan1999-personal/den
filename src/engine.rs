use std::path::PathBuf;

use color_eyre::eyre;
use den_stdlib_console::Console;
use den_stdlib_core::{js_core, CancellationTokenWrapper};
use den_stdlib_fs::js_fs;
use den_stdlib_networking::js_networking;
use den_stdlib_text::js_text;
use den_stdlib_timer::js_timer;
#[cfg(feature = "wasm")]
use den_stdlib_wasm::js_wasm;
use rquickjs::{
    async_with,
    context::EvalOptions,
    loader::{BuiltinLoader, BuiltinResolver, FileResolver, ModuleLoader},
    AsyncContext, AsyncRuntime, FromJs, Module, Object, Promise,
};
use tokio::{signal, sync::mpsc, task::yield_now};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "transpile")]
use {
    crate::transpile::EasySwcTranspiler,
    den_utils::{get_best_transpiling, infer_transpile_syntax_by_extension},
    std::sync::Arc,
    swc_core::{
        base::{config::IsModule, sourcemap::SourceMap},
        ecma::parser::Syntax,
    },
};

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::http::HttpResolver,
};

#[derive(Clone)]
pub struct Engine {
    #[cfg(feature = "transpile")]
    pub(crate) transpiler: Arc<EasySwcTranspiler>,
    pub(crate) runtime:    AsyncRuntime,
    pub(crate) context:    AsyncContext,
    pub(crate) stop_token: CancellationToken,
}

#[allow(dead_code)]
impl Engine {
    pub async fn new() -> Engine {
        let runtime = AsyncRuntime::new().unwrap();

        {
            let resolver = (
                {
                    #[allow(unused_mut)]
                    let mut resolver = BuiltinResolver::default();

                    #[cfg(feature = "stdlib-core")]
                    {
                        resolver = resolver.with_module("den:core");
                    }
                    #[cfg(feature = "stdlib-networking")]
                    {
                        resolver = resolver.with_module("den:networking");
                    }
                    #[cfg(feature = "stdlib-text")]
                    {
                        resolver = resolver.with_module("den:text");
                    }
                    #[cfg(feature = "stdlib-timer")]
                    {
                        resolver = resolver.with_module("den:timer");
                    }
                    #[cfg(feature = "stdlib-fs")]
                    {
                        resolver = resolver.with_module("den:fs");
                    }
                    #[cfg(feature = "wasm")]
                    {
                        resolver = resolver.with_module("den:wasm");
                    }
                    resolver
                },
                HttpResolver::default(),
                {
                    #[allow(unused_mut)]
                    let mut resolver = FileResolver::default()
                        .with_path("./")
                        .with_pattern("{}.js")
                        .with_pattern("{}.mjs");

                    #[cfg(feature = "react")]
                    {
                        resolver = resolver.with_pattern("{}.jsx");
                        resolver = resolver.with_pattern("{}.mjsx");
                    }

                    #[cfg(feature = "typescript")]
                    {
                        resolver = resolver.with_pattern("{}.ts");

                        #[cfg(feature = "react")]
                        {
                            resolver = resolver.with_pattern("{}.tsx");
                        }
                    }

                    resolver
                },
            );
            let loader = (
                BuiltinLoader::default(),
                {
                    #[allow(unused_mut)]
                    let mut loader = ModuleLoader::default();

                    #[cfg(feature = "stdlib-core")]
                    {
                        loader = loader.with_module("den:core", js_core);
                    }

                    #[cfg(feature = "stdlib-networking")]
                    {
                        loader = loader.with_module("den:networking", js_networking);
                    }

                    #[cfg(feature = "stdlib-text")]
                    {
                        loader = loader.with_module("den:text", js_text);
                    }

                    #[cfg(feature = "stdlib-timer")]
                    {
                        loader = loader.with_module("den:timer", js_timer);
                    }

                    #[cfg(feature = "stdlib-fs")]
                    {
                        loader = loader.with_module("den:fs", js_fs);
                    }

                    #[cfg(feature = "wasm")]
                    {
                        loader = loader.with_module("den:wasm", js_wasm)
                    }
                    loader
                },
                HttpLoader::default(),
                {
                    #[allow(unused_mut)]
                    let mut loader = MmapScriptLoader::default();
                    loader = loader.with_extension("js");
                    loader = loader.with_extension("mjs");

                    #[cfg(feature = "react")]
                    {
                        loader = loader.with_extension("jsx");
                        loader = loader.with_extension("mjsx");
                    }

                    #[cfg(feature = "typescript")]
                    {
                        loader = loader.with_extension("ts");

                        #[cfg(feature = "react")]
                        {
                            loader = loader.with_extension("tsx");
                        }
                    }

                    loader
                },
            );
            runtime.set_loader(resolver, loader).await;
        }

        let stop_token = CancellationToken::new();

        runtime
            .set_interrupt_handler({
                let world_end = stop_token.clone();
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

        context
            .with(|ctx| {
                let global = ctx.globals();

                #[cfg(feature = "stdlib-console")]
                {
                    global.set("console", Console {})?;
                }

                #[cfg(feature = "stdlib-text")]
                {
                    let _ = Module::evaluate_def::<js_text, _>(ctx.clone(), "den:text")?;
                }

                #[cfg(feature = "stdlib-timer")]
                {
                    let _ = Module::evaluate_def::<js_timer, _>(ctx.clone(), "den:timer")?;
                }

                #[cfg(feature = "wasm")]
                {
                    let _ = Module::evaluate_def::<js_wasm, _>(ctx.clone(), "den:wasm")?;
                }

                ctx.globals().set(
                    "WORLD_END",
                    CancellationTokenWrapper {
                        token: stop_token.clone(),
                    },
                )?;

                Ok::<_, rquickjs::Error>(())
            })
            .await
            .unwrap();

        Self {
            #[cfg(feature = "transpile")]
            transpiler: Arc::new(EasySwcTranspiler::default()),
            runtime,
            context,
            stop_token,
        }
    }

    pub async fn run_file(&self, filename: PathBuf) -> eyre::Result<()> {
        async_with!(self.context => |ctx| {
            let promise: Promise = ctx.eval_with_options(format!(r#"await import(`{}`)"#, filename.to_str().unwrap()), {
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                options
            })?;
            promise.into_future::<Object>().await?.get("value")
        })
        .await?;
        Ok(())
    }

    pub async fn run_immediate<U: for<'a> FromJs<'a> + Sync + Send + 'static>(
        &self,
        src: &str,
    ) -> eyre::Result<U> {
        cfg_if::cfg_if! {
            if #[cfg(feature = "transpile")] {
                let syntax = infer_transpile_syntax_by_extension(get_best_transpiling())?;
                let (src, _) = self.transpile(
                    src,
                    syntax,
                    IsModule::Bool(true),
                )?;
            }
        }

        Ok(async_with!(self.context => |ctx| {
            let promise: Promise = ctx.eval_with_options(
                src,
                {
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.backtrace_barrier = true;
                    options
                }
            )?;
            promise.into_future::<Object>().await?.get("value")
        })
        .await?)
    }

    #[cfg(feature = "transpile")]
    pub fn transpile(
        &self,
        src: &str,
        syntax: Syntax,
        module: IsModule,
    ) -> eyre::Result<(String, Option<SourceMap>)> {
        self.transpiler.transpile(src, syntax, module, false)
    }

    pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(
        &self,
        src: &str,
    ) -> eyre::Result<U> {
        cfg_if::cfg_if! {
            if #[cfg(feature = "transpile")] {
                let syntax = infer_transpile_syntax_by_extension(get_best_transpiling())?;
                let (src, _) = self.transpile(
                    src,
                    syntax,
                    IsModule::Bool(false),
                )?;
            }
        }

        // // evil hack
        // let src = src.trim_end_matches(";\n");

        Ok(async_with!(self.context => |ctx| {
            let promise: Promise = ctx.eval_with_options(
                src,
                {
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.backtrace_barrier = true;
                    options
                }
            )?;
            promise.into_future::<Object>().await?.get("value")
        })
        .await?)
    }

    pub fn stop(&self) {
        self.stop_token.cancel()
    }

    pub fn stop_token(&self) -> CancellationToken {
        self.stop_token.clone()
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;

    use crate::engine::Engine;

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
            console.log(hello ?? "123")
        "#,
            )
            .await?;
        Ok(())
    }
}
