use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    Engine::new().await.run_file(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn set_timeout_resolves_a_promise_the_eval_is_awaiting() -> eyre::Result<()> {
    run("timeout.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_timeout_cancels_a_pending_callback() -> eyre::Result<()> {
    run("clear_timeout.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_timeout_is_a_function_and_set_timeout_returns_a_number() -> eyre::Result<()> {
    run("types.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn timer_callbacks_are_native_functions_with_forwarded_arguments() -> eyre::Result<()> {
    run("string_timer.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn immediate_callbacks_are_native_and_forward_arguments() -> eyre::Result<()> {
    run("immediate.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn set_interval_repeats_until_cleared() -> eyre::Result<()> { run("interval.js").await }
