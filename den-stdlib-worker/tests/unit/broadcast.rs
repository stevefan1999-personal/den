use std::{thread, time::Duration};

use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt as _, FromJs, Module};
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

    async fn run(&self, source: &'static str) { self.eval::<()>(source).await }

    async fn text(&self, source: &'static str) -> String { self.eval::<String>(source).await }

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
        .run(include_str!(
            "../fixtures/unit/broadcast/the_sender_does_not_receive_its_own_message.js"
        ))
        .await;
    fixture.settle().await;
    assert_eq!(fixture.text("log.join()").await, "listener:ping");
}

#[tokio::test]
async fn every_other_channel_of_the_same_name_receives_the_message() {
    let fixture = Fixture::new().await;
    fixture
        .run(include_str!(
            "../fixtures/unit/broadcast/every_other_channel_of_the_same_name_receives_the_message.\
             js"
        ))
        .await;
    fixture.settle().await;
    assert_eq!(fixture.text("log.sort().join()").await, "0:7,1:7");
}

#[tokio::test]
async fn channels_with_different_names_are_isolated() {
    let fixture = Fixture::new().await;
    fixture
        .run(include_str!(
            "../fixtures/unit/broadcast/channels_with_different_names_are_isolated.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/broadcast/close_stops_delivery_and_lets_the_runtime_go_idle.js"
        ))
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
            .text(include_str!(
                "../fixtures/unit/broadcast/\
                 posting_on_a_closed_channel_throws_invalid_state_error.js"
            ),)
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
            .text(include_str!(
                "../fixtures/unit/broadcast/a_message_port_in_the_payload_throws_data_clone_error.\
                 js"
            ),)
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
            .text(include_str!(
                "../fixtures/unit/broadcast/a_channel_reports_the_name_it_was_constructed_with.js"
            ),)
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
        .run(include_str!(
            "../fixtures/unit/broadcast/a_message_that_cannot_be_rebuilt_fires_messageerror.js"
        ))
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
        .run(include_str!(
            "../fixtures/unit/broadcast/a_channel_in_another_thread_receives_the_message.js"
        ))
        .await;

    let sender = thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the sending thread");
        runtime.block_on(async {
            let sender = Fixture::new().await;
            sender
                .run(include_str!(
                    "../fixtures/unit/broadcast/\
                     a_channel_in_another_thread_receives_the_message_2.js"
                ))
                .await;
            sender.settle().await;
        });
    });
    sender.join().expect("the sending thread finishes");

    fixture.settle().await;
    assert_eq!(fixture.text("log.join()").await, "from another thread");
}
