use std::{fs, path::Path, process, sync::Arc, time::Duration};

use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt as _, Ctx, Exception, FromJs, Module, Object,
    Promise,
    context::EvalOptions,
    loader::{Resolver, ScriptLoader},
};
use tokio::{
    task::block_in_place,
    time::{self},
};
use url::Url;

use crate::host::{BaseUrl, HostHandle, WorkerEngine, WorkerHost, WorkerHostError};

/// The bound on every cross-thread wait in this module. Nothing here ever
/// sleeps for an expected duration: a test either observes the message it
/// is waiting for or fails on this.
const DEADLINE: Duration = Duration::from_secs(10);

/// `den:worker`, the way an embedder gets it: the real module definition,
/// evaluated into this realm so that its natives, its whole prelude chain,
/// its exports and its globals are all the ones under test. A harness that
/// re-implemented `lib.rs`'s wiring would stay green through any mutation
/// of it.
fn install_worker_api(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let (_, evaluated) = Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
    evaluated.finish::<()>()
}

/// `file:` URLs and the absolute paths they turn into — neither of which
/// rquickjs's own `FileResolver` can do, because it joins every specifier
/// onto a search path. den-core has the real one
/// (`den-core/src/resolver/file.rs`); a bare host needs just enough of it
/// to load a fixture and whatever the fixture imports.
struct FileUrlResolver;

impl Resolver for FileUrlResolver {
    fn resolve<'js>(
        &mut self, ctx: &Ctx<'js>, base: &str, name: &str,
        _attributes: Option<rquickjs::loader::ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let base = Url::from_file_path(base)
            .ok()
            .or_else(|| Url::parse(base).ok());
        Url::options()
            .base_url(base.as_ref())
            .parse(name)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .ok_or_else(|| {
                den_util::stack::throw_error(ctx, &format!("cannot resolve module {name:?}"))
            })
    }
}

/// A [`WorkerHost`] with no den-core in it: a runtime, a context, the
/// worker API and the two userdata slots a worker needs to spawn workers of
/// its own. Enough for classic and module scripts; the real stdlib, the
/// transpiler chain and `http(s)` loading are den-core's business and are
/// tested there.
struct BareHost;

impl WorkerHost for BareHost {
    fn build_engine(&self, base: BaseUrl) -> Result<WorkerEngine, WorkerHostError> {
        // Same pair den-core's host uses: a synchronous trait method
        // reaching an async constructor from inside a runtime.
        block_in_place(|| tokio::runtime::Handle::current().block_on(Self::build(base)))
            .map_err(|error| WorkerHostError(error.to_string()))
    }
}

impl BareHost {
    /// The one directory name that makes engine construction panic, so that
    /// "a worker thread that panics reaches its parent as an error instead
    /// of taking the process down" has a test. A real host has no such
    /// seam; this is the only way to reach the thread body's `catch_unwind`
    /// without a bug to trigger it.
    /// It has to be a *sub*directory of the fixture: a worker's base URL is
    /// its own script's directory, and the parent realm is built through
    /// this very function too.
    const PANIC_DIRECTORY: &'static str = "panicking-host";
    /// The directory name that makes a worker's own tokio runtime still
    /// have work to do when its thread body returns — den's real loaders
    /// reach the blocking pool, this is the smallest thing that does.
    /// A *sub*directory, like the one above, so that only the worker built
    /// from it is slow and not the parent realm.
    const SLOW_DIRECTORY: &'static str = "slow-teardown";
    /// How long that work takes. Long enough that a shutdown which merely
    /// asked the runtime to stop would return while the blocking thread is
    /// still there, short enough to be a blink in a test.
    const SLOW_TEARDOWN: Duration = Duration::from_millis(400);

    async fn build(base: BaseUrl) -> rquickjs::Result<WorkerEngine> {
        assert!(
            !base.0.contains(Self::PANIC_DIRECTORY),
            "the host panicked while building a worker engine"
        );
        if base.0.contains(Self::SLOW_DIRECTORY) {
            // Detached on purpose: it models a load that nobody is waiting
            // for, which is the case `shutdown_background` abandons.
            tokio::task::spawn_blocking(|| std::thread::sleep(Self::SLOW_TEARDOWN));
        }
        // No interrupt handler: a worker's is installed by `serve_engine` on
        // the runtime this hands back, which is exactly what the host seam no
        // longer owes this crate.
        let runtime = AsyncRuntime::new()?;
        runtime
            .set_loader(
                FileUrlResolver,
                ScriptLoader::default().with_extension("mjs"),
            )
            .await;
        let context = AsyncContext::full(&runtime).await?;
        context
            .with(|ctx| {
                install_worker_api(&ctx)?;
                Self::store(&ctx, HostHandle(Arc::new(BareHost)))?;
                Self::store(&ctx, base)
            })
            .await?;
        Ok(WorkerEngine { runtime, context })
    }

    fn store<T>(ctx: &Ctx<'_>, data: T) -> rquickjs::Result<()>
    where
        T: for<'js> rquickjs::JsLifetime<'js>,
    {
        ctx.store_userdata(data)
            .map(|_| ())
            .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))
    }
}

/// Write `files` under the workspace's `target/` (gitignored, and one
/// directory per test so parallel tests never share a file) and return the
/// directory URL a realm can use as its base.
fn fixture(test: &str, files: &[(&str, &str)]) -> String {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../target/den-worker-fixtures-{}", process::id()))
        .join(test);
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("a fixture directory");
    for (name, body) in files {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("a fixture subdirectory");
        }
        fs::write(path, body).expect("a fixture file");
    }
    let directory = directory.canonicalize().expect("a real fixture directory");
    Url::from_directory_path(directory)
        .expect("a fixture directory URL")
        .into()
}

/// A parent realm: the worker API, a bare host to spawn through, and a base
/// URL its `./`-relative worker specifiers resolve against.
struct Fixture {
    runtime: AsyncRuntime,
    context: AsyncContext,
}

impl Fixture {
    async fn new(test: &str, files: &[(&str, &str)]) -> Self {
        let base = BaseUrl(fixture(test, files));
        let engine = BareHost::build(base)
            .await
            .expect("the bare host builds a parent realm");
        Self {
            runtime: engine.runtime,
            context: engine.context,
        }
    }

    /// Evaluate `source` with top-level `await` and return its completion
    /// value.
    ///
    /// Everything cross-thread happens *while that promise is pending*:
    /// `async_with` polls the spawner — every port and fault pump — between
    /// polls of the user future, so a test can simply await the message it
    /// is waiting for.
    async fn eval<T>(&self, source: &'static str) -> T
    where
        T: for<'js> FromJs<'js> + Send + 'static,
    {
        let evaluated = time::timeout(
            DEADLINE,
            self.context.async_with(async |ctx| {
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                options.filename = Some("den:worker/test.js".to_owned());
                let outcome = async {
                    ctx.eval_with_options::<Promise<'_>, _>(source, options)?
                        .into_future::<Object<'_>>()
                        .await?
                        .get::<_, T>("value")
                }
                .await;
                outcome.catch(&ctx).map_err(|error| error.to_string())
            }),
        )
        .await
        .expect("the script settles within the deadline");
        evaluated.unwrap_or_else(|error| panic!("{error}"))
    }

    /// Drive the runtime until nothing is pending. A live worker never
    /// settles — that is the lifetime rule these tests pin — so this is
    /// also the assertion that one has really gone.
    async fn settle(&self) {
        time::timeout(DEADLINE, self.runtime.idle())
            .await
            .expect("the runtime goes idle");
    }

    /// Whether something is still keeping the runtime alive. A *negative*
    /// assertion — nothing will ever wake `idle()` — so it is the one place
    /// here that waits out a duration.
    async fn is_busy(&self) -> bool {
        time::timeout(Duration::from_millis(200), self.runtime.idle())
            .await
            .is_err()
    }

    /// How many worker handles this realm's registry is still holding.
    /// Reaching into the userdata is the only way to see it: the registry
    /// exists so that nothing else has to.
    async fn registered_workers(&self) -> usize {
        self.context
            .with(|ctx| {
                ctx.userdata::<super::WorkerRegistry>()
                    .and_then(|registry| {
                        registry
                            .threads
                            .try_borrow()
                            .ok()
                            .map(|threads| threads.len())
                    })
                    .expect("the realm has a worker registry")
            })
            .await
    }

    async fn shutdown(&self) {
        time::timeout(DEADLINE, super::shutdown(&self.context))
            .await
            .expect("every worker thread stops and is joined");
    }
}

/// Every thread of this process whose name is exactly `name`.
///
/// Linux truncates a thread's `comm` to 15 bytes, and every thread of a
/// worker — the one den starts and the ones that worker's own tokio runtime
/// starts — is named `den-worker:<name>`: eleven characters, so four of the
/// worker's own name survive. Every test in this binary shares the process,
/// which is why this matches one exact name rather than the prefix.
#[cfg(target_os = "linux")]
fn threads_named(name: &str) -> usize {
    fs::read_dir("/proc/self/task")
        .expect("this process' threads")
        .filter_map(|task| fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .filter(|comm| comm.trim_end() == name)
        .count()
}

/// Elsewhere there is no `/proc`, so the thread assertions degrade to
/// nothing rather than to a lie; the joins they follow are still exercised.
#[cfg(not(target_os = "linux"))]
fn threads_named(_name: &str) -> usize { 0 }

/// Await every thread called `name` going away, bounded by [`DEADLINE`].
///
/// A thread ending is not a future anybody can await, so this is the one
/// shape a poll is allowed to take here: a condition, not a duration. It
/// returns the moment the condition holds, so a healthy run pays one tick.
///
/// It is also the *exact* assertion, where an immediate read is not — even
/// after a join. `pthread_join` returns as soon as the kernel clears the
/// child's `CLONE_CHILD_CLEARTID` futex, which is before the task is
/// released and its `/proc/self/task/<tid>` entry unlinked; measured in
/// den-core's suite, the entry outlives the join by well under a
/// millisecond about three runs in a hundred.
async fn no_threads_named(name: &str) {
    const TICK: Duration = Duration::from_millis(1);
    time::timeout(DEADLINE, async {
        while threads_named(name) > 0 {
            time::sleep(TICK).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} thread(s) named {name} outlived it", threads_named(name)));
}

const ECHO: (&str, &str) = (
    "echo.js",
    "self.onmessage = (event) => postMessage(`echo:${event.data}`);",
);
/// Posts once, then never yields the interpreter again: only the interrupt
/// handler can end this one.
const SPIN: (&str, &str) = ("spin.js", r#"postMessage("spinning"); while (true) {}"#);

#[tokio::test(flavor = "multi_thread")]
async fn a_classic_worker_echoes_a_message_across_the_thread_boundary() {
    let fixture = Fixture::new("echo", &[ECHO]).await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/a_classic_worker_echoes_a_message_across_the_thread_boundary.\
             js"
        ))
        .await;
    assert_eq!(reply, "echo:ping");
    fixture.shutdown().await;
    fixture.settle().await;
}

/// HTML §10.2.4 step 2.13: the worker's message queue opens only after its
/// script has run, so a message the parent posts first is *queued*, not
/// delivered to a handler that does not exist yet.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_posted_before_the_script_finished_is_delivered_to_it() {
    let fixture = Fixture::new("queued", &[(
        "late.js",
        include_str!(
            "../fixtures/unit/worker/\
             a_message_posted_before_the_script_finished_is_delivered_to_it.js"
        ),
    )])
    .await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_message_posted_before_the_script_finished_is_delivered_to_it_2.js"
        ))
        .await;
    assert_eq!(reply, "late:early:true");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn terminate_stops_a_worker_that_never_yields() {
    let fixture = Fixture::new("terminate", &[SPIN]).await;
    let started: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/terminate_stops_a_worker_that_never_yields.js"
        ))
        .await;
    assert_eq!(started, "spinning");
    // The join is the assertion: a worker the interrupt handler could not
    // reach would still be spinning here.
    fixture.shutdown().await;
    fixture.settle().await;
}

/// HTML §10.2.1.2 "close a worker": the task that called `close()` runs to
/// its end — so the message posted before it still arrives — and then the
/// worker is gone, which is what lets the parent's runtime go idle with no
/// `terminate()` anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn close_from_inside_the_worker_delivers_the_last_message_and_ends_it() {
    let fixture = Fixture::new("close", &[(
        "close.js",
        r#"postMessage("bye"); close(); postMessage("after");"#,
    )])
    .await;
    let last: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             close_from_inside_the_worker_delivers_the_last_message_and_ends_it.js"
        ))
        .await;
    assert_eq!(last, "bye");
    fixture.settle().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_uncaught_error_becomes_an_error_event_on_the_worker_object() {
    let fixture = Fixture::new("uncaught", &[(
        "throw.js",
        "// line 1\n// line 2\nthrow new TypeError(\"boom\");\n",
    )])
    .await;
    let reported: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             an_uncaught_error_becomes_an_error_event_on_the_worker_object.js"
        ))
        .await;
    assert_eq!(reported, "boom|true|3|true|true");
    fixture.shutdown().await;
}

/// HTML §8.1.8.1: the global's `onerror` takes five positional arguments
/// and cancels the event by returning `true`, which ends the chain before
/// the parent ever hears about it.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_onerror_returning_true_suppresses_the_parent_error_event() {
    let fixture = Fixture::new("suppressed", &[(
        "suppress.js",
        include_str!(
            "../fixtures/unit/worker/\
             a_worker_onerror_returning_true_suppresses_the_parent_error_event.js"
        ),
    )])
    .await;
    let seen: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_worker_onerror_returning_true_suppresses_the_parent_error_event_2.js"
        ))
        .await;
    assert_eq!(seen, "caught:hidden:5:true|pong:ping|0");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn import_scripts_runs_a_script_in_the_worker_global_relative_to_it() {
    let fixture = Fixture::new("import-scripts", &[
        ("helper.js", r#"globalThis.helped = "helper";"#),
        (
            "importer.js",
            r#"
                importScripts("./helper.js");
                self.onmessage = () => postMessage(`${helped}:${typeof importScripts}`);
                "#,
        ),
    ])
    .await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             import_scripts_runs_a_script_in_the_worker_global_relative_to_it.js"
        ))
        .await;
    assert_eq!(reply, "helper:function");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_module_worker_runs_through_the_engine_loader_chain() {
    let fixture = Fixture::new("module", &[
        ("lib.js", r#"export const greeting = "from a module";"#),
        (
            "module.js",
            r#"
                import { greeting } from "./lib.js";
                self.onmessage = () => postMessage(`${greeting}:${typeof importScripts}`);
                "#,
        ),
    ])
    .await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/a_module_worker_runs_through_the_engine_loader_chain.js"
        ))
        .await;
    assert_eq!(reply, "from a module:function");
    fixture.shutdown().await;
}

/// Nesting needs nothing of its own: a worker realm gets the same host and
/// the same `Worker` global its parent has, and a base URL of its own
/// script's directory.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_can_spawn_a_worker() {
    let fixture = Fixture::new("nested", &[
        (
            "inner.js",
            "self.onmessage = (event) => postMessage(`inner:${event.data}`);",
        ),
        (
            "outer.js",
            include_str!("../fixtures/unit/worker/a_worker_can_spawn_a_worker.js"),
        ),
    ])
    .await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/a_worker_can_spawn_a_worker_2.js"
        ))
        .await;
    assert_eq!(reply, "outer:inner:ping");
    fixture.shutdown().await;
}

/// The rule den hangs on without (docs/research/11 §2.1 rule 2, test I-18):
/// a worker that registered no `message` listener cannot observe anything
/// its parent sends, so its own end of the port stops keeping its event
/// loop alive and the thread ends by itself — no `close()`, no
/// `terminate()`. Node exits on the identical program; before this rule den
/// did not.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_listens_to_nothing_lets_its_realm_go_idle() {
    let fixture = Fixture::new("silent", &[("silent.js", "globalThis.ran = true;")]).await;
    fixture
        .eval::<()>(r#"globalThis.worker = new Worker("./silent.js");"#)
        .await;
    // The parent goes idle only once the worker thread has exited: its
    // fault pump lives exactly as long as the thread does.
    fixture.settle().await;
    fixture.shutdown().await;
}

/// The other half of the rule (test I-27): the listener that arrives and
/// leaves again. While it is there the worker is alive; when the last one
/// goes, the worker ends like one that never had any.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_drops_its_last_listener_ends() {
    let fixture = Fixture::new("fickle", &[(
        "fickle.js",
        include_str!("../fixtures/unit/worker/a_worker_that_drops_its_last_listener_ends.js"),
    )])
    .await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/a_worker_that_drops_its_last_listener_ends_2.js"
        ))
        .await;
    assert_eq!(reply, "echo:ping");
    // No terminate() anywhere: the worker let go of its own port, which is
    // what lets both realms go idle.
    fixture.settle().await;
    fixture.shutdown().await;
}

/// HTML §9.4.4's port message queue, on the parent's end: a message the
/// worker posts before this realm attaches a listener waits in the channel
/// for it. Dispatching it into a `Worker` that nobody is listening to would
/// drop it — which is what made a worker that posts from its top level
/// intermittently hang its parent.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_posted_before_the_parent_listens_is_queued_for_it() {
    let fixture = Fixture::new("eager", &[(
        "eager.js",
        include_str!(
            "../fixtures/unit/worker/a_message_posted_before_the_parent_listens_is_queued_for_it.\
             js"
        ),
    )])
    .await;
    let queued: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_message_posted_before_the_parent_listens_is_queued_for_it_2.js"
        ))
        .await;
    assert_eq!(queued, "posted before anyone listened");
    fixture.shutdown().await;
}

/// `terminate()` on its own, with nothing left to rescue it.
///
/// Every other terminate test here is saved twice over: `Worker.terminate`
/// also closes the parent's end of the port, which a worker sitting in its
/// message pump notices, and the `shutdown` that follows cancels the very
/// same token again. This worker is inside a loop that never returns to its
/// event loop, so the port close is invisible to it, and nothing is shut
/// down until the runtime has already been made to go idle — which it can
/// only do once the fault pump has seen the cancellation
/// `NativeWorker::terminate` is the sole source of.
#[tokio::test(flavor = "multi_thread")]
async fn terminate_alone_stops_a_worker_that_can_never_see_its_port() {
    let fixture = Fixture::new("terminate-alone", &[SPIN]).await;
    let started: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/terminate_alone_stops_a_worker_that_can_never_see_its_port.js"
        ))
        .await;
    assert_eq!(started, "spinning");
    // Only Linux's /proc-backed helper can prove this intermediate state.
    #[cfg(target_os = "linux")]
    assert!(
        threads_named("den-worker:tttt") > 0,
        "the worker is running"
    );

    fixture.eval::<()>("worker.terminate();").await;
    // The parent's fault pump ends only when the worker's token is
    // cancelled or the worker thread exits, and a spinning worker does
    // neither by itself.
    fixture.settle().await;
    no_threads_named("den-worker:tttt").await;
    fixture.shutdown().await;
}

/// `WorkerRegistry::register` reaps as it records: a realm that spawns
/// workers in a loop keeps one `JoinHandle` per *live* worker, not one per
/// worker it has ever had.
#[tokio::test(flavor = "multi_thread")]
async fn the_registry_forgets_the_workers_that_have_already_finished() {
    /// Enough that an accumulating registry is unmistakable, few enough to
    /// stay a blink.
    const ROUNDS: usize = 8;
    /// What a reaping registry may still be holding: the worker just
    /// registered, plus at most one predecessor whose thread has returned
    /// but has not been marked finished yet.
    const TOLERATED: usize = 2;

    let fixture = Fixture::new("registry", &[("silent.js", "globalThis.ran = true;")]).await;
    // A worker with no listeners ends by itself, so `settle` is the proof
    // that the previous one is gone rather than a wait for it. The reap
    // happens inside `register`, so the count is read after the spawn.
    let mut registered = 0;
    for _ in 0..ROUNDS {
        fixture
            .eval::<()>(r#"globalThis.worker = new Worker("./silent.js");"#)
            .await;
        fixture.settle().await;
        registered = fixture.registered_workers().await;
    }
    assert!(
        registered <= TOLERATED,
        "the registry kept {registered} handles across {ROUNDS} finished workers"
    );
    fixture.shutdown().await;
}

/// The re-entrancy guard in `installWorkerScope`: an exception thrown by
/// the very handler that is reporting one is *printed*, not reported again.
///
/// So the chain terminates at depth two — the parent hears the handler's
/// own failure, then the original — and the third throw is printed to
/// stderr, which is expected output rather than a failure. Without the
/// guard the chain re-enters itself for as long as the stack lasts and the
/// parent hears the same failure once per level.
#[tokio::test(flavor = "multi_thread")]
async fn a_throwing_worker_onerror_reports_the_original_error_exactly_once() {
    let fixture = Fixture::new("reentrant-onerror", &[(
        "reentrant.js",
        include_str!(
            "../fixtures/unit/worker/\
             a_throwing_worker_onerror_reports_the_original_error_exactly_once.js"
        ),
    )])
    .await;
    let seen: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_throwing_worker_onerror_reports_the_original_error_exactly_once_2.js"
        ))
        .await;
    assert_eq!(seen, "alive:one|alive:two|from onerror,the original");
    fixture.shutdown().await;
}

/// `Worker.prototype.onmessageerror`: a payload this realm cannot rebuild
/// reaches the `Worker` as a `messageerror`, and assigning that handler is
/// itself enough to make the parent's end of the port deliver.
#[tokio::test(flavor = "multi_thread")]
async fn a_payload_the_parent_cannot_rebuild_becomes_messageerror_on_the_worker() {
    let fixture = Fixture::new("parent-messageerror", &[(
        "bad.js",
        // A clone tag whose revival throws on the far side: a DataView
        // cannot be built past the end of its buffer.
        include_str!(
            "../fixtures/unit/worker/\
             a_payload_the_parent_cannot_rebuild_becomes_messageerror_on_the_worker.js"
        ),
    )])
    .await;
    let seen: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_payload_the_parent_cannot_rebuild_becomes_messageerror_on_the_worker_2.js"
        ))
        .await;
    assert_eq!(seen, "messageerror:null");
    fixture.shutdown().await;
}

/// `shutdown` joins, it does not merely ask: when it returns, every thread
/// each worker had — the one den started and the ones its own tokio runtime
/// started under the same name — is gone. Both workers here are
/// uninterruptible from JS, so only the interrupt handler and the join can
/// end them.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_returns_with_every_thread_of_every_worker_already_gone() {
    let script = format!("{}/echo.js", BareHost::SLOW_DIRECTORY);
    let fixture = Fixture::new("threads", &[(&script, ECHO.1)]).await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             shutdown_returns_with_every_thread_of_every_worker_already_gone.js"
        ))
        .await;
    // The round trip is the proof that the worker has an engine, a tokio
    // runtime and the threads that come with it.
    assert_eq!(reply, "echo:up");
    // Only Linux's /proc-backed helper can count the worker's runtime threads.
    #[cfg(target_os = "linux")]
    assert!(
        threads_named("den-worker:cccc") >= 2,
        "the worker should have its own thread and its runtime's, got {}",
        threads_named("den-worker:cccc")
    );
    fixture.shutdown().await;
    // `shutdown` joined the worker, and the worker joined its own runtime
    // before letting itself be joined — so this waits on nothing but
    // `/proc` catching up with a fact that is already true (see
    // [`no_threads_named`]).
    no_threads_named("den-worker:cccc").await;
}

/// A realm that simply goes away — no `terminate()`, no [`shutdown`] — must
/// still stop what it started: the registry is realm userdata, so dropping
/// the realm cancels one level down. The worker posted its message and then
/// stopped yielding the interpreter, so only its interrupt handler can end
/// it, and only the cancellation can arm that.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_parent_realm_stops_a_spinning_worker() {
    let fixture = Fixture::new("realm-drop", &[SPIN]).await;
    fixture
        .eval::<()>(include_str!(
            "../fixtures/unit/worker/dropping_the_parent_realm_stops_a_spinning_worker.js"
        ))
        .await;
    let Fixture { runtime, context } = fixture;
    // Context first, then the runtime: userdata — the registry with it —
    // is freed with the runtime, and the context holds a handle on one.
    drop(context);
    drop(runtime);
    no_threads_named("den-worker:dddd").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_worker_keeps_the_runtime_busy_until_it_is_terminated() {
    let fixture = Fixture::new("lifetime", &[("idle.js", "self.onmessage = () => {};")]).await;
    let ready: String = fixture
        .eval(r#"globalThis.worker = new Worker("./idle.js"); "spawned""#)
        .await;
    assert_eq!(ready, "spawned");
    assert!(
        fixture.is_busy().await,
        "a live worker must keep idle() pending"
    );
    fixture.eval::<()>("worker.terminate();").await;
    fixture.settle().await;
    fixture.shutdown().await;
}

/// Structural, not wall-clock: worker B's round trip cannot complete while
/// worker A holds the interpreter unless they really are separate threads.
#[tokio::test(flavor = "multi_thread")]
async fn workers_run_in_parallel_with_each_other() {
    let fixture = Fixture::new("parallel", &[ECHO, SPIN]).await;
    let reply: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/workers_run_in_parallel_with_each_other.js"
        ))
        .await;
    assert_eq!(reply, "echo:parallel");
    fixture.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_and_joins_every_worker_the_realm_spawned() {
    let fixture = Fixture::new("shutdown", &[SPIN]).await;
    fixture
        .eval::<()>(include_str!(
            "../fixtures/unit/worker/shutdown_stops_and_joins_every_worker_the_realm_spawned.js"
        ))
        .await;
    // Neither worker will ever stop on its own; the bounded join inside
    // `shutdown` is the whole assertion.
    fixture.shutdown().await;
    fixture.settle().await;
}

/// The hard rule: nothing a worker does may reach the process as a panic —
/// every other thread here is holding a live QuickJS runtime. The panic
/// message this test provokes is printed by Rust's default hook and is
/// expected output, not a failure.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_thread_that_panics_reaches_the_parent_as_an_error_event() {
    let script = format!("{}/echo.js", BareHost::PANIC_DIRECTORY);
    let fixture = Fixture::new("panic", &[(&script, ECHO.1)]).await;
    let reported: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             a_worker_thread_that_panics_reaches_the_parent_as_an_error_event.js"
        ))
        .await;
    assert!(
        reported.contains("panicked") && reported.contains("the host panicked"),
        "the panic should name itself, got {reported:?}"
    );
    fixture.settle().await;
    fixture.shutdown().await;
}

/// v1 divergence, pinned: den's loaders only produce modules, so a classic
/// worker is read by this crate and only from a file.
#[tokio::test(flavor = "multi_thread")]
async fn an_http_classic_worker_is_a_type_error_pointing_at_the_module_type() {
    let fixture = Fixture::new("http-classic", &[ECHO]).await;
    let failures: String = fixture
        .eval(include_str!(
            "../fixtures/unit/worker/\
             an_http_classic_worker_is_a_type_error_pointing_at_the_module_type.js"
        ))
        .await;
    assert_eq!(failures, "TypeError|TypeError");
    fixture.shutdown().await;
    fixture.settle().await;
}
