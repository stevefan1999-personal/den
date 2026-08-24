use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file::<()>(case(name)).await?;
    engine.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn blob_file_form_data_and_file_reader_are_globals() -> eyre::Result<()> {
    run("blob.js").await?;
    run("globals.js").await?;
    run("file.js").await?;
    run("form_data.js").await?;
    run("file_reader.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn url_pattern_matches_a_pathname_group() -> eyre::Result<()> { run("url_pattern.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn compression_round_trips_gzip_deflate_and_raw() -> eyre::Result<()> {
    run("compression.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn readable_stream_yields_enqueued_chunks() -> eyre::Result<()> {
    run("readable_stream.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn locking_a_response_body_marks_it_used() -> eyre::Result<()> {
    run("response_body_used.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_constructor_and_send_follow_idl() -> eyre::Result<()> {
    run("websocket_idl.js").await
}
