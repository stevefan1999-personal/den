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
async fn in_memory_execute_and_query_rows() -> eyre::Result<()> { run("memory.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn closed_connection_rejects_execute_and_close() -> eyre::Result<()> {
    run("closed.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn query_rows_round_trip_sqlite_types() -> eyre::Result<()> { run("types.js").await }
