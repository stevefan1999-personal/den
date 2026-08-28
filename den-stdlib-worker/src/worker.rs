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

use den_stdlib_core::exceptions::print_exception;
#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile};
use den_util::{coerce_string, inherit};
use rquickjs::{
    AsyncContext, Class, Coerced, Ctx, Error, Exception, FromJs, Function, IntoJs, JsLifetime,
    Module, Object, Persistent, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    context::EvalOptions,
    function::{Func, FuncArg, Opt, Rest},
    object::Property,
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    time,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    events::{ErrorEvent, EventTarget, define_event_handler, dispatch_trusted},
    host::{BaseUrl, HostHandle, WorkerEngine, WorkerHost},
    message::clone::split_transfer,
    port::{NativePort, pair, track_message_listeners},
    report::{report_uncaught, sink_hook},
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
        Some(options) if options.is_object() => {
            options
                .as_object()
                .and_then(|object| object.get("transfer").ok())
                .filter(|value: &Value| !value.is_undefined() && !value.is_null())
        }
        _ => None,
    }
}

/// HTML §8.1.4.6 step 3: fire a cancelable `error` at `target`. `true` means
/// something called `preventDefault()` — the error was claimed.
fn report_error_at<'js>(
    ctx: &Ctx<'js>, target: Value<'js>, message: String, filename: String, lineno: u32, colno: u32,
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
        report_uncaught(ctx, hook.call::<_, ()>((message, filename, lineno, colno)));
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
                coerce_string(ctx, value)?
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
                coerce_string(ctx, value)?
            }
        }
        None => String::new(),
    };
    Ok((kind, name))
}

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

impl Drop for WorkerRegistry {
    /// How a realm that ends *without* [`shutdown`] — an embedder simply
    /// letting its `Engine` go — still stops the workers it started: the
    /// registry dies with the realm's userdata, and cancelling is the last
    /// thing it does. Idempotent, and it does not join: a spinning worker
    /// leaves at its next interrupt poll and nobody is left to care when.
    ///
    /// It lives here rather than on [`WorkerHandle`] because [`shutdown`]
    /// moves the join handles out of one, which `Drop` would forbid.
    fn drop(&mut self) {
        self.threads
            .get_mut()
            .iter()
            .for_each(|thread| thread.stop.cancel());
    }
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
        ctx: Ctx<'js>, mut inbox: UnboundedReceiver<WorkerFault>, stop: CancellationToken,
        on_fault: Function<'js>,
    ) {
        while let Some(Some(fault)) = stop.run_until_cancelled(inbox.recv()).await {
            let dispatched =
                on_fault.call::<_, ()>((fault.message, fault.filename, fault.lineno, fault.colno));
            // The handler for "nobody handled an error" throwing is the end of
            // the line; report it rather than lose it.
            report_uncaught(&ctx, dispatched);
        }
    }
}

#[rquickjs::methods]
impl NativeWorker {
    /// `nativeWorker.terminate()` — HTML §10.2.4 "terminate a worker": the
    /// interrupt handler aborts whatever the worker is running without letting
    /// a `finally` block observe it, and the cancelled token releases a worker
    /// that is merely parked. Idempotent.
    pub fn terminate(&self) { self.stop.cancel(); }
}

/// Spawn a worker thread (HTML §10.2.6.3 step 10, "run a worker in parallel").
pub fn spawn<'js>(
    ctx: Ctx<'js>, url: String, kind: String, name: String, port: Class<'js, NativePort>,
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
    // The worker's own, with no link to the realm that started it: a parent
    // ends by dropping its registry (`impl Drop for WorkerRegistry`), not
    // cancelling a tree.
    let stop = CancellationToken::new();
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

    WorkerRegistry::register(&ctx, WorkerHandle {
        stop: stop.clone(),
        join,
    })?;
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
        let engine = match self.host.build_engine(self.base.clone()) {
            Ok(engine) => engine,
            Err(error) => {
                let _ = self
                    .faults
                    .send(WorkerFault::from_message(error.to_string()));
                return;
            }
        };
        // Bytecode already running on this thread can only be stopped by a flag
        // polled at back-edges, and the handler cannot be installed once
        // `idle()` holds the runtime mutex — so it goes on here, before the
        // script does.
        engine
            .runtime
            .set_interrupt_handler({
                let stop = self.stop.clone();
                Some(Box::new(move || stop.is_cancelled()))
            })
            .await;

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
                async move |ctx| {
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
        ctx: &Ctx<'js>, channel: PortHandle, name: &str, kind: ScriptKind, script: &Url,
        closing: &CancellationToken, faults: UnboundedSender<WorkerFault>,
    ) -> Option<WorkerFault> {
        match Self::install_scope(ctx, channel, name, kind, script, closing, faults) {
            Ok(scope) => Self::run_and_report(ctx, &scope, kind, script).await,
            Err(error) => Some(WorkerFault::take(ctx, error)),
        }
    }

    /// Hand the prelude's installer the worker's port, name and the two things
    /// only Rust can do, and keep the two hooks it gives back.
    fn install_scope<'js>(
        ctx: &Ctx<'js>, channel: PortHandle, name: &str, kind: ScriptKind, script: &Url,
        closing: &CancellationToken, faults: UnboundedSender<WorkerFault>,
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
        ctx: &Ctx<'js>, scope: &Object<'js>, kind: ScriptKind, script: &Url,
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
        // The native loader entry point reaches den's resolver/transpiler chain
        // without evaluating an `await import(...)` bootstrap string.
        let module = Module::import(ctx, script.as_str())
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
        ctx: &Ctx<'_>, base: &Url, kind: ScriptKind, urls: Vec<String>,
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

    /// The top level is a script, where `import` is a syntax error and `this`
    /// is the global — both of which a classic worker script relies on.
    #[cfg(feature = "transpile")]
    fn transpile(source: String, extension: &str) -> std::result::Result<String, String> {
        let syntax = infer_transpile_syntax_by_extension(extension).unwrap_or_default();
        transpile(&source, syntax.with_script(true)).map_err(|error| error.to_string())
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
    pub fn to_string_tag() -> &'static str { "WorkerGlobalScope" }
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
    pub fn to_string_tag() -> &'static str { "DedicatedWorkerGlobalScope" }
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
        ctx: Ctx<'js>, script_url: Opt<Value<'js>>, options: Opt<Value<'js>>,
    ) -> Result<Class<'js, Self>> {
        let Some(script_url) = script_url.0 else {
            return Err(Exception::throw_type(
                &ctx,
                "Worker constructor: at least 1 argument required",
            ));
        };
        let url = coerce_string(&ctx, script_url)?;
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
        let worker = Class::instance(ctx.clone(), Self {
            port: outside.clone(),
            thread,
        })?;
        on_fault.set("_worker", worker.clone())?;
        let arm = track_message_listeners(ctx.clone(), worker.clone().into_value(), outside)?;
        arm.call::<_, ()>(())?;
        Ok(worker)
    }

    pub fn post_message(
        &self, ctx: Ctx<'js>, message: Value<'js>, options: Opt<Value<'js>>,
    ) -> Result<()> {
        let (buffers, ports) = split_transfer(&ctx, transfer_list(options.0))?;
        self.port.borrow().post(ctx, message, buffers, ports)
    }

    pub fn terminate(&self) {
        self.thread.borrow().terminate();
        self.port.borrow().close();
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Worker" }
}

/// Called from the worker thread after the engine exists and before its
/// script runs. Returns `{ start, reportError }` for the thread body.
fn install_worker_scope<'js>(
    ctx: &Ctx<'js>, scope: Object<'js>, native: Class<'js, NativePort>, name: &str,
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
        |ctx: Ctx<'js>, function: FuncArg<Function<'js>>, urls: Rest<Value<'js>>| -> Result<()> {
            let hook: Function<'js> = function.0.get("_import")?;
            let mut strings = Vec::with_capacity(urls.0.len());
            for url in urls.0 {
                strings.push(coerce_string(&ctx, url)?);
            }
            hook.call::<_, ()>((strings,))
        },
    )?;
    import.set("_import", hooks.get::<_, Function<'js>>("importScripts")?)?;
    scope.prop(
        "importScripts",
        Property::from(import).writable().configurable(),
    )?;

    EventTarget::bind_on(ctx, &scope)?;

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
#[path = "../tests/unit/worker.rs"]
mod tests;
