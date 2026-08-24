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
async fn headers_and_request_are_globals_and_constructible() -> eyre::Result<()> {
    run(include_str!("js/headers.js")).await
}
