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
async fn den_worker_exports_and_installs_every_documented_name() -> eyre::Result<()> {
    insta::assert_snapshot!(snapshot("exports.js").await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_realm_global_is_an_event_target() -> eyre::Result<()> { run("event_target.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn dom_exception_comes_from_the_engine_and_is_an_error() -> eyre::Result<()> {
    run("dom_exception.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_propagation_and_stop_immediate() -> eyre::Result<()> {
    run("stop_propagation.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_event_carries_origin_last_event_id_and_source() -> eyre::Result<()> {
    run("message_event.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn abort_controller_and_signal_follow_the_dom_abort_algorithm() -> eyre::Result<()> {
    run("abort.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn performance_now_is_monotonic() -> eyre::Result<()> { run("performance.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn navigator_user_agent_data_reports_den() -> eyre::Result<()> { run("navigator.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn every_platform_class_brands_itself_with_a_to_string_tag() -> eyre::Result<()> {
    run("to_string_tag.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_promise_rejection_event_carries_its_promise_and_reason() -> eyre::Result<()> {
    run("promise_rejection.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn message_and_broadcast_channels_deliver_through_the_installed_api() -> eyre::Result<()> {
    run("channels.js").await
}
