use color_eyre::eyre;
use den_core::engine::Engine;

async fn run(source: &str) -> eyre::Result<()> {
    let _: String = Engine::new()
        .await
        .eval(&format!("{source}\n\"ok\""))
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn set_timeout_resolves_a_promise_the_eval_is_awaiting() -> eyre::Result<()> {
    run(include_str!("js/timeout.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_timeout_cancels_a_pending_callback() -> eyre::Result<()> {
    run(include_str!("js/clear_timeout.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_timeout_is_a_function_and_set_timeout_returns_a_number() -> eyre::Result<()> {
    run(include_str!("js/types.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn set_timeout_of_a_string_evaluates_it() -> eyre::Result<()> {
    run(include_str!("js/string_timer.js")).await
}
