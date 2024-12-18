use std::path::PathBuf;

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
    den_transpiler_swc::{
        get_best_transpiling, infer_transpile_syntax_by_extension, EasySwcTranspiler,
        EasySwcTranspilerError, IsModule, SourceMap, Syntax,
    },
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
        #[cfg(feature = "transpile")]
        let transpiler = Arc::new(EasySwcTranspiler::default());

        let runtime = AsyncRuntime::new().unwrap();
        runtime.set_max_stack_size(0).await;

        {
            let resolver = (
                {
                    #[allow(unused_mut)]
                    let mut resolver = BuiltinResolver::default();

                    #[cfg(feature = "stdlib-core")]
                    {
                        resolver = resolver.with_module("den:core");
                    }
                    #[cfg(feature = "stdlib-console")]
                    {
                        resolver = resolver.with_module("den:console");
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
                    #[cfg(feature = "stdlib-sqlite")]
                    {
                        resolver = resolver.with_module("den:sqlite");
                    }
                    #[cfg(feature = "stdlib-whatwg-fetch")]
                    {
                        resolver = resolver.with_module("den:whatcg-fetch");
                    }
                    #[cfg(feature = "stdlib-crypto")]
                    {
                        resolver = resolver.with_module("den:crypto");
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
                        loader = loader.with_module("den:core", den_stdlib_core::js_core);
                    }

                    #[cfg(feature = "stdlib-console")]
                    {
                        loader = loader.with_module("den:console", den_stdlib_console::js_console);
                    }

                    #[cfg(feature = "stdlib-networking")]
                    {
                        loader = loader
                            .with_module("den:networking", den_stdlib_networking::js_networking);
                    }

                    #[cfg(feature = "stdlib-text")]
                    {
                        loader = loader.with_module("den:text", den_stdlib_text::js_text);
                    }

                    #[cfg(feature = "stdlib-timer")]
                    {
                        loader = loader.with_module("den:timer", den_stdlib_timer::js_timer);
                    }

                    #[cfg(feature = "stdlib-fs")]
                    {
                        loader = loader.with_module("den:fs", den_stdlib_fs::js_fs);
                    }

                    #[cfg(feature = "stdlib-sqlite")]
                    {
                        loader = loader.with_module("den:sqlite", den_stdlib_sqlite::js_sqlite);
                    }
                    #[cfg(feature = "stdlib-whatwg-fetch")]
                    {
                        loader = loader
                            .with_module("den:whatcg-fetch", den_stdlib_whatwg_fetch::js_whatwg);
                    }
                    #[cfg(feature = "stdlib-crypto")]
                    {
                        loader = loader.with_module("den:crypto", den_stdlib_crypto::js_crypto);
                    }
                    #[cfg(feature = "wasm")]
                    {
                        loader = loader.with_module("den:wasm", den_stdlib_wasm::js_wasm)
                    }
                    loader
                },
                {
                    let builder = HttpLoader::builder();
                    #[cfg(feature = "transpile")]
                    {
                        builder.transpiler(transpiler.clone())
                    }
                    #[cfg(not(feature = "transpile"))]
                    {
                        builder
                    }
                }
                .build(),
                {
                    #[allow(unused_mut)]
                    let mut loader = {
                        let mut builder = MmapScriptLoader::builder();
                        #[cfg(feature = "transpile")]
                        {
                            builder.transpiler(transpiler.clone())
                        }
                        #[cfg(not(feature = "transpile"))]
                        {
                            builder
                        }
                    }
                    .build();

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

        let context = AsyncContext::full(&runtime).await.unwrap();

        context
            .with(|ctx| {
                #[cfg(feature = "stdlib-console")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_console::js_console, _>(
                        ctx.clone(),
                        "den:console",
                    )?;
                }

                #[cfg(feature = "stdlib-core")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_core::js_core, _>(
                        ctx.clone(),
                        "den:core",
                    )?;
                }

                #[cfg(feature = "stdlib-text")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_text::js_text, _>(
                        ctx.clone(),
                        "den:text",
                    )?;
                }

                #[cfg(feature = "stdlib-timer")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_timer::js_timer, _>(
                        ctx.clone(),
                        "den:timer",
                    )?;
                }

                #[cfg(feature = "stdlib-whatwg-fetch")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_whatwg_fetch::js_whatwg, _>(
                        ctx.clone(),
                        "den:whatwg-fetch",
                    )?;
                }

                #[cfg(feature = "stdlib-crypto")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_crypto::js_crypto, _>(
                        ctx.clone(),
                        "den:crypto",
                    )?;
                }

                #[cfg(feature = "wasm")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_wasm::js_wasm, _>(
                        ctx.clone(),
                        "den:wasm",
                    )?;
                }

                Ok::<_, rquickjs::Error>(())
            })
            .await
            .unwrap();

        Self {
            #[cfg(feature = "transpile")]
            transpiler,
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
            let src = format!(r#"await import(`{}`)"#, filename.to_str().unwrap());
            ctx.eval_with_options::<Promise, _>(src, {
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
                let syntax = infer_transpile_syntax_by_extension(get_best_transpiling()).unwrap_or_default();
                let (src, _) = self.transpile(
                    src,
                    syntax,
                    IsModule::Unknown,
                )?;
            }
        }

        Ok(async_with!(self.context => |ctx| {
            ctx.eval_with_options::<Promise, _>(src, {
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                options
            })?.into_future::<Object>().await?.get("value")
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
    InferTranspileSyntaxError(den_transpiler_swc::InferTranspileSyntaxError),
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
