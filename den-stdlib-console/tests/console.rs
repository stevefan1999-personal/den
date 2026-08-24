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
async fn console_logging_reaches_the_writer_without_throwing() -> eyre::Result<()> {
    run(include_str!("js/console.js")).await
}
