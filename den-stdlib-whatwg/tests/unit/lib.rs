use std::path::PathBuf;

use den_core::engine::Engine;
use futures::{SinkExt as _, StreamExt as _};
use rquickjs::{
    CatchResultExt as _, Class, Context, Module, Promise, Runtime, Value, prelude::This,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::fetch::Response;

#[test]
fn whatwg_installs_its_event_dependency_when_evaluated_alone() {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let install = || {
            let (_, evaluated) =
                Module::evaluate_def::<crate::js_whatwg, _>(ctx.clone(), "den:whatwg")?;
            evaluated.finish::<()>()
        };
        install()
            .catch(&ctx)
            .map_err(|error| error.to_string())
            .expect("den:whatwg evaluates without den:worker");
        assert!(
            ctx.eval::<bool, _>(
                r#"
                const reader = new FileReader();
                let hits = 0;
                reader.addEventListener("load", () => hits++);
                reader.dispatchEvent(new Event("load"));
                reader.onload = () => hits += 2;
                reader.dispatchEvent(new Event("load"));
                reader instanceof EventTarget && hits === 4
                "#,
            )
            .catch(&ctx)
            .map_err(|error| error.to_string())
            .expect("standalone EventTarget works")
        );
    });
}

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) {
    Engine::new()
        .await
        .run_file(case(name))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_echoes_a_text_frame_on_a_local_listener() {
    let port = echo_ws().await;
    // SAFETY: this test is the only writer for this process-local key.
    unsafe {
        std::env::set_var("DEN_TEST_WS_URL", format!("ws://127.0.0.1:{port}/"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), run("websocket_echo.js"))
        .await
        .expect("WebSocket test timed out");
}

async fn echo_ws() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ws bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let Ok(mut ws) = accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Text(_) | Message::Binary(_) => {
                            if ws.send(message).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(frame) => {
                            let _ = ws.send(Message::Close(frame)).await;
                            break;
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    port
}

/// A Rust byte source must be pulled only while the consumer has demand: with
/// a high-water mark of one chunk, exactly one chunk may sit buffered ahead of
/// the reader. This is the property `den:http` and `den:fetch` will rely on.
#[tokio::test(flavor = "multi_thread")]
async fn a_native_source_is_pulled_only_while_the_consumer_has_demand() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rquickjs::Function;

    let engine = Engine::new().await;
    let pulls = Arc::new(AtomicUsize::new(0));
    let installed = Arc::clone(&pulls);
    engine
        .context
        .async_with(async move |ctx| {
            let remaining = std::cell::Cell::new(3_usize);
            let counter = Arc::clone(&installed);
            let stream = crate::streams::ReadableStream::from_native(
                &ctx,
                1.0,
                move |_ctx| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let chunk = (remaining.get() > 0).then(|| {
                        remaining.set(remaining.get() - 1);
                        vec![b'x'; 4]
                    });
                    Box::pin(async move { Ok(chunk) })
                },
                |_| {},
            )
            .expect("a native stream");
            ctx.globals().set("nativeStream", stream).expect("install");
            let reported = Arc::clone(&installed);
            ctx.globals()
                .set(
                    "nativePulls",
                    Function::new(ctx.clone(), move || reported.load(Ordering::SeqCst) as f64)
                        .expect("a counter"),
                )
                .expect("install");
        })
        .await;

    let report: String = engine
        .eval(
            "
            const reader = nativeStream.getReader();
            const first = await reader.read();
            const paced = nativePulls();
            let text = new TextDecoder().decode(first.value);
            for (;;) {
              const { value, done } = await reader.read();
              if (done) break;
              text += new TextDecoder().decode(value);
            }
            `${text}|${paced}|${nativePulls()}`
            ",
        )
        .await
        .expect("the native stream drains");
    engine.shutdown().await;

    let (text, pacing) = report.split_once('|').expect("a report");
    assert_eq!(
        text, "xxxxxxxxxxxx",
        "every native chunk reaches the reader"
    );
    let (after_first, total) = pacing.split_once('|').expect("a pull count");
    assert!(
        after_first.parse::<usize>().expect("a number") <= 2,
        "at most one chunk may be buffered ahead of the reader, saw {after_first} pulls"
    );
    assert_eq!(total, "4", "three chunks and one end-of-stream pull");
}

/// A body handed to script has to be a buffer QuickJS itself allocated.
/// Lending it a Rust allocation registers a free hook that quickjs-ng runs
/// twice on detach (quickjs.c:58037 and :57935), and `transfer` reallocs
/// that foreign pointer, so `(await response.arrayBuffer()).transfer(2)`
/// aborted the process — an abort that takes this test binary with it, so
/// the snippet returning at all is the assertion.
#[tokio::test]
async fn a_response_body_survives_transfer_and_detach() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            // Built from an `http::Response`, so the body is real but no
            // socket is involved. `Response` holds an `Rc`, so it cannot be
            // captured by the `Send` closure and is made here.
            let respond = || {
                Response::from_reqwest(&ctx, http::Response::new("body").into(), "basic")
                    .expect("response")
            };
            let run = async {
                let buffer = Response::array_buffer(
                    This(Class::instance(ctx.clone(), respond())?),
                    ctx.clone(),
                )?
                .into_future::<Value>()
                .await?;
                let view =
                    Response::bytes(This(Class::instance(ctx.clone(), respond())?), ctx.clone())?
                        .into_future::<Value>()
                        .await?;
                ctx.globals().set("body", buffer)?;
                ctx.globals().set("view", view)?;
                ctx.eval::<String, _>(include_str!(
                    "../fixtures/unit/lib/a_response_body_survives_transfer_and_detach.js"
                ))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the snippet evaluates");
    assert_eq!(outcome, "98-111,body,true,0");
}

#[tokio::test]
async fn response_blob_wraps_the_body_when_blob_exists() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let response = Response::from_reqwest(
                    &ctx,
                    http::Response::builder()
                        .header("content-type", "text/plain")
                        .body("hello")
                        .expect("response")
                        .into(),
                    "basic",
                )
                .expect("from_reqwest");
                let blob =
                    Response::blob(This(Class::instance(ctx.clone(), response)?), ctx.clone())?
                        .into_future::<Value>()
                        .await?;
                ctx.globals().set("blob", blob)?;
                ctx.eval::<Promise, _>(include_str!(
                    "../fixtures/unit/lib/response_blob_wraps_the_body_when_blob_exists.js"
                ))?
                .into_future::<String>()
                .await
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(outcome, "true|text/plain|hello");
}
