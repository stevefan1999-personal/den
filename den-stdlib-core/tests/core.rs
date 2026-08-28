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
async fn base64_round_trips_through_btoa_and_atob() -> eyre::Result<()> { run("base64.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn atob_rejects_invalid_base64_and_decodes_padding() -> eyre::Result<()> {
    run("atob_invalid.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn btoa_coerces_non_strings_the_same_as_string_input() -> eyre::Result<()> {
    run("btoa_coercion.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn gc_is_a_callable_global() -> eyre::Result<()> { run("gc.js").await }
