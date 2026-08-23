#[cfg(any(feature = "stdlib-worker", feature = "transpile"))]
use std::sync::Arc;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::{Path, PathBuf},
};

#[cfg(feature = "transpile")]
use den_transpiler_oxc::{
    EasyOxcTranspiler, EasyOxcTranspilerError, IsModule, SourceMap, Syntax, get_best_transpiling,
    infer_transpile_syntax_by_extension,
};
use derive_more::{Debug, Display, Error, From};
use rquickjs::{
    AsyncContext, AsyncRuntime, Coerced, Ctx, FromJs, JsLifetime, Module, Object, Persistent,
    Promise, Value,
    context::EvalOptions,
    function::{Constructor, This},
    loader::{BuiltinLoader, BuiltinResolver, FileResolver, ModuleLoader},
    runtime::UserDataError,
};
use tokio::task::yield_now;
use tokio_util::sync::CancellationToken;
use url::Url;
#[cfg(feature = "stdlib-worker")]
use {
    den_stdlib_worker::{BaseUrl, WorkerEngine, WorkerHost, WorkerHostError},
    tokio::{runtime::Handle, task::block_in_place},
};

use crate::{
    loader::{http::HttpLoader, mmap_script::MmapScriptLoader},
    resolver::{
        file::AbsolutePathResolver,
        http::HttpResolver,
        import_map::{ImportMap, ImportMapError, ImportMapResolver},
    },
};

/// A rejection: the promise, and the value it rejected with. Both halves are
/// kept alive with `Persistent`, because identity is what everything here
/// matches on — the promise's for a retraction, the reason's for the duplicate
/// reports one module evaluation makes.
type Rejection = (Persistent<Value<'static>>, Persistent<Value<'static>>);

/// Promise rejections seen with no handler attached, waiting for the microtask
/// checkpoint that decides whether they were really unhandled.
///
/// QuickJS reports a rejection the moment it happens and reports it a second
/// time — as *handled* — if a `catch` is attached later, so a report has to be
/// deferred by one checkpoint or every `const p = Promise.reject(x);
/// p.catch(…)` would be a false positive.
#[derive(Default)]
struct PendingRejections {
    unhandled: RefCell<Vec<Rejection>>,
    /// The reasons of the rejections retracted during the current turn.
    ///
    /// A module body is called as an async function
    /// (`js_execute_sync_module`, quickjs.c), and when it throws, QuickJS reads
    /// the reason straight off that promise with `JS_PromiseResult` and frees
    /// it — without ever attaching a handler. The tracker therefore sees a
    /// rejection for a promise no script can reach, carrying the very same
    /// reason as the module's real promise, which *is* handled a few lines
    /// later. Suppressing a reason that something claimed in the same turn is
    /// what keeps `den main.js`, for a `main.js` that throws, from printing the
    /// failure twice.
    claimed: RefCell<Vec<Persistent<Value<'static>>>>,
    /// Rejections already reported, so that a handler attached to one *later*
    /// can still fire `rejectionhandled` (HTML §8.1.7.5).
    outstanding: RefCell<VecDeque<Rejection>>,
    /// How many rejections have been printed so far. Printing goes to stderr,
    /// which a test inside this process cannot read; this counter is its only
    /// seam on the decision the tracker actually makes.
    reported: Cell<usize>,
}

// SAFETY: `PendingRejections` borrows no `'js` lifetime — a `Persistent` owns
// its value outright and is tied to the runtime, not to a scope — so the type
// is the same type for every choice of `'to`.
unsafe impl<'js> JsLifetime<'js> for PendingRejections {
    type Changed<'to> = PendingRejections;
}

/// den-core's side of the worker crate's engine seam. A worker thread asks for
/// an engine and gets the very same one the main script runs on — same loaders,
/// same stdlib, same `den:worker` — differing only in its base URL and in the
/// cancellation token that stops it.
#[cfg(feature = "stdlib-worker")]
struct DenWorkerHost;

#[cfg(feature = "stdlib-worker")]
impl WorkerHost for DenWorkerHost {
    fn build_engine(
        &self,
        stop: CancellationToken,
        base: BaseUrl,
    ) -> Result<WorkerEngine, WorkerHostError> {
        // Called on the worker's own OS thread, inside that thread's
        // multi-threaded runtime: `block_in_place` + `block_on` is what lets a
        // synchronous trait method reach an async constructor, and it is the
        // same pair den's module loaders already use one layer down.
        let engine = block_in_place(|| {
            Handle::current().block_on(async move {
                let engine = Engine::new_with_stop_token(stop).await;
                engine.set_base_url(base).await.map(|()| engine)
            })
        })
        .map_err(|error| WorkerHostError(error.to_string()))?;

        Ok(WorkerEngine {
            runtime: engine.runtime,
            context: engine.context,
        })
    }
}

#[derive(Clone)]
pub struct Engine {
    #[cfg(feature = "transpile")]
    pub transpiler: Arc<EasyOxcTranspiler>,
    pub runtime: AsyncRuntime,
    pub context: AsyncContext,
    pub stop_token: CancellationToken,
}

#[allow(dead_code)]
impl Engine {
    /// What a script with no file of its own is called.
    const EVAL_SCRIPT_NAME: &'static str = "<eval>";
    /// How many reported rejections stay remembered, so that a handler attached
    /// to one afterwards still fires `rejectionhandled`.
    ///
    /// ponytail: a fixed ring. `Persistent` is a strong reference, so
    /// remembering every rejection a long-running process ever reported would
    /// pin them all; a weak handle — which rquickjs does not expose — is the
    /// upgrade path.
    const OUTSTANDING_REJECTIONS: usize = 64;
    /// The file names a specifier without an extension may stand for. Both
    /// resolvers get the list: which side of the filesystem root a script sits
    /// on is no reason for `import "./lib"` to mean something different.
    const SCRIPT_PATTERNS: &'static [&'static str] = &[
        "{}.js",
        "{}.mjs",
        #[cfg(feature = "react")]
        "{}.jsx",
        #[cfg(feature = "react")]
        "{}.mjsx",
        #[cfg(feature = "typescript")]
        "{}.ts",
        #[cfg(all(feature = "typescript", feature = "react"))]
        "{}.tsx",
    ];

    pub async fn new() -> Engine {
        Self::new_with_stop_token(CancellationToken::new()).await
    }

    /// Build an engine whose interrupt handler observes `stop_token`.
    ///
    /// Handing the token in is what lets a worker thread be stopped by its
    /// parent: the worker's token is a child of the parent's, so one
    /// `cancel` — Ctrl-C, or `worker.terminate()` — reaches a script that is
    /// already running.
    pub async fn new_with_stop_token(stop_token: CancellationToken) -> Engine {
        #[cfg(feature = "transpile")]
        let transpiler = Arc::new(EasyOxcTranspiler);

        let runtime = AsyncRuntime::new().unwrap();
        runtime.set_max_stack_size(0).await;

        {
            let resolver = (
                ImportMapResolver,
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
                        resolver = resolver.with_module("den:whatwg-fetch");
                    }
                    #[cfg(feature = "stdlib-crypto")]
                    {
                        resolver = resolver.with_module("den:crypto");
                    }
                    #[cfg(feature = "stdlib-process")]
                    {
                        resolver = resolver.with_module("den:process");
                    }
                    #[cfg(any(feature = "wasm-wasmtime", feature = "wasm-wasmi"))]
                    {
                        resolver = resolver.with_module("den:wasm");
                    }
                    #[cfg(feature = "stdlib-worker")]
                    {
                        resolver = resolver.with_module("den:worker");
                    }
                    #[cfg(feature = "stdlib-whatwg")]
                    {
                        resolver = resolver.with_module("den:whatwg");
                    }
                    resolver
                },
                HttpResolver::default(),
                // Ahead of `FileResolver`, which reads every name as relative
                // to the working directory and so can answer neither for an
                // absolute path nor for anything imported from one.
                AbsolutePathResolver::new(Self::SCRIPT_PATTERNS.iter().copied()),
                Self::SCRIPT_PATTERNS.iter().copied().fold(
                    FileResolver::default().with_path("./"),
                    FileResolver::with_pattern,
                ),
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
                            .with_module("den:whatwg-fetch", den_stdlib_whatwg_fetch::js_whatwg);
                    }
                    #[cfg(feature = "stdlib-crypto")]
                    {
                        loader = loader.with_module("den:crypto", den_stdlib_crypto::js_crypto);
                    }
                    #[cfg(feature = "stdlib-process")]
                    {
                        loader = loader.with_module("den:process", den_stdlib_process::js_process);
                    }
                    #[cfg(any(feature = "wasm-wasmtime", feature = "wasm-wasmi"))]
                    {
                        loader = loader.with_module("den:wasm", den_stdlib_wasm::js_wasm)
                    }
                    #[cfg(feature = "stdlib-worker")]
                    {
                        loader = loader.with_module("den:worker", den_stdlib_worker::js_worker);
                    }
                    #[cfg(feature = "stdlib-whatwg")]
                    {
                        loader = loader.with_module("den:whatwg", den_stdlib_whatwg::js_whatwg);
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

        runtime
            .set_interrupt_handler({
                let world_end = stop_token.clone();
                Some(Box::new(move || world_end.is_cancelled()))
            })
            .await;

        runtime
            .set_host_promise_rejection_tracker(Some(Box::new(Self::track_rejection)))
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
                    // Stored before the module evaluates so a timer armed during
                    // install (none today) would still observe Ctrl-C. The
                    // interrupt handler reads the same token.
                    Self::store_userdata(&ctx, den_stdlib_timer::StopToken(stop_token.clone()))?;
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

                #[cfg(feature = "stdlib-process")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_process::js_process, _>(
                        ctx.clone(),
                        "den:process",
                    )?;
                }

                #[cfg(any(feature = "wasm-wasmtime", feature = "wasm-wasmi"))]
                {
                    let _ = Module::evaluate_def::<den_stdlib_wasm::js_wasm, _>(
                        ctx.clone(),
                        "den:wasm",
                    )?;
                }

                #[cfg(feature = "stdlib-worker")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_worker::js_worker, _>(
                        ctx.clone(),
                        "den:worker",
                    )?;

                    // Every context gets the host and a base URL, worker
                    // contexts included: that — and nothing else — is what
                    // makes a worker able to spawn workers of its own.
                    Self::store_userdata(
                        &ctx,
                        den_stdlib_worker::HostHandle(Arc::new(DenWorkerHost)),
                    )?;
                    Self::store_userdata(&ctx, Self::working_directory_url())?;
                    // The realm's own stop token, which every worker spawned
                    // here takes a child of. Without it a top-level worker's
                    // token is a fresh root and `Engine::stop()` — which is
                    // documented to reach workers — would leave it running.
                    Self::store_userdata(&ctx, den_stdlib_worker::RealmStop(stop_token.clone()))?;
                }

                // After `den:worker` so FileReader / XHR / EventSource / WebSocket
                // can extend EventTarget. Fetch is already wired above.
                #[cfg(feature = "stdlib-whatwg")]
                {
                    let _ = Module::evaluate_def::<den_stdlib_whatwg::js_whatwg, _>(
                        ctx.clone(),
                        "den:whatwg",
                    )?;
                }

                Self::store_userdata(&ctx, PendingRejections::default())?;

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
        // `den file:///home/me/app.js` is the same request as
        // `den /home/me/app.js`, and the URL shape is what a worker's module
        // specifier arrives as.
        let from_url = Url::parse(&filename.to_string_lossy())
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|url| url.to_file_path().ok());

        // The entry point is made absolute up front: it is what lets the realm
        // name a base URL and a script name that agree with each other, and
        // `AbsolutePathResolver` is what makes the import below find it.
        let path = from_url.unwrap_or(filename);
        let path = path.canonicalize().unwrap_or(path);
        let file_url = Url::from_file_path(&path).ok();

        #[cfg(feature = "stdlib-worker")]
        if let Some(directory) = path
            .parent()
            .and_then(|directory| Url::from_directory_path(directory).ok())
        {
            self.set_base_url(BaseUrl(directory.into())).await?;
        }

        // A backslash starts an escape sequence inside a template literal, and
        // a Windows path is made of them; every file API here takes `/` too.
        let specifier = path.to_string_lossy().replace('\\', "/");
        let script_name = file_url.map_or_else(|| specifier.clone(), String::from);

        Ok(self
            .context
            .async_with(async |ctx| {
                // Evil hack by using top-level await, so that the eval will transfer the import
                // to our file resolver then we can use it to transpile
                // Typescript and other stuff However, this is the problem
                // because rather than returning the underlying value,
                // the implementation of QuickJS decided to make this a {"value": <TLA
                // evaluation value>} so we have to directly fetch the "value"
                // key and so we can transmigrate within Technically we can do
                // an optimization to just run the future and discard the returned value,
                // since we run under an assumption of running this function on a file
                // However, with REPL continuation, things could change
                let src = format!(r#"await import(`{specifier}`)"#);
                ctx.eval_with_options::<Promise, _>(src, {
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    // Without a name of its own this wrapper — and every stack
                    // trace and `script_or_module_name` under it — is called
                    // `eval_script`.
                    options.filename = Some(script_name);
                    options
                })?
                .into_future::<Object>()
                .await?
                .get("value")
            })
            .await?)
    }

    #[cfg(feature = "transpile")]
    pub fn transpile(
        &self,
        src: &str,
        syntax: Syntax,
        module: IsModule,
    ) -> Result<(String, Option<SourceMap>), EasyOxcTranspilerError> {
        self.transpiler.transpile(src, syntax, module, false)
    }

    /// Transpile a REPL/eval snippet when this engine was built with the
    /// transpiler; otherwise the source is used as-is. Independent of the
    /// runtime lock, so a `ctx.spawn`ed pump can prepare a line without
    /// waiting for `idle()`.
    pub fn prepare_eval_source(&self, src: &str) -> Result<String, EngineError> {
        cfg_if::cfg_if! {
            if #[cfg(feature = "transpile")] {
                let syntax = infer_transpile_syntax_by_extension(get_best_transpiling()).unwrap_or_default();
                let (src, _) = self.transpile(src, syntax, IsModule::Unknown)?;
                Ok(src)
            } else {
                let _ = self;
                Ok(src.to_owned())
            }
        }
    }

    /// Evaluate an already-prepared snippet on a context the caller is holding.
    ///
    /// Used by `eval` (under `async_with`) and by the REPL pump (`ctx.spawn`
    /// while `idle()` holds the runtime lock). A separate tokio task calling
    /// `async_with` during `idle()` would park on the mutex until idle
    /// returned.
    pub async fn eval_prepared<'js, U: FromJs<'js>>(
        ctx: Ctx<'js>,
        src: &str,
    ) -> rquickjs::Result<U> {
        ctx.eval_with_options::<Promise, _>(src, {
            let mut options = EvalOptions::default();
            options.global = true;
            options.promise = true;
            options.strict = true;
            // A REPL line has no file; naming it after one would be a
            // lie the resolver could act on, so it gets a name no URL
            // parser accepts.
            options.filename = Some(Self::EVAL_SCRIPT_NAME.to_owned());
            options
        })?
        .into_future::<Object>()
        .await?
        .get("value")
    }

    pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(
        &self,
        src: &str,
    ) -> Result<U, EngineError> {
        let src = self.prepare_eval_source(src)?;
        Ok(self
            .context
            .async_with(async |ctx| Self::eval_prepared(ctx, &src).await)
            .await?)
    }

    /// Stop and reap every worker this realm spawned.
    ///
    /// Cancelling is the half den-core owns: it reaches a main script that is
    /// still running, wherever it is. The other half — interrupting each
    /// worker and joining its thread, which a parked worker needs because it
    /// has nothing to interrupt — is `den_stdlib_worker`'s worker registry.
    pub async fn shutdown(&self) {
        self.stop_token.cancel();
        #[cfg(feature = "stdlib-worker")]
        den_stdlib_worker::worker::shutdown(&self.context).await;
    }

    /// Install an [import map](https://wicg.github.io/import-maps/) on this
    /// realm. Relative `./` / `../` targets join against `base_dir`. Calling
    /// again replaces the previous map.
    pub async fn set_import_map(
        &self,
        json: &str,
        base_dir: impl AsRef<Path> + Send,
    ) -> Result<(), EngineError> {
        let map = ImportMap::parse(json, base_dir.as_ref())?;
        self.context
            .with(move |ctx| Self::store_userdata(&ctx, map))
            .await?;
        Ok(())
    }

    /// Point the realm's relative script URLs at `base`.
    #[cfg(feature = "stdlib-worker")]
    async fn set_base_url(&self, base: BaseUrl) -> rquickjs::Result<()> {
        self.context
            .with(|ctx| Self::store_userdata(&ctx, base))
            .await
    }

    /// The base URL a realm starts with. A REPL line, an `eval` and a worker
    /// script named relatively all resolve against the process' working
    /// directory; when there is no such directory to name, the realm gets an
    /// empty base and every relative URL fails to resolve, which is the honest
    /// answer rather than a guess.
    #[cfg(feature = "stdlib-worker")]
    fn working_directory_url() -> BaseUrl {
        BaseUrl(
            std::env::current_dir()
                .ok()
                .and_then(|directory| Url::from_directory_path(directory).ok())
                .map(String::from)
                .unwrap_or_default(),
        )
    }

    /// Store `data` as context userdata, reporting the "somebody is holding a
    /// guard" refusal as an error instead of dropping the value on the floor.
    fn store_userdata<'js, U>(ctx: &Ctx<'js>, data: U) -> rquickjs::Result<()>
    where
        U: JsLifetime<'js>,
        U::Changed<'static>: std::any::Any,
    {
        ctx.store_userdata(data)
            .map(|_| ())
            .map_err(|_| rquickjs::Error::UserData(UserDataError(())))
    }

    /// QuickJS' promise rejection tracker: `handled` is false when a promise
    /// rejects with nothing attached to it, and true when something is attached
    /// afterwards.
    ///
    /// It captures nothing on purpose. The tracker is `Send`, outlives every
    /// context and is shared by all of them, while everything it touches is per
    /// context — so it reaches its state through the `Ctx` it is handed.
    fn track_rejection<'js>(ctx: Ctx<'js>, promise: Value<'js>, reason: Value<'js>, handled: bool) {
        let Some(pending) = ctx.userdata::<PendingRejections>() else {
            return;
        };
        let rejected = Persistent::save(&ctx, promise.clone());
        if !handled {
            let Ok(mut unhandled) = pending.unhandled.try_borrow_mut() else {
                return;
            };
            unhandled.push((rejected, Persistent::save(&ctx, reason)));
            drop(unhandled);
            drop(pending);
            return Self::schedule_checkpoint(&ctx);
        }

        // A handler arrived. Whether that retracts a report or answers one
        // depends on whether the checkpoint has already been through: before
        // it, the rejection simply never happened as far as the realm is
        // concerned; after it, the realm was told, and has to be told again.
        if let Ok(mut unhandled) = pending.unhandled.try_borrow_mut() {
            unhandled.retain(|(promise, _)| *promise != rejected);
        }
        if let Ok(mut claimed) = pending.claimed.try_borrow_mut() {
            claimed.push(Persistent::save(&ctx, reason.clone()));
        }
        let was_reported = pending
            .outstanding
            .try_borrow_mut()
            .map(|mut outstanding| {
                let before = outstanding.len();
                outstanding.retain(|(promise, _)| *promise != rejected);
                outstanding.len() != before
            })
            .unwrap_or(false);
        drop(pending);

        if was_reported {
            Self::fire_rejection_event(&ctx, "rejectionhandled", &promise, &reason, false);
        }
        // Even a pure retraction needs the checkpoint: it is what drains
        // `claimed`, which is a record of this turn only.
        Self::schedule_checkpoint(&ctx);
    }

    /// Let the realm settle, then decide. `idle()` and `drive()` both drain
    /// every pending job before they poll a spawned future, so by the time this
    /// wakes up, a handler attached later in the same turn has already
    /// retracted its rejection.
    fn schedule_checkpoint(ctx: &Ctx<'_>) {
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                yield_now().await;
                Self::report_unhandled_rejections(&ctx);
            }
        });
    }

    /// Offer every rejection still unclaimed at the checkpoint to the realm as
    /// an `unhandledrejection` event, print what the realm did not cancel, and
    /// forget the rest.
    fn report_unhandled_rejections(ctx: &Ctx<'_>) {
        let Some(pending) = ctx.userdata::<PendingRejections>() else {
            return;
        };
        let (Ok(mut queue), Ok(mut claims)) = (
            pending.unhandled.try_borrow_mut(),
            pending.claimed.try_borrow_mut(),
        ) else {
            return;
        };
        let unclaimed = core::mem::take(&mut *queue);
        let claimed = core::mem::take(&mut *claims);
        drop((queue, claims));
        drop(pending);

        // One report per *reason*, not per promise: QuickJS rejects two
        // promises with the same value for one failed module evaluation (see
        // `PendingRejections::claimed`), and the realm has no way to tell them
        // apart either.
        let mut seen: Vec<Persistent<Value<'static>>> = Vec::new();
        for (promise, reason) in unclaimed {
            if claimed.contains(&reason) || seen.contains(&reason) {
                continue;
            }
            seen.push(reason.clone());
            let (Ok(rejected), Ok(value)) =
                (promise.clone().restore(ctx), reason.clone().restore(ctx))
            else {
                continue;
            };
            if Self::fire_rejection_event(ctx, "unhandledrejection", &rejected, &value, true) {
                continue;
            }
            eprintln!("Uncaught (in promise) {}", Self::describe(&value));
            Self::remember_reported(ctx, (promise, reason));
        }
    }

    /// Keep a reported rejection around for a late `rejectionhandled`, oldest
    /// first out of the ring.
    fn remember_reported(ctx: &Ctx<'_>, rejection: Rejection) {
        let Some(pending) = ctx.userdata::<PendingRejections>() else {
            return;
        };
        pending.reported.set(pending.reported.get() + 1);
        if let Ok(mut outstanding) = pending.outstanding.try_borrow_mut() {
            while outstanding.len() >= Self::OUTSTANDING_REJECTIONS {
                outstanding.pop_front();
            }
            outstanding.push_back(rejection);
        }
    }

    /// Fire `kind` — `unhandledrejection` or `rejectionhandled` (HTML §8.1.7.5)
    /// — at the realm's global, and say whether a listener cancelled it.
    ///
    /// A realm whose global is not an `EventTarget`, or that has no
    /// `PromiseRejectionEvent` to construct, cancels nothing and the rejection
    /// goes to stderr as it always did. That is den's main realm today: only a
    /// worker global is an event target, so this is the seam the JS side plugs
    /// into rather than a promise that it already works everywhere.
    fn fire_rejection_event<'js>(
        ctx: &Ctx<'js>,
        kind: &str,
        promise: &Value<'js>,
        reason: &Value<'js>,
        cancelable: bool,
    ) -> bool {
        let globals = ctx.globals();
        let dispatched = (|| -> rquickjs::Result<bool> {
            let constructor = globals.get::<_, Constructor<'js>>("PromiseRejectionEvent")?;
            let dispatch = globals.get::<_, rquickjs::Function<'js>>("dispatchEvent")?;
            let init = Object::new(ctx.clone())?;
            init.set("promise", promise.clone())?;
            init.set("reason", reason.clone())?;
            init.set("cancelable", cancelable)?;
            let event = constructor.construct::<_, Value<'js>>((kind, init))?;
            // DOM: an event the runtime fires is trusted. `dispatchTrusted` is
            // the marker for that and it is deliberately not on `globalThis`,
            // so that no script can forge one; `den:worker`'s natives bag,
            // which doubles as the realm's exception sink, is how a host-side
            // dispatcher reaches it. Without that crate there is no bag and no
            // marker, so the plain dispatch is all there is.
            #[cfg(feature = "stdlib-worker")]
            if let Some(trusted) = den_stdlib_worker::report::sink_hook(ctx, "dispatchTrusted") {
                return trusted.call((globals.clone(), event));
            }
            dispatch.call((This(globals.clone()), event))
        })();
        match dispatched {
            // `dispatchEvent` answers false exactly when `preventDefault()` was
            // called, which is how a realm says it has handled the rejection.
            Ok(completed) => !completed,
            // Reporting must not itself throw, and a pending exception left
            // behind here would surface at the next unrelated call into this
            // context.
            Err(rquickjs::Error::Exception) => {
                eprintln!("{}", Self::describe(&ctx.catch()));
                false
            }
            Err(_) => false,
        }
    }

    /// How den renders a value nobody caught: an exception prints itself,
    /// message and stack included; anything else is coerced to a string.
    fn describe(value: &Value<'_>) -> String {
        value
            .as_exception()
            .map(|exception| exception.to_string())
            .or_else(|| {
                value
                    .get::<Coerced<String>>()
                    .ok()
                    .map(|Coerced(text)| text)
            })
            .unwrap_or_else(|| "unknown error".to_owned())
    }

    /// Interrupt this realm, and with it every worker it spawned: a worker's
    /// token is a child of the one this realm publishes as `RealmStop`, all the
    /// way down the tree. Reaping the threads afterwards is [`Self::shutdown`].
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
    EasyOxcTranspiler(EasyOxcTranspilerError),
    #[from]
    Rquickjs(rquickjs::Error),
    #[from]
    ImportMap(ImportMapError),
    #[cfg(feature = "transpile")]
    #[from]
    InferTranspileSyntaxError(den_transpiler_oxc::InferTranspileSyntaxError),
}

#[cfg(test)]
mod tests {
    use std::{env::temp_dir, fs, path::PathBuf, process};

    use color_eyre::eyre;

    use crate::engine::{Engine, EngineError, PendingRejections};

    /// A script written outside the working directory, so that running it
    /// proves the absolute path was resolved rather than joined onto `./`.
    /// Named after the test and the process so that two of them never collide.
    fn write_script(name: &str, source: &str) -> PathBuf {
        let path = temp_dir().join(format!("den-engine-{}-{name}.js", process::id()));
        fs::write(&path, source).expect("the temporary directory is writable");
        path
    }

    /// Let every spawned task run to a standstill, then ask the realm how many
    /// unhandled rejections it decided to report. Reporting itself goes to
    /// stderr, which this process cannot read back.
    async fn reported_rejections(engine: &Engine) -> usize {
        engine.runtime.idle().await;
        engine
            .context
            .with(|ctx| {
                ctx.userdata::<PendingRejections>()
                    .map_or(0, |pending| pending.reported.get())
            })
            .await
    }

    /// The other half of the absolute entry point: everything it imports is
    /// named relative to *it*, extension optional, exactly as it would be for
    /// an entry point named relative to the working directory.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_absolute_entry_point_resolves_its_relative_siblings() -> eyre::Result<()> {
        let library = write_script("sibling-lib", "export const answer = 42;\n");
        let stem = library
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("the fixture name is text")
            .to_owned();
        let entry = write_script(
            "sibling-main",
            &format!(
                "import {{ answer }} from \"./{stem}\";\nglobalThis.siblingAnswer = answer;\n"
            ),
        );

        let engine = Engine::new().await;
        engine.run_file::<()>(entry.clone()).await?;
        assert_eq!(engine.eval::<usize>("globalThis.siblingAnswer").await?, 42);

        fs::remove_file(entry)?;
        fs::remove_file(library)?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_file_accepts_an_absolute_path() -> eyre::Result<()> {
        let path = write_script("absolute", "globalThis.absoluteRan = 7;\n");
        let engine = Engine::new().await;
        engine.run_file::<()>(path.clone()).await?;
        assert_eq!(engine.eval::<usize>("globalThis.absoluteRan").await?, 7);
        fs::remove_file(path)?;
        Ok(())
    }

    /// A worker takes a child of the token its spawning realm published, and
    /// `new Worker` in the main script is spawning from *this* realm — so
    /// without the token in the context userdata, a top-level worker's token is
    /// a fresh root and `stop()` never reaches it.
    #[cfg(feature = "stdlib-worker")]
    #[tokio::test(flavor = "multi_thread")]
    async fn stop_reaches_the_workers_the_main_realm_spawns() -> eyre::Result<()> {
        use den_stdlib_worker::RealmStop;

        let engine = Engine::new().await;
        let published = engine
            .context
            .with(|ctx| ctx.userdata::<RealmStop>().map(|realm| realm.0.clone()))
            .await
            .expect("the main realm publishes its stop token");

        assert!(!published.is_cancelled());
        engine.stop();
        assert!(published.is_cancelled());
        Ok(())
    }

    /// The base URL is what `new Worker("./child.js")` resolves against, so it
    /// has to follow the entry point rather than the working directory.
    #[cfg(feature = "stdlib-worker")]
    #[tokio::test(flavor = "multi_thread")]
    async fn run_file_points_the_base_url_at_the_entry_points_directory() -> eyre::Result<()> {
        use den_stdlib_worker::BaseUrl;
        use url::Url;

        let path = write_script("base-url", "globalThis.baseUrlRan = true;\n");
        let engine = Engine::new().await;
        engine.run_file::<()>(path.clone()).await?;

        let directory = path.canonicalize()?;
        let directory = directory.parent().expect("a file has a parent").to_owned();
        let expected = Url::from_directory_path(directory).expect("an absolute directory");
        let actual = engine
            .context
            .with(|ctx| ctx.userdata::<BaseUrl>().map(|base| base.0.clone()))
            .await;

        assert_eq!(actual.as_deref(), Some(expected.as_str()));
        fs::remove_file(path)?;
        Ok(())
    }

    /// How long a worker's error is given to climb back to its parent before
    /// the test calls the chain broken. Generous: it is a thread spawn, an
    /// engine build and a module load, on whatever the CI box is doing.
    #[cfg(all(feature = "stdlib-worker", feature = "stdlib-timer"))]
    const WORKER_FAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// A realm that wants a say in unhandled rejections needs two things on
    /// its global: something to construct the event with, and somewhere to
    /// dispatch it. A worker global has both; den's main realm has neither, so
    /// the tests that want the event stand them up by hand — which is also the
    /// shape the JS side has to provide (see `notFixed`).
    const REJECTION_HARNESS: &str = r#"
        globalThis.seen = [];
        globalThis.PromiseRejectionEvent = class PromiseRejectionEvent {
          constructor(type, init) {
            this.type = type;
            this.promise = init.promise;
            this.reason = init.reason;
            this.cancelable = !!init.cancelable;
            this.defaultPrevented = false;
          }
          preventDefault() { if (this.cancelable) this.defaultPrevented = true }
        };
        globalThis.dispatchEvent = (event) => {
          globalThis.seen.push(`${event.type}:${event.reason.message}`);
          if (globalThis.claim) event.preventDefault();
          return !event.defaultPrevented;
        };
    "#;

    /// An uncaught error in the main script used to print twice: once from
    /// `main.rs`, which is handed the failure, and once from the rejection
    /// tracker, which sees the promise QuickJS rejects for the module body and
    /// then frees without ever attaching a handler to it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_top_level_throw_is_not_also_an_unhandled_rejection() -> eyre::Result<()> {
        let path = write_script("top-level-throw", "throw new Error('boom');\n");
        let engine = Engine::new().await;
        let outcome = engine.run_file::<()>(path.clone()).await;

        assert!(matches!(outcome, Err(EngineError::Rquickjs(_))));
        assert_eq!(reported_rejections(&engine).await, 0);
        fs::remove_file(path)?;
        Ok(())
    }

    /// The other direction of the same fix: suppressing the module's own
    /// duplicate must not suppress a rejection the script really did leave
    /// lying around.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_rejection_the_entry_point_leaves_behind_is_still_reported() -> eyre::Result<()> {
        let path = write_script(
            "entry-point-rejection",
            "Promise.reject(new Error('nobody claims this'));\nexport const ran = true;\n",
        );
        let engine = Engine::new().await;
        engine.run_file::<()>(path.clone()).await?;

        assert_eq!(reported_rejections(&engine).await, 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_realm_that_cancels_unhandledrejection_stops_the_report() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(&format!(
                "{REJECTION_HARNESS}
                 globalThis.claim = true;
                 Promise.reject(new Error('claimed by the realm'));
                 undefined;"
            ))
            .await?;

        assert_eq!(reported_rejections(&engine).await, 0);
        assert_eq!(
            engine.eval::<String>("globalThis.seen.join(',')").await?,
            "unhandledrejection:claimed by the realm"
        );
        Ok(())
    }

    /// And a realm that hears the event out without cancelling it gets the
    /// print anyway, plus the `rejectionhandled` that a handler arriving after
    /// the report owes it (HTML §8.1.7.5).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_handler_attached_after_the_report_fires_rejectionhandled() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(&format!(
                "{REJECTION_HARNESS}
                 globalThis.late = Promise.reject(new Error('late'));
                 undefined;"
            ))
            .await?;
        assert_eq!(reported_rejections(&engine).await, 1);

        engine
            .eval::<()>("globalThis.late.catch(() => {});\nundefined;")
            .await?;
        engine.runtime.idle().await;

        assert_eq!(
            engine.eval::<String>("globalThis.seen.join(',')").await?,
            "unhandledrejection:late,rejectionhandled:late"
        );
        Ok(())
    }

    /// The whole error chain, end to end: an exception thrown from a timer
    /// callback inside a worker is reported by *Rust*, and used to stop there —
    /// on stderr — because only the JS-side reporters went through the worker
    /// scope's escalation. Now every reporter resolves the realm's sink, so it
    /// fires the worker's own `error` event and, uncancelled, the parent's.
    #[cfg(all(feature = "stdlib-worker", feature = "stdlib-timer"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_throwing_timer_in_a_worker_reaches_the_parents_onerror() -> eyre::Result<()> {
        let child = write_script(
            "timer-fault-child",
            "setTimeout(() => { throw new Error('from the timer') }, 1);\n",
        );
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .expect("the fixture name is text")
            .to_owned();
        let parent = write_script(
            "timer-fault-parent",
            &format!(
                "globalThis.seen = 'nothing';
                 const worker = new Worker('./{name}');
                 worker.onerror = (event) => {{
                   event.preventDefault();
                   globalThis.seen = event.message;
                   worker.terminate();
                 }};\n"
            ),
        );

        let engine = Engine::new().await;
        engine.run_file::<()>(parent.clone()).await?;
        // The worker's fault travels over a channel into a task this realm
        // spawned, so draining the realm is what delivers it. Bounded, because
        // a chain that stays broken would otherwise hang the suite.
        let drained = tokio::time::timeout(WORKER_FAULT_TIMEOUT, engine.runtime.idle()).await;
        assert!(drained.is_ok(), "the worker never finished");

        assert_eq!(
            engine.eval::<String>("globalThis.seen").await?,
            "from the timer"
        );
        engine.shutdown().await;
        fs::remove_file(parent)?;
        fs::remove_file(child)?;
        Ok(())
    }

    /// Ctrl-C / `Engine::stop()` must complete every `ctx.spawn`ed timer so
    /// `idle()` can return; dropping `idle()` itself does not cancel them.
    #[cfg(feature = "stdlib-timer")]
    #[tokio::test(flavor = "multi_thread")]
    async fn stopping_the_engine_releases_idle_from_a_long_timer() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>("setTimeout(() => {}, 60000);\nundefined;")
            .await?;
        engine.stop();
        tokio::time::timeout(std::time::Duration::from_secs(2), engine.runtime.idle())
            .await
            .expect("idle() should return once the timer observes the stop token");
        Ok(())
    }

    #[cfg(feature = "stdlib-timer")]
    #[tokio::test(flavor = "multi_thread")]
    async fn set_timeout_returns_a_number_and_clear_timeout_is_a_function() -> eyre::Result<()> {
        let engine = Engine::new().await;
        let report: String = engine
            .eval(
                r#"
                  const id = setTimeout("x", 0);
                  [typeof clearTimeout, typeof id].join(",")
                "#,
            )
            .await?;
        assert_eq!(report, "function,number");
        Ok(())
    }

    /// `setInterval(f, 0)` reached `tokio::time::interval`, which panics on a
    /// zero period — on the main thread, that is the whole process.
    #[cfg(feature = "stdlib-timer")]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_zero_delay_timer_is_clamped_instead_of_panicking() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(
                "globalThis.ticks = 0;
                 const handle = setInterval(() => {
                   if (++globalThis.ticks >= 2) clearInterval(handle);
                 }, 0);
                 setTimeout(() => { globalThis.timedOut = true }, 0);
                 undefined;",
            )
            .await?;
        engine.runtime.idle().await;

        assert_eq!(engine.eval::<usize>("globalThis.ticks").await?, 2);
        assert!(engine.eval::<bool>("globalThis.timedOut === true").await?);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unhandled_rejection_is_reported_after_the_turn_ends() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>("Promise.reject(new Error('nobody claims this'));\nundefined;")
            .await?;
        assert_eq!(reported_rejections(&engine).await, 1);
        Ok(())
    }

    /// The other half of the same decision: QuickJS reports the rejection
    /// first and the handler second, so reporting eagerly would make every
    /// `const p = Promise.reject(…); p.catch(…)` a false alarm.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_rejection_handled_later_in_the_turn_is_not_reported() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>(
                "const p = Promise.reject(new Error('claimed')); p.catch(() => {});\nundefined;",
            )
            .await?;
        assert_eq!(reported_rejections(&engine).await, 0);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn eval_runs_script_and_converts_result_to_rust_type() -> eyre::Result<()> {
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

    // `Engine::eval` deliberately evaluates as global script code (that is what a
    // REPL line is), so module syntax is a QuickJS error rather than a panic.
    // Loading a module goes through `Engine::run_file`, which imports instead
    // of evaluating.
    #[tokio::test(flavor = "multi_thread")]
    async fn eval_rejects_module_syntax_as_a_recoverable_error() -> eyre::Result<()> {
        let engine = Engine::new().await;
        let outcome = engine
            .eval::<()>(
                r#"
            export const hello = "world"
        "#,
            )
            .await;
        assert!(matches!(outcome, Err(EngineError::Rquickjs(_))));
        Ok(())
    }
}
