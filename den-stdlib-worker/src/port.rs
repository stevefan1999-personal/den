//! `MessagePort` / `MessageChannel`: the channel end, the pump, and the
//! EventTarget wrappers.
//!
//! Each [`MessagePort`] keeps its [`NativePort`] under the symbol published as
//! `natives.portHandleKey`, which is how the structured-clone pre-pass
//! recognises a port in a message graph. `MessagePort.prototype` is reparented
//! onto `EventTarget.prototype` so `port instanceof EventTarget` holds.

use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    rc::Rc,
    task::Poll,
};

use rquickjs::{
    Class, Ctx, Error, Exception, Function, IntoJs, JsLifetime, Object, Result, Value,
    class::Trace,
    function::{Opt, This},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::{
    message::{Message, throw_data_clone},
    report::report_exception,
    transport::{Envelope, PortHandle},
};

/// Everything a port shares with its pump.
///
/// It is one `Rc` rather than three because the pump future outlives every
/// individual call into the port and has to reach all of it: deliver from the
/// inbox, detach the handle when the peer disappears, and record that it is no
/// longer the run that delivers.
#[derive(Default)]
struct PortState {
    /// The channel end. `None` once the port has been transferred away or
    /// closed — the spec's `[[Detached]]`.
    handle: RefCell<Option<PortHandle>>,
    /// This end's inbox, from the first [`NativePort::start`] onwards.
    ///
    /// It lives here rather than inside the pump so that a pump which stops
    /// can leave it behind: unreffing a port disables *delivery*, not receipt,
    /// and every envelope the peer already sent has to still be there when the
    /// next pump picks the queue up (HTML §9.4.4).
    inbox:  RefCell<Option<UnboundedReceiver<Envelope>>>,
    /// The token of the pump run that is currently delivering, if any. `Some`
    /// is exactly "a pump future is live and this port is keeping the event
    /// loop awake".
    run:    RefCell<Option<CancellationToken>>,
}

/// The transport end of one `MessagePort`.
///
/// Every accessor is `&self` over a `RefCell` so that a JS method holding a
/// `Class` borrow can still reach the state.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "NativePort")]
pub struct NativePort {
    #[qjs(skip_trace)]
    state:   Rc<PortState>,
    /// Ends the port for good, and every pump run with it — each run's token
    /// is a child of this one. Nothing but `close()` can: the inbox is fed by
    /// the peer, and a peer that is merely quiet is indistinguishable from one
    /// that is gone.
    #[qjs(skip_trace)]
    stop:    CancellationToken,
    /// Whether the inbox has left the handle. Once it has, the port cannot be
    /// handed to another realm, which is why a started port refuses transfer.
    #[qjs(skip_trace)]
    started: Cell<bool>,
}

impl NativePort {
    pub fn from_handle(handle: PortHandle) -> Self {
        Self {
            state:   Rc::new(PortState {
                handle: RefCell::new(Some(handle)),
                ..PortState::default()
            }),
            stop:    CancellationToken::new(),
            started: Cell::new(false),
        }
    }

    /// Move the channel end out — transferring the port. The port is detached
    /// afterwards, so a second transfer finds `None` and fails.
    ///
    /// `None` when the port was already transferred or closed, when it is
    /// started (see [`NativePort::is_started`]), or when the cell is borrowed
    /// elsewhere; a re-entrant borrow is a den bug, but returning `None` turns
    /// it into a `DataCloneError` rather than a panic reachable from script.
    pub fn take_handle(&self) -> Option<PortHandle> {
        if self.is_started() {
            return None;
        }
        self.state.handle.try_borrow_mut().ok()?.take()
    }

    /// Whether the port owns its inbox — true from the first
    /// [`NativePort::start`] onwards — which is what makes it untransferable:
    /// `PortHandle` can hand an inbox out but not take one back, so a port
    /// whose queue has been enabled can no longer be packed up for another
    /// realm (v1 divergence from HTML §9.4.4, which ships a started port
    /// together with its undelivered messages).
    ///
    /// Exposed so that the transfer list can be validated *before* anything is
    /// detached: `take_handle` refuses a started port, and
    /// a refusal discovered halfway through the transfer is a port the sender
    /// silently loses.
    pub fn is_started(&self) -> bool { self.started.get() }

    /// Whether this port still holds a live channel end.
    pub fn is_open(&self) -> bool {
        self.state
            .handle
            .try_borrow()
            .is_ok_and(|handle| handle.as_ref().is_some_and(PortHandle::is_open))
    }

    /// Identity, by the one thing two `Class` handles to the same instance
    /// necessarily share. `Class::try_borrow` cannot fail here — every method
    /// on this type takes `&self`, so only shared borrows are ever live — but
    /// a `false` is a safe answer anyway: the caller falls through to
    /// serialisation, which refuses a detached port on its own.
    fn is_same(&self, other: &Class<'_, Self>) -> bool {
        other
            .try_borrow()
            .is_ok_and(|other| Rc::ptr_eq(&self.state, &other.state))
    }

    /// Detach this end: stop the pump, drop whatever the peer sent and nobody
    /// dispatched (HTML §10.2.4 step 4), hand the peer its `Close` (through
    /// `PortHandle`'s `Drop`), and make every later operation a no-op.
    /// Idempotent.
    fn detach(&self) {
        self.stop.cancel();
        if let Ok(mut inbox) = self.state.inbox.try_borrow_mut() {
            *inbox = None;
        }
        if let Ok(mut handle) = self.state.handle.try_borrow_mut() {
            *handle = None;
        }
    }

    /// Rebuild one message in this realm and hand it to the port's callbacks.
    ///
    /// A message that cannot be rebuilt here is a `messageerror` event, not an
    /// error of the pump's (HTML §9.4.4), so the pending exception is caught
    /// and dropped rather than reported. Broadcast delivery uses the same
    /// helper: a channel never carries ports, so the JS callback just ignores
    /// the empty second argument.
    pub(crate) fn dispatch<'js>(
        ctx: &Ctx<'js>, message: Message, on_message: &Function<'js>,
        on_message_error: &Function<'js>,
    ) {
        let outcome = match message.deserialize(ctx) {
            Ok((value, ports)) => on_message.call::<_, ()>((value, ports)),
            Err(error) => {
                Self::discard(ctx, error);
                on_message_error.call::<_, ()>(())
            }
        };
        // A handler that throws has nobody to propagate to: whoever posted the
        // message is on another thread, or returned long ago. Print it like any
        // other uncaught error instead of swallowing it.
        if let Err(error) = outcome {
            match error {
                Error::Exception => report_exception(ctx, &ctx.catch()),
                error => eprintln!("{error}"),
            }
        }
    }

    /// Clear the pending exception behind an `Err`, so that it cannot surface
    /// at the next unrelated call into this context.
    fn discard(ctx: &Ctx<'_>, error: Error) {
        if let Error::Exception = error {
            ctx.catch();
        }
    }

    /// Await the next envelope without moving the inbox out of the port, so
    /// that a run which ends leaves the queue behind for the next one.
    ///
    /// The inbox is borrowed for the length of a single poll and nothing else
    /// borrows it across an await, so the borrow cannot actually fail; waking
    /// and retrying is still the only answer to one that could not lose a
    /// queue.
    fn recv(state: &PortState) -> impl Future<Output = Option<Envelope>> + '_ {
        poll_fn(|cx| {
            match state.inbox.try_borrow_mut() {
                Ok(mut inbox) => {
                    match inbox.as_mut() {
                        Some(inbox) => inbox.poll_recv(cx),
                        // Detached, or transferred away before this run noticed.
                        None => Poll::Ready(None),
                    }
                }
                Err(_) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        })
    }

    /// One pump run: deliver every envelope until the peer goes away, until
    /// `close()` cancels the port, or until [`NativePort::pause`] ends this run
    /// alone.
    ///
    /// This future is *the* process-lifetime mechanism for ports.
    /// `AsyncRuntime::idle()` resolves only when no `ctx.spawn`-ed future is
    /// left (docs/research/09 §2.2), so a port that is delivering keeps the
    /// event loop alive and a paused one does not. The future also owns the two
    /// JS callbacks, which reach the `MessagePort` through their closures: that
    /// is what keeps a port with a listener from being collected while its peer
    /// can still reach it (HTML §9.4.5).
    async fn pump<'js>(
        ctx: Ctx<'js>, state: Rc<PortState>, run: CancellationToken, on_message: Function<'js>,
        on_message_error: Function<'js>, on_close: Function<'js>,
    ) {
        loop {
            let envelope = tokio::select! {
                // Biased, and cancellation first: a paused run must not take an
                // envelope on its way out, because the listener it would have
                // been dispatched to is the one that just went away.
                biased;
                () = run.cancelled() => return,
                envelope = Self::recv(&state) => envelope,
            };
            match envelope {
                Some(Envelope::Message(message)) => {
                    Self::dispatch(&ctx, message, &on_message, &on_message_error)
                }
                // The peer is gone. Only a live run reaches this — a cancelled
                // one returns above — so clearing `run` cannot clear a newer
                // run's token.
                Some(Envelope::Close) | None => break,
            }
        }
        // Nothing will ever arrive again. Dropping our end announces the
        // closure to the peer in turn, and makes a later `post` the silent
        // no-op the spec asks for.
        if let Ok(mut inbox) = state.inbox.try_borrow_mut() {
            *inbox = None;
        }
        if let Ok(mut handle) = state.handle.try_borrow_mut() {
            *handle = None;
        }
        if let Ok(mut run) = state.run.try_borrow_mut() {
            *run = None;
        }
        // HTML §9.4.4 "disentangle": the port is now closed, and the script
        // gets its one `close` event. Fired after the teardown above so that a
        // listener sees a port that is already dead — `post` is a no-op and
        // `close()` idempotent — rather than one mid-collapse.
        if let Err(error) = on_close.call::<_, ()>(()) {
            match error {
                Error::Exception => report_exception(&ctx, &ctx.catch()),
                error => eprintln!("{error}"),
            }
        }
    }
}

#[rquickjs::methods]
impl NativePort {
    /// `nativePort.close()` — HTML §9.4.4 "close() method steps": detach, and
    /// disentangle, which is what tells the peer. Idempotent.
    pub fn close(&self) { self.detach(); }

    /// `nativePort.post(value, buffers, ports)` — the "message port post
    /// message steps" (HTML §9.4.4), with the transfer list already split by
    /// `natives.splitTransfer`.
    ///
    /// Serialisation happens *before* the entanglement check, exactly as the
    /// spec orders it: a `DataCloneError` is synchronous and transfers
    /// nothing, while a message to a closed or unentangled port is dropped in
    /// silence *after* its transfer list has been consumed.
    pub fn post<'js>(
        &self, ctx: Ctx<'js>, value: Value<'js>, buffers: Vec<Value<'js>>,
        ports: Vec<Class<'js, Self>>,
    ) -> Result<()> {
        for port in &ports {
            // Step 1: a port cannot be shipped through itself.
            if self.is_same(port) {
                return Err(throw_data_clone(
                    &ctx,
                    "a MessagePort cannot be transferred through itself",
                ));
            }
            // v1 divergence (pinned by a test): the spec ships a started port
            // together with its undelivered messages, but here those messages
            // are inside a pump future in *this* runtime and cannot be moved to
            // another one. Refusing is the honest failure; `take_handle`
            // refuses too, so a path that does not come through here still
            // cannot lose a queue silently.
            if port.try_borrow().is_ok_and(|port| port.started.get()) {
                return Err(throw_data_clone(
                    &ctx,
                    "a started MessagePort cannot be transferred; transfer it before start()",
                ));
            }
        }

        let message = Message::serialize(&ctx, value, buffers, ports)?;
        if let Ok(handle) = self.state.handle.try_borrow()
            && let Some(handle) = handle.as_ref()
        {
            let _ = handle.send(Envelope::Message(message));
        }
        Ok(())
    }

    /// `nativePort.start(onMessage, onMessageError, onClose)` — enable the
    /// port's message queue, or re-enable it after a [`NativePort::pause`]. A
    /// no-op while a run is already delivering, and on a closed or detached
    /// port.
    ///
    /// `onMessage` is called with the rebuilt value and the array of
    /// transferred [`NativePort`]s; the other two take no arguments. `onClose`
    /// fires once, when the peer has gone and nothing can arrive again — never
    /// for a [`NativePort::pause`], which is this realm's own doing.
    pub fn start<'js>(
        &self, ctx: Ctx<'js>, on_message: Function<'js>, on_message_error: Function<'js>,
        on_close: Function<'js>,
    ) {
        let Ok(mut run) = self.state.run.try_borrow_mut() else {
            return;
        };
        if run.is_some() || self.stop.is_cancelled() {
            return;
        }
        // The very first run moves the inbox out of the handle for good: the
        // port can no longer be transferred, but every later run finds the
        // queue — and whatever the peer sent while nobody was listening — right
        // where the last one left it. The destination is borrowed before the
        // inbox is taken, so no path here can drop a queue on the floor.
        if !self.started.get() {
            let Ok(mut inbox) = self.state.inbox.try_borrow_mut() else {
                return;
            };
            let taken = self
                .state
                .handle
                .try_borrow_mut()
                .ok()
                .and_then(|mut handle| handle.as_mut().and_then(PortHandle::take_receiver));
            let Some(taken) = taken else {
                return;
            };
            *inbox = Some(taken);
            self.started.set(true);
        }
        // A child of `stop`, so that `close()` reaches whichever run is current
        // without having to know about it.
        let token = self.stop.child_token();
        *run = Some(token.clone());
        ctx.spawn(Self::pump(
            ctx.clone(),
            Rc::clone(&self.state),
            token,
            on_message,
            on_message_error,
            on_close,
        ));
    }

    /// `nativePort.pause()` — stop delivering without detaching: the port stays
    /// entangled and its queue keeps filling, but nothing keeps the event loop
    /// alive on its behalf any more. The next `start` resumes where this run
    /// stopped. Idempotent, and a no-op on a port that is not delivering.
    pub fn pause(&self) {
        let taken = self
            .state
            .run
            .try_borrow_mut()
            .ok()
            .and_then(|mut run| run.take());
        if let Some(run) = taken {
            run.cancel();
        }
    }
}

/// `natives.pair()` — two entangled ports, the guts of `new MessageChannel()`.
#[rquickjs::function(rename = "pair")]
pub fn pair<'js>(ctx: Ctx<'js>) -> Result<Vec<Class<'js, NativePort>>> {
    let (first, second) = PortHandle::pair();
    Ok(vec![
        Class::instance(ctx.clone(), NativePort::from_handle(first))?,
        Class::instance(ctx, NativePort::from_handle(second))?,
    ])
}

const WRAPPER_SLOT: &str = "\0den:port-wrapper";

fn dispatch_messages_at<'js>(
    ctx: &Ctx<'js>, target: Value<'js>, native: Class<'js, NativePort>, after: Function<'js>,
) {
    let on_message = Function::new(ctx.clone(), {
        let target = target.clone();
        let after = after.clone();
        move |ctx: Ctx<'js>, data: Value<'js>, ports: Vec<Class<'js, NativePort>>| -> Result<()> {
            let wrapped = ports
                .into_iter()
                .map(|port| MessagePort::wrap(&ctx, port))
                .collect::<Result<Vec<_>>>()?;
            let init = Object::new(ctx.clone())?;
            init.set("data", data)?;
            init.set("ports", wrapped)?;
            let event = Class::instance(
                ctx.clone(),
                crate::events::MessageEvent::new(
                    ctx.clone(),
                    "message".into_js(&ctx)?,
                    Opt(Some(init.into_value())),
                )?,
            )?;
            crate::events::dispatch_trusted(ctx.clone(), target.clone(), event.into_value())?;
            after.call::<_, ()>(("message",))?;
            Ok(())
        }
    })
    .expect("on_message");
    let on_message_error = Function::new(ctx.clone(), {
        let target = target.clone();
        let after = after.clone();
        move |ctx: Ctx<'js>| -> Result<()> {
            let event = Class::instance(
                ctx.clone(),
                crate::events::MessageEvent::new(
                    ctx.clone(),
                    "messageerror".into_js(&ctx)?,
                    Opt(None),
                )?,
            )?;
            crate::events::dispatch_trusted(ctx.clone(), target.clone(), event.into_value())?;
            after.call::<_, ()>(("messageerror",))?;
            Ok(())
        }
    })
    .expect("on_message_error");
    let on_close = Function::new(ctx.clone(), {
        let target = target.clone();
        move |ctx: Ctx<'js>| -> Result<()> {
            let event = Class::instance(
                ctx.clone(),
                crate::events::Event::new(ctx.clone(), "close".into_js(&ctx)?, Opt(None))?,
            )?;
            crate::events::dispatch_trusted(ctx.clone(), target.clone(), event.into_value())?;
            Ok(())
        }
    })
    .expect("on_close");
    native
        .borrow()
        .start(ctx.clone(), on_message, on_message_error, on_close);
}

/// HTML §9.4.4 `MessagePort`. Ports come from a channel, a transfer, or a
/// Worker; the constructor is illegal.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct MessagePort<'js> {
    native: Class<'js, NativePort>,
}

impl<'js> MessagePort<'js> {
    pub fn wrap(ctx: &Ctx<'js>, native: Class<'js, NativePort>) -> Result<Class<'js, Self>> {
        if let Ok(existing) = native.get::<_, Class<'js, Self>>(WRAPPER_SLOT) {
            return Ok(existing);
        }
        let port = Class::instance(ctx.clone(), Self {
            native: native.clone(),
        })?;
        native.set(WRAPPER_SLOT, port.clone())?;
        let handle = crate::message::clone::CloneState::port_handle(ctx)?;
        port.set(handle, native)?;
        Ok(port)
    }

    fn native(&self) -> Class<'js, NativePort> { self.native.clone() }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> MessagePort<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    pub fn post_message(
        &self, ctx: Ctx<'js>, message: Value<'js>, options: Opt<Value<'js>>,
    ) -> Result<()> {
        let transfer = match options.0 {
            Some(options) if options.is_array() => Some(options),
            Some(options) if options.is_object() => options.as_object().unwrap().get("transfer")?,
            _ => None,
        };
        let (buffers, ports) = crate::message::clone::split_transfer(&ctx, transfer)?;
        self.native.borrow().post(ctx, message, buffers, ports)
    }

    pub fn start(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<()> {
        let native = this.0.borrow().native();
        let noop = Function::new(ctx.clone(), |_: String| ())?;
        dispatch_messages_at(&ctx, this.0.clone().into_value(), native, noop);
        Ok(())
    }

    pub fn close(&self) { self.native.borrow().close(); }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "MessagePort" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct MessageChannel<'js> {
    port1: Class<'js, MessagePort<'js>>,
    port2: Class<'js, MessagePort<'js>>,
}

#[rquickjs::methods]
impl<'js> MessageChannel<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        let pair = pair(ctx.clone())?;
        Ok(Self {
            port1: MessagePort::wrap(&ctx, pair[0].clone())?,
            port2: MessagePort::wrap(&ctx, pair[1].clone())?,
        })
    }

    #[qjs(get)]
    pub fn port1(&self) -> Class<'js, MessagePort<'js>> { self.port1.clone() }

    #[qjs(get)]
    pub fn port2(&self) -> Class<'js, MessagePort<'js>> { self.port2.clone() }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "MessageChannel" }
}

/// Ref-on-listener: returns the arm function. See docs/research/11 §2.1 rule 2.
pub fn track_message_listeners<'js>(
    ctx: Ctx<'js>, target: Value<'js>, native: Class<'js, NativePort>,
) -> Result<Function<'js>> {
    let helper: Function<'js> = ctx.eval(
        r#"(function (target, native, dispatchMessagesAt) {
          const buckets = new Map();
          const bucketFor = (type, capture) => {
            const key = `${type}|${capture}`;
            const bucket = buckets.get(key) ?? new Map();
            buckets.set(key, bucket);
            return bucket;
          };
          let armed = false;
          let refed = false;
          const refresh = () => {
            const listening = armed &&
              [...buckets.values()].some((bucket) => bucket.size > 0);
            if (listening === refed) return;
            refed = listening;
            if (listening) dispatchMessagesAt(target, native, retire);
            else native.pause();
          };
          const retire = (type) => {
            for (const capture of [false, true]) {
              const bucket = bucketFor(type, capture);
              for (const [callback, once] of bucket) {
                if (once) bucket.delete(callback);
              }
            }
            refresh();
          };
          const inherited = {
            add: target.addEventListener.bind(target),
            remove: target.removeEventListener.bind(target),
          };
          const flatten = (options) =>
            typeof options === "boolean"
              ? { capture: options, once: false, signal: undefined }
              : {
                  capture: !!options?.capture,
                  once: !!options?.once,
                  signal: options?.signal ?? undefined,
                };
          const track = (type, callback, options) => {
            const { capture, once, signal } = flatten(options);
            if (callback === null || callback === undefined || signal?.aborted) return;
            const bucket = bucketFor(type, capture);
            if (bucket.has(callback)) return;
            bucket.set(callback, once);
            signal?.addEventListener("abort", () => {
              bucket.delete(callback);
              refresh();
            }, { once: true });
          };
          const property = (value) => ({ value, writable: true, configurable: true });
          Object.defineProperties(target, {
            addEventListener: property((...args) => {
              inherited.add(...args);
              const [type, callback, options] = args;
              if (!["message", "messageerror"].includes(String(type))) return;
              track(String(type), callback, options);
              refresh();
            }),
            removeEventListener: property((...args) => {
              inherited.remove(...args);
              const [type, callback, options] = args;
              if (!["message", "messageerror"].includes(String(type))) return;
              bucketFor(String(type), flatten(options).capture).delete(callback);
              refresh();
            }),
          });
          return () => { armed = true; refresh(); };
        })"#,
    )?;
    let dispatch = Function::new(ctx.clone(), {
        move |ctx: Ctx<'js>,
              target: Value<'js>,
              native: Class<'js, NativePort>,
              after: Function<'js>| {
            dispatch_messages_at(&ctx, target, native, after);
        }
    })?;
    helper.call((target, native, dispatch))
}

/// Install pair + wrapPort. Public classes are module exports.
pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    natives.set("pair", js_pair)?;
    natives.set(
        "wrapPort",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, native: Class<'js, NativePort>| MessagePort::wrap(&ctx, native),
        )?,
    )?;
    Ok(())
}

pub fn finish<'js>(ctx: &Ctx<'js>) -> Result<()> {
    crate::events::inherit::<MessagePort, crate::events::EventTarget>(ctx)?;
    if let Some(proto) = Class::<MessagePort>::prototype(ctx)? {
        crate::events::define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessage".to_owned(),
            Opt(None),
        )?;
        crate::events::define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessageerror".to_owned(),
            Opt(None),
        )?;
        crate::events::define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onclose".to_owned(),
            Opt(None),
        )?;
        let wrap_onmessage: Function<'js> = ctx.eval(
            r#"(function (proto) {
              const desc = Object.getOwnPropertyDescriptor(proto, "onmessage");
              Object.defineProperty(proto, "onmessage", {
                ...desc,
                set(value) {
                  desc.set.call(this, value);
                  this.start();
                },
              });
            })"#,
        )?;
        wrap_onmessage.call::<_, ()>((proto,))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Function, Module};
    use tokio::time;

    use super::NativePort;
    use crate::transport::PortHandle;

    /// The one piece of `den:worker` these tests need that `den:worker` does
    /// not export: `__trackMessageListeners`, the ref rule itself. Worker
    /// construction is its production caller; the tests reach it as a global
    /// the fixture installs, and `nativeOf` reads the port-handle symbol the
    /// clone pre-pass uses.
    const LIFT_TRACKER: &str = r#"(function () {
      const port = new MessageChannel().port1;
      const handle = Object.getOwnPropertySymbols(port)
        .find((symbol) => symbol.description === "den:port-handle");
      if (handle === undefined) throw new TypeError("a MessagePort has no native handle");
      globalThis.nativeOf = (wrapper) => wrapper[handle];
      port.close();
    })"#;

    /// One async runtime with `MessageChannel`/`MessagePort` installed. The
    /// pumps are `ctx.spawn`-ed futures, so nothing is delivered until the
    /// runtime is driven — which every test does through [`Fixture::settle`].
    struct Fixture {
        runtime: AsyncRuntime,
        context: AsyncContext,
    }

    impl Fixture {
        async fn new() -> Self {
            let runtime = AsyncRuntime::new().expect("runtime");
            let context = AsyncContext::full(&runtime).await.expect("context");
            context
                .with(|ctx| {
                    // The real `den:worker`: a harness that re-implemented
                    // lib.rs's wiring would survive every mutation of it.
                    let install = || -> rquickjs::Result<()> {
                        let (_, evaluated) =
                            Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                        evaluated.finish::<()>()?;
                        ctx.globals().set(
                            "__trackMessageListeners",
                            Function::new(ctx.clone(), crate::port::track_message_listeners)?,
                        )?;
                        let lift: Function<'_> = ctx.eval(LIFT_TRACKER)?;
                        lift.call(())
                    };
                    install().catch(&ctx).map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| panic!("den:worker installs: {error}"));
            Self { runtime, context }
        }

        async fn eval<T>(&self, source: &'static str) -> T
        where
            T: for<'js> FromJs<'js> + Send + 'static,
        {
            self.context
                .with(move |ctx| {
                    ctx.eval::<T, _>(source)
                        .catch(&ctx)
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| panic!("{error}"))
        }

        async fn run(&self, source: &'static str) { self.eval::<()>(source).await }

        async fn text(&self, source: &'static str) -> String { self.eval::<String>(source).await }

        /// Drive the runtime until every spawned future is done. A port whose
        /// pump is still running never settles, which is exactly the
        /// process-lifetime rule these tests pin.
        async fn settle(&self) {
            time::timeout(Duration::from_secs(5), self.runtime.idle())
                .await
                .expect("the runtime goes idle");
        }

        /// Drive the runtime for a moment without requiring it to go idle: a
        /// port that is reffed never does, and a test still has to let its pump
        /// run. What is asserted afterwards is what the pump delivered, and by
        /// then the pump has nothing left to wait for.
        async fn drain(&self) {
            let _ = time::timeout(Duration::from_millis(200), self.runtime.idle()).await;
        }

        /// Whether a spawned future is still alive. This is a *negative*
        /// assertion — nothing will ever wake `idle()` — so it is the one place
        /// in these tests where a duration is waited out.
        async fn is_busy(&self) -> bool {
            time::timeout(Duration::from_millis(200), self.runtime.idle())
                .await
                .is_err()
        }
    }

    /// Assigning `onmessage` also enables the queue (HTML §9.4.4), so this pins
    /// both delivery order and the implicit `start()`.
    #[tokio::test]
    async fn a_channel_delivers_messages_in_the_order_they_were_posted() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.onmessage = (event) => {
                  log.push(event.data);
                  if (log.length === 3) channel.port2.close();
                };
                for (const item of [1, 2, 3]) channel.port1.postMessage(item);
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "1,2,3");
    }

    #[tokio::test]
    async fn a_listener_alone_does_not_start_the_port_and_the_message_waits_for_start() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                globalThis.channel = new MessageChannel();
                channel.port1.postMessage("early");
                channel.port2.addEventListener("message", (event) => {
                  log.push(event.data);
                  channel.port2.close();
                });
                "#,
            )
            .await;
        // Nothing is pumping, so the runtime is idle at once and the message is
        // still sitting in the channel.
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");

        fixture.run("channel.port2.start();").await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "early");
    }

    #[tokio::test]
    async fn close_stops_delivery_and_every_later_post_is_a_silent_no_op() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.onmessage = (event) => log.push(event.data);
                channel.port2.close();
                channel.port1.postMessage("to a closed peer");
                channel.port2.postMessage("from a closed port");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");
    }

    #[tokio::test]
    async fn a_started_port_keeps_the_runtime_busy_until_it_is_closed() {
        let fixture = Fixture::new().await;
        fixture
            .run("globalThis.channel = new MessageChannel(); channel.port2.start();")
            .await;
        assert!(
            fixture.is_busy().await,
            "a started port's pump must keep idle() pending"
        );
        fixture.run("channel.port2.close();").await;
        fixture.settle().await;
    }

    #[tokio::test]
    async fn a_transferred_port_arrives_as_one_wrapper_and_leaves_the_source_detached() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const outer = new MessageChannel();
                const inner = new MessageChannel();
                outer.port2.onmessage = (event) => {
                  const moved = event.ports[0];
                  // `restore` splices the wrapper into the data graph while Rust
                  // hands the same NativePort back for `ports`: one wrapper.
                  log.push(`same:${event.data.port === moved}`);
                  log.push(`fresh:${moved !== inner.port2}`);
                  moved.onmessage = (relayed) => {
                    log.push(`through:${relayed.data}`);
                    moved.close();
                    outer.port2.close();
                  };
                  inner.port1.postMessage("relayed");
                };
                outer.port1.postMessage({ port: inner.port2 }, [inner.port2]);
                // The source is detached now, so this goes nowhere and throws
                // nothing.
                inner.port2.postMessage("into the void");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "same:true,fresh:true,through:relayed"
        );
    }

    /// v1 divergence from HTML §9.4.4, which ships a started port together with
    /// its undelivered messages: here those messages live in a pump future
    /// belonging to this runtime and cannot be moved to another one.
    #[tokio::test]
    async fn a_started_port_refuses_to_be_transferred() {
        let fixture = Fixture::new().await;
        let failure = fixture
            .text(
                r#"(() => {
                     globalThis.channel = new MessageChannel();
                     const carrier = new MessageChannel();
                     channel.port2.start();
                     try {
                       carrier.port1.postMessage(null, [channel.port2]);
                       return "no throw";
                     } catch (error) {
                       return error instanceof DOMException
                         ? error.name : `wrong error: ${error}`;
                     }
                   })()"#,
            )
            .await;
        assert_eq!(failure, "DataCloneError");
        // The refusal left the port intact — including closable, or the runtime
        // would never go idle again.
        fixture.run("channel.port2.close();").await;
        fixture.settle().await;
    }

    /// The prologue every ref-rule test starts from: a target the tracker
    /// drives — a plain `EventTarget`, because the two real ones are a `Worker`
    /// and a worker global, neither of which is a `MessagePort` — wired to one
    /// end of a channel, and armed.
    const TRACKED: &str = r#"
        globalThis.log = [];
        globalThis.channel = new MessageChannel();
        globalThis.listener = (event) => log.push(event.data);
        globalThis.target = new EventTarget();
        globalThis.arm = __trackMessageListeners(target, nativeOf(channel.port2));
        arm();
    "#;

    /// The ref rule, in one port (docs/research/11 §2.1 rule 2, test I-27): a
    /// tracked port keeps the runtime awake exactly while it has a listener,
    /// across as many transitions as the script cares to make.
    #[tokio::test]
    async fn a_tracked_port_refs_on_its_first_listener_and_unrefs_with_its_last() {
        let fixture = Fixture::new().await;
        fixture.run(TRACKED).await;
        // Armed but silent: nothing is listening, so nothing is pumping.
        fixture.settle().await;

        fixture
            .run(r#"target.addEventListener("message", listener);"#)
            .await;
        assert!(
            fixture.is_busy().await,
            "a listener must keep the port's pump alive"
        );

        // A second listener, then the first one back off again: only the *last*
        // one leaving unrefs the port.
        fixture
            .run(
                r#"
                globalThis.second = () => {};
                target.addEventListener("message", second);
                target.removeEventListener("message", listener);
                "#,
            )
            .await;
        assert!(
            fixture.is_busy().await,
            "one listener leaving must not unref a port another is watching"
        );

        fixture
            .run(r#"target.removeEventListener("message", second);"#)
            .await;
        fixture.settle().await;

        // And back again, as often as the script likes.
        fixture
            .run(r#"target.addEventListener("message", listener);"#)
            .await;
        assert!(fixture.is_busy().await, "the pump must come back");
        fixture
            .run(r#"target.removeEventListener("message", listener);"#)
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");
    }

    /// The invariant that makes unreffing safe: an unreffed port stops
    /// *delivering*, not receiving. Everything the peer sent while nobody was
    /// listening is still there for the listener that turns up later.
    #[tokio::test]
    async fn messages_sent_while_a_tracked_port_is_unreffed_are_queued_not_lost() {
        let fixture = Fixture::new().await;
        fixture.run(TRACKED).await;
        fixture
            .run(r#"channel.port1.postMessage("before any listener");"#)
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");

        // One listener, one dispatch of the message that was already waiting.
        fixture
            .run(r#"target.addEventListener("message", listener);"#)
            .await;
        fixture.drain().await;
        assert_eq!(fixture.text("log.join()").await, "before any listener");

        // Unref, post two, ref again: order is the channel's, not the pump's.
        fixture
            .run(
                r#"
                target.removeEventListener("message", listener);
                channel.port1.postMessage("while unreffed 1");
                channel.port1.postMessage("while unreffed 2");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "before any listener");

        fixture
            .run(r#"target.addEventListener("message", listener);"#)
            .await;
        fixture.drain().await;
        fixture.run("channel.port2.close();").await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "before any listener,while unreffed 1,while unreffed 2"
        );
    }

    /// A `once` listener is retired by the dispatch that runs it, and
    /// EventTarget does that without telling anyone — so the mirror the ref
    /// rule counts has to notice, or the port would stay reffed for a listener
    /// that is gone.
    #[tokio::test]
    async fn a_once_listener_unrefs_the_port_after_it_has_fired() {
        let fixture = Fixture::new().await;
        fixture.run(TRACKED).await;
        fixture
            .run(
                r#"
                target.addEventListener("message", listener, { once: true });
                channel.port1.postMessage("only one");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "only one");
    }

    /// The same for a listener removed through an `AbortSignal`: EventTarget
    /// duck-types the signal, so the mirror does too.
    #[tokio::test]
    async fn aborting_a_listener_signal_unrefs_the_port() {
        let fixture = Fixture::new().await;
        fixture.run(TRACKED).await;
        fixture
            .run(
                r#"
                // A handmade signal: EventTarget only ever reads `aborted`
                // and listens for `abort`, so this is a faithful enough one.
                globalThis.signal = new EventTarget();
                signal.aborted = false;
                target.addEventListener("message", listener, { signal });
                "#,
            )
            .await;
        assert!(fixture.is_busy().await, "the listener refs the port");
        fixture
            .run(r#"signal.aborted = true; signal.dispatchEvent(new Event("abort"));"#)
            .await;
        fixture.settle().await;
    }

    #[tokio::test]
    async fn a_data_clone_error_is_synchronous_and_leaves_the_port_usable() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.onmessage = (event) => {
                  log.push(event.data);
                  channel.port2.close();
                };
                try { channel.port1.postMessage(() => {}); }
                catch (error) { log.push(error.name); }
                channel.port1.postMessage("still usable");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "DataCloneError,still usable"
        );
    }

    #[tokio::test]
    async fn a_message_that_cannot_be_rebuilt_fires_messageerror() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.onmessageerror = (event) => {
                  log.push(`${event.type}:${event.data}`);
                  channel.port2.close();
                };
                // `onmessageerror` does not enable the queue; only `onmessage`
                // and `start()` do.
                channel.port2.start();
                // A clone tag whose revival throws on the far side: a DataView
                // cannot be built past the end of its buffer.
                channel.port1.postMessage({
                  "\u0000den:structured-clone": "DataView",
                  buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
                });
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "messageerror:null");
    }

    /// HTML §9.4.4 "close a MessagePort": closing detaches the port, and a
    /// detached port has no queue — neither the envelopes already sitting in it
    /// nor a `start()` that comes afterwards can produce a dispatch. The
    /// closing happens *inside* a handler, so the two later messages were
    /// already in the channel when it did.
    #[tokio::test]
    async fn close_inside_a_handler_discards_the_rest_of_the_queue_for_good() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                globalThis.channel = new MessageChannel();
                channel.port2.onmessage = (event) => {
                  log.push(event.data);
                  channel.port2.close();
                };
                for (const item of ["first", "second", "third"]) {
                  channel.port1.postMessage(item);
                }
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "first");

        // And it stays closed: re-enabling the queue on a detached port is a
        // no-op, so nothing that was dropped comes back.
        fixture.run("channel.port2.start();").await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "first");
    }

    /// HTML §9.4.4 postMessage step 1: a port cannot be shipped through itself.
    /// The refusal is a `DataCloneError` and it transfers nothing, so the port
    /// is still entangled and still usable afterwards.
    #[tokio::test]
    async fn a_port_transferred_through_itself_is_a_data_clone_error() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                globalThis.channel = new MessageChannel();
                channel.port2.onmessage = (event) => {
                  log.push(event.data);
                  channel.port2.close();
                };
                try { channel.port1.postMessage(null, [channel.port1]); log.push("no throw"); }
                catch (error) {
                  log.push(error instanceof DOMException ? error.name : `wrong: ${error}`);
                }
                channel.port1.postMessage("still entangled");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "DataCloneError,still entangled"
        );
    }

    /// Spec step 2 of `StructuredSerializeWithTransfer`, the case
    /// `JS_DetachArrayBuffer` does not guard: an immutable `ArrayBuffer` cannot
    /// be transferred. Detaching one would free bytes that are shared with
    /// every buffer it was sliced from.
    #[tokio::test]
    async fn an_immutable_array_buffer_cannot_be_transferred() {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture
                .text(
                    r#"(() => {
                         const channel = new MessageChannel();
                         const immutable = new Uint8Array([1, 2, 3]).buffer.sliceToImmutable();
                         try {
                           channel.port1.postMessage(immutable, [immutable]);
                           return "no throw";
                         } catch (error) {
                           return error instanceof DOMException
                             ? `${error.name}:${immutable.byteLength}` : `wrong: ${error}`;
                         } finally { channel.port1.close(); channel.port2.close(); }
                       })()"#,
                )
                .await,
            // Still three bytes long: the refusal detached nothing.
            "DataCloneError:3"
        );
    }

    /// [`NativePort::take_handle`]'s own guard, reached directly because no
    /// script can: `post` refuses a started port before serialisation ever
    /// begins, so this last line of defence — the one that keeps a transfer
    /// from consuming half a list and then failing — has no JS caller left.
    #[tokio::test]
    async fn take_handle_refuses_a_port_whose_queue_has_been_enabled() {
        let fixture = Fixture::new().await;
        fixture
            .context
            .with(|ctx| {
                let (first, second) = PortHandle::pair();
                // The peer is kept alive for the length of the test: a port
                // whose peer has hung up detaches itself, which would make the
                // refusal below true for the wrong reason.
                let _peer = second;
                let port = NativePort::from_handle(first);
                assert!(!port.is_started(), "a fresh port owns no inbox");

                let noop = Function::new(ctx.clone(), || {}).expect("a callback");
                port.start(ctx.clone(), noop.clone(), noop.clone(), noop);
                assert!(port.is_started(), "start() moves the inbox into the port");
                assert!(
                    port.take_handle().is_none(),
                    "a started port must refuse to hand its channel end away"
                );
                assert!(
                    port.is_open(),
                    "the refusal must leave the port entangled, not half-transferred"
                );
                port.close();
            })
            .await;
        fixture.settle().await;
    }

    #[tokio::test]
    async fn the_options_overload_transfers_a_buffer_and_detaches_it_before_delivery() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                const buffer = new Uint8Array([1, 2, 3]).buffer;
                channel.port2.onmessage = (event) => {
                  log.push(new Uint8Array(event.data).join("-"));
                  channel.port2.close();
                };
                channel.port1.postMessage(buffer, { transfer: [buffer] });
                log.push(`detached:${buffer.detached}`);
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "detached:true,1-2-3");
    }

    /// HTML §9.4.4: closing one end disentangles the other, and a started port
    /// hears about it once, as a `close` event — through `onclose` as much as
    /// through a listener. The port is already dead by then, so a `postMessage`
    /// from inside the handler is the documented silent no-op.
    #[tokio::test]
    async fn a_peer_that_closes_fires_close_at_a_started_port() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                globalThis.channel = new MessageChannel();
                channel.port2.onclose = (event) => {
                  log.push(`onclose:${event.type}:${event.isTrusted}`);
                  channel.port2.postMessage("after close");
                };
                channel.port2.addEventListener("close", () => log.push("listener"));
                channel.port2.start();
                channel.port1.close();
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "onclose:close:true,listener"
        );
    }

    /// The mirror image: a port that closes *itself* is not disentangled by
    /// anyone, so it fires nothing — and neither does a port whose queue was
    /// never enabled, which has no pump to notice (the documented ceiling on
    /// `dispatchMessagesAt`'s third callback).
    #[tokio::test]
    async fn closing_a_port_yourself_and_never_starting_one_fire_no_close_event() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const own = new MessageChannel();
                own.port2.onclose = () => log.push("own");
                own.port2.start();
                own.port2.close();

                const unstarted = new MessageChannel();
                unstarted.port2.addEventListener("close", () => log.push("unstarted"));
                unstarted.port1.close();
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");
    }

    /// DOM: an event the platform fires is trusted, one a script dispatches is
    /// not. Both of a port's message events come from the pump, so both are —
    /// and the `MessageEvent` a script builds and dispatches itself is not,
    /// which is what proves the flag is not simply always on.
    #[tokio::test]
    async fn the_events_a_port_fires_are_trusted_and_a_scripted_one_is_not() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.addEventListener("scripted", (event) =>
                  log.push(`scripted:${event.isTrusted}`));
                channel.port2.onmessage = (event) => {
                  log.push(`message:${event.isTrusted}`);
                  channel.port2.dispatchEvent(new MessageEvent("scripted"));
                  channel.port2.close();
                };
                channel.port1.postMessage(1);
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(
            fixture.text("log.join()").await,
            "message:true,scripted:false"
        );
    }

    /// An out-of-bounds `ArrayBufferView` is refused on the *sender*, before a
    /// byte leaves the realm. quickjs writes the view's stale offset happily
    /// and only its reader complains, so this used to cross the channel and
    /// land as a `messageerror` on the far side — the receiver being told about
    /// a mistake only the sender could fix. Nothing is delivered, and the port
    /// is unharmed.
    #[tokio::test]
    async fn an_out_of_bounds_view_is_refused_at_post_time_and_never_crosses() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const channel = new MessageChannel();
                channel.port2.onmessage = () => log.push("message");
                channel.port2.addEventListener("messageerror", () => log.push("messageerror"));
                const buffer = new ArrayBuffer(8, { maxByteLength: 8 });
                const view = new Uint8Array(buffer, 4);
                buffer.resize(0);
                try { channel.port1.postMessage(view); }
                catch (error) { log.push(`${error.name}`); }
                channel.port1.postMessage("still usable");
                "#,
            )
            .await;
        fixture.drain().await;
        assert_eq!(fixture.text("log.join()").await, "DataCloneError,message");
    }
}
