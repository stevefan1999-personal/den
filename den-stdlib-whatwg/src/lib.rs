//! WHATWG web-platform APIs for den: Blob/File/FileReader/FormData,
//! XMLHttpRequest, EventSource, URLPattern, compression streams and WebSocket.
//!
//! The split follows txiki.js and `den-stdlib-worker`: **Rust owns bytes-in /
//! bytes-out natives**, the JS-visible classes that `extend EventTarget` live
//! in `src/prelude/*.js`. FileReader, XHR, EventSource and WebSocket all need
//! `EventTarget`, so `den:whatwg` is evaluated **after** `den:worker`.

pub mod compression;
pub mod urlpattern;
pub mod websocket;

/// Everything `den:whatwg` exports and installs as a global.
///
/// Headers and Request live in `den-stdlib-whatwg-fetch` — they are fetch's
/// job, and putting them here would duplicate the types fetch already
/// constructs.
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

/// The prelude, in dependency order. Filenames show up in stack traces.
const PRELUDE: [(&str, &str); 10] = [
    ("den:whatwg/streams.js", include_str!("prelude/streams.js")),
    (
        "den:whatwg/platform.js",
        include_str!("prelude/platform.js"),
    ),
    ("den:whatwg/blob.js", include_str!("prelude/blob.js")),
    ("den:whatwg/file.js", include_str!("prelude/file.js")),
    (
        "den:whatwg/form-data.js",
        include_str!("prelude/form-data.js"),
    ),
    (
        "den:whatwg/file-reader.js",
        include_str!("prelude/file-reader.js"),
    ),
    ("den:whatwg/xhr.js", include_str!("prelude/xhr.js")),
    (
        "den:whatwg/eventsource.js",
        include_str!("prelude/eventsource.js"),
    ),
    (
        "den:whatwg/compression.js",
        include_str!("prelude/compression.js"),
    ),
    (
        "den:whatwg/websocket.js",
        include_str!("prelude/websocket.js"),
    ),
];

/// WritableStream is part of the streams polyfill CompressionStream needs, but
/// is not on [`API`]: the WinterTC surface this crate is asked to install does
/// not list it, and a test that iterated API would then demand it as a global.
const STREAM_EXTRAS: [&str; 1] = ["WritableStream"];

#[rquickjs::module]
pub mod whatwg {
    use rquickjs::{
        Ctx, Function, Object, Result, Value,
        class::JsClass,
        context::EvalOptions,
        module::{Declarations, Exports},
    };

    use crate::{
        compression::{Compressor, Decompressor},
        urlpattern::URLPattern,
        websocket::NativeWebSocket,
    };

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        for name in crate::API {
            declare.declare(name)?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let natives = Object::new(ctx.clone())?;
        natives.set(
            "Compressor",
            Compressor::constructor(ctx)?.ok_or_else(|| {
                rquickjs::Exception::throw_internal(ctx, "Compressor constructor missing")
            })?,
        )?;
        natives.set(
            "Decompressor",
            Decompressor::constructor(ctx)?.ok_or_else(|| {
                rquickjs::Exception::throw_internal(ctx, "Decompressor constructor missing")
            })?,
        )?;
        natives.set(
            "NativeWebSocket",
            NativeWebSocket::constructor(ctx)?.ok_or_else(|| {
                rquickjs::Exception::throw_internal(ctx, "NativeWebSocket constructor missing")
            })?,
        )?;
        let mut api = Object::new(ctx.clone())?;
        for (filename, source) in crate::PRELUDE {
            let mut options = EvalOptions::default();
            options.filename = Some(filename.to_owned());
            let factory: Function<'js> = ctx.eval_with_options(source, options)?;
            api = factory.call((natives.clone(), api))?;
        }

        let globals = ctx.globals();
        for name in crate::API {
            let value: Value<'js> = if name == "URLPattern" {
                URLPattern::constructor(ctx)?
                    .ok_or_else(|| {
                        rquickjs::Exception::throw_internal(ctx, "URLPattern constructor missing")
                    })?
                    .into_value()
            } else {
                api.get(name)?
            };
            if !value.is_undefined() {
                globals.set(name, value.clone())?;
            }
            exports.export(name, value)?;
        }
        for name in crate::STREAM_EXTRAS {
            let value: Value<'js> = api.get(name)?;
            if !value.is_undefined() {
                globals.set(name, value)?;
            }
        }
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

    async fn text(source: &str) -> String {
        eval::<String>(source).await
    }

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
                    status: 200,
                    headers: vec![
                        ("Content-Type".into(), "text/plain".into()),
                        ("X-Echo".into(), "yes".into()),
                    ],
                    body: b"hello-xhr".to_vec(),
                    hang: false,
                    silent: false,
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
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    use futures::{SinkExt, StreamExt};
                    while let Some(message) = ws.next().await {
                        let Ok(message) = message else { break };
                        if message.is_close() {
                            break;
                        }
                        if message.is_text() || message.is_binary() {
                            let _ = ws.send(message).await;
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
                    return [data, ws.readyState === WebSocket.CLOSING || ws.readyState === WebSocket.CLOSED].join("|");
                  }})()
                "#
            )),
        )
        .await
        .expect("WebSocket test timed out");
        assert_eq!(report, "ping|true");
    }
}

#[cfg(test)]
mod local_http;
