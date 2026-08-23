//! The `Worker` itself: one OS thread, one engine, one script, and the error
//! chain between that thread and the realm that started it.
//!
//! Everything a worker needs from the embedder is [`WorkerHost`]; everything
//! it hands back is bytes. Nothing with a `'js` lifetime crosses the thread
//! boundary — the parent and the worker each own a runtime, and they speak
//! only through the channel a [`PortHandle`] pair is made of.
//!
//! The JS-visible [`Worker`] is an EventTarget wrapper around [`NativeWorker`]
//! and the outside [`NativePort`]. `DedicatedWorkerGlobalScope` is the same
//! for the worker's global object.
//!
//! See docs/research/09-rquickjs-threads-and-event-loop.md §4, §6, §7 and
//! docs/research/11-workers-den-integration-and-tests.md §9.

use std::{
    cell::RefCell,
    panic::{self, AssertUnwindSafe},
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use den_stdlib_core::report::print_exception;
#[cfg(feature = "transpile")]
use den_transpiler_oxc::{EasyOxcTranspiler, IsModule, infer_transpile_syntax_by_extension};
use rquickjs::{
    AsyncContext, Class, Coerced, Ctx, Error, Exception, FromJs, Function, IntoJs, JsLifetime,
    Object, Persistent, Promise, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    context::EvalOptions,
    function::{Func, FuncArg, Opt, Rest, This},
    object::Property,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    events::{ErrorEvent, EventTarget, define_event_handler, dispatch_trusted, inherit},
    host::{BaseUrl, HostHandle, WorkerEngine, WorkerHost},
    message::clone::split_transfer,
    port::{NativePort, pair, track_message_listeners},
    report::{report_exception, sink_hook},
    transport::PortHandle,
};

/// Hidden own property: the worker global's closing flag (`close()`).
const CLOSING_SLOT: &str = "\0den:worker-closing";

/// How long [`shutdown`] gives a worker thread to notice its cancellation.
///
/// A worker that is running JS stops at its next interrupt poll and one parked
/// on a future stops when its `idle()` is dropped, both quickly; a worker
/// inside a blocking load cannot be stopped at all, and waiting forever would
/// turn one stuck fetch into a stuck process.
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);
/// The same bound, spent on the other side of the join: how long a worker
/// thread waits for its *own* tokio runtime before returning. It is what makes
/// joining the thread mean "every thread this worker had is gone" rather than
/// "the thread that started them is gone".
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = JOIN_TIMEOUT;

/// The two ways HTML knows to run a worker script (`WorkerOptions.type`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Classic,
    Module,
}

impl ScriptKind {
    /// WebIDL enum conversion: anything but the two names is a `TypeError`,
    /// thrown before a thread is started.
    fn parse(ctx: &Ctx<'_>, kind: &str) -> Result<Self> {
        match kind {
            "classic" => Ok(Self::Classic),
            "module" => Ok(Self::Module),
            other => {
                Err(Exception::throw_type(
                    ctx,
                    &format!("Worker: '{other}' is not a valid worker type"),
                ))
            }
        }
    }

    /// Resolve a worker specifier against the realm's API base URL — the
    /// directory of the script that called `new Worker`, never the process'
    /// working directory (HTML §10.2.6.3 step 2).
    fn resolve(self, ctx: &Ctx<'_>, url: &str) -> Result<Url> {
        let base = ctx
            .userdata::<BaseUrl>()
            .map(|base| base.0.clone())
            .unwrap_or_default();
        let script = Url::options()
            .base_url(Url::parse(&base).ok().as_ref())
            .parse(url)
            // HTML asks for a `"SyntaxError"` DOMException here. `error.name`
            // says `SyntaxError` either way, and this crate builds
            // DOMExceptions only where a test can tell the difference.
            .map_err(|error| {
                Exception::throw_syntax(ctx, &format!("Worker: cannot resolve {url:?}: {error}"))
            })?;
        // v1 divergence (pinned by a test): den's loaders only ever produce
        // *modules*, so a classic script is read by this crate — and reading it
        // over the network is a whole fetch stack that the module type already
        // has (docs/research/11 §7.3).
        match (self, script.scheme()) {
            (Self::Classic, scheme) if scheme != "file" => {
                Err(Exception::throw_type(
                    ctx,
                    "classic workers must be files; use { type: \"module\" }",
                ))
            }
            _ => Ok(script),
        }
    }
}

/// What a worker tells its parent about an error nobody in the worker claimed.
///
/// Plain `Send` data: `error` is missing on purpose, because an `Error` does
/// not survive serialisation (docs/research/10 §4.5), and the location is
/// parsed out of the exception's stack because quickjs-ng keeps it nowhere
/// else (docs/research/08 §1.4).
#[derive(Debug, Default)]
struct WorkerFault {
    message:  String,
    filename: String,
    lineno:   u32,
    colno:    u32,
}

impl WorkerFault {
    /// A fault with no script location: the engine itself failed, or the
    /// thread did.
    fn from_message(message: String) -> Self {
        Self {
            message,
            ..Self::default()
        }
    }

    /// A fault from a thrown value, the way den's entry point reads one
    /// (`src/main.rs`): the exception's own message when it is one, its string
    /// coercion when it is not.
    fn from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Self {
        let exception = value.as_exception();
        let message = exception
            .and_then(Exception::message)
            .or_else(|| {
                Coerced::<String>::from_js(ctx, value.clone())
                    .ok()
                    .map(|Coerced(text)| text)
            })
            .unwrap_or_else(|| "uncaught error".to_owned());
        let (filename, lineno, colno) = exception
            .and_then(Exception::stack)
            .as_deref()
            .map(Self::locate)
            .unwrap_or_default();
        Self {
            message,
            filename,
            lineno,
            colno,
        }
    }

    /// Turn a failed call into a fault, taking the pending exception with it so
    /// that it cannot resurface at the next unrelated entry into the context.
    fn take<'js>(ctx: &Ctx<'js>, error: Error) -> Self {
        match error {
            Error::Exception => Self::from_value(ctx, &ctx.catch()),
            other => Self::from_message(other.to_string()),
        }
    }

    /// The first frame of a quickjs-ng stack, which is one of
    /// `    at name (file:line:col)` or `    at file:line:col`. Anything else
    /// leaves the spec's defaults of `""` and `0` in place.
    fn locate(stack: &str) -> (String, u32, u32) {
        stack
            .lines()
            .find_map(|line| {
                let frame = line.trim().strip_prefix("at ")?;
                let location = frame
                    .rsplit_once('(')
                    .map_or(frame, |(_, inside)| inside.trim_end_matches(')'));
                let (head, colno) = location.rsplit_once(':')?;
                let (filename, lineno) = head.rsplit_once(':')?;
                Some((
                    filename.to_owned(),
                    lineno.parse().ok()?,
                    colno.parse().ok()?,
                ))
            })
            .unwrap_or_default()
    }
}

/// A fault crosses into JS as the four members of an `ErrorEvent` that survive
/// a thread — which is what the prelude's half of the report chain passes on.
impl<'js> IntoJs<'js> for WorkerFault {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let located = Object::new(ctx.clone())?;
        located.set("message", self.message)?;
        located.set("filename", self.filename)?;
        located.set("lineno", self.lineno)?;
        located.set("colno", self.colno)?;
        Ok(located.into_value())
    }
}

/// Why a worker's script did not run to completion. The three cases differ in
/// what happens to the worker afterwards, which is the whole reason to tell
/// them apart.
enum ScriptError {
    /// The script never ran: unresolvable, unreadable or unparseable. HTML
    /// "run a worker" discards the environment, so the message queue never
    /// opens and the thread falls through to its teardown.
    Load(WorkerFault),
    /// The script ran and threw. Per HTML §10.2.5 the worker keeps going: the
    /// error is reported, and its queues open as if nothing had happened.
    Uncaught(WorkerFault),
    /// `terminate()` landed mid-script. The interrupt is not an error and
    /// there is nobody left to tell about it.
    Terminated,
}

impl ScriptError {
    /// Sort a failed evaluation into the three outcomes above.
    fn from_eval(ctx: &Ctx<'_>, error: Error) -> Self {
        let Error::Exception = error else {
            return Self::Load(WorkerFault::from_message(error.to_string()));
        };
        let value = ctx.catch();
        if value.is_uncatchable_error() {
            return Self::Terminated;
        }
        match Self::is_load_failure(&value) {
            true => Self::Load(WorkerFault::from_value(ctx, &value)),
            false => Self::Uncaught(WorkerFault::from_value(ctx, &value)),
        }
    }

    /// Whether the failure happened *before* any of the script ran. rquickjs
    /// words resolver and loader failures itself, and a script that did not
    /// parse throws a `SyntaxError` from the evaluation call.
    ///
    /// ponytail ceiling: a script that deliberately throws a `SyntaxError` of
    /// its own is read as a load failure, and the worker ends instead of
    /// carrying on. Distinguishing them needs the parse to be a separate step.
    fn is_load_failure(value: &Value<'_>) -> bool {
        let named_syntax_error = value
            .as_object()
            .and_then(|error| error.get::<_, String>("name").ok())
            .is_some_and(|name| name == "SyntaxError");
        let message = value
            .as_exception()
            .and_then(Exception::message)
            .unwrap_or_default();
        named_syntax_error
            || message.starts_with("Error resolving module")
            || message.starts_with("Error loading module")
            || message.starts_with("could not load module")
    }
}

/// The natives bag `events::install` parks as the exception sink. Worker
/// scope installation replaces `reportException` and `escalate` on it.
struct NativesBag(Persistent<Object<'static>>);

// SAFETY: `Persistent` owns its value and is tied to the runtime, not a scope.
unsafe impl<'js> JsLifetime<'js> for NativesBag {
    type Changed<'to> = NativesBag;
}

fn natives<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    ctx.userdata::<NativesBag>()
        .ok_or_else(|| Exception::throw_internal(ctx, "den:worker is not installed"))?
        .0
        .clone()
        .restore(ctx)
        .map_err(|_| Exception::throw_internal(ctx, "den:worker natives vanished"))
}

fn transfer_list(options: Option<Value<'_>>) -> Option<Value<'_>> {
    match options {
        Some(options) if options.is_array() => Some(options),
        Some(options) if options.is_object() => options
            .as_object()
            .and_then(|object| object.get("transfer").ok())
            .filter(|value: &Value| !value.is_undefined() && !value.is_null()),
        _ => None,
    }
}

fn bind<'js>(function: &Function<'js>, this: Value<'js>) -> Result<Function<'js>> {
    let bind: Function<'js> = function.get("bind")?;
    bind.call((This(function.clone()), this))
}

/// HTML §8.1.4.6 step 3: fire a cancelable `error` at `target`. `true` means
/// something called `preventDefault()` — the error was claimed.
fn report_error_at<'js>(
    ctx: &Ctx<'js>,
    target: Value<'js>,
    message: String,
    filename: String,
    lineno: u32,
    colno: u32,
) -> Result<bool> {
    let init = Object::new(ctx.clone())?;
    init.set("message", message)?;
    init.set("filename", filename)?;
    init.set("lineno", lineno)?;
    init.set("colno", colno)?;
    init.set("cancelable", true)?;
    let event = Class::instance(
        ctx.clone(),
        ErrorEvent::new(
            ctx.clone(),
            "error".into_js(ctx)?,
            Opt(Some(init.into_value())),
        )?,
    )?;
    Ok(!dispatch_trusted(ctx.clone(), target, event.into_value())?)
}

fn escalate(ctx: &Ctx<'_>, message: String, filename: String, lineno: u32, colno: u32) {
    if let Some(hook) = sink_hook(ctx, "escalate") {
        if let Err(error) = hook.call::<_, ()>((message, filename, lineno, colno)) {
            match error {
                Error::Exception => report_exception(ctx, &ctx.catch()),
                other => eprintln!("{other}"),
            }
        }
        return;
    }
    let text = format!("{message}\n    at {filename}:{lineno}:{colno}");
    if let Ok(value) = text.into_js(ctx) {
        print_exception(ctx, &value);
    }
}

fn worker_options<'js>(ctx: &Ctx<'js>, options: Option<Value<'js>>) -> Result<(String, String)> {
    let object = options.as_ref().and_then(Value::as_object);
    let kind = match object {
        Some(options) => {
            let value: Value<'js> = options.get("type")?;
            if value.is_undefined() {
                "classic".to_owned()
            } else {
                Coerced::<String>::from_js(ctx, value)?.0
            }
        }
        None => "classic".to_owned(),
    };
    if kind != "classic" && kind != "module" {
        return Err(Exception::throw_type(
            ctx,
            &format!("Worker constructor: '{kind}' is not a valid worker type"),
        ));
    }
    let name = match object {
        Some(options) => {
            let value: Value<'js> = options.get("name")?;
            if value.is_undefined() {
                String::new()
            } else {
                Coerced::<String>::from_js(ctx, value)?.0
            }
        }
        None => String::new(),
    };
    Ok((kind, name))
}

/// The stop token of the realm a context belongs to — what `Engine::stop()`
/// cancels.
///
/// Every worker a realm spawns gets a *child* of it, so that stopping a realm
/// interrupts the workers it started instead of leaving them running under a
/// realm that is already gone. Stored by whoever builds the context: den-core
/// for the main realm, `WorkerThread::install_scope` for a worker's own. A
/// context without one still spawns workers — their tokens are simply roots,
/// reachable only through `terminate()` and [`shutdown`].
#[derive(Clone, JsLifetime)]
pub struct RealmStop(pub CancellationToken);

/// The threads one realm spawned, so that shutting that realm down can stop and
/// reap them rather than let the process exit around them.
#[derive(Default, JsLifetime)]
struct WorkerRegistry {
    threads: RefCell<Vec<WorkerHandle>>,
}

struct WorkerHandle {
    stop: CancellationToken,
    join: JoinHandle<()>,
}

impl WorkerRegistry {
    /// Record `handle`, dropping the handles of threads that have already
    /// exited — a realm that spawns workers in a loop must not accumulate
    /// them.
    fn register(ctx: &Ctx<'_>, handle: WorkerHandle) -> Result<()> {
        if ctx.userdata::<Self>().is_none() {
            ctx.store_userdata(Self::default())
                .map_err(|error| Exception::throw_internal(ctx, &error.to_string()))?;
        }
        let registry = ctx
            .userdata::<Self>()
            .ok_or_else(|| Exception::throw_internal(ctx, "the worker registry vanished"))?;
        let mut threads = registry
            .threads
            .try_borrow_mut()
            .map_err(|_| Exception::throw_internal(ctx, "the worker registry is busy"))?;
        threads.retain(|thread| !thread.join.is_finished());
        threads.push(handle);
        Ok(())
    }

    fn take(ctx: &Ctx<'_>) -> Vec<WorkerHandle> {
        ctx.userdata::<Self>()
            .and_then(|registry| {
                registry
                    .threads
                    .try_borrow_mut()
                    .ok()
                    .map(|mut threads| std::mem::take(&mut *threads))
            })
            .unwrap_or_default()
    }
}

/// Stop and reap every worker `context` spawned.
///
/// Called by the embedder when its realm ends, and by a worker thread for its
/// own children — so a tree of nested workers is torn down bottom-up, each
/// parent outliving the workers it started.
pub async fn shutdown(context: &AsyncContext) {
    let threads = context.with(|ctx| WorkerRegistry::take(&ctx)).await;
    for thread in &threads {
        thread.stop.cancel();
    }
    // A real join, not a poll for `is_finished`: a worker thread shuts its own
    // tokio runtime down before it returns (see [`WorkerThread::serve`]), so a
    // joined thread is a worker whose threads are *all* gone — which is what
    // an embedder that is about to unload the process needs. A panicking
    // worker joins like any other; its panic already reached the parent as an
    // error event.
    let pending = threads.len();
    let joined = tokio::task::spawn_blocking(move || {
        threads
            .into_iter()
            .for_each(|thread| drop(thread.join.join()));
    });
    // Bounded, because a worker inside a blocking load never observes its
    // cancellation at all and one stuck fetch must not become a stuck process.
    // The blocking task cannot be cancelled, so what the timeout buys is this
    // caller moving on, not the thread going away.
    if time::timeout(JOIN_TIMEOUT, joined).await.is_err() {
        tracing::warn!(
            "{pending} worker thread(s) did not stop within {JOIN_TIMEOUT:?}; detaching them"
        );
    }
}

/// The parent realm's handle on one worker thread.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "NativeWorker")]
pub struct NativeWorker {
    /// Cancels the worker: the engine's interrupt handler observes it (a
    /// running script), and so does the `closing` child token the thread body
    /// waits on (a parked one).
    #[qjs(skip_trace)]
    stop: CancellationToken,
}

impl NativeWorker {
    /// Deliver the worker's faults to the parent-side half of the error chain.
    ///
    /// The pump ends when the worker thread drops its sender — which it does
    /// by exiting — or at `terminate()`, whichever comes first.
    async fn pump_faults<'js>(
        ctx: Ctx<'js>,
        mut inbox: UnboundedReceiver<WorkerFault>,
        stop: CancellationToken,
        on_fault: Function<'js>,
    ) {
        while let Some(Some(fault)) = stop.run_until_cancelled(inbox.recv()).await {
            let dispatched =
                on_fault.call::<_, ()>((fault.message, fault.filename, fault.lineno, fault.colno));
            // The handler for "nobody handled an error" throwing is the end of
            // the line; print it rather than lose it.
            if let Err(error) = dispatched {
                match error {
                    Error::Exception => report_exception(&ctx, &ctx.catch()),
                    other => eprintln!("{other}"),
                }
            }
        }
    }
}

#[rquickjs::methods]
impl NativeWorker {
    /// `nativeWorker.terminate()` — HTML §10.2.4 "terminate a worker": the
    /// interrupt handler aborts whatever the worker is running without letting
    /// a `finally` block observe it, and the cancelled token releases a worker
    /// that is merely parked. Idempotent.
    pub fn terminate(&self) {
        self.stop.cancel();
    }
}

/// Spawn a worker thread (HTML §10.2.6.3 step 10, "run a worker in parallel").
pub fn spawn<'js>(
    ctx: Ctx<'js>,
    url: String,
    kind: String,
    name: String,
    port: Class<'js, NativePort>,
    on_fault: Function<'js>,
) -> Result<Class<'js, NativeWorker>> {
    let kind = ScriptKind::parse(&ctx, &kind)?;
    let script = kind.resolve(&ctx, &url)?;
    // The worker realm's own base URL is its script's directory, which is what
    // makes a nested `new Worker("./x.js")` mean what it says.
    let base = BaseUrl(script.join(".").map(String::from).unwrap_or_default());
    let host = ctx
        .userdata::<HostHandle>()
        .map(|host| host.0.clone())
        .ok_or_else(|| Exception::throw_type(&ctx, "this realm cannot spawn workers"))?;
    // The port the prelude just built for the worker's end: its channel moves
    // to the thread, leaving the parent's end entangled with it.
    let channel = port
        .try_borrow()
        .ok()
        .and_then(|port| port.take_handle())
        .ok_or_else(|| Exception::throw_type(&ctx, "the worker's port is not usable"))?;

    let (faults, fault_inbox) = mpsc::unbounded_channel();
    // A child of the spawning realm's token, so that stopping that realm — not
    // only `terminate()` and `shutdown` — reaches this worker and, through its
    // own `RealmStop`, everything it spawns in turn.
    let stop = ctx
        .userdata::<RealmStop>()
        .map_or_else(CancellationToken::new, |realm| realm.0.child_token());
    let thread = WorkerThread {
        host,
        stop: stop.clone(),
        script,
        kind,
        name: name.clone(),
        base,
        channel,
        faults,
    };
    let join = thread::Builder::new()
        .name(format!("den-worker:{name}"))
        .spawn(move || thread.run())
        .map_err(|error| {
            Exception::throw_internal(&ctx, &format!("cannot start a worker thread: {error}"))
        })?;

    WorkerRegistry::register(
        &ctx,
        WorkerHandle {
            stop: stop.clone(),
            join,
        },
    )?;
    ctx.spawn(NativeWorker::pump_faults(
        ctx.clone(),
        fault_inbox,
        stop.clone(),
        on_fault,
    ));
    Class::instance(ctx, NativeWorker { stop })
}

/// Everything one worker thread needs, in the order its body uses it.
struct WorkerThread {
    host:    Arc<dyn WorkerHost>,
    stop:    CancellationToken,
    script:  Url,
    kind:    ScriptKind,
    name:    String,
    base:    BaseUrl,
    channel: PortHandle,
    faults:  UnboundedSender<WorkerFault>,
}

impl WorkerThread {
    /// The thread's entry point.
    fn run(self) {
        let faults = self.faults.clone();
        let name = self.name.clone();
        // A worker that panics must reach its parent as an error, not take the
        // process down: every other thread here is still holding a live
        // QuickJS runtime.
        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| self.serve())) {
            let reason = payload
                .downcast_ref::<&str>()
                .map(|reason| (*reason).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".to_owned());
            let _ = faults.send(WorkerFault::from_message(format!(
                "worker {name:?} panicked: {reason}"
            )));
        }
    }

    fn serve(self) {
        // ponytail: multi_thread with a single worker, because den's loaders
        // call `block_in_place`, which panics on the current-thread scheduler
        // (docs/research/09 §6.1). Fold it to current_thread the day they stop.
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name(format!("den-worker:{}", self.name))
            .enable_all()
            .build();
        let tokio = match tokio {
            Ok(tokio) => tokio,
            Err(error) => {
                let _ = self
                    .faults
                    .send(WorkerFault::from_message(error.to_string()));
                return;
            }
        };
        tokio.block_on(self.serve_engine());
        // Everything QuickJS is already dropped; the only thing that could
        // still be running is a blocking load nobody is waiting for. Waiting
        // for it *here* — on the thread the parent joins, and bounded — is what
        // makes `shutdown` deterministic: this runtime's worker and blocking
        // threads carry the same `den-worker:` name, so a parent that only
        // joined the outer thread would return with them still alive.
        tokio.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    }

    async fn serve_engine(self) {
        // `close()` cancels this one, `terminate()` cancels its parent — which
        // reaches this one too, and additionally arms the interrupt handler.
        let closing = self.stop.child_token();
        let engine = match self.host.build_engine(self.stop.clone(), self.base.clone()) {
            Ok(engine) => engine,
            Err(error) => {
                let _ = self
                    .faults
                    .send(WorkerFault::from_message(error.to_string()));
                return;
            }
        };

        let Self {
            script,
            kind,
            name,
            channel,
            faults,
            ..
        } = self;

        // One lock acquisition covers installing the global scope, running the
        // script and opening the message queue: no other thread may see this
        // realm half-built.
        let fault = engine
            .context
            .async_with({
                let closing = closing.clone();
                let faults = faults.clone();
                let stop = self.stop.clone();
                async move |ctx| {
                    // This realm's own token, for the workers this worker
                    // spawns: a tree of workers is cancelled from any node
                    // down. It has to be in place before the script runs,
                    // which is the first thing that can call `new Worker`.
                    if let Err(error) = ctx.store_userdata(RealmStop(stop)) {
                        return Some(WorkerFault::from_message(error.to_string()));
                    }
                    Self::boot(&ctx, channel, &name, kind, &script, &closing, faults).await
                }
            })
            .await;
        if let Some(fault) = fault {
            let _ = faults.send(fault);
        }

        // `idle()` stays pending while the message pump — or a timer, or a
        // fetch — lives, which *is* the lifetime rule: a worker ends at
        // `close()`, at `terminate()`, or when its parent hangs up and the pump
        // sees the channel close (docs/research/09 §2.2).
        closing.run_until_cancelled(engine.runtime.idle()).await;
        // Bottom-up: this worker's own children go before it does.
        shutdown(&engine.context).await;
        let WorkerEngine { runtime, context } = engine;
        // Order matters: every JS value dies with the context, and the runtime
        // frees the spawner — and the pump's captured callbacks with it — only
        // when it goes (docs/research/09 §6.3). Dropping the context also drops
        // the worker's end of the port, which is what tells the parent's pump
        // to stop and releases the parent's `idle()`.
        drop(context);
        drop(runtime);
    }

    /// Install the global scope, run the script, open the message queue.
    /// Returns whatever the parent still needs to hear about.
    async fn boot<'js>(
        ctx: &Ctx<'js>,
        channel: PortHandle,
        name: &str,
        kind: ScriptKind,
        script: &Url,
        closing: &CancellationToken,
        faults: UnboundedSender<WorkerFault>,
    ) -> Option<WorkerFault> {
        match Self::install_scope(ctx, channel, name, kind, script, closing, faults) {
            Ok(scope) => Self::run_and_report(ctx, &scope, kind, script).await,
            Err(error) => Some(WorkerFault::take(ctx, error)),
        }
    }

    /// Hand the prelude's installer the worker's port, name and the two things
    /// only Rust can do, and keep the two hooks it gives back.
    fn install_scope<'js>(
        ctx: &Ctx<'js>,
        channel: PortHandle,
        name: &str,
        kind: ScriptKind,
        script: &Url,
        closing: &CancellationToken,
        faults: UnboundedSender<WorkerFault>,
    ) -> Result<Object<'js>> {
        let port = Class::instance(ctx.clone(), NativePort::from_handle(channel))?;
        let hooks = Object::new(ctx.clone())?;
        hooks.set(
            "close",
            Func::from({
                let closing = closing.clone();
                move || closing.cancel()
            }),
        )?;
        hooks.set(
            "importScripts",
            Func::from({
                let base = script.clone();
                move |ctx: Ctx<'_>, urls: Vec<String>| Self::import_scripts(&ctx, &base, kind, urls)
            }),
        )?;
        // The two halves of HTML §8.1.4.6 "report an exception" that only Rust
        // can do: reading a thrown value's location out of its stack, and
        // reaching the parent. The prelude puts them together with the worker
        // global's own `onerror`, which is the half that belongs in JS.
        hooks.set(
            "locate",
            // The value carries its own context, which is also what keeps the
            // closure's one lifetime from splitting in two.
            Func::from(|value: Value<'_>| WorkerFault::from_value(value.ctx(), &value)),
        )?;
        hooks.set(
            "fault",
            Func::from(
                move |message: String, filename: String, lineno: u32, colno: u32| {
                    let _ = faults.send(WorkerFault {
                        message,
                        filename,
                        lineno,
                        colno,
                    });
                },
            ),
        )?;
        install_worker_scope(ctx, ctx.globals(), port, name, hooks)
    }

    async fn run_and_report<'js>(
        ctx: &Ctx<'js>,
        scope: &Object<'js>,
        kind: ScriptKind,
        script: &Url,
    ) -> Option<WorkerFault> {
        let outcome = match kind {
            ScriptKind::Classic => Self::run_classic(ctx, script),
            ScriptKind::Module => Self::run_module(ctx, script).await,
        };
        let fault = match outcome {
            Ok(()) => None,
            // Nothing ran and nothing will. HTML fires a plain `error` `Event`
            // at the `Worker` for this case; den sends the reason instead —
            // strictly more useful, and one code path fewer (v1 divergence).
            Err(ScriptError::Load(fault)) => return Self::report(ctx, scope, fault),
            Err(ScriptError::Uncaught(fault)) => Self::report(ctx, scope, fault),
            Err(ScriptError::Terminated) => return None,
        };
        // HTML §10.2.4 step 2.13, and the reason it comes last: the queue opens
        // only once the script has run, so a script that assigns `onmessage` on
        // its very last line still receives what the parent posted before it
        // existed.
        let started = scope
            .get::<_, Function<'js>>("start")
            .and_then(|start| start.call::<_, ()>(()));
        match started {
            Ok(()) => fault,
            Err(error) => Some(WorkerFault::take(ctx, error)),
        }
    }

    /// The worker half of HTML §8.1.4.6: the worker's own global hears about
    /// the error first, and only what nothing there cancelled travels on.
    fn report<'js>(ctx: &Ctx<'js>, scope: &Object<'js>, fault: WorkerFault) -> Option<WorkerFault> {
        let cancelled = scope
            .get::<_, Function<'js>>("reportError")
            .and_then(|report| {
                report.call::<_, bool>((
                    fault.message.clone(),
                    fault.filename.clone(),
                    fault.lineno,
                    fault.colno,
                ))
            });
        match cancelled {
            Ok(true) => None,
            Ok(false) => Some(fault),
            Err(error) => Some(WorkerFault::take(ctx, error)),
        }
    }

    fn run_classic(ctx: &Ctx<'_>, script: &Url) -> std::result::Result<(), ScriptError> {
        let source = Self::load(script)
            .map_err(|error| ScriptError::Load(WorkerFault::from_message(error)))?;
        ctx.eval_with_options::<(), _>(source, Self::classic_options(script))
            .map_err(|error| ScriptError::from_eval(ctx, error))
    }

    /// `global: true` because a classic script's top level *is* the global
    /// scope; `strict: false` because a classic script is sloppy unless it says
    /// otherwise; `promise: false` because top-level `await` is a module
    /// feature and, with it on, a synchronous throw would come back as a
    /// rejection instead of an `Err` (docs/research/09 §7.1).
    fn classic_options(script: &Url) -> EvalOptions {
        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = false;
        options.promise = false;
        options.filename = Some(script.to_string());
        options
    }

    async fn run_module(ctx: &Ctx<'_>, script: &Url) -> std::result::Result<(), ScriptError> {
        let mut options = EvalOptions::default();
        options.global = true;
        options.promise = true;
        options.strict = true;
        options.filename = Some(script.to_string());
        // The wrapper `Engine::run_file` uses, so a worker module reaches den's
        // resolvers and loaders — and its transpiler — exactly like an entry
        // point does. The specifier is already absolute, and `{:?}` escapes it
        // into a JS string literal.
        let source = format!("await import({:?})", script.as_str());
        let module = ctx
            .eval_with_options::<Promise, _>(source, options)
            .map_err(|error| ScriptError::from_eval(ctx, error))?;
        module
            .into_future::<Value<'_>>()
            .await
            .map(|_| ())
            .map_err(|error| ScriptError::from_eval(ctx, error))
    }

    /// `importScripts(...urls)` — HTML §10.3.1. Synchronous by definition, in
    /// order, relative to the worker's own script, and with "rethrow errors":
    /// the first URL that fails aborts the rest and the exception continues
    /// into the calling script.
    fn import_scripts(
        ctx: &Ctx<'_>,
        base: &Url,
        kind: ScriptKind,
        urls: Vec<String>,
    ) -> Result<()> {
        if let ScriptKind::Module = kind {
            return Err(Exception::throw_type(
                ctx,
                "importScripts is not available in a module worker",
            ));
        }
        // Step 3: every URL is parsed before any of them runs.
        let scripts = urls
            .iter()
            .map(|url| {
                base.join(url).map_err(|error| {
                    Exception::throw_syntax(
                        ctx,
                        &format!("importScripts: cannot resolve {url:?}: {error}"),
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for script in scripts {
            let source =
                Self::load(&script).map_err(|error| Exception::throw_message(ctx, &error))?;
            ctx.eval_with_options::<(), _>(source, Self::classic_options(&script))?;
        }
        Ok(())
    }

    /// Read one classic script and transpile it. Classic workers are file-only
    /// ([`ScriptKind::resolve`] refuses everything else), so this is a plain
    /// blocking read on the worker's own thread with nothing else to starve.
    fn load(script: &Url) -> std::result::Result<String, String> {
        let path = script
            .to_file_path()
            .map_err(|()| format!("{script} is not a file"))?;
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot load {script}: {error}"))?;
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("js");
        Self::transpile(source, extension)
    }

    /// `IsModule::Bool(false)`: the top level is a script, where `import` is a
    /// syntax error and `this` is the global — both of which a classic worker
    /// script is entitled to rely on.
    #[cfg(feature = "transpile")]
    fn transpile(source: String, extension: &str) -> std::result::Result<String, String> {
        let syntax = infer_transpile_syntax_by_extension(extension).unwrap_or_default();
        EasyOxcTranspiler
            .transpile(&source, syntax, IsModule::Bool(false), false)
            .map(|(source, _)| source)
            .map_err(|error| error.to_string())
    }

    #[cfg(not(feature = "transpile"))]
    fn transpile(source: String, _extension: &str) -> std::result::Result<String, String> {
        Ok(source)
    }
}

/// HTML §10.2.1 / §10.3.1: empty classes so `self instanceof EventTarget` and
/// `Object.prototype.toString.call(self)` say what the spec says. Not
/// constructible and not exported.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct WorkerGlobalScope {}

#[rquickjs::methods]
impl WorkerGlobalScope {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "WorkerGlobalScope"
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct DedicatedWorkerGlobalScope {}

#[rquickjs::methods]
impl DedicatedWorkerGlobalScope {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "DedicatedWorkerGlobalScope"
    }
}

/// HTML §10.2.6 `Worker`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Worker<'js> {
    port:   Class<'js, NativePort>,
    thread: Class<'js, NativeWorker>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Worker<'js> {
    #[qjs(constructor)]
    pub fn new(
        ctx: Ctx<'js>,
        script_url: Opt<Value<'js>>,
        options: Opt<Value<'js>>,
    ) -> Result<Class<'js, Self>> {
        let Some(script_url) = script_url.0 else {
            return Err(Exception::throw_type(
                &ctx,
                "Worker constructor: at least 1 argument required",
            ));
        };
        let url = Coerced::<String>::from_js(&ctx, script_url)?.0;
        let (kind, name) = worker_options(&ctx, options.0)?;
        let ports = pair(ctx.clone())?;
        let outside = ports[0].clone();
        let inside = ports[1].clone();
        let on_fault = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             function: FuncArg<Function<'js>>,
             message: String,
             filename: String,
             lineno: u32,
             colno: u32|
             -> Result<()> {
                let worker: Value<'js> = function.0.get("_worker")?;
                if !report_error_at(
                    &ctx,
                    worker,
                    message.clone(),
                    filename.clone(),
                    lineno,
                    colno,
                )? {
                    escalate(&ctx, message, filename, lineno, colno);
                }
                Ok(())
            },
        )?;
        let thread = spawn(ctx.clone(), url, kind, name, inside, on_fault.clone())?;
        let worker = Class::instance(
            ctx.clone(),
            Self {
                port: outside.clone(),
                thread,
            },
        )?;
        on_fault.set("_worker", worker.clone())?;
        let arm = track_message_listeners(ctx.clone(), worker.clone().into_value(), outside)?;
        arm.call::<_, ()>(())?;
        Ok(worker)
    }

    pub fn post_message(
        &self,
        ctx: Ctx<'js>,
        message: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        let (buffers, ports) = split_transfer(&ctx, transfer_list(options.0))?;
        self.port.borrow().post(ctx, message, buffers, ports)
    }

    pub fn terminate(&self) {
        self.thread.borrow().terminate();
        self.port.borrow().close();
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Worker"
    }
}

/// Called from the worker thread after the engine exists and before its
/// script runs. Returns `{ start, reportError }` for the thread body.
fn install_worker_scope<'js>(
    ctx: &Ctx<'js>,
    scope: Object<'js>,
    native: Class<'js, NativePort>,
    name: &str,
    hooks: Object<'js>,
) -> Result<Object<'js>> {
    if let Some(proto) = Class::<DedicatedWorkerGlobalScope>::prototype(ctx)? {
        scope.set_prototype(Some(&proto))?;
    }
    scope.set(CLOSING_SLOT, false)?;
    scope.prop("self", Property::from(scope.clone()).configurable())?;
    scope.prop("name", Property::from(name).writable().configurable())?;

    let post = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>,
         function: FuncArg<Function<'js>>,
         message: Value<'js>,
         options: Opt<Value<'js>>|
         -> Result<()> {
            let native: Class<'js, NativePort> = function.0.get("_native")?;
            let (buffers, ports) = split_transfer(&ctx, transfer_list(options.0))?;
            native.borrow().post(ctx, message, buffers, ports)
        },
    )?;
    post.set("_native", native.clone())?;
    scope.prop(
        "postMessage",
        Property::from(post).writable().configurable(),
    )?;

    let close = Function::new(
        ctx.clone(),
        |function: FuncArg<Function<'js>>| -> Result<()> {
            let scope: Object<'js> = function.0.get("_scope")?;
            scope.set(CLOSING_SLOT, true)?;
            let hook: Function<'js> = function.0.get("_close")?;
            hook.call::<_, ()>(())
        },
    )?;
    close.set("_scope", scope.clone())?;
    close.set("_close", hooks.get::<_, Function<'js>>("close")?)?;
    scope.prop("close", Property::from(close).writable().configurable())?;

    let import = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>,
         function: FuncArg<Function<'js>>,
         urls: Rest<Value<'js>>|
         -> Result<()> {
            let hook: Function<'js> = function.0.get("_import")?;
            let mut strings = Vec::with_capacity(urls.0.len());
            for url in urls.0 {
                strings.push(Coerced::<String>::from_js(&ctx, url)?.0);
            }
            hook.call::<_, ()>((strings,))
        },
    )?;
    import.set("_import", hooks.get::<_, Function<'js>>("importScripts")?)?;
    scope.prop(
        "importScripts",
        Property::from(import).writable().configurable(),
    )?;

    if let Some(proto) = Class::<EventTarget>::prototype(ctx)? {
        for method_name in ["addEventListener", "removeEventListener", "dispatchEvent"] {
            let method: Function<'js> = proto.get(method_name)?;
            let bound = bind(&method, scope.clone().into_value())?;
            scope.prop(
                method_name,
                Property::from(bound).writable().configurable(),
            )?;
        }
    }

    let arm = track_message_listeners(ctx.clone(), scope.clone().into_value(), native)?;
    define_event_handler(
        ctx.clone(),
        scope.clone(),
        "onmessage".to_owned(),
        Opt(None),
    )?;
    define_event_handler(
        ctx.clone(),
        scope.clone(),
        "onmessageerror".to_owned(),
        Opt(None),
    )?;
    define_event_handler(
        ctx.clone(),
        scope.clone(),
        "onerror".to_owned(),
        Opt(Some(true)),
    )?;
    define_event_handler(
        ctx.clone(),
        scope.clone(),
        "onunhandledrejection".to_owned(),
        Opt(None),
    )?;
    define_event_handler(
        ctx.clone(),
        scope.clone(),
        "onrejectionhandled".to_owned(),
        Opt(None),
    )?;

    let natives = natives(ctx)?;
    let escalate_fn = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>,
         function: FuncArg<Function<'js>>,
         message: String,
         filename: String,
         lineno: u32,
         colno: u32|
         -> Result<()> {
            let target: Value<'js> = function.0.get("_scope")?;
            if !report_error_at(
                &ctx,
                target,
                message.clone(),
                filename.clone(),
                lineno,
                colno,
            )? {
                let fault: Function<'js> = function.0.get("_fault")?;
                fault.call::<_, ()>((message, filename, lineno, colno))?;
            }
            Ok(())
        },
    )?;
    escalate_fn.set("_scope", scope.clone())?;
    escalate_fn.set("_fault", hooks.get::<_, Function<'js>>("fault")?)?;
    natives.set("escalate", escalate_fn.clone())?;

    let print: Function<'js> = natives.get("reportException")?;
    let reporter = Function::new(
        ctx.clone(),
        |_ctx: Ctx<'js>, function: FuncArg<Function<'js>>, value: Value<'js>| -> Result<()> {
            let reporting: bool = function.0.get("_reporting")?;
            if reporting {
                let print: Function<'js> = function.0.get("_print")?;
                print.call::<_, ()>((value,))?;
                return Ok(());
            }
            function.0.set("_reporting", true)?;
            let locate: Function<'js> = function.0.get("_locate")?;
            let escalate: Function<'js> = function.0.get("_escalate")?;
            let outcome = (|| {
                let located: Object<'js> = locate.call((value,))?;
                escalate.call::<_, ()>((
                    located.get::<_, String>("message")?,
                    located.get::<_, String>("filename")?,
                    located.get::<_, u32>("lineno")?,
                    located.get::<_, u32>("colno")?,
                ))
            })();
            function.0.set("_reporting", false)?;
            outcome
        },
    )?;
    reporter.set("_print", print)?;
    reporter.set("_locate", hooks.get::<_, Function<'js>>("locate")?)?;
    reporter.set("_escalate", escalate_fn)?;
    reporter.set("_reporting", false)?;
    natives.set("reportException", reporter)?;

    let start = Function::new(
        ctx.clone(),
        |function: FuncArg<Function<'js>>| -> Result<()> {
            let scope: Object<'js> = function.0.get("_scope")?;
            let closing: bool = scope.get(CLOSING_SLOT)?;
            if !closing {
                let arm: Function<'js> = function.0.get("_arm")?;
                arm.call::<_, ()>(())?;
            }
            Ok(())
        },
    )?;
    start.set("_scope", scope.clone())?;
    start.set("_arm", arm)?;

    let report = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>,
         function: FuncArg<Function<'js>>,
         message: String,
         filename: String,
         lineno: u32,
         colno: u32|
         -> Result<bool> {
            let target: Value<'js> = function.0.get("_scope")?;
            report_error_at(&ctx, target, message, filename, lineno, colno)
        },
    )?;
    report.set("_scope", scope.clone())?;

    let returned = Object::new(ctx.clone())?;
    returned.set("start", start)?;
    returned.set("reportError", report)?;
    Ok(returned)
}

/// Park the natives bag and the default (print) escalate hook.
pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    ctx.store_userdata(NativesBag(Persistent::save(ctx, natives.clone())))
        .map_err(|_| Exception::throw_internal(ctx, "den:worker is already installed"))?;
    natives.set(
        "escalate",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, message: String, filename: String, lineno: u32, colno: u32| {
                let text = format!("{message}\n    at {filename}:{lineno}:{colno}");
                if let Ok(value) = text.into_js(&ctx) {
                    print_exception(&ctx, &value);
                }
            },
        )?,
    )?;
    Ok(())
}

/// Prototype chain, `onX` slots, constructor `length`.
pub fn finish<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let hidden = Object::new(ctx.clone())?;
    Class::<WorkerGlobalScope>::define(&hidden)?;
    Class::<DedicatedWorkerGlobalScope>::define(&hidden)?;
    inherit::<WorkerGlobalScope, EventTarget>(ctx)?;
    inherit::<DedicatedWorkerGlobalScope, WorkerGlobalScope>(ctx)?;
    inherit::<Worker, EventTarget>(ctx)?;
    if let Some(proto) = Class::<Worker>::prototype(ctx)? {
        define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessage".to_owned(),
            Opt(None),
        )?;
        define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessageerror".to_owned(),
            Opt(None),
        )?;
        define_event_handler(ctx.clone(), proto, "onerror".to_owned(), Opt(None))?;
    }
    if let Some(ctor) = Class::<Worker>::create_constructor(ctx)? {
        ctor.set_length(1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, sync::Arc, time::Duration};

    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Exception, FromJs, Module, Object,
        Promise,
        context::EvalOptions,
        loader::{Resolver, ScriptLoader},
    };
    use tokio::{
        task::block_in_place,
        time::{self},
    };
    use tokio_util::sync::CancellationToken;
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
        let (_, evaluated) =
            Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
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
            &mut self,
            ctx: &Ctx<'js>,
            base: &str,
            name: &str,
            _attributes: Option<rquickjs::loader::ImportAttributes<'js>>,
        ) -> rquickjs::Result<String> {
            let base = Url::parse(base)
                .ok()
                .or_else(|| Url::from_file_path(base).ok());
            Url::options()
                .base_url(base.as_ref())
                .parse(name)
                .ok()
                .and_then(|url| url.to_file_path().ok())
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| {
                    Exception::throw_message(ctx, &format!("cannot resolve module {name:?}"))
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
        fn build_engine(
            &self,
            stop: CancellationToken,
            base: BaseUrl,
        ) -> Result<WorkerEngine, WorkerHostError> {
            // Same pair den-core's host uses: a synchronous trait method
            // reaching an async constructor from inside a runtime.
            block_in_place(|| tokio::runtime::Handle::current().block_on(Self::build(stop, base)))
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

        async fn build(stop: CancellationToken, base: BaseUrl) -> rquickjs::Result<WorkerEngine> {
            let interrupt = stop.clone();
            assert!(
                !base.0.contains(Self::PANIC_DIRECTORY),
                "the host panicked while building a worker engine"
            );
            if base.0.contains(Self::SLOW_DIRECTORY) {
                // Detached on purpose: it models a load that nobody is waiting
                // for, which is the case `shutdown_background` abandons.
                tokio::task::spawn_blocking(|| std::thread::sleep(Self::SLOW_TEARDOWN));
            }
            let runtime = AsyncRuntime::new()?;
            runtime
                .set_interrupt_handler(Some(Box::new(move || interrupt.is_cancelled())))
                .await;
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
                    // The one line den-core owes this crate: without it a
                    // worker's token is a root and stopping the realm never
                    // reaches it.
                    Self::store(&ctx, super::RealmStop(stop))?;
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
            .join("../target/den-worker-fixtures")
            .join(test);
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
        /// The realm's own token — den-core's `Engine::stop()`. Every worker
        /// this realm spawns gets a child of it.
        stop:    CancellationToken,
    }

    impl Fixture {
        async fn new(test: &str, files: &[(&str, &str)]) -> Self {
            let base = BaseUrl(fixture(test, files));
            let stop = CancellationToken::new();
            let engine = BareHost::build(stop.clone(), base)
                .await
                .expect("the bare host builds a parent realm");
            Self {
                runtime: engine.runtime,
                context: engine.context,
                stop,
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
    fn threads_named(_name: &str) -> usize {
        0
    }

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
        r#"self.onmessage = (event) => postMessage(`echo:${event.data}`);"#,
    );
    /// Posts once, then never yields the interpreter again: only the interrupt
    /// handler can end this one.
    const SPIN: (&str, &str) = ("spin.js", r#"postMessage("spinning"); while (true) {}"#);

    #[tokio::test(flavor = "multi_thread")]
    async fn a_classic_worker_echoes_a_message_across_the_thread_boundary() {
        let fixture = Fixture::new("echo", &[ECHO]).await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./echo.js");
                worker.postMessage("ping");
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                reply
                "#,
            )
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
        let fixture = Fixture::new(
            "queued",
            &[(
                "late.js",
                r#"
            // Long enough that the parent's postMessage is certainly first, and
            // synchronous so the queue cannot open in the middle of it.
            let sum = 0;
            for (let step = 0; step < 3_000_000; step += 1) sum += step;
            self.onmessage = (event) => postMessage(`late:${event.data}:${sum > 0}`);
            "#,
            )],
        )
        .await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./late.js");
                worker.postMessage("early");
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                reply
                "#,
            )
            .await;
        assert_eq!(reply, "late:early:true");
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn terminate_stops_a_worker_that_never_yields() {
        let fixture = Fixture::new("terminate", &[SPIN]).await;
        let started: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./spin.js");
                const started = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                started
                "#,
            )
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
        let fixture = Fixture::new(
            "close",
            &[(
                "close.js",
                r#"postMessage("bye"); close(); postMessage("after");"#,
            )],
        )
        .await;
        let last: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./close.js");
                const last = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                last
                "#,
            )
            .await;
        assert_eq!(last, "bye");
        fixture.settle().await;
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_uncaught_error_becomes_an_error_event_on_the_worker_object() {
        let fixture = Fixture::new(
            "uncaught",
            &[(
                "throw.js",
                "// line 1\n// line 2\nthrow new TypeError(\"boom\");\n",
            )],
        )
        .await;
        let reported: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./throw.js");
                const reported = await new Promise((resolve) => {
                  worker.onerror = (event) => {
                    // Uncancelled, this would be printed to stderr by the last
                    // step of the chain.
                    event.preventDefault();
                    resolve([
                      event.message,
                      event.filename.endsWith("throw.js"),
                      event.lineno,
                      event.error,
                    ].join("|"));
                  };
                });
                worker.terminate();
                reported
                "#,
            )
            .await;
        // `error` is undefined across threads: an Error does not serialise.
        assert_eq!(reported, "boom|true|3|");
        fixture.shutdown().await;
    }

    /// HTML §8.1.8.1: the global's `onerror` takes five positional arguments
    /// and cancels the event by returning `true`, which ends the chain before
    /// the parent ever hears about it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_onerror_returning_true_suppresses_the_parent_error_event() {
        let fixture = Fixture::new(
            "suppressed",
            &[(
                "suppress.js",
                r#"
            self.onmessage = (event) => postMessage(`pong:${event.data}`);
            self.onerror = function (message, filename, lineno, colno, error) {
              postMessage(`caught:${message}:${arguments.length}:${error}`);
              return true;
            };
            throw new Error("hidden");
            "#,
            )],
        )
        .await;
        let seen: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./suppress.js");
                globalThis.errors = 0;
                worker.onerror = () => { errors += 1; };
                const inbox = [];
                const next = () => new Promise((resolve) => { worker.onmessage = (event) => resolve(event.data); });
                inbox.push(await next());
                // A second round trip: a fault sent right after the error
                // dispatch would have arrived by now, so `errors` is a real
                // observation and not a race.
                worker.postMessage("ping");
                inbox.push(await next());
                worker.terminate();
                `${inbox.join("|")}|${errors}`
                "#,
            )
            .await;
        assert_eq!(seen, "caught:hidden:5:undefined|pong:ping|0");
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn import_scripts_runs_a_script_in_the_worker_global_relative_to_it() {
        let fixture = Fixture::new(
            "import-scripts",
            &[
                ("helper.js", r#"globalThis.helped = "helper";"#),
                (
                    "importer.js",
                    r#"
                importScripts("./helper.js");
                self.onmessage = () => postMessage(`${helped}:${typeof importScripts}`);
                "#,
                ),
            ],
        )
        .await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./importer.js");
                worker.postMessage(null);
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                reply
                "#,
            )
            .await;
        assert_eq!(reply, "helper:function");
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_module_worker_runs_through_the_engine_loader_chain() {
        let fixture = Fixture::new(
            "module",
            &[
                ("lib.js", r#"export const greeting = "from a module";"#),
                (
                    "module.js",
                    r#"
                import { greeting } from "./lib.js";
                self.onmessage = () => postMessage(`${greeting}:${typeof importScripts}`);
                "#,
                ),
            ],
        )
        .await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./module.js", { type: "module" });
                worker.postMessage(null);
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                reply
                "#,
            )
            .await;
        assert_eq!(reply, "from a module:function");
        fixture.shutdown().await;
    }

    /// Nesting needs nothing of its own: a worker realm gets the same host and
    /// the same `Worker` global its parent has, and a base URL of its own
    /// script's directory.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_can_spawn_a_worker() {
        let fixture = Fixture::new(
            "nested",
            &[
                (
                    "inner.js",
                    r#"self.onmessage = (event) => postMessage(`inner:${event.data}`);"#,
                ),
                (
                    "outer.js",
                    r#"
                const inner = new Worker("./inner.js");
                inner.onmessage = (event) => postMessage(`outer:${event.data}`);
                self.onmessage = (event) => inner.postMessage(event.data);
                "#,
                ),
            ],
        )
        .await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./outer.js");
                worker.postMessage("ping");
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                reply
                "#,
            )
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
        let fixture = Fixture::new("silent", &[("silent.js", r#"globalThis.ran = true;"#)]).await;
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
        let fixture = Fixture::new(
            "fickle",
            &[(
                "fickle.js",
                r#"
            self.onmessage = (event) => {
              if (event.data === "stop") {
                self.onmessage = null;
                return;
              }
              postMessage(`echo:${event.data}`);
            };
            "#,
            )],
        )
        .await;
        let reply: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./fickle.js");
                worker.postMessage("ping");
                const reply = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.postMessage("stop");
                reply
                "#,
            )
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
        let fixture = Fixture::new(
            "eager",
            &[(
                "eager.js",
                r#"
            postMessage("posted before anyone listened");
            // Out-of-band proof that the post above has already happened: the
            // error chain is not the port, and an uncaught error does not stop
            // a worker (§10.2.5).
            throw new Error("the worker has posted");
            "#,
            )],
        )
        .await;
        let queued: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./eager.js");
                await new Promise((resolve) => {
                  worker.onerror = (event) => { event.preventDefault(); resolve(); };
                });
                // Only now is anything in this realm listening for a message.
                const queued = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.terminate();
                queued
                "#,
            )
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
            .eval(
                r#"
                globalThis.worker = new Worker("./spin.js", { name: "tttt" });
                await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                "#,
            )
            .await;
        assert_eq!(started, "spinning");
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
        let fixture = Fixture::new(
            "reentrant-onerror",
            &[(
                "reentrant.js",
                r#"
            self.onerror = () => { throw new Error("from onerror"); };
            self.onmessage = (event) => postMessage(`alive:${event.data}`);
            throw new Error("the original");
            "#,
            )],
        )
        .await;
        let seen: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./reentrant.js");
                globalThis.errors = [];
                worker.onerror = (event) => {
                  event.preventDefault();
                  errors.push(event.message);
                };
                const next = () => new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                // Two round trips before `errors` is read: a fault sent at any
                // point during the report would have arrived by then, so the
                // count is an observation rather than a race.
                worker.postMessage("one");
                const first = await next();
                worker.postMessage("two");
                const second = await next();
                worker.terminate();
                `${first}|${second}|${errors.join(",")}`
                "#,
            )
            .await;
        assert_eq!(seen, "alive:one|alive:two|from onerror,the original");
        fixture.shutdown().await;
    }

    /// `Worker.prototype.onmessageerror`: a payload this realm cannot rebuild
    /// reaches the `Worker` as a `messageerror`, and assigning that handler is
    /// itself enough to make the parent's end of the port deliver.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_payload_the_parent_cannot_rebuild_becomes_messageerror_on_the_worker() {
        let fixture = Fixture::new(
            "parent-messageerror",
            &[(
                "bad.js",
                // A clone tag whose revival throws on the far side: a DataView
                // cannot be built past the end of its buffer.
                r#"postMessage({
                     "\u0000den:structured-clone": "DataView",
                     buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
                   });"#,
            )],
        )
        .await;
        let seen: String = fixture
            .eval(
                r#"
                globalThis.worker = new Worker("./bad.js");
                const seen = await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(`message:${event.data}`);
                  worker.onmessageerror = (event) => resolve(`${event.type}:${event.data}`);
                });
                worker.terminate();
                seen
                "#,
            )
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
            .eval(
                r#"
                globalThis.worker = new Worker("./slow-teardown/echo.js", { name: "cccc" });
                worker.postMessage("up");
                await new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                "#,
            )
            .await;
        // The round trip is the proof that the worker has an engine, a tokio
        // runtime and the threads that come with it.
        assert_eq!(reply, "echo:up");
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

    /// A worker's token is a child of the token of the realm that spawned it,
    /// so stopping that realm — den-core's `Engine::stop()`, which has nothing
    /// else to interrupt in a parked parent — reaches the workers too.
    #[tokio::test(flavor = "multi_thread")]
    async fn stopping_the_realm_interrupts_the_workers_it_spawned() {
        let fixture = Fixture::new("realm-stop", &[SPIN]).await;
        fixture
            .eval::<()>(
                r#"
                globalThis.worker = new Worker("./spin.js");
                await new Promise((resolve) => { worker.onmessage = resolve; });
                "#,
            )
            .await;
        fixture.stop.cancel();
        // No terminate(), and the worker never yields the interpreter: the
        // realm going idle means its interrupt handler saw the cancellation.
        fixture.settle().await;
        fixture.shutdown().await;
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
            .eval(
                r#"
                globalThis.spinner = new Worker("./spin.js");
                globalThis.echo = new Worker("./echo.js");
                await new Promise((resolve) => { spinner.onmessage = () => resolve(); });
                echo.postMessage("parallel");
                const reply = await new Promise((resolve) => {
                  echo.onmessage = (event) => resolve(event.data);
                });
                spinner.terminate();
                echo.terminate();
                reply
                "#,
            )
            .await;
        assert_eq!(reply, "echo:parallel");
        fixture.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_stops_and_joins_every_worker_the_realm_spawned() {
        let fixture = Fixture::new("shutdown", &[SPIN]).await;
        fixture
            .eval::<()>(
                r#"
                globalThis.first = new Worker("./spin.js");
                globalThis.second = new Worker("./spin.js");
                "#,
            )
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
            .eval(
                r#"
                globalThis.worker = new Worker("./panicking-host/echo.js");
                const reported = await new Promise((resolve) => {
                  worker.onerror = (event) => {
                    event.preventDefault();
                    resolve(event.message);
                  };
                });
                reported
                "#,
            )
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
            .eval(
                r#"
                const failure = (build) => { try { build(); return "no throw"; } catch (error) { return `${error.constructor.name}` } };
                [
                  failure(() => new Worker("https://example.com/w.js")),
                  failure(() => new Worker("./echo.js", { type: "worklet" })),
                ].join("|")
                "#,
            )
            .await;
        assert_eq!(failures, "TypeError|TypeError");
        fixture.shutdown().await;
        fixture.settle().await;
    }
}
