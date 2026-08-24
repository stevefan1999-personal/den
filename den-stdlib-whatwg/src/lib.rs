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
    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Promise};

    /// A realm with text, fetch, worker and whatwg evaluated, in that order:
    /// Blob uses TextEncoder, XHR uses fetch, FileReader extends EventTarget.
    async fn realm() -> (AsyncRuntime, AsyncContext) {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .with(|ctx| {
                let install = || -> rquickjs::Result<()> {
                    Module::evaluate_def::<den_stdlib_text::js_text, _>(ctx.clone(), "den:text")?
                        .1
                        .finish::<()>()?;
                    Module::evaluate_def::<den_stdlib_whatwg_fetch::js_whatwg, _>(
                        ctx.clone(),
                        "den:whatwg-fetch",
                    )?
                    .1
                    .finish::<()>()?;
                    Module::evaluate_def::<den_stdlib_worker::js_worker, _>(
                        ctx.clone(),
                        "den:worker",
                    )?
                    .1
                    .finish::<()>()?;
                    Module::evaluate_def::<crate::js_whatwg, _>(ctx.clone(), "den:whatwg")?
                        .1
                        .finish::<()>()?;
                    Ok(())
                };
                install()
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
                    .expect("den:whatwg evaluates");
            })
            .await;
        (runtime, context)
    }

    async fn eval<T>(source: &str) -> T
    where
        T: for<'js> FromJs<'js> + Send + 'static,
    {
        let (_runtime, context) = realm().await;
        context
            .with(|ctx| {
                ctx.eval::<T, _>(source)
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    async fn text(source: &str) -> String { eval::<String>(source).await }

    async fn text_async(source: &str) -> String {
        let (_runtime, context) = realm().await;
        context
            .async_with(async |ctx| {
                let run = async {
                    let promise: Promise<'_> = ctx.eval(source)?;
                    promise.into_future::<String>().await
                };
                run.await.catch(&ctx).map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    const DOCUMENTED: [&str; 14] = [
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

    #[tokio::test]
    async fn den_whatwg_installs_every_documented_name() {
        assert_eq!(crate::API, DOCUMENTED, "the API list and its tests drifted");
        let report = eval::<Vec<String>>(
            r#"
        "Blob,CloseEvent,CompressionStream,DecompressionStream,EventSource,File,FileReader,FormData,ProgressEvent,ReadableStream,TransformStream,URLPattern,WebSocket,XMLHttpRequest"
          .split(",").map((name) => {
            const value = globalThis[name];
            if (typeof value !== "function") return `${name}: missing`;
            if (value.name !== name) return `${name}: named ${value.name}`;
            return `${name}: ok`;
          })
      "#,
        )
        .await;
        let expected: Vec<String> = DOCUMENTED
            .iter()
            .map(|name| format!("{name}: ok"))
            .collect();
        assert_eq!(report, expected);
    }

    #[tokio::test]
    async fn blob_concatenates_parts_and_slices() {
        assert_eq!(
            text_async(
                r#"
          (async () => {
            const blob = new Blob(["hello ", "world"], { type: "text/plain" });
            const sliced = blob.slice(6);
            const buffer = await blob.arrayBuffer();
            const stream = blob.stream();
            const reader = stream.getReader();
            const first = await reader.read();
            return [
              blob.size,
              blob.type,
              await blob.text(),
              await sliced.text(),
              buffer.byteLength,
              first.done ? "done" : new TextDecoder().decode(first.value).slice(0, 5),
              blob instanceof Blob,
            ].join("|");
          })()
        "#,
            )
            .await,
            "11|text/plain|hello world|world|11|hello|true"
        );
    }

    #[tokio::test]
    async fn file_extends_blob_and_keeps_its_name() {
        assert_eq!(
            text(
                r#"
          (() => {
            const file = new File(["data"], "name.txt", { type: "text/plain", lastModified: 1 });
            return [
              file.name,
              file.type,
              file.lastModified,
              file instanceof Blob,
              file instanceof File,
              file.size,
            ].join("|");
          })()
        "#,
            )
            .await,
            "name.txt|text/plain|1|true|true|4"
        );
    }

    #[tokio::test]
    async fn form_data_appends_gets_and_iterates() {
        assert_eq!(
            text(
                r#"
          (() => {
            const form = new FormData();
            form.append("a", "1");
            form.append("a", "2");
            form.set("b", "3");
            form.append("c", new Blob(["x"], { type: "text/plain" }), "x.txt");
            const file = form.get("c");
            const multipart = form[Symbol.for("den.toMultipartBlob")]();
            return [
              form.get("a"),
              form.getAll("a").join(","),
              form.has("b"),
              [...form.keys()].join(","),
              file instanceof File,
              file.name,
              multipart.type.startsWith("multipart/form-data; boundary="),
              multipart.size > 0,
            ].join("|");
          })()
        "#,
            )
            .await,
            "1|1,2|true|a,a,b,c|true|x.txt|true|true"
        );
    }

    #[tokio::test]
    async fn file_reader_reads_a_blob_as_text() {
        assert_eq!(
            text_async(
                r#"
          (async () => {
            const reader = new FileReader();
            const result = await new Promise((resolve, reject) => {
              reader.onload = () => resolve(reader.result);
              reader.onerror = () => reject(reader.error);
              reader.readAsText(new Blob(["hello"]));
            });
            return [
              result,
              reader.readyState === FileReader.DONE,
              reader instanceof EventTarget,
            ].join("|");
          })()
        "#,
            )
            .await,
            "hello|true|true"
        );
    }

    #[tokio::test]
    async fn xhr_get_and_post_against_a_local_listener() {
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
        let get_url = server.url("/get");
        let post_url = server.url("/post");
        let report = text_async(&format!(
            r#"
              (async () => {{
                const get = await new Promise((resolve, reject) => {{
                  const xhr = new XMLHttpRequest();
                  xhr.open("GET", "{get_url}");
                  xhr.onload = () => resolve(xhr);
                  xhr.onerror = () => reject(new Error("xhr error"));
                  xhr.send();
                }});
                const posted = await new Promise((resolve, reject) => {{
                  const xhr = new XMLHttpRequest();
                  xhr.open("POST", "{post_url}");
                  xhr.setRequestHeader("Content-Type", "text/plain");
                  xhr.onload = () => resolve(xhr);
                  xhr.onerror = () => reject(new Error("xhr error"));
                  xhr.send("ping");
                }});
                let syncThrew = false;
                try {{
                  const xhr = new XMLHttpRequest();
                  xhr.open("GET", "{get_url}", false);
                }} catch (error) {{
                  syncThrew = error instanceof TypeError;
                }}
                return [
                  get.status,
                  get.responseText,
                  get.getResponseHeader("x-echo"),
                  get.readyState === XMLHttpRequest.DONE,
                  posted.responseText,
                  get.responseXML === null,
                  get instanceof EventTarget,
                  syncThrew,
                ].join("|");
              }})()
            "#
        ))
        .await;
        assert_eq!(report, "200|hello-xhr|yes|true|ping|true|true|true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_source_reads_two_events_from_a_local_listener() {
        let server = super::local_http::serve(|_| {
            super::local_http::Outgoing::ok(
                b"event: custom\ndata: a\n\ndata: b\n\n".to_vec(),
                "text/event-stream",
            )
        })
        .await;
        let url = server.url("/");
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            text_async(&format!(
            r#"
              (async () => {{
                try {{
                  let relativeThrew = false;
                  try {{ new EventSource("/relative"); }}
                  catch (error) {{ relativeThrew = error.name === "SyntaxError"; }}
                  const es = new EventSource("{url}");
                  const custom = new Promise((resolve) => es.addEventListener("custom", (e) => resolve(e)));
                  const message = new Promise((resolve) => {{ es.onmessage = (e) => resolve(e); }});
                  await new Promise((resolve, reject) => {{
                    es.onopen = () => resolve();
                    es.onerror = () => reject(new Error("eventsource error " + es.readyState));
                  }});
                  const first = await custom;
                  const second = await message;
                  es.close();
                  return [
                    relativeThrew,
                    es.readyState === EventSource.CLOSED,
                    first.data,
                    first instanceof MessageEvent,
                    second.data,
                    second.origin.startsWith("http://127.0.0.1"),
                  ].join("|");
                }} catch (error) {{
                  return "ERR:" + (error && (error.stack || error.message || String(error)));
                }}
              }})()
            "#
        )))
        .await
        .expect("EventSource test timed out");
        assert_eq!(report, "true|true|a|true|b|true");
    }

    #[tokio::test]
    async fn url_pattern_matches_a_pathname_group() {
        assert_eq!(
            text(
                r#"
                  (() => {
                    const pattern = new URLPattern({ pathname: "/books/:id" });
                    const hit = pattern.test("https://x/books/1");
                    const miss = pattern.test("https://x/authors/1");
                    const exec = pattern.exec("https://x/books/1");
                    return [hit, miss, exec.pathname.groups.id].join("|");
                  })()
                "#,
            )
            .await,
            "true|false|1"
        );
    }

    #[tokio::test]
    async fn compression_stream_round_trips_gzip_deflate_and_raw() {
        assert_eq!(
            text_async(
                r#"
                  (async () => {
                    const encoder = new TextEncoder();
                    const decoder = new TextDecoder();
                    const input = encoder.encode("Hello, WinterTC compression streams.");
                    const roundTrip = async (format) => {
                      const cs = new CompressionStream(format);
                      const writer = cs.writable.getWriter();
                      const reader = cs.readable.getReader();
                      writer.write(input);
                      writer.close();
                      const chunks = [];
                      for (;;) {
                        const { value, done } = await reader.read();
                        if (done) break;
                        chunks.push(value);
                      }
                      const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
                      const compressed = new Uint8Array(total);
                      let offset = 0;
                      for (const chunk of chunks) {
                        compressed.set(chunk, offset);
                        offset += chunk.length;
                      }
                      const ds = new DecompressionStream(format);
                      const dwriter = ds.writable.getWriter();
                      const dreader = ds.readable.getReader();
                      dwriter.write(compressed);
                      dwriter.close();
                      const out = [];
                      for (;;) {
                        const { value, done } = await dreader.read();
                        if (done) break;
                        out.push(value);
                      }
                      const outTotal = out.reduce((sum, chunk) => sum + chunk.length, 0);
                      const plain = new Uint8Array(outTotal);
                      offset = 0;
                      for (const chunk of out) {
                        plain.set(chunk, offset);
                        offset += chunk.length;
                      }
                      return {
                        empty: compressed.length === 0,
                        gzipMagic: format !== "gzip" || (compressed[0] === 0x1f && compressed[1] === 0x8b),
                        text: decoder.decode(plain),
                      };
                    };
                    const gzip = await roundTrip("gzip");
                    const deflate = await roundTrip("deflate");
                    const raw = await roundTrip("deflate-raw");
                    let invalid = false;
                    try { new CompressionStream("nope"); } catch (error) { invalid = error instanceof TypeError; }
                    return [
                      gzip.text,
                      deflate.text,
                      raw.text,
                      gzip.gzipMagic,
                      !gzip.empty,
                      invalid,
                    ].join("|");
                  })()
                "#,
            )
            .await,
            "Hello, WinterTC compression streams.|Hello, WinterTC compression streams.|Hello, WinterTC compression streams.|true|true|true"
        );
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

    #[tokio::test(flavor = "multi_thread")]
    async fn websocket_echoes_a_text_frame_on_a_local_listener() {
        let port = echo_ws().await;
        let url = format!("ws://127.0.0.1:{port}/");
        let report = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            text_async(&format!(
                r#"
                  (async () => {{
                    const ws = new WebSocket("{url}");
                    await new Promise((resolve, reject) => {{
                      ws.onopen = () => resolve();
                      ws.onerror = (event) => reject(new Error(event.message || "ws error"));
                    }});
                    ws.send("ping");
                    const data = await new Promise((resolve) => {{
                      ws.onmessage = (event) => resolve(event.data);
                    }});
                    ws.close();
                    return [
                      data,
                      ws.readyState === WebSocket.CLOSING || ws.readyState === WebSocket.CLOSED,
                      ws instanceof EventTarget,
                      WebSocket.CONNECTING === 0,
                      WebSocket.OPEN === 1,
                    ].join("|");
                  }})()
                "#
            )),
        )
        .await
        .expect("WebSocket test timed out");
        assert_eq!(report, "ping|true|true|true|true");
    }

    #[tokio::test]
    async fn websocket_constructor_and_send_follow_idl() {
        assert_eq!(
            text(
                r#"
                  (() => {
                    const infoOf = (fn) => {
                      try { fn(); return "none"; }
                      catch (error) {
                        return [
                          error instanceof DOMException ? "dom" : "plain",
                          error.name,
                          error.code,
                        ].join(":");
                      }
                    };
                    const ws = new WebSocket("ws://127.0.0.1:1/");
                    ws.onopen = () => {};
                    ws.onerror = () => {};
                    ws.addEventListener("close", () => {});
                    ws.binaryType = "nope";
                    const kept = ws.binaryType;
                    ws.binaryType = "arraybuffer";
                    return [
                      infoOf(() => new WebSocket("not a url")),
                      infoOf(() => new WebSocket("http://example.com/")),
                      infoOf(() => new WebSocket("ws://example.com/#frag")),
                      infoOf(() => new WebSocket("ws://user@example.com/")),
                      infoOf(() => new WebSocket("ws://example.com/", ["a", "a"])),
                      infoOf(() => new WebSocket("ws://example.com/", ["bad protocol"])),
                      infoOf(() => ws.send("early")),
                      infoOf(() => ws.close(1001)),
                      infoOf(() => ws.close(1000, "x".repeat(124))),
                      kept,
                      ws.binaryType,
                      ws.CONNECTING === 0,
                    ].join("|");
                  })()
                "#,
            )
            .await,
            "dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:\
             SyntaxError:12|dom:SyntaxError:12|dom:InvalidStateError:11|dom:InvalidAccessError:\
             15|dom:SyntaxError:12|blob|arraybuffer|true"
        );
    }

    #[tokio::test]
    async fn readable_stream_read_all_bytes_drains_enqueued_chunks() {
        let (_runtime, context) = realm().await;
        let bytes: Vec<u8> = context
            .async_with(async |ctx| {
                let run = async {
                    let stream: rquickjs::Class<crate::streams::ReadableStream> = ctx.eval(
                        r#"
                          new ReadableStream({
                            start(controller) {
                              controller.enqueue(new Uint8Array([1, 2]));
                              controller.enqueue(new Uint8Array([3]));
                              controller.close();
                            }
                          })
                        "#,
                    )?;
                    crate::streams::ReadableStream::read_all_bytes(&stream, ctx.clone()).await
                };
                run.await.catch(&ctx).map_err(|error| error.to_string())
            })
            .await
            .expect("drain");
        assert_eq!(bytes, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn response_body_used_tracks_a_pending_stream_read() {
        assert_eq!(
            text(
                r#"
                  (() => {
                    const stream = new ReadableStream();
                    const response = new Response(stream);
                    const before = response.bodyUsed;
                    const reader = stream.getReader();
                    const afterReader = response.bodyUsed;
                    reader.read();
                    return [before, afterReader, response.bodyUsed].join("|");
                  })()
                "#,
            )
            .await,
            "false|false|true"
        );
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod local_http;
