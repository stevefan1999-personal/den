use std::path::PathBuf;

use den_stdlib_console::Console;
use den_stdlib_core::{js_core, CancellationTokenWrapper};
use den_stdlib_fs::js_fs;
use den_stdlib_networking::js_networking;
use den_stdlib_text::js_text;
use den_stdlib_timer::js_timer;
#[cfg(feature = "wasm")]
use den_stdlib_wasm::js_wasm;
use den_utils::FutureExt;
use derive_more::{Debug, Display, Error, From};
use rquickjs::{
    async_with,
    context::EvalOptions,
    loader::{BuiltinLoader, BuiltinResolver, FileResolver, ModuleLoader},
    AsyncContext, AsyncRuntime, FromJs, Module, Object, Promise,
};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "transpile")]
use {
    den_transpiler_swc::swc_core::{
        base::{config::IsModule, sourcemap::SourceMap},
        ecma::parser::Syntax,
    },
    den_transpiler_swc::{EasySwcTranspiler, EasySwcTranspilerError},
    den_utils::{get_best_transpiling, infer_transpile_syntax_by_extension},
    std::sync::Arc,
};

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::http::HttpResolver,
};

#[derive(Clone)]
pub struct Engine {
    #[cfg(feature = "transpile")]
    pub transpiler: Arc<EasySwcTranspiler>,
    pub runtime:    AsyncRuntime,
    pub context:    AsyncContext,
    pub stop_token: CancellationToken,
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
                let world_end = stop_token.child_token();
                Some(Box::new(move || world_end.is_cancelled()))
            })
            .await;

        tokio::spawn(runtime.drive().with_cancellation(&stop_token.child_token()));

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

    pub async fn run_file<U: for<'a> FromJs<'a> + Sync + Send + 'static>(
        &self,
        filename: PathBuf,
    ) -> Result<U, EngineError> {
        Ok(async_with!(self.context => |ctx| {
            // Evil hack by using top-level await, so that the eval will transfer the import to our file resolver
            // then we can use it to transpile Typescript and other stuff
            // However, this is the problem because rather than returning the underlying value, 
            // the implementation of QuickJS decided to make this a {"value": <TLA evaluation value>}
            // so we have to directly fetch the "value" key and so we can transmigrate within
            // Technically we can do an optimization to just run the future and discard the returned value,
            // since we run under an assumption of running this function on a file
            // However, with REPL continuation, things could change
            ctx.eval_with_options::<Promise, _>(format!(r#"await import(`{}`)"#, filename.to_str().unwrap()), {
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                options
            })?.into_future::<Object>().await?.get("value")
        })
        .await?)
    }

    #[cfg(feature = "transpile")]
    pub fn transpile(
        &self,
        src: &str,
        syntax: Syntax,
        module: IsModule,
    ) -> Result<(String, Option<SourceMap>), EasySwcTranspilerError> {
        self.transpiler.transpile(src, syntax, module, false)
    }

    pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(
        &self,
        src: &str,
    ) -> Result<U, EngineError> {
        cfg_if::cfg_if! {
            if #[cfg(feature = "transpile")] {
                let syntax = infer_transpile_syntax_by_extension(get_best_transpiling())?;
                let (src, _) = self.transpile(
                    src,
                    syntax,
                    IsModule::Unknown,
                )?;
            }
        }

        // // evil hack
        // let src = src.trim_end_matches(";\n");

        Ok(async_with!(self.context => |ctx| {
            ctx.eval_with_options::<Promise, _>(
                src,
                {
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    options.backtrace_barrier = true;
                    options
                }
            )?.into_future::<Object>().await?.get("value")
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

#[derive(Display, From, Error, Debug)]
pub enum EngineError {
    #[cfg(feature = "transpile")]
    #[from]
    EasySwcTranspiler(EasySwcTranspilerError),
    #[from]
    Rquickjs(rquickjs::Error),
    #[cfg(feature = "transpile")]
    #[from]
    InferTranspileSyntaxError(den_utils::InferTranspileSyntaxError),
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;

    use crate::engine::Engine;

    #[tokio::test(flavor = "multi_thread")]
    async fn my_test() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(
                r#"
            console.log("hello world")
        "#,
            )
            .await?;
        assert_eq!(engine.eval::<String>(r#"null ?? "123""#).await?, "123");
        assert_eq!(engine.eval::<usize>(r#"null ?? 123"#).await?, 123);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn my_test2() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(
                r#"
            export const hello = "world"
        "#,
            )
            .await?;
        Ok(())
    }
}
