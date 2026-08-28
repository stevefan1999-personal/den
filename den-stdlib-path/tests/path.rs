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
async fn path_module_exports_posix_and_windows_namespaces() -> eyre::Result<()> {
    run("exports.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn posix_normalizes_joins_and_parses_lexical_paths() -> eyre::Result<()> {
    run("posix.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn windows_normalizes_drives_and_unc_shares() -> eyre::Result<()> { run("windows.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn matches_glob_expands_braces_and_extglobs() -> eyre::Result<()> { run("glob.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn path_operations_reject_non_string_arguments() -> eyre::Result<()> {
    run("errors.js").await
}
