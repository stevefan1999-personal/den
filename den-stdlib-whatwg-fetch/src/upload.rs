//! Streaming a request body into reqwest.
//!
//! `duplex: "half"` promises the body is sent as it is produced. The body is a
//! `ReadableStream`, so the transport end is a `WritableStream` over a bounded
//! channel: the sink's write settles only once reqwest has taken the chunk, and
//! the stream core turns that into real backpressure on the producer. Nothing
//! here holds a JS value, so the closures the sink hands to QuickJS stay
//! traceable.

use std::{
    cell::RefCell,
    io,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
};

use den_stdlib_whatwg::streams::{ReadableStream, StreamError, WritableStream, mark_handled};
use rquickjs::{
    Class, Ctx, Result,
    function::{Opt, This},
};
use tokio::sync::mpsc;

/// Chunks the transport may hold. One is enough to keep the socket busy while
/// still stopping the producer a chunk after the network does.
const IN_FLIGHT_CHUNKS: usize = 1;

const GONE: &str = "the request body is no longer being sent";

type Chunks = mpsc::Sender<io::Result<Vec<u8>>>;

/// Pipe `source` into a fresh `reqwest::Body`. The pipe runs on the JS event
/// loop and outlives this call; the returned body ends when the stream closes.
pub(crate) fn stream_request_body<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>,
) -> Result<reqwest::Body> {
    let (sender, mut receiver) = mpsc::channel::<io::Result<Vec<u8>>>(IN_FLIGHT_CHUNKS);
    // Shared so `close` and `abort` can drop the last sender, which is what
    // ends the request body; `write` only borrows a clone for the send.
    let open: Rc<RefCell<Option<Chunks>>> = Rc::new(RefCell::new(Some(sender)));
    let aborted = Arc::new(AtomicBool::new(false));

    let sink = WritableStream::to_native(
        ctx,
        IN_FLIGHT_CHUNKS as f64,
        {
            let open = Rc::clone(&open);
            move |_ctx, bytes| {
                let sender = open.borrow().clone();
                Box::pin(async move {
                    let Some(sender) = sender else {
                        return Err(StreamError::Message(GONE.to_owned()));
                    };
                    if sender.send(Ok(bytes)).await.is_err() {
                        return Err(StreamError::Message(GONE.to_owned()));
                    }
                    Ok(())
                })
            }
        },
        {
            let open = Rc::clone(&open);
            move |_ctx| {
                open.borrow_mut().take();
                Box::pin(async { Ok(()) })
            }
        },
        {
            let aborted = Arc::clone(&aborted);
            move |_reason| {
                // Order matters: the flag has to be visible before the channel
                // wakes the transport, or the body ends cleanly and the server
                // sees a truncated upload as a complete one.
                aborted.store(true, Ordering::SeqCst);
                open.borrow_mut().take();
            }
        },
    )?;

    let pipe = ReadableStream::pipe_to(
        This(source.clone()),
        ctx.clone(),
        Opt(Some(sink.into_value())),
        Opt(None),
    )?;
    // A failed pipe already reaches the transport through `abort`; keep its
    // promise from tripping the unhandled-rejection tracker. Reading `then` off
    // the promise here would let a script that patched `Promise.prototype`
    // break the upload, so this goes through the core's pristine `then`.
    mark_handled(&ctx, &pipe);

    Ok(reqwest::Body::wrap_stream(futures::stream::poll_fn(
        move |cx| {
            if aborted.swap(false, Ordering::SeqCst) {
                return Poll::Ready(Some(Err(io::Error::other(
                    "the request body was aborted before it was sent",
                ))));
            }
            receiver.poll_recv(cx)
        },
    )))
}
