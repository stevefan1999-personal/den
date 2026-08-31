#[cfg(feature = "stdlib-worker")]
use std::sync::Arc;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::{Path, PathBuf},
};

#[cfg(feature = "transpile")]
use den_transpiler_oxc::{
    EasyOxcTranspilerError, get_best_transpiling, infer_transpile_syntax_by_extension,
    transpile_with_source_map,
};
use derive_more::{Debug, Display, Error, From};
use rquickjs::{
    AsyncContext, AsyncRuntime, Ctx, FromJs, JsLifetime, Module, Object, Persistent, Promise,
    Value,
    context::EvalOptions,
    function::{Constructor, This},
    loader::{BuiltinLoader, BuiltinResolver, Bundle, FileResolver, ModuleLoader},
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

#[derive(Default)]
struct EvalSequence(Cell<u64>);

// SAFETY: the sequence counter contains no JavaScript-lifetime data.
unsafe impl JsLifetime<'_> for EvalSequence {
    type Changed<'to> = EvalSequence;
}

/// JavaScript ready for evaluation under the unique filename whose source map
/// was registered with this realm.
pub struct PreparedSource {
    code:     String,
    filename: String,
}

// SAFETY: `PendingRejections` borrows no `'js` lifetime — a `Persistent` owns
// its value outright and is tied to the runtime, not to a scope — so the type
// is the same type for every choice of `'to`.
unsafe impl JsLifetime<'_> for PendingRejections {
    type Changed<'to> = PendingRejections;
}

/// den-core's side of the worker crate's engine seam. A worker thread asks for
/// an engine and gets the very same one the main script runs on — same loaders,
/// same stdlib, same `den:worker` — differing only in its base URL. Stopping it
/// is the worker crate's business: it owns the token and installs the interrupt
/// handler on the runtime this hands back.
#[cfg(feature = "stdlib-worker")]
struct DenWorkerHost {
    bundle: Bundle,
}

#[cfg(feature = "stdlib-worker")]
impl WorkerHost for DenWorkerHost {
    fn build_engine(&self, base: BaseUrl) -> Result<WorkerEngine, WorkerHostError> {
        // Called on the worker's own OS thread, inside that thread's
        // multi-threaded runtime: `block_in_place` + `block_on` is what lets a
        // synchronous trait method reach an async constructor, and it is the
        // same pair den's module loaders already use one layer down.
        let bundle = self.bundle;
        let engine = block_in_place(|| {
            Handle::current().block_on(async move {
                let engine = Engine::build(bundle).await;
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

    #[cfg(feature = "stdlib-kv")]
    fn shutdown<'a>(
        &'a self, context: &'a AsyncContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(den_stdlib_kv::KvRegistry::shutdown(context))
    }
}

/// One QuickJS realm — an [`AsyncRuntime`] plus its [`AsyncContext`] — and the
/// whole of den's embedding surface.
///
/// There is deliberately no stop token here. A host stops a realm by dropping
/// its program future, awaiting [`Engine::shutdown`], then dropping the engine;
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
///     engine.shutdown().await; // finish deferred QuickJS/resource teardown
///     drop(engine);
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
/// 5. [`Engine`] is [`Clone`], so the realm dies only with the *last* clone,
///    and a clone must never be moved into a `ctx.spawn`ed future: runtime →
///    spawner → future → `Engine` → runtime is a cycle that drop cannot break.
/// 6. For a deadline instead of an event, run the loop under a timeout: `let _
///    = tokio::time::timeout(grace, engine.run_event_loop()).await;` then flip
///    the flag, await [`Engine::shutdown`], and drop.
///
/// Ctrl-C is not in this list on purpose: den installs no signal handler, and a
/// script that wants a graceful one installs it itself with
/// `den:process`'s `addSignalListener`. See `ARCHITECTURE.md` §2.
#[derive(Clone)]
pub struct Engine {
    pub runtime: AsyncRuntime,
    pub context: AsyncContext,
}

impl Engine {
    const EMPTY_BUNDLE: Bundle = rquickjs::loader::bundle::Bundle(&rquickjs::phf::Map::new());
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

    pub async fn new() -> Engine { Self::build(Self::EMPTY_BUNDLE).await }

    /// Build an engine that resolves application-owned modules from bytecode
    /// produced by [`rquickjs::embed!`].
    ///
    /// rquickjs compiles inputs in declaration order on the build host, so
    /// static dependencies must precede their importers. Cross-compiled
    /// bytecode requires the target to use the same QuickJS version and
    /// endianness as the host.
    pub async fn new_with_bundle(bundle: Bundle) -> Engine { Self::build(bundle).await }

    async fn build(bundle: Bundle) -> Engine {
        let runtime = AsyncRuntime::new()
            .unwrap_or_else(|error| panic!("could not create QuickJS runtime: {error}"));
        runtime.set_max_stack_size(0).await;

        {
            let resolver = (
                ImportMapResolver,
                {
                    let resolver = BuiltinResolver::default();
                    #[cfg(feature = "stdlib-assert")]
                    let resolver = resolver.with_module("den:assert");
                    #[cfg(feature = "stdlib-core")]
                    let resolver = resolver.with_module("den:core");
                    #[cfg(feature = "stdlib-console")]
                    let resolver = resolver.with_module("den:console");
                    #[cfg(feature = "stdlib-networking")]
                    let resolver = resolver.with_module("den:networking");
                    #[cfg(feature = "stdlib-path")]
                    let resolver = resolver.with_module("den:path");
                    #[cfg(feature = "stdlib-text")]
                    let resolver = resolver.with_module("den:text");
                    #[cfg(feature = "stdlib-timer")]
                    let resolver = resolver.with_module("den:timer");
                    #[cfg(feature = "stdlib-fs")]
                    let resolver = resolver.with_module("den:fs");
                    #[cfg(feature = "stdlib-http")]
                    let resolver = resolver.with_module("den:http");
                    #[cfg(feature = "stdlib-kv")]
                    let resolver = resolver.with_module("den:kv");
                    #[cfg(feature = "stdlib-ffi")]
                    let resolver = resolver.with_module("den:ffi");
                    #[cfg(feature = "stdlib-sqlite")]
                    let resolver = resolver.with_module("den:sqlite");
                    #[cfg(feature = "stdlib-whatwg-fetch")]
                    let resolver = resolver.with_module("den:whatwg-fetch");
                    #[cfg(feature = "stdlib-crypto")]
                    let resolver = resolver.with_module("den:crypto");
                    #[cfg(feature = "stdlib-process")]
                    let resolver = resolver.with_module("den:process");
                    #[cfg(feature = "stdlib-temporal")]
                    let resolver = resolver.with_module("den:temporal");
                    #[cfg(feature = "wasm")]
                    let resolver = resolver.with_module("den:wasm");
                    #[cfg(feature = "stdlib-worker")]
                    let resolver = resolver.with_module("den:worker");
                    #[cfg(feature = "stdlib-whatwg")]
                    let resolver = resolver.with_module("den:whatwg");
                    resolver
                },
                bundle,
                HttpResolver,
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
                    let loader = ModuleLoader::default();
                    #[cfg(feature = "stdlib-core")]
                    let loader = loader.with_module("den:core", den_stdlib_core::js_core);
                    #[cfg(feature = "stdlib-assert")]
                    let loader = loader.with_module("den:assert", den_stdlib_assert::js_assert);
                    #[cfg(feature = "stdlib-console")]
                    let loader = loader.with_module("den:console", den_stdlib_console::js_console);
                    #[cfg(feature = "stdlib-networking")]
                    let loader =
                        loader.with_module("den:networking", den_stdlib_networking::js_networking);
                    #[cfg(feature = "stdlib-path")]
                    let loader = loader.with_module("den:path", den_stdlib_path::js_path);
                    #[cfg(feature = "stdlib-text")]
                    let loader = loader.with_module("den:text", den_stdlib_text::js_text);
                    #[cfg(feature = "stdlib-timer")]
                    let loader = loader.with_module("den:timer", den_stdlib_timer::js_timer);
                    #[cfg(feature = "stdlib-fs")]
                    let loader = loader.with_module("den:fs", den_stdlib_fs::js_fs);
                    #[cfg(feature = "stdlib-http")]
                    let loader = loader.with_module("den:http", den_stdlib_http::js_http);
                    #[cfg(feature = "stdlib-kv")]
                    let loader = loader.with_module("den:kv", den_stdlib_kv::js_kv);
                    #[cfg(feature = "stdlib-ffi")]
                    let loader = loader.with_module("den:ffi", den_stdlib_ffi::js_ffi);
                    #[cfg(feature = "stdlib-sqlite")]
                    let loader = loader.with_module("den:sqlite", den_stdlib_sqlite::js_sqlite);
                    #[cfg(feature = "stdlib-whatwg-fetch")]
                    let loader =
                        loader.with_module("den:whatwg-fetch", den_stdlib_whatwg::fetch::js_whatwg);
                    #[cfg(feature = "stdlib-crypto")]
                    let loader = loader.with_module("den:crypto", den_stdlib_crypto::js_crypto);
                    #[cfg(feature = "stdlib-process")]
                    let loader = loader.with_module("den:process", den_stdlib_process::js_process);
                    #[cfg(feature = "stdlib-temporal")]
                    let loader =
                        loader.with_module("den:temporal", den_stdlib_temporal::js_temporal);
                    #[cfg(feature = "wasm")]
                    let loader = loader.with_module("den:wasm", den_stdlib_wasm::js_wasm);
                    #[cfg(feature = "stdlib-worker")]
                    let loader = loader.with_module("den:worker", den_stdlib_worker::js_worker);
                    #[cfg(feature = "stdlib-whatwg")]
                    let loader = loader.with_module("den:whatwg", den_stdlib_whatwg::js_whatwg);
                    loader
                },
                bundle,
                HttpLoader,
                {
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

        runtime
            .set_host_promise_rejection_tracker(Some(Box::new(Self::track_rejection)))
            .await;

        let context = AsyncContext::full(&runtime)
            .await
            .unwrap_or_else(|error| panic!("could not create QuickJS context: {error}"));

        context
            .with(|ctx| {
                den_util::stack::install(&ctx)?;
                Self::store_userdata(&ctx, EvalSequence::default())?;
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
                evaluate_stdlib_module!(den_stdlib_whatwg::fetch::js_whatwg, "den:whatwg-fetch");

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
                        den_stdlib_worker::HostHandle(Arc::new(DenWorkerHost { bundle })),
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
            .unwrap_or_else(|error| panic!("could not initialize QuickJS context: {error}"));

        Self { runtime, context }
    }

    pub async fn run_file(&self, filename: PathBuf) -> Result<(), EngineError> {
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
        let path = dunce::canonicalize(&path).unwrap_or(path);

        #[cfg(feature = "stdlib-worker")]
        if let Some(directory) = path
            .parent()
            .and_then(|directory| Url::from_directory_path(directory).ok())
        {
            self.set_base_url(BaseUrl(directory.into())).await?;
        }

        let specifier = path.to_string_lossy().replace('\\', "/");
        self.run_module(&specifier).await
    }

    /// Import an entry module by specifier and wait for its top-level promise.
    ///
    /// This is the entry point for modules supplied through
    /// [`rquickjs::embed!`]. File entry points should continue to use
    /// [`Self::run_file`], which canonicalizes the path and establishes its
    /// worker base URL.
    pub async fn run_module(&self, specifier: &str) -> Result<(), EngineError> {
        #[cfg(feature = "stdlib-worker")]
        if !Path::new(specifier).is_absolute()
            && let Ok(url) = Url::parse(specifier)
            && let Ok(directory) = url.join(".")
        {
            self.set_base_url(BaseUrl(directory.into())).await?;
        }

        let specifier = specifier.to_owned();
        let entry = self.context.async_with(async |ctx| {
            let result = async {
                let _: Object = Module::import(&ctx, specifier)?.into_future().await?;
                Ok::<_, rquickjs::Error>(())
            }
            .await;
            result.map_err(|error| Self::take_error(&ctx, error))
        });
        // A server spends its whole life inside the entry module's top-level
        // await, so a signal that lands there has to reach JS there.
        #[cfg(feature = "stdlib-process")]
        den_stdlib_process::signal::SignalHub::deliver_while(&self.context, entry).await?;
        #[cfg(not(feature = "stdlib-process"))]
        entry.await?;
        Ok(())
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

    /// Transpile a REPL/eval snippet when this engine was built with the
    /// transpiler; otherwise the source is used as-is. Independent of the
    /// runtime lock, so a `ctx.spawn`ed pump can prepare a line without
    /// waiting for `idle()`.
    pub fn prepare_eval_source(
        &self, ctx: &Ctx<'_>, src: &str,
    ) -> Result<PreparedSource, EngineError> {
        let filename = Self::next_eval_filename(ctx);
        #[cfg(feature = "transpile")]
        {
            let source_type = infer_transpile_syntax_by_extension(get_best_transpiling())
                .unwrap_or_default()
                .with_unambiguous(true);
            let output = transpile_with_source_map(src, source_type, &filename)?;
            den_util::stack::register_source(ctx, &filename, output.code.clone(), [output
                .source_map
                .into_inner()])?;
            Ok(PreparedSource {
                code: output.code,
                filename,
            })
        }
        #[cfg(not(feature = "transpile"))]
        {
            let _ = self;
            den_util::stack::register_source(ctx, &filename, src.to_owned(), std::iter::empty())?;
            Ok(PreparedSource {
                code: src.to_owned(),
                filename,
            })
        }
    }

    fn next_eval_filename(ctx: &Ctx<'_>) -> String {
        ctx.userdata::<EvalSequence>().map_or_else(
            || "<eval>".to_owned(),
            |sequence| {
                let next = sequence.0.get().saturating_add(1);
                sequence.0.set(next);
                format!("<eval:{next}>")
            },
        )
    }

    /// Evaluate an already-prepared snippet on a context the caller is holding.
    ///
    /// Used by `eval` (under `async_with`) and by the REPL pump (`ctx.spawn`
    /// while `idle()` holds the runtime lock). A separate tokio task calling
    /// `async_with` during `idle()` would park on the mutex until idle
    /// returned.
    pub async fn eval_prepared<'js, U: FromJs<'js>>(
        ctx: Ctx<'js>, source: &PreparedSource,
    ) -> rquickjs::Result<U> {
        den_util::stack::register_source(
            &ctx,
            &source.filename,
            source.code.clone(),
            std::iter::empty(),
        )?;
        ctx.eval_with_options::<Promise, _>(source.code.as_str(), {
            let mut options = EvalOptions::default();
            options.global = true;
            options.promise = true;
            options.strict = true;
            // A REPL line has no file; naming it after one would be a
            // lie the resolver could act on, so it gets a name no URL
            // parser accepts.
            options.filename = Some(source.filename.clone());
            options
        })?
        .into_future::<Object>()
        .await?
        .get("value")
    }

    pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(
        &self, src: &str,
    ) -> Result<U, EngineError> {
        self.context
            .async_with(async |ctx| {
                let src = self.prepare_eval_source(&ctx, src)?;
                Self::eval_prepared(ctx.clone(), &src)
                    .await
                    .map_err(|error| Self::take_error(&ctx, error))
            })
            .await
    }

    /// Take a pending exception that a host handled directly, and retract the
    /// promise-tracker copy of the same reason before formatting it.
    pub fn take_exception(ctx: &Ctx<'_>) -> den_util::stack::JsError {
        let value = ctx.catch();
        if let Some(pending) = ctx.userdata::<PendingRejections>() {
            let reason = Persistent::save(ctx, value.clone());
            if let Ok(mut unhandled) = pending.unhandled.try_borrow_mut() {
                unhandled.retain(|(_, candidate)| *candidate != reason);
            }
            if let Ok(mut claimed) = pending.claimed.try_borrow_mut() {
                claimed.push(reason);
            }
        }
        den_util::stack::JsError::from_value(ctx, &value)
    }

    /// Finish host resources and deferred QuickJS cleanup before dropping the
    /// engine. This is required after cancelling `run_file`, `run_module`, or
    /// `eval` mid-await; dropping such a future can defer value release until
    /// the runtime lock is reacquired here.
    pub async fn shutdown(&self) {
        #[cfg(feature = "stdlib-worker")]
        den_stdlib_worker::worker::shutdown(&self.context).await;
        #[cfg(feature = "stdlib-kv")]
        den_stdlib_kv::KvRegistry::shutdown(&self.context).await;
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
    pub async fn set_import_map<P: AsRef<Path> + Send>(
        &self, json: &str, base_dir: P,
    ) -> Result<(), EngineError> {
        let map = ImportMap::parse(json, base_dir.as_ref())?;
        self.context
            .with(move |ctx| Self::store_userdata(&ctx, map))
            .await?;
        Ok(())
    }

    /// Hand this realm the FFI capability. Without one, `den:ffi`'s `grant()`
    /// answers `null` and `open()` refuses every path: the module is
    /// importable but binds nothing.
    #[cfg(feature = "stdlib-ffi")]
    pub async fn set_ffi_grant(&self, grant: den_stdlib_ffi::FfiGrant) -> Result<(), EngineError> {
        self.context
            .with(move |ctx| Self::store_userdata(&ctx, grant))
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
            .map_err(|_error| rquickjs::Error::UserData(UserDataError(())))
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
            .is_ok_and(|mut outstanding| {
                let before = outstanding.len();
                outstanding.retain(|(promise, _)| *promise != rejected);
                outstanding.len() != before
            });
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
    fn describe(value: &Value<'_>) -> String { den_util::stack::format_thrown(value.ctx(), value) }

    fn take_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> EngineError {
        match error {
            rquickjs::Error::Exception => {
                EngineError::JavaScript(Box::new(Self::take_exception(ctx)))
            }
            error => EngineError::Rquickjs(error),
        }
    }
}

#[expect(
    clippy::module_name_repetitions,
    reason = "EngineError is the public error paired with Engine"
)]
#[derive(Display, From, Error, Debug)]
pub enum EngineError {
    #[cfg(feature = "transpile")]
    #[from]
    EasyOxcTranspiler(EasyOxcTranspilerError),
    #[from]
    Rquickjs(rquickjs::Error),
    JavaScript(Box<den_util::stack::JsError>),
    #[from]
    ImportMap(ImportMapError),
}

#[cfg(test)]
#[path = "../tests/unit/engine.rs"]
mod tests;
