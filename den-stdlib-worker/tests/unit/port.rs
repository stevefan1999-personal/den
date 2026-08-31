use std::time::Duration;

use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt as _, FromJs, Function, Module};
use tokio::time;

use super::NativePort;
use crate::transport::PortHandle;

/// The one piece of `den:worker` these tests need that `den:worker` does
/// not export: `__trackMessageListeners`, the ref rule itself. Worker
/// construction is its production caller; the tests reach it as a global
/// the fixture installs, and `nativeOf` reads the port-handle symbol the
/// clone pre-pass uses.
const LIFT_TRACKER: &str = include_str!("../fixtures/unit/port/lift_tracker.js");

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
        .run(include_str!(
            "../fixtures/unit/port/a_channel_delivers_messages_in_the_order_they_were_posted.js"
        ))
        .await;
    fixture.settle().await;
    assert_eq!(fixture.text("log.join()").await, "1,2,3");
}

#[tokio::test]
async fn a_listener_alone_does_not_start_the_port_and_the_message_waits_for_start() {
    let fixture = Fixture::new().await;
    fixture
        .run(include_str!(
            "../fixtures/unit/port/\
             a_listener_alone_does_not_start_the_port_and_the_message_waits_for_start.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/close_stops_delivery_and_every_later_post_is_a_silent_no_op.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/\
             a_transferred_port_arrives_as_one_wrapper_and_leaves_the_source_detached.js"
        ))
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
        .text(include_str!(
            "../fixtures/unit/port/a_started_port_refuses_to_be_transferred.js"
        ))
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
const TRACKED: &str = include_str!("../fixtures/unit/port/tracked.js");

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
        .run(include_str!(
            "../fixtures/unit/port/\
             a_tracked_port_refs_on_its_first_listener_and_unrefs_with_its_last.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/\
             messages_sent_while_a_tracked_port_is_unreffed_are_queued_not_lost.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/a_once_listener_unrefs_the_port_after_it_has_fired.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/aborting_a_listener_signal_unrefs_the_port.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/a_data_clone_error_is_synchronous_and_leaves_the_port_usable.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/a_message_that_cannot_be_rebuilt_fires_messageerror.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/close_inside_a_handler_discards_the_rest_of_the_queue_for_good.\
             js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/a_port_transferred_through_itself_is_a_data_clone_error.js"
        ))
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
            .text(include_str!(
                "../fixtures/unit/port/an_immutable_array_buffer_cannot_be_transferred.js"
            ),)
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
        .run(include_str!(
            "../fixtures/unit/port/\
             the_options_overload_transfers_a_buffer_and_detaches_it_before_delivery.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/a_peer_that_closes_fires_close_at_a_started_port.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/\
             closing_a_port_yourself_and_never_starting_one_fire_no_close_event.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/the_events_a_port_fires_are_trusted_and_a_scripted_one_is_not.\
             js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/port/\
             an_out_of_bounds_view_is_refused_at_post_time_and_never_crosses.js"
        ))
        .await;
    fixture.drain().await;
    assert_eq!(fixture.text("log.join()").await, "DataCloneError,message");
}
