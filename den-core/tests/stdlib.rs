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

/// FIPS 180-4 SHA-256 of `"abc"`. `TextEncoder` is the usual way a script
/// produces the `BufferSource` `subtle.digest` hashes.
#[tokio::test(flavor = "multi_thread")]
#[cfg(all(feature = "stdlib-crypto", feature = "stdlib-text"))]
async fn crypto_subtle_digest_sha256_of_abc_matches_the_well_known_hex() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let hex: String = engine
        .eval(
            r#"
              const digest = await crypto.subtle.digest(
                "SHA-256",
                new TextEncoder().encode("abc"),
              );
              [...new Uint8Array(digest)]
                .map((byte) => byte.toString(16).padStart(2, "0"))
                .join("")
            "#,
        )
        .await?;
    assert_eq!(
        hex,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    Ok(())
}
