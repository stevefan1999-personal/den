//! `BroadcastChannel`'s native half (HTML §9.5): the process-global registry
//! of subscribers by channel name, and the pump that delivers to one of them.
//!
//! The JS-visible `BroadcastChannel` lives in `src/prelude/broadcast.js`
//! because it `extends EventTarget`.
//!
//! The registry is a plain process-global map rather than anything per-realm,
//! and that is the whole cross-thread story: a `BroadcastChannel` in a worker
//! registers in the same map as one on the main thread, so a post reaches it
//! with no plumbing between the two runtimes. Only `Message` — `Send`, tied to
//! no runtime — crosses; the receiving realm rebuilds the value itself.

use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        LazyLock, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use rquickjs::{Class, Ctx, Error, Function, JsLifetime, Object, Result, Value, class::Trace};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::{message::Message, report::report_exception};

/// One live `BroadcastChannel`, as seen by every *other* one with its name.
struct Subscriber {
    /// Identity, so that a post can skip its own channel. A raw pointer would
    /// do it too, but an id is comparable across threads without being one.
    id:    u64,
    inbox: UnboundedSender<Message>,
}

/// Every open channel in the process, keyed by name.
///
/// The lock is only ever held around a map lookup and a batch of non-blocking
/// `send`s — never across an await, and never while running JS.
// ponytail: one global lock for all names; shard by name if a broadcast-heavy
// workload ever shows contention.
static SUBSCRIBERS: LazyLock<Mutex<HashMap<String, Vec<Subscriber>>>> =
    LazyLock::new(Mutex::default);

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
        let mut subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(peers) = subscribers.get_mut(&self.name) else {
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
    fn unregister(&self) {
        let mut subscribers = SUBSCRIBERS.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(peers) = subscribers.get_mut(&self.name) {
            peers.retain(|peer| peer.id != self.id);
            if peers.is_empty() {
                subscribers.remove(&self.name);
            }
        }
    }

    /// Rebuild one message in this realm and hand it to the channel's
    /// callbacks. A message that cannot be rebuilt here is a `messageerror`
    /// event, not an error of the pump's, so its exception is caught and
    /// dropped rather than reported.
    fn dispatch<'js>(
        ctx: &Ctx<'js>,
        message: Message,
        on_message: &Function<'js>,
        on_message_error: &Function<'js>,
    ) {
        let outcome = match message.deserialize(ctx) {
            // A broadcast never carries ports — they are not serialisable and
            // `post` offers no transfer list — so the second half is empty.
            Ok((value, _)) => on_message.call::<_, ()>((value,)),
            Err(error) => {
                // Clear the pending exception, or it surfaces at the next
                // unrelated call into this context.
                if let Error::Exception = error {
                    ctx.catch();
                }
                on_message_error.call::<_, ()>(())
            }
        };
        // A handler that throws has nobody to propagate to: whoever posted is
        // on another thread, or returned long ago.
        if let Err(error) = outcome {
            match error {
                Error::Exception => report_exception(ctx, &ctx.catch()),
                error => eprintln!("{error}"),
            }
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
        ctx: Ctx<'js>,
        mut inbox: UnboundedReceiver<Message>,
        stop: CancellationToken,
        on_message: Function<'js>,
        on_message_error: Function<'js>,
    ) {
        while let Some(Some(message)) = stop.run_until_cancelled(inbox.recv()).await {
            Self::dispatch(&ctx, message, &on_message, &on_message_error);
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
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        &self,
        ctx: Ctx<'js>,
        on_message: Function<'js>,
        on_message_error: Function<'js>,
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

/// Install the broadcast natives: the class constructor the prelude wraps.
pub fn install<'js>(_ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    Class::<NativeBroadcast>::define(natives)
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module};
    use tokio::time;

    /// One async runtime with `BroadcastChannel` installed. Delivery happens in
    /// a `ctx.spawn`-ed pump, so nothing arrives until the runtime is driven —
    /// which every test does through [`Fixture::settle`].
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
                    // The real `den:worker`, not a hand-spliced copy of what
                    // lib.rs does with the preludes: a harness that rebuilt the
                    // module's wiring would survive every mutation of it.
                    let install = || -> rquickjs::Result<()> {
                        let (_, evaluated) =
                            Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                        evaluated.finish::<()>()
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

        async fn run(&self, source: &'static str) {
            self.eval::<()>(source).await
        }

        async fn text(&self, source: &'static str) -> String {
            self.eval::<String>(source).await
        }

        /// Drive the runtime until every spawned future is done. An open
        /// channel whose pump is still running never settles, which is exactly
        /// the process-lifetime rule these tests pin.
        async fn settle(&self) {
            time::timeout(Duration::from_secs(5), self.runtime.idle())
                .await
                .expect("the runtime goes idle");
        }

        /// Whether a spawned future is still alive. A *negative* assertion —
        /// nothing will ever wake `idle()` — so it is the one place in these
        /// tests where a duration is waited out.
        async fn is_busy(&self) -> bool {
            time::timeout(Duration::from_millis(200), self.runtime.idle())
                .await
                .is_err()
        }
    }

    /// HTML §9.5: "remove source from destinations" — a channel never receives
    /// what it posted itself.
    #[tokio::test]
    async fn the_sender_does_not_receive_its_own_message() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const sender = new BroadcastChannel("sender-excluded");
                const listener = new BroadcastChannel("sender-excluded");
                sender.onmessage = (event) => log.push(`sender:${event.data}`);
                listener.onmessage = (event) => {
                  log.push(`listener:${event.data}`);
                  sender.close();
                  listener.close();
                };
                sender.postMessage("ping");
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "listener:ping");
    }

    #[tokio::test]
    async fn every_other_channel_of_the_same_name_receives_the_message() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const sender = new BroadcastChannel("two-receivers");
                const channels = [
                  new BroadcastChannel("two-receivers"),
                  new BroadcastChannel("two-receivers"),
                ];
                channels.forEach((channel, index) => {
                  channel.onmessage = (event) => {
                    log.push(`${index}:${event.data.value}`);
                    channel.close();
                  };
                });
                sender.postMessage({ value: 7 });
                sender.close();
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.sort().join()").await, "0:7,1:7");
    }

    #[tokio::test]
    async fn channels_with_different_names_are_isolated() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const sender = new BroadcastChannel("isolated-a");
                const other = new BroadcastChannel("isolated-b");
                const same = new BroadcastChannel("isolated-a");
                other.onmessage = (event) => log.push(`b:${event.data}`);
                same.onmessage = (event) => {
                  log.push(`a:${event.data}`);
                  other.close();
                  same.close();
                };
                sender.postMessage("only for a");
                sender.close();
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "a:only for a");
    }

    /// Two things at once: an open channel holds the runtime busy (its pump is
    /// the process-lifetime mechanism), and `close()` both releases it and
    /// stops delivery.
    #[tokio::test]
    async fn close_stops_delivery_and_lets_the_runtime_go_idle() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                globalThis.sender = new BroadcastChannel("closed-receiver");
                globalThis.receiver = new BroadcastChannel("closed-receiver");
                receiver.onmessage = (event) => log.push(event.data);
                "#,
            )
            .await;
        assert!(
            fixture.is_busy().await,
            "an open channel's pump must keep idle() pending"
        );

        fixture
            .run(r#"receiver.close(); sender.postMessage("after close"); sender.close();"#)
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "");
    }

    #[tokio::test]
    async fn posting_on_a_closed_channel_throws_invalid_state_error() {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture
                .text(
                    r#"(() => {
                         const channel = new BroadcastChannel("post-after-close");
                         channel.close();
                         try { channel.postMessage(1); return "no throw"; }
                         catch (error) {
                           return error instanceof DOMException ? error.name : `wrong: ${error}`;
                         }
                       })()"#,
                )
                .await,
            "InvalidStateError"
        );
    }

    /// A `MessagePort` is [Transferable] but not [Serializable], and
    /// `BroadcastChannel.postMessage` has no transfer list at all — so a port
    /// in the graph can only ever be a `DataCloneError`.
    #[tokio::test]
    async fn a_message_port_in_the_payload_throws_data_clone_error() {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture
                .text(
                    r#"(() => {
                         const channel = new BroadcastChannel("port-payload");
                         const listener = new BroadcastChannel("port-payload");
                         try { channel.postMessage({ port: new MessageChannel().port1 }); return "no throw"; }
                         catch (error) {
                           return error instanceof DOMException ? error.name : `wrong: ${error}`;
                         } finally { channel.close(); listener.close(); }
                       })()"#,
                )
                .await,
            "DataCloneError"
        );
    }

    /// §9.5: `name` is a readonly attribute of the channel, stringified at
    /// construction. Nothing else in this file reads it back, so without this
    /// the getter could return anything at all.
    #[tokio::test]
    async fn a_channel_reports_the_name_it_was_constructed_with() {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture
                .text(
                    r#"(() => {
                         const named = new BroadcastChannel("with a name");
                         const coerced = new BroadcastChannel(7);
                         const report = [
                           named.name,
                           `${coerced.name}:${typeof coerced.name}`,
                           // Readonly: an accessor with no setter, so the
                           // assignment throws here rather than being ignored.
                           (() => {
                             try { named.name = "other"; return "assigned"; }
                             catch (error) { return `${error.constructor.name}:${named.name}`; }
                           })(),
                         ].join("|");
                         named.close();
                         coerced.close();
                         return report;
                       })()"#,
                )
                .await,
            "with a name|7:string|TypeError:with a name"
        );
    }

    /// A payload this realm cannot rebuild is a `messageerror` on the receiving
    /// channel — not a lost message and not an exception in the pump. The
    /// `data` of such an event is `null` (HTML §9.5, §9.4.4).
    #[tokio::test]
    async fn a_message_that_cannot_be_rebuilt_fires_messageerror() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const sender = new BroadcastChannel("bad-payload");
                const receiver = new BroadcastChannel("bad-payload");
                receiver.onmessage = (event) => log.push(`message:${event.data}`);
                receiver.onmessageerror = (event) => {
                  log.push(`${event.type}:${event.data}`);
                  receiver.close();
                };
                // A clone tag whose revival throws on the far side: a DataView
                // cannot be built past the end of its buffer.
                sender.postMessage({
                  "\u0000den:structured-clone": "DataView",
                  buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
                });
                sender.close();
                "#,
            )
            .await;
        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "messageerror:null");
    }

    /// The registry is process-global, so a channel in another *runtime on
    /// another OS thread* — which is what a worker is — is reached with no
    /// plumbing between the two. The receiver registers before the sender
    /// thread starts, and closes itself on delivery, so the assertion is the
    /// runtime going idle rather than a sleep.
    #[tokio::test]
    async fn a_channel_in_another_thread_receives_the_message() {
        let fixture = Fixture::new().await;
        fixture
            .run(
                r#"
                globalThis.log = [];
                const receiver = new BroadcastChannel("across-threads");
                receiver.onmessage = (event) => {
                  log.push(event.data);
                  receiver.close();
                };
                "#,
            )
            .await;

        let sender = thread::spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the sending thread");
            runtime.block_on(async {
                let sender = Fixture::new().await;
                sender
                    .run(
                        r#"
                        const channel = new BroadcastChannel("across-threads");
                        channel.postMessage("from another thread");
                        channel.close();
                        "#,
                    )
                    .await;
                sender.settle().await;
            });
        });
        sender.join().expect("the sending thread finishes");

        fixture.settle().await;
        assert_eq!(fixture.text("log.join()").await, "from another thread");
    }
}
