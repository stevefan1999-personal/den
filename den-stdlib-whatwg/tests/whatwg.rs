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
    engine.run_file(case(name)).await?;
    engine.shutdown().await;
    Ok(())
}

async fn snapshot(name: &str) -> eyre::Result<String> {
    let engine = Engine::new().await;
    engine.run_file(case(name)).await?;
    let report = engine.eval("globalThis.snapshot").await?;
    engine.shutdown().await;
    Ok(report)
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
async fn url_and_search_params_normalize_and_mutate_live() -> eyre::Result<()> {
    insta::assert_snapshot!(snapshot("url.js").await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn compression_round_trips_gzip_deflate_and_raw() -> eyre::Result<()> {
    run("compression.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn readable_stream_yields_enqueued_chunks() -> eyre::Result<()> {
    run("readable_stream.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn piping_teeing_and_iterating_move_every_chunk() -> eyre::Result<()> {
    run("streams_pipe.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn locking_a_response_body_marks_it_used() -> eyre::Result<()> {
    run("response_body_used.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_constructor_and_send_follow_idl() -> eyre::Result<()> {
    run("websocket_idl.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_request_and_fetch_exports_are_constructible() -> eyre::Result<()> {
    run("headers.js").await?;
    run("fetch_globals.js").await?;
    run("request.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_get_post_range_and_retry_against_den_http() -> eyre::Result<()> {
    run("fetch_http.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_aborts_when_the_signal_is_already_aborted() -> eyre::Result<()> {
    run("fetch_abort_already.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_aborts_an_in_flight_request() -> eyre::Result<()> {
    run("fetch_abort_inflight.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn cloning_a_stream_backed_response_tees_it() -> eyre::Result<()> {
    run("response_clone_stream.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cacheable_response_streams_and_still_fills_the_cache() -> eyre::Result<()> {
    run("fetch_stream_body.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_request_body_is_uploaded_without_buffering() -> eyre::Result<()> {
    run("fetch_stream_upload.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn text_stream_decodes_a_buffered_body_without_aborting() -> eyre::Result<()> {
    run("response_text_stream.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn xhr_get_and_post_against_den_http() -> eyre::Result<()> { run("xhr.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn event_source_reads_two_events_from_den_http() -> eyre::Result<()> {
    run("event_source.js").await
}
