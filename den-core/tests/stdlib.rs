//! Smoke tests for the globals `Engine::new` installs, driven the way a script
//! would use them.
//!
//! Deliberately shallow — each stdlib crate owns the depth. What is checked
//! here is that the dependency bumps left every global reachable and working
//! end to end. JS cases live next to this file and import `den:assert`.
#![cfg(feature = "stdlib")]

use color_eyre::eyre;
use den_core::engine::Engine;

async fn run(source: &str) -> eyre::Result<()> {
    let engine = Engine::new().await;
    let _: String = engine.eval(&format!("{source}\n\"ok\"")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-console")]
async fn console_logging_reaches_the_writer_without_throwing() -> eyre::Result<()> {
    run(include_str!("stdlib/console.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-core")]
async fn base64_round_trips_through_btoa_and_atob() -> eyre::Result<()> {
    run(include_str!("stdlib/base64.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-text")]
async fn text_encoder_and_decoder_round_trip_multibyte_text() -> eyre::Result<()> {
    run(include_str!("stdlib/text.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-timer")]
async fn set_timeout_resolves_a_promise_the_eval_is_awaiting() -> eyre::Result<()> {
    run(include_str!("stdlib/timeout.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-timer")]
async fn clear_timeout_cancels_a_pending_callback() -> eyre::Result<()> {
    run(include_str!("stdlib/clear_timeout.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-temporal")]
async fn temporal_is_installed_as_a_global() -> eyre::Result<()> {
    run(include_str!("stdlib/temporal.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-crypto")]
async fn crypto_random_uuid_has_the_version_4_shape() -> eyre::Result<()> {
    run(include_str!("stdlib/uuid.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-crypto")]
async fn crypto_get_random_values_fills_the_array_in_place() -> eyre::Result<()> {
    run(include_str!("stdlib/random_values.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(all(feature = "stdlib-crypto", feature = "stdlib-text"))]
async fn crypto_subtle_digest_sha256_of_abc_matches_the_well_known_hex() -> eyre::Result<()> {
    run(include_str!("stdlib/digest.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-fs")]
async fn den_fs_metadata_reports_a_regular_file() -> eyre::Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "den-fs-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("hello.txt");
    std::fs::write(&file, b"abc")?;
    let engine = Engine::new().await;
    let source = format!(
        r#"
          const {{ assertEquals }} = await import("den:assert");
          const {{ metadata }} = await import("den:fs");
          const meta = await metadata({path:?});
          assertEquals(meta.len, 3);
          assertEquals(meta.isFile, true);
          assertEquals(meta.isDir, false);
          assertEquals(meta.isSymlink, false);
          "ok"
        "#,
        path = file
    );
    let _: String = engine.eval(&source).await?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-process")]
async fn process_global_exposes_pid_argv_and_env() -> eyre::Result<()> {
    run(include_str!("stdlib/process.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-whatwg")]
async fn blob_file_form_data_and_file_reader_are_globals() -> eyre::Result<()> {
    run(include_str!("stdlib/blob.js")).await
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "stdlib-whatwg-fetch")]
async fn headers_and_request_are_globals_and_constructible() -> eyre::Result<()> {
    run(include_str!("stdlib/headers.js")).await
}
