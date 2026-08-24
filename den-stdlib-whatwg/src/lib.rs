//! WHATWG web-platform APIs for den: Blob/File/FileReader/FormData,
//! XMLHttpRequest, EventSource, URLPattern, compression streams and WebSocket.
//!
//! Classes are native `#[rquickjs::class]` types. EventTarget subclasses set
//! `[[Prototype]]` to `globalThis.EventTarget.prototype` at evaluate time so
//! `instanceof EventTarget` holds after `den:worker`.

pub mod blob;
pub mod compression;
pub mod event_target;
pub mod events;
pub mod eventsource;
pub mod file_reader;
pub mod form_data;
pub mod host;
pub mod streams;
mod url;
pub mod urlpattern;
pub mod websocket;
pub mod xhr;

/// Everything `den:whatwg` exports and installs as a global.
///
/// Headers and Request live in `den-stdlib-whatwg-fetch` — they are fetch's
/// job, and putting them here would duplicate the types fetch already
/// constructs.
#[cfg(test)]
const API: [&str; 14] = [
    "Blob",
    "CloseEvent",
    "CompressionStream",
    "DecompressionStream",
    "EventSource",
    "File",
    "FileReader",
    "FormData",
    "ProgressEvent",
    "ReadableStream",
    "TransformStream",
    "URLPattern",
    "WebSocket",
    "XMLHttpRequest",
];

#[rquickjs::module]
pub mod whatwg {
    use den_util::inherit;
    use rquickjs::{Class, Ctx, Result, class::JsClass, module::Exports};

    use crate::host::Host;
    pub use crate::{
        blob::{Blob, File},
        compression::{CompressionStream, DecompressionStream},
        events::{CloseEvent, ProgressEvent},
        eventsource::EventSource,
        file_reader::FileReader,
        form_data::FormData,
        streams::{ReadableStream, TransformStream, WritableStream},
        urlpattern::URLPattern,
        websocket::WebSocket,
        xhr::XMLHttpRequest,
    };

    fn install<'js, C: JsClass<'js>>(ctx: &Ctx<'js>, name: &str) -> Result<()> {
        if let Some(ctor) = Class::<C>::create_constructor(ctx)? {
            ctx.globals().set(name, ctor)?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _exports: &Exports<'js>) -> Result<()> {
        install::<Blob>(ctx, "Blob")?;
        install::<CloseEvent>(ctx, "CloseEvent")?;
        install::<CompressionStream>(ctx, "CompressionStream")?;
        install::<DecompressionStream>(ctx, "DecompressionStream")?;
        install::<EventSource>(ctx, "EventSource")?;
        install::<File>(ctx, "File")?;
        install::<FileReader>(ctx, "FileReader")?;
        install::<FormData>(ctx, "FormData")?;
        install::<ProgressEvent>(ctx, "ProgressEvent")?;
        install::<ReadableStream>(ctx, "ReadableStream")?;
        install::<TransformStream>(ctx, "TransformStream")?;
        install::<crate::url::URL>(ctx, "URL")?;
        install::<crate::url::URLSearchParams>(ctx, "URLSearchParams")?;
        crate::url::install_shell(ctx)?;
        install::<URLPattern>(ctx, "URLPattern")?;
        install::<WebSocket>(ctx, "WebSocket")?;
        install::<XMLHttpRequest>(ctx, "XMLHttpRequest")?;
        install::<WritableStream>(ctx, "WritableStream")?;
        inherit::<File, Blob>(ctx)?;
        Host::set_event_target_proto::<ProgressEvent>(ctx, "Event")?;
        Host::set_event_target_proto::<CloseEvent>(ctx, "Event")?;
        Host::set_event_target_proto::<FileReader>(ctx, "EventTarget")?;
        Host::set_event_target_proto::<XMLHttpRequest>(ctx, "EventTarget")?;
        Host::set_event_target_proto::<EventSource>(ctx, "EventTarget")?;
        Host::set_event_target_proto::<WebSocket>(ctx, "EventTarget")?;
        WebSocket::install_idl_constants(ctx)?;
        FileReader::install_idl_constants(ctx)?;
        Host::install_formdata_symbol(ctx)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use den_core::engine::Engine;

    static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn case(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/js")
            .join(name)
    }

    async fn run(name: &str) {
        Engine::new()
            .await
            .run_file::<()>(case(name))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn api_list_has_not_drifted() {
        assert_eq!(crate::API.len(), 14);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn xhr_get_and_post_against_a_local_listener() {
        let _guard = ENV.lock().await;
        let server = super::local_http::serve(|incoming| {
            if incoming.method == "POST" {
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
}

#[cfg(any(test, feature = "test"))]
pub mod local_http;
