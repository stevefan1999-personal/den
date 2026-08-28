use std::path::PathBuf;

use den_core::engine::Engine;
use rquickjs::{CatchResultExt, Context, Module, Runtime};

static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
async fn xhr_get_and_post_against_a_local_listener() {
    let _guard = ENV.lock().await;
    let server = super::local_http::serve(|incoming| {
        if incoming.path.ends_with("/post") {
            super::local_http::Outgoing::ok(incoming.body, "text/plain")
        } else {
            super::local_http::Outgoing {
                status:  200,
                headers: vec![
                    ("Content-Type".into(), "text/plain".into()),
                    ("X-Echo".into(), "yes".into()),
                ],
                body:    b"hello-xhr".to_vec(),
                hang:    false,
                silent:  false,
            }
        }
    })
    .await;
    // SAFETY: test-only keys, held under `ENV` so cargo-test threads cannot race.
    unsafe {
        std::env::set_var("DEN_TEST_GET_URL", server.url("/get"));
        std::env::set_var("DEN_TEST_POST_URL", server.url("/post"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), run("xhr.js"))
        .await
        .expect("XMLHttpRequest test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn event_source_reads_two_events_from_a_local_listener() {
    let _guard = ENV.lock().await;
    let server = super::local_http::serve(|_| {
        super::local_http::Outgoing::ok(
            b"event: custom\ndata: a\n\ndata: b\n\n".to_vec(),
            "text/event-stream",
        )
    })
    .await;
    // SAFETY: test-only key, held under `ENV` so cargo-test threads cannot race.
    unsafe {
        std::env::set_var("DEN_TEST_URL", server.url("/"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), run("event_source.js"))
        .await
        .expect("EventSource test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_echoes_a_text_frame_on_a_local_listener() {
    let _guard = ENV.lock().await;
    let port = echo_ws().await;
    // SAFETY: test-only key, held under `ENV` so cargo-test threads cannot race.
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
                let Ok(ws) =
                    den_stdlib_networking::websocket::NativeWebSocket::accept(stream).await
                else {
                    return;
                };
                while let Some(event) = ws.next_event().await {
                    match event {
                        den_stdlib_networking::websocket::NativeWsEvent::Text(text) => {
                            let _ = ws.send_text(text);
                        }
                        den_stdlib_networking::websocket::NativeWsEvent::Binary(bytes) => {
                            let _ = ws.send_binary(bytes);
                        }
                        den_stdlib_networking::websocket::NativeWsEvent::Close { .. } => {
                            break;
                        }
                        den_stdlib_networking::websocket::NativeWsEvent::Open { .. }
                        | den_stdlib_networking::websocket::NativeWsEvent::Error(_) => {}
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

    let _guard = ENV.lock().await;
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
            r#"
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
            "#,
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
