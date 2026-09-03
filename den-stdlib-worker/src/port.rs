//! `MessagePort` / `MessageChannel`: the channel end, the pump, and the
//! EventTarget wrappers.
//!
//! The structured-clone pre-pass recognises a port in a message graph by
//! probing for the [`MessagePort`] class and reading its [`NativePort`].
//! `MessagePort.prototype` is reparented onto `EventTarget.prototype` so
//! `port instanceof EventTarget` holds.

use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    rc::Rc,
    task::Poll,
};

use rquickjs::{
    Class, Ctx, Error, Exception, Function, IntoJs as _, JsLifetime, Object, Result, Value,
    class::Trace,
    function::{FuncArg, Opt, This},
    object::Accessor,
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::{
    message::{Message, throw_data_clone},
    report::report_uncaught,
    transport::PortHandle,
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
    inbox:  RefCell<Option<UnboundedReceiver<Message>>>,
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
    pub const fn is_started(&self) -> bool { self.started.get() }

    /// Whether this port still holds a live channel end.
    pub fn is_open(&self) -> bool {
        self.state
            .handle
            .try_borrow()
            .is_ok_and(|handle| handle.is_some())
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
    /// dispatched (HTML §10.2.4 step 4), drop the handle — which is what tells
    /// the peer — and make every later operation a no-op. Idempotent.
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
        // message is on another thread, or returned long ago. Report it like
        // any other uncaught error instead of swallowing it.
        report_uncaught(ctx, outcome);
    }

    /// Clear the pending exception behind an `Err`, so that it cannot surface
    /// at the next unrelated call into this context.
    fn discard(ctx: &Ctx<'_>, error: Error) {
        if matches!(error, Error::Exception) {
            ctx.catch();
        }
    }

    /// Await the next message without moving the inbox out of the port, so
    /// that a run which ends leaves the queue behind for the next one.
    ///
    /// The inbox is borrowed for the length of a single poll and nothing else
    /// borrows it across an await, so the borrow cannot actually fail; waking
    /// and retrying is still the only answer to one that could not lose a
    /// queue.
    fn recv(state: &PortState) -> impl Future<Output = Option<Message>> + '_ {
        poll_fn(|cx| {
            if let Ok(mut inbox) = state.inbox.try_borrow_mut() {
                inbox
                    .as_mut()
                    .map_or_else(|| Poll::Ready(None), |inbox| inbox.poll_recv(cx))
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
    }

    /// One pump run: deliver every message until the peer goes away, until
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
            let message = tokio::select! {
                // Biased, and cancellation first: a paused run must not take a
                // message on its way out, because the listener it would have
                // been dispatched to is the one that just went away.
                biased;
                () = run.cancelled() => return,
                message = Self::recv(&state) => message,
            };
            // `None` is the peer gone. Only a live run reaches this — a
            // cancelled one returns above — so clearing `run` cannot clear a
            // newer run's token.
            let Some(message) = message else { break };
            Self::dispatch(&ctx, message, &on_message, &on_message_error);
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
        report_uncaught(&ctx, on_close.call::<_, ()>(()));
    }
}

#[rquickjs::methods]
impl NativePort {
    /// `nativePort.close()` — HTML §9.4.4 "close() method steps": detach, and
    /// disentangle, which is what tells the peer. Idempotent.
    pub fn close(&self) { self.detach(); }

    /// `nativePort.post(value, buffers, ports)` — the "message port post
    /// message steps" (HTML §9.4.4), with the transfer list already split by
    /// `clone::split_transfer`.
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
            let _ = handle.send(message);
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

/// Two entangled ports, the guts of `new MessageChannel()` and `new Worker()`.
pub fn pair(ctx: Ctx<'_>) -> Result<(Class<'_, NativePort>, Class<'_, NativePort>)> {
    let (first, second) = PortHandle::pair();
    Ok((
        Class::instance(ctx.clone(), NativePort::from_handle(first))?,
        Class::instance(ctx, NativePort::from_handle(second))?,
    ))
}

const WRAPPER_SLOT: &str = "\0den:port-wrapper";

fn dispatch_messages_at<'js>(
    ctx: &Ctx<'js>, target: Value<'js>, native: Class<'js, NativePort>,
) -> Result<()> {
    let on_message = Function::new(ctx.clone(), {
        let target = target.clone();
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
            Ok(())
        }
    })?;
    let on_message_error = Function::new(ctx.clone(), {
        let target = target.clone();
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
            Ok(())
        }
    })?;
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
    })?;
    native
        .borrow()
        .start(ctx.clone(), on_message, on_message_error, on_close);
    Ok(())
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
        Ok(port)
    }

    pub(crate) fn native(&self) -> Class<'js, NativePort> { self.native.clone() }
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
            Some(options) if options.is_object() => {
                options
                    .as_object()
                    .ok_or_else(|| {
                        Exception::throw_type(&ctx, "transfer options must be an object")
                    })?
                    .get("transfer")?
            }
            _ => None,
        };
        let (buffers, ports) = crate::message::clone::split_transfer(&ctx, transfer)?;
        self.native.borrow().post(ctx, message, buffers, ports)
    }

    pub fn start(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<()> {
        let native = this.0.borrow().native();
        dispatch_messages_at(&ctx, this.0.clone().into_value(), native)
    }

    pub fn close(&self) { self.native.borrow().close(); }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "MessagePort" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct MessageChannel<'js> {
    #[qjs(get)]
    port1: Class<'js, MessagePort<'js>>,
    #[qjs(get)]
    port2: Class<'js, MessagePort<'js>>,
}

#[rquickjs::methods]
impl<'js> MessageChannel<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        let (first, second) = pair(ctx.clone())?;
        Ok(Self {
            port1: MessagePort::wrap(&ctx, first)?,
            port2: MessagePort::wrap(&ctx, second)?,
        })
    }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "MessageChannel" }
}

/// The ref rule's one decision: a pump exactly while the target is armed and
/// has a `message`/`messageerror` listener. Idempotent, and safe to reach from
/// inside a dispatch — a paused run leaves its queue behind and returns at its
/// next poll, and a fresh `start` finds `run` already cleared.
fn refresh<'js>(ctx: &Ctx<'js>, bag: &Object<'js>) -> Result<()> {
    let target: Value<'js> = bag.get("target")?;
    let listening = bag.get::<_, bool>("armed")?
        && crate::events::EventTarget::has_listeners(ctx, &target, &["message", "messageerror"])?;
    if listening == bag.get::<_, bool>("refed")? {
        return Ok(());
    }
    bag.set("refed", listening)?;
    let native: Class<'js, NativePort> = bag.get("native")?;
    if listening {
        return dispatch_messages_at(ctx, target, native);
    }
    native.borrow().pause();
    Ok(())
}

/// Ref-on-listener: returns the arm function. See docs/research/11 §2.1 rule 2.
pub fn track_message_listeners<'js>(
    ctx: Ctx<'js>, target: Value<'js>, native: Class<'js, NativePort>,
) -> Result<Function<'js>> {
    // State lives on a JS object hung off the hook rather than in a Rust
    // closure: RustFunction::trace is empty, so a target or port captured in
    // Rust would be invisible to the cycle GC and leak until JS_FreeRuntime.
    let bag = Object::new(ctx.clone())?;
    bag.set("target", target.clone())?;
    bag.set("native", native)?;
    bag.set("armed", false)?;
    bag.set("refed", false)?;
    let hook = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>, function: FuncArg<Function<'js>>| -> Result<()> {
            refresh(&ctx, &function.0.get("_bag")?)
        },
    )?;
    let arm = Function::new(
        ctx.clone(),
        |ctx: Ctx<'js>, function: FuncArg<Function<'js>>| -> Result<()> {
            let bag: Object<'js> = function.0.get("_bag")?;
            bag.set("armed", true)?;
            refresh(&ctx, &bag)
        },
    )?;
    hook.set("_bag", bag.clone())?;
    arm.set("_bag", bag)?;
    crate::events::EventTarget::set_change_hook(&ctx, &target, hook)?;
    Ok(arm)
}

pub fn finish<'js>(ctx: &Ctx<'js>) -> Result<()> {
    den_util::inherit::<MessagePort, crate::events::EventTarget>(ctx)?;
    if let Some(proto) = Class::<MessagePort>::prototype(ctx)? {
        proto.prop(
            "onmessage",
            Accessor::new(
                |this: This<Value<'js>>, ctx: Ctx<'js>| -> Result<Value<'js>> {
                    crate::events::EventTarget::handler_value(&ctx, &this.0, "onmessage")
                },
                |this: This<Value<'js>>, ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
                    crate::events::EventTarget::set_handler(
                        &ctx,
                        &this.0,
                        "onmessage",
                        "message",
                        value,
                        false,
                    )?;
                    let start: Function<'js> = this
                        .0
                        .as_object()
                        .ok_or_else(|| Exception::throw_type(&ctx, "Illegal invocation"))?
                        .get("start")?;
                    start.call((This(this.0.clone()),))
                },
            )
            .configurable(),
        )?;
        crate::events::define_event_handler(
            ctx.clone(),
            proto.clone(),
            "onmessageerror".to_owned(),
            Opt(None),
        )?;
        crate::events::define_event_handler(ctx.clone(), proto, "onclose".to_owned(), Opt(None))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/port.rs"]
mod tests;
