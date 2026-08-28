//! `BroadcastChannel` (HTML §9.5): the process-global registry of subscribers
//! by channel name, the pump that delivers to one of them, and the EventTarget
//! wrapper.
//!
//! The registry is a plain process-global map rather than anything per-realm,
//! and that is the whole cross-thread story: a `BroadcastChannel` in a worker
//! registers in the same map as one on the main thread, so a post reaches it
//! with no plumbing between the two runtimes. Only `Message` — `Send`, tied to
//! no runtime — crosses; the receiving realm rebuilds the value itself.

use std::{
    cell::{Cell, RefCell},
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use dashmap::DashMap;
use den_util::{coerce_string, inherit, throw_dom_exception};
use rquickjs::{
    Class, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{FuncArg, Opt},
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::{
    events::{EventTarget, MessageEvent, define_event_handler, dispatch_trusted},
    message::Message,
    port::NativePort,
};

/// One live `BroadcastChannel`, as seen by every *other* one with its name.
struct Subscriber {
    /// Identity, so that a post can skip its own channel. A raw pointer would
    /// do it too, but an id is comparable across threads without being one.
    id:    u64,
    inbox: UnboundedSender<Message>,
}

/// Every open channel in the process, keyed by name.
///
/// Sharded by name, so two names contend only by accident of hashing. A shard
/// lock is only ever held around a lookup and a batch of non-blocking `send`s —
/// never across an await, and never while running JS. It is also never held
/// while touching the map a second time: that is a self-deadlock, not a wait,
/// which is why [`NativeBroadcast::unregister`] is written the way it is.
static SUBSCRIBERS: LazyLock<DashMap<String, Vec<Subscriber>>> = LazyLock::new(DashMap::new);

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// The transport end of one `BroadcastChannel`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "NativeBroadcast")]
pub struct NativeBroadcast {
    #[qjs(skip_trace)]
    name:  String,
    #[qjs(skip_trace)]
    id:    u64,
    /// This channel's own inbox, until the pump takes it.
    #[qjs(skip_trace)]
    inbox: RefCell<Option<UnboundedReceiver<Message>>>,
    /// Ends the pump, and doubles as the closed flag: nothing else can tell a
    /// quiet channel from a closed one.
    #[qjs(skip_trace)]
    stop:  CancellationToken,
}

impl NativeBroadcast {
    /// Hand `message` to every other channel of this name, dropping the ones
    /// whose realm went away without closing them (a worker that exited mid-
    /// flight): their receiver is gone, so the send is how we find out.
    fn fan_out(&self, message: &Message) {
        let Some(mut peers) = SUBSCRIBERS.get_mut(&self.name) else {
            return;
        };
        peers.retain(|peer| {
            // "remove source from destinations" — a channel never hears itself.
            peer.id == self.id
                || message
                    .try_clone()
                    .is_none_or(|copy| peer.inbox.send(copy).is_ok())
        });
    }

    /// Leave the registry. Idempotent, and the reason a channel that is merely
    /// dropped — never `close()`d, because its worker died — does not leak a
    /// sender that keeps its name alive forever.
    ///
    /// Two steps rather than one because the entry's guard *is* its shard's
    /// lock: dropping the name while still holding it would block this thread
    /// on itself. The re-check inside `remove_if` is what makes the gap between
    /// the two safe — a channel opened on this name in the meantime has already
    /// refilled the list, and survives.
    fn unregister(&self) {
        let emptied = SUBSCRIBERS.get_mut(&self.name).is_some_and(|mut peers| {
            peers.retain(|peer| peer.id != self.id);
            peers.is_empty()
        });
        if emptied {
            SUBSCRIBERS.remove_if(&self.name, |_, peers| peers.is_empty());
        }
    }

    /// The pump: deliver until `close()` cancels us.
    ///
    /// Like a port's pump this is *the* process-lifetime mechanism —
    /// `AsyncRuntime::idle()` resolves only when no `ctx.spawn`-ed future is
    /// left, so an open channel with a listener keeps the process alive (Node's
    /// semantics) and `close()` is the release. `inbox.recv()` returning `None`
    /// cannot end it: this channel's own sender lives in the registry until
    /// then.
    async fn pump<'js>(
        ctx: Ctx<'js>, mut inbox: UnboundedReceiver<Message>, stop: CancellationToken,
        on_message: Function<'js>, on_message_error: Function<'js>,
    ) {
        while let Some(Some(message)) = stop.run_until_cancelled(inbox.recv()).await {
            NativePort::dispatch(&ctx, message, &on_message, &on_message_error);
        }
    }
}

#[rquickjs::methods]
impl NativeBroadcast {
    /// `new NativeBroadcast(name)` — "create a new BroadcastChannel object"
    /// (HTML §9.5): the channel joins its name's subscriber list at once, so
    /// that a message posted before anything is listening is still queued for
    /// it.
    #[qjs(constructor)]
    pub fn new(name: String) -> Self {
        let (inbox, outbox) = mpsc::unbounded_channel();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        SUBSCRIBERS
            .entry(name.clone())
            .or_default()
            .push(Subscriber { id, inbox });
        Self {
            name,
            id,
            inbox: RefCell::new(Some(outbox)),
            stop: CancellationToken::new(),
        }
    }

    /// `nativeBroadcast.post(value)` — the "BroadcastChannel postMessage"
    /// steps. Serialisation happens once, before the fan-out, so a
    /// `DataCloneError` is synchronous and nobody receives a half-formed
    /// message; each subscriber then gets its own copy of the bytes and
    /// deserialises in its own realm.
    ///
    /// There is no transfer list: `BroadcastChannel` takes none, which also
    /// means the clone pre-pass refuses any `MessagePort` in the graph.
    pub fn post<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        if self.stop.is_cancelled() {
            return Ok(());
        }
        self.fan_out(&Message::serialize(&ctx, value, Vec::new(), Vec::new())?);
        Ok(())
    }

    /// `nativeBroadcast.subscribe(onMessage, onMessageError)` — start
    /// delivering. Idempotent, and a no-op once closed.
    pub fn subscribe<'js>(
        &self, ctx: Ctx<'js>, on_message: Function<'js>, on_message_error: Function<'js>,
    ) {
        let inbox = self
            .inbox
            .try_borrow_mut()
            .ok()
            .and_then(|mut inbox| inbox.take());
        let Some(inbox) = inbox else {
            return;
        };
        ctx.spawn(Self::pump(
            ctx.clone(),
            inbox,
            self.stop.clone(),
            on_message,
            on_message_error,
        ));
    }

    /// `nativeBroadcast.close()` — leave the registry and end the pump, which
    /// is what lets the runtime go idle. Idempotent.
    pub fn close(&self) {
        self.stop.cancel();
        self.unregister();
        if let Ok(mut inbox) = self.inbox.try_borrow_mut() {
            *inbox = None;
        }
    }
}

impl Drop for NativeBroadcast {
    fn drop(&mut self) {
        self.stop.cancel();
        self.unregister();
    }
}

/// HTML §9.5 `BroadcastChannel`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct BroadcastChannel<'js> {
    #[qjs(get, skip_trace)]
    name:   String,
    native: Class<'js, NativeBroadcast>,
    #[qjs(skip_trace)]
    closed: Cell<bool>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> BroadcastChannel<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, name: Opt<Value<'js>>) -> Result<Class<'js, Self>> {
        let name = match name.0 {
            Some(value) => coerce_string(&ctx, value)?,
            None => "undefined".to_owned(),
        };
        let native = Class::instance(ctx.clone(), NativeBroadcast::new(name.clone()))?;
        let channel = Class::instance(ctx.clone(), Self {
            name,
            native: native.clone(),
            closed: Cell::new(false),
        })?;
        let on_message = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             function: FuncArg<Function<'js>>,
             data: Value<'js>,
             _ports: Opt<Value<'js>>|
             -> Result<()> {
                let target: Value<'js> = function.0.get("_target")?;
                let init = Object::new(ctx.clone())?;
                init.set("data", data)?;
                let event = Class::instance(
                    ctx.clone(),
                    MessageEvent::new(
                        ctx.clone(),
                        "message".into_js(&ctx)?,
                        Opt(Some(init.into_value())),
                    )?,
                )?;
                dispatch_trusted(ctx.clone(), target, event.into_value())?;
                Ok(())
            },
        )?;
        on_message.set("_target", channel.clone())?;
        let on_error = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, function: FuncArg<Function<'js>>| -> Result<()> {
                let target: Value<'js> = function.0.get("_target")?;
                let event = Class::instance(
                    ctx.clone(),
                    MessageEvent::new(ctx.clone(), "messageerror".into_js(&ctx)?, Opt(None))?,
                )?;
                dispatch_trusted(ctx.clone(), target, event.into_value())?;
                Ok(())
            },
        )?;
        on_error.set("_target", channel.clone())?;
        native.borrow().subscribe(ctx.clone(), on_message, on_error);
        Ok(channel)
    }

    pub fn post_message(&self, ctx: Ctx<'js>, message: Value<'js>) -> Result<()> {
        if self.closed.get() {
            return Err(throw_dom_exception(
                &ctx,
                "InvalidStateError",
                "the BroadcastChannel is closed",
            ));
        }
        self.native.borrow().post(ctx, message)
    }

    pub fn close(&self) {
        self.closed.set(true);
        self.native.borrow().close();
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "BroadcastChannel" }
}

/// NativeBroadcast stays off the public surface; the wrapper is the export.
pub fn install<'js>(_ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    Class::<NativeBroadcast>::define(natives)
}

/// Prototype chain and `onmessage` / `onmessageerror`.
pub fn finish<'js>(ctx: &Ctx<'js>) -> Result<()> {
    inherit::<BroadcastChannel, EventTarget>(ctx)?;
    if let Some(proto) = Class::<BroadcastChannel>::prototype(ctx)? {
        define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessage".to_owned(),
            Opt(None),
        )?;
        define_event_handler(ctx.clone(), proto, "onmessageerror".to_owned(), Opt(None))?;
    }
    if let Some(ctor) = Class::<BroadcastChannel>::create_constructor(ctx)? {
        ctor.set_length(1)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/broadcast.rs"]
mod tests;

/// The registry's own concurrency, below the JS surface: these reach
/// [`SUBSCRIBERS`] directly, which no test outside this module can.
#[cfg(test)]
mod registry_tests {
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use super::{NativeBroadcast, SUBSCRIBERS};

    /// Long enough that a slow machine finishes the storm, short enough that a
    /// deadlock is a failing test rather than a hung CI job.
    const WATCHDOG: Duration = Duration::from_secs(30);
    const CHURN: usize = 20_000;
    const THREADS: usize = 3;

    /// Open/close churn on one shared name, from several threads at once.
    ///
    /// Two regressions in one, both of which the obvious `get_mut` + `remove`
    /// spelling of [`NativeBroadcast::unregister`] would hit:
    ///
    /// * the removal must not run under the entry's own guard — that is the
    ///   same shard lock twice on one thread, so a *single* iteration hangs;
    /// * the removal must re-check emptiness, or a channel that another thread
    ///   opened in the gap is dropped from the registry while still live. Each
    ///   iteration asserts its own registration between construction and close,
    ///   so a stale removal by a peer thread fails the test instead of silently
    ///   losing that channel's messages.
    ///
    /// The watchdog is a `recv_timeout` on this thread rather than a join: a
    /// deadlocked worker never comes back, and the point is to still report.
    #[test]
    fn concurrent_open_and_close_of_one_name_neither_deadlocks_nor_drops_a_live_channel() {
        const NAME: &str = "registry_tests::churn";
        let (finished, outcome) = mpsc::channel();
        for _ in 0..THREADS {
            let finished = finished.clone();
            thread::spawn(move || {
                let verdict = (0..CHURN).try_for_each(|_| {
                    let channel = NativeBroadcast::new(NAME.to_owned());
                    let registered = SUBSCRIBERS
                        .get(NAME)
                        .is_some_and(|peers| peers.iter().any(|peer| peer.id == channel.id));
                    channel.close();
                    registered
                        .then_some(())
                        .ok_or("a live channel was dropped from the registry by a peer's close()")
                });
                let _ = finished.send(verdict);
            });
        }
        drop(finished);

        let deadline = Instant::now() + WATCHDOG;
        for _ in 0..THREADS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match outcome.recv_timeout(remaining) {
                Ok(verdict) => verdict.unwrap_or_else(|reason| panic!("{reason}")),
                Err(_) => panic!("the registry deadlocked: a churn thread never finished"),
            }
        }
        assert!(
            SUBSCRIBERS.get(NAME).is_none(),
            "the last close() must drop the name, or the registry leaks one entry per name"
        );
    }
}
