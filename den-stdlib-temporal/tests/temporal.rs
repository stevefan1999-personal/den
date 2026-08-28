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
async fn temporal_now_instant_duration_and_plain_date() -> eyre::Result<()> { run("now.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn temporal_constructors_and_compare() -> eyre::Result<()> { run("types.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn temporal_now_fields_and_zoned_date_time() -> eyre::Result<()> {
    run("now_fields.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn temporal_plain_date_add_and_subtract_days() -> eyre::Result<()> { run("add.js").await }
