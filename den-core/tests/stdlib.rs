//! Smoke tests for the globals `Engine::new` installs, driven the way a script
//! would use them.
//!
//! Deliberately shallow — each stdlib crate owns the depth. What is checked
//! here is that the dependency bumps left every global reachable and working
//! end to end.
#![cfg(feature = "stdlib")]

use color_eyre::eyre;
use den_core::engine::Engine;

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-console")]
async fn console_logging_reaches_the_writer_without_throwing() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let logged: String = engine
        .eval(
            r#"
              console.log("integration", { nested: [1, 2] }, 3);
              console.error("to stderr");
              typeof console.log
            "#,
        )
        .await?;
    assert_eq!(logged, "function");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-core")]
async fn base64_round_trips_through_btoa_and_atob() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let round_tripped: String = engine
        .eval(r#"`${btoa("den runtime")}|${atob(btoa("den runtime"))}`"#)
        .await?;
    assert_eq!(round_tripped, "ZGVuIHJ1bnRpbWU=|den runtime");
    Ok(())
}

/// Multi-byte input is the interesting case: the encoder counts bytes, the
/// decoder has to put the code points back together.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-text")]
async fn text_encoder_and_decoder_round_trip_multibyte_text() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let failures: String = engine
        .eval(
            r#"
              const encoded = new TextEncoder().encode("héllo €");
              const decoded = new TextDecoder().decode(encoded);
              Object.entries({
                encodesToBytes: encoded instanceof Uint8Array,
                countsUtf8Bytes: encoded.length === 10,
                roundTrips: decoded === "héllo €",
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// The timer future is spawned onto the rquickjs scheduler, so this also proves
/// the engine drives pending jobs while an eval is awaiting.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-timer")]
async fn set_timeout_resolves_a_promise_the_eval_is_awaiting() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let resolved: String = engine
        .eval(r#"await new Promise((resolve) => setTimeout(() => resolve("fired"), 1))"#)
        .await?;
    assert_eq!(resolved, "fired");
    Ok(())
}

/// `clearTimeout` has to win the race, so the losing branch is decided by a
/// second timer rather than by wall-clock luck.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-timer")]
async fn clear_timeout_cancels_a_pending_callback() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome: String = engine
        .eval(
            r#"
              let fired = false;
              const pending = setTimeout(() => { fired = true; }, 50);
              clearTimeout(pending);
              await new Promise((resolve) => setTimeout(resolve, 1));
              fired ? "fired anyway" : "cancelled"
            "#,
        )
        .await?;
    assert_eq!(outcome, "cancelled");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-crypto")]
async fn crypto_random_uuid_has_the_version_4_shape() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let failures: String = engine
        .eval(
            r#"
              const uuid = crypto.randomUUID();
              Object.entries({
                isVersion4: /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
                  .test(uuid),
                isFresh: crypto.randomUUID() !== uuid,
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// 64 bytes left at zero by a working generator is a 1-in-2^512 event, so this
/// is deterministic in every sense that matters.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-crypto")]
async fn crypto_get_random_values_fills_the_array_in_place() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let failures: String = engine
        .eval(
            r#"
              const array = new Uint8Array(64);
              const returned = crypto.getRandomValues(array);
              Object.entries({
                returnsTheSameArray: returned === array,
                keepsTheLength: array.length === 64,
                wroteSomething: array.some((byte) => byte !== 0),
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// Blob/File/FormData/FileReader are installed by `den:whatwg` after
/// `den:worker`, so this also proves that evaluate order (EventTarget first).
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-whatwg")]
async fn blob_file_form_data_and_file_reader_are_globals() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let failures: String = engine
        .eval(
            r#"
              const blob = new Blob(["hello ", "world"], { type: "text/plain" });
              const file = new File(["x"], "x.txt", { type: "text/plain", lastModified: 1 });
              const form = new FormData();
              form.append("a", "1");
              const reader = new FileReader();
              const text = await blob.text();
              const read = await new Promise((resolve, reject) => {
                reader.onload = () => resolve(reader.result);
                reader.onerror = () => reject(reader.error);
                reader.readAsText(new Blob(["from-reader"]));
              });
              Object.entries({
                blobSize: blob.size === 11,
                blobText: text === "hello world",
                fileIsBlob: file instanceof Blob,
                fileName: file.name === "x.txt",
                formGet: form.get("a") === "1",
                readerIsTarget: reader instanceof EventTarget,
                readerText: read === "from-reader",
                globals: [Blob, File, FileReader, FormData, XMLHttpRequest, EventSource, URLPattern].every((value) => typeof value === "function"),
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await?;
    assert_eq!(failures, "");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-whatwg-fetch")]
async fn headers_and_request_are_globals_and_constructible() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let failures: String = engine
        .eval(
            r#"
              const headers = new Headers({ "X-A": "b" });
              headers.append("X-A", "c");
              const request = new Request("http://127.0.0.1/post", {
                method: "post",
                headers,
                body: "hi",
              });
              Object.entries({
                headersGet: headers.get("x-a") === "b, c",
                requestMethod: request.method === "POST",
                requestUrl: request.url === "http://127.0.0.1/post",
                fetchIsFn: typeof fetch === "function",
              }).filter(([, held]) => !held).map(([name]) => name).join(",")
            "#,
        )
        .await?;
    assert_eq!(failures, "");
    Ok(())
}
