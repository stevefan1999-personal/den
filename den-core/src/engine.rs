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
    unhandled:   RefCell<Vec<Rejection>>,
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
    claimed:     RefCell<Vec<Persistent<Value<'static>>>>,
    /// Rejections already reported, so that a handler attached to one *later*
    /// can still fire `rejectionhandled` (HTML §8.1.7.5).
    outstanding: RefCell<VecDeque<Rejection>>,
    /// How many rejections have been printed so far. Printing goes to stderr,
    /// which a test inside this process cannot read; this counter is its only
    /// seam on the decision the tracker actually makes.
    reported:    Cell<usize>,
}

// SAFETY: `PendingRejections` borrows no `'js` lifetime — a `Persistent` owns
// its value outright and is tied to the runtime, not to a scope — so the type
// is the same type for every choice of `'to`.
unsafe impl<'js> JsLifetime<'js> for PendingRejections {
    type Changed<'to> = PendingRejections;
}

/// den-core's side of the worker crate's engine seam. A worker thread asks for
/// an engine and gets the very same one the main script runs on — same loaders,
/// same stdlib, same `den:worker` — differing only in its base URL. Stopping it
/// is the worker crate's business: it owns the token and installs the interrupt
/// handler on the runtime this hands back.
#[cfg(feature = "stdlib-worker")]
struct DenWorkerHost;

#[cfg(feature = "stdlib-worker")]
impl WorkerHost for DenWorkerHost {
    fn build_engine(&self, base: BaseUrl) -> Result<WorkerEngine, WorkerHostError> {
        // Called on the worker's own OS thread, inside that thread's
        // multi-threaded runtime: `block_in_place` + `block_on` is what lets a
        // synchronous trait method reach an async constructor, and it is the
        // same pair den's module loaders already use one layer down.
        let engine = block_in_place(|| {
            Handle::current().block_on(async move {
                let engine = Engine::new().await;
                // Signals belong to the process, and only the realm running the
                // root event loop can deliver them: a worker's loop is `idle()`
                // and never drains an inbox.
                #[cfg(feature = "stdlib-process")]
                engine
                    .context
                    .with(|ctx| den_stdlib_process::signal::SignalHub::disable(&ctx))
                    .await;
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

/// One QuickJS realm — an [`AsyncRuntime`] plus its [`AsyncContext`] — and the
/// whole of den's embedding surface.
///
/// There is deliberately no stop token here. Stopping a realm is *dropping* it,
/// which drops every `ctx.spawn`ed future before the QuickJS runtime is freed;
/// interrupting a script already spinning in bytecode is a flag the **host**
/// owns, polled by QuickJS's interrupt handler every few thousand back-edges.
/// A host that wants both composes them itself, the way `axum` takes a
/// shutdown future and owns no token of its own:
///
/// ```no_run
/// use std::path::PathBuf;
///
/// use den_core::engine::{Engine, EngineError};
///
/// #[tokio::main(flavor = "multi_thread")]
/// async fn main() -> Result<(), EngineError> {
///     let entry = PathBuf::from("main.js");
///     // Host-owned stop signal. An `Arc<AtomicBool>` plus any awaitable the
///     // host already holds is the same recipe.
///     let (stop, mut stopped) = tokio::sync::watch::channel(false);
///
///     let engine = Engine::new().await;
///     // Before the first run: installing takes the runtime lock that the
///     // event loop holds while it is parked.
///     engine
///         .runtime
///         .set_interrupt_handler(Some(Box::new({
///             let flag = stop.subscribe();
///             move || *flag.borrow()
///         })))
///         .await;
///
///     // Entry module and event loop are one future, so one `select!` covers
///     // the script, its timers and its in-flight I/O alike.
///     let program = async {
///         engine.run_file(entry).await?;
///         engine.run_event_loop().await;
///         Ok::<_, EngineError>(())
///     };
///     tokio::select! {
///       // A stopped script reports `Err(interrupted)`: that is the host's own
///       // stop, not a failing script, so the flag decides which it was.
///       result = program => if !*stop.borrow() { result? },
///       _ = stopped.changed() => {}
///     }
///     drop(engine); // the cancel; the losing arm is dropped mid-await
///     Ok(())
/// }
/// ```
///
/// The rules that recipe encodes, none of which the type can enforce:
///
/// 1. Install the interrupt handler *before* any JS runs. It takes the same
///    runtime mutex [`AsyncRuntime::idle`] holds, so a handler installed while
///    the loop is parked waits for the loop it was meant to interrupt.
/// 2. Flip the flag from somewhere the JS loop does not block: a task on a
///    multi-thread runtime, or a `std::thread`. On a `current_thread` runtime
///    the canceller never gets to run and the stop never arrives.
/// 3. The program arm can win with `Err`, because an interrupted script *is* an
///    uncatchable QuickJS exception. Ask the host's own flag before reporting a
///    script failure.
/// 4. A true flag stays true for the whole runtime: every later `eval` in it
///    dies at its first interrupt poll. The engine is single-use after a hard
///    stop — including a "goodbye" script, which needs the interrupter to hold
///    a second flag if it is to survive at all.
/// 5. [`Engine`] is [`Clone`], so `drop` cancels only when the *last* clone
///    dies, and a clone must never be moved into a `ctx.spawn`ed future:
///    runtime → spawner → future → `Engine` → runtime is a cycle that drop
///    cannot break.
/// 6. For a deadline instead of an event, run the loop under a timeout: `let _
///    = tokio::time::timeout(grace, engine.run_event_loop()).await;` then flip
///    the flag and drop.
///
/// Ctrl-C is not in this list on purpose: den installs no signal handler, and a
/// script that wants a graceful one installs it itself with
/// `den:process`'s `addSignalListener`. See `ARCHITECTURE.md` §2.
#[derive(Clone)]
pub struct Engine {
    #[cfg(feature = "transpile")]
    pub transpiler: Arc<EasyOxcTranspiler>,
    pub runtime:    AsyncRuntime,
    pub context:    AsyncContext,
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

                    #[cfg(feature = "stdlib-assert")]
                    {
                        resolver = resolver.with_module("den:assert");
                    }
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
                    #[cfg(feature = "stdlib-temporal")]
                    {
                        resolver = resolver.with_module("den:temporal");
                    }
                    #[cfg(feature = "wasm")]
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

                    #[cfg(feature = "stdlib-assert")]
                    {
                        loader = loader.with_module("den:assert", den_stdlib_assert::js_assert);
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
                    #[cfg(feature = "stdlib-temporal")]
                    {
                        loader =
                            loader.with_module("den:temporal", den_stdlib_temporal::js_temporal);
                    }
                    #[cfg(feature = "wasm")]
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
            .set_host_promise_rejection_tracker(Some(Box::new(Self::track_rejection)))
            .await;

        let context = AsyncContext::full(&runtime).await.unwrap();

        context
            .with(|ctx| {
                // Every stdlib module is wired the same way: evaluate its
                // definition under its `den:` name. The resolver and loader
                // lists above decide which modules exist; this list decides
                // which of them run.
                #[cfg(any(
                    feature = "stdlib-console",
                    feature = "stdlib-core",
                    feature = "stdlib-text",
                    feature = "stdlib-timer",
                    feature = "stdlib-whatwg-fetch",
                    feature = "stdlib-crypto",
                    feature = "stdlib-process",
                    feature = "stdlib-temporal",
                    feature = "wasm",
                    feature = "stdlib-worker",
                    feature = "stdlib-whatwg",
                ))]
                macro_rules! evaluate_stdlib_module {
                    ($module:path, $name:literal) => {
                        let _ = Module::evaluate_def::<$module, _>(ctx.clone(), $name)?;
                    };
                }

                #[cfg(feature = "stdlib-console")]
                evaluate_stdlib_module!(den_stdlib_console::js_console, "den:console");

                #[cfg(feature = "stdlib-core")]
                evaluate_stdlib_module!(den_stdlib_core::js_core, "den:core");

                #[cfg(feature = "stdlib-text")]
                evaluate_stdlib_module!(den_stdlib_text::js_text, "den:text");

                #[cfg(feature = "stdlib-timer")]
                evaluate_stdlib_module!(den_stdlib_timer::js_timer, "den:timer");

                #[cfg(feature = "stdlib-whatwg-fetch")]
                evaluate_stdlib_module!(den_stdlib_whatwg_fetch::js_whatwg, "den:whatwg-fetch");

                #[cfg(feature = "stdlib-crypto")]
                evaluate_stdlib_module!(den_stdlib_crypto::js_crypto, "den:crypto");

                #[cfg(feature = "stdlib-process")]
                evaluate_stdlib_module!(den_stdlib_process::js_process, "den:process");

                #[cfg(feature = "stdlib-temporal")]
                evaluate_stdlib_module!(den_stdlib_temporal::js_temporal, "den:temporal");

                #[cfg(feature = "wasm")]
                evaluate_stdlib_module!(den_stdlib_wasm::js_wasm, "den:wasm");

                #[cfg(feature = "stdlib-worker")]
                {
                    evaluate_stdlib_module!(den_stdlib_worker::js_worker, "den:worker");

                    // Every context gets the host and a base URL, worker
                    // contexts included: that — and nothing else — is what
                    // makes a worker able to spawn workers of its own.
                    Self::store_userdata(
                        &ctx,
                        den_stdlib_worker::HostHandle(Arc::new(DenWorkerHost)),
                    )?;
                    Self::store_userdata(&ctx, Self::working_directory_url())?;
                }

                // After `den:worker` so FileReader / XHR / EventSource / WebSocket
                // can extend EventTarget. Fetch is already wired above.
                #[cfg(feature = "stdlib-whatwg")]
                evaluate_stdlib_module!(den_stdlib_whatwg::js_whatwg, "den:whatwg");

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
        }
    }

    pub async fn run_file<U: for<'a> FromJs<'a> + Sync + Send + 'static>(
        &self, filename: PathBuf,
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

        let entry = self
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
            });

        // A server spends its whole life inside the entry module's top-level
        // await, so a signal that lands there has to reach JS there.
        #[cfg(feature = "stdlib-process")]
        let value = den_stdlib_process::signal::SignalHub::deliver_while(&self.context, entry);
        #[cfg(not(feature = "stdlib-process"))]
        let value = entry;
        Ok(value.await?)
    }

    /// Run this realm's event loop until nothing is left spawned, delivering
    /// signals to its JS listeners along the way.
    ///
    /// [`Self::run_file`] only awaits the entry module's own promise; this is
    /// what a host runs afterwards, and what makes `addSignalListener` mean
    /// anything once the module has returned.
    pub async fn run_event_loop(&self) {
        #[cfg(feature = "stdlib-process")]
        den_stdlib_process::signal::SignalHub::drive(&self.runtime, &self.context).await;
        #[cfg(not(feature = "stdlib-process"))]
        self.runtime.idle().await;
    }

    #[cfg(feature = "transpile")]
    pub fn transpile(
        &self, src: &str, syntax: Syntax, module: IsModule,
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
        ctx: Ctx<'js>, src: &str,
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
        &self, src: &str,
    ) -> Result<U, EngineError> {
        let src = self.prepare_eval_source(src)?;
        Ok(self
            .context
            .async_with(async |ctx| Self::eval_prepared(ctx, &src).await)
            .await?)
    }

    /// Reap every worker this realm spawned: interrupt each one and join its
    /// thread, which a parked worker needs because it has nothing to
    /// interrupt. Stopping the realm itself is dropping the `Engine`, so an
    /// embedder calls this and then drops it.
    pub async fn shutdown(&self) {
        #[cfg(feature = "stdlib-worker")]
        den_stdlib_worker::worker::shutdown(&self.context).await;
        let _ = self
            .context
            .with(|ctx| {
                // The checkpoint that normally reports these rides the event
                // loop, and shutdown is the last moment one can still run: say
                // what the realm never handled before forgetting it.
                Self::report_unhandled_rejections(&ctx);
                if let Some(pending) = ctx.userdata::<PendingRejections>() {
                    if let Ok(mut unhandled) = pending.unhandled.try_borrow_mut() {
                        unhandled.clear();
                    }
                    if let Ok(mut claimed) = pending.claimed.try_borrow_mut() {
                        claimed.clear();
                    }
                    if let Ok(mut outstanding) = pending.outstanding.try_borrow_mut() {
                        outstanding.clear();
                    }
                }
                ctx.run_gc();
                ctx.run_gc();
                ctx.run_gc();
                Ok::<(), rquickjs::Error>(())
            })
            .await;
    }

    /// Install an [import map](https://wicg.github.io/import-maps/) on this
    /// realm. Relative `./` / `../` targets join against `base_dir`. Calling
    /// again replaces the previous map.
    pub async fn set_import_map(
        &self, json: &str, base_dir: impl AsRef<Path> + Send,
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
    /// `den:worker` makes the main global an EventTarget and installs
    /// `PromiseRejectionEvent`. A realm without those (stdlib-worker off)
    /// cancels nothing and the rejection goes to stderr as it always did.
    fn fire_rejection_event<'js>(
        ctx: &Ctx<'js>, kind: &str, promise: &Value<'js>, reason: &Value<'js>, cancelable: bool,
    ) -> bool {
        let globals = ctx.globals();
        let dispatched = (|| -> rquickjs::Result<bool> {
            let constructor = globals.get::<_, Constructor<'js>>("PromiseRejectionEvent")?;
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
            let dispatch = globals.get::<_, rquickjs::Function<'js>>("dispatchEvent")?;
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

    /// `den:worker` already made the main global an EventTarget, so the tests
    /// that want the event listen — they do not stand up a JS Event class.
    const REJECTION_HARNESS: &str = r#"
        globalThis.seen = [];
        const record = (event) => {
          globalThis.seen.push(`${event.type}:${event.reason.message}`);
          if (globalThis.claim) event.preventDefault();
        };
        addEventListener("unhandledrejection", record);
        addEventListener("rejectionhandled", record);
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

    /// Cancellation of a pending timer is drop, not a flag: the runtime clears
    /// its spawner before `JS_FreeRuntime`, so an embedder that drops the
    /// `Engine` never waits out a 60-second `setTimeout`.
    #[cfg(feature = "stdlib-timer")]
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_an_engine_with_a_pending_timer_returns_promptly() -> eyre::Result<()> {
        let engine = Engine::new().await;
        engine
            .eval::<()>("setTimeout(() => {}, 60000);\nundefined;")
            .await?;

        let started = std::time::Instant::now();
        drop(engine);
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "dropping the engine waited {elapsed:?} for the timer"
        );
        Ok(())
    }

    /// The embedder recipe cancels by dropping the `Engine`, and a server
    /// spends its life inside the entry module's top-level await — so the drop
    /// lands mid-`async_with`, with the module's promise and the op it is
    /// parked on both still pending. QuickJS has to free that context without
    /// waiting for them. `multi_thread` so the stop can arrive while the JS
    /// loop owns the runtime.
    #[cfg(feature = "stdlib-timer")]
    #[tokio::test(flavor = "multi_thread")]
    async fn hosts_token_drops_an_engine_parked_on_a_top_level_await() -> eyre::Result<()> {
        let entry = write_script(
            "parked-on-a-long-await",
            "await new Promise((resolve) => setTimeout(resolve, 60000));\n",
        );
        let engine = Engine::new().await;

        // Stands in for the host's stop signal (a `watch` flip, a Ctrl-C, an
        // admin endpoint): whatever it is, it only ever races the program
        // future.
        let started = std::time::Instant::now();
        let stopped_the_program = tokio::select! {
            _ = engine.run_file(entry) => false,
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => true,
        };
        drop(engine);
        let elapsed = started.elapsed();

        assert!(
            stopped_the_program,
            "the entry module returned instead of staying parked"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "stopping a parked engine took {elapsed:?}"
        );
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
