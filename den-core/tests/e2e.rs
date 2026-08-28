#![cfg(feature = "stdlib")]

use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

#[tokio::test(flavor = "multi_thread")]
async fn all_stdlib_modules_are_reachable_from_one_engine() -> eyre::Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/stdlib.js");
    Engine::new().await.run_file(path).await?;
    Ok(())
}
