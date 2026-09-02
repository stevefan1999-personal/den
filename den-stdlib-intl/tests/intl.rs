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
async fn den_intl_module_exports_the_installed_namespace() -> eyre::Result<()> {
    run("module.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn locale_canonicalizes_around_its_options() -> eyre::Result<()> { run("locale.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn get_canonical_locales_reads_every_argument_shape() -> eyre::Result<()> {
    run("canonical.js").await
}
