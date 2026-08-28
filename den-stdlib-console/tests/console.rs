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
async fn console_logging_reaches_the_writer_without_throwing() -> eyre::Result<()> {
    run("console.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn console_debug_warn_and_error_are_callable() -> eyre::Result<()> { run("methods.js").await }
