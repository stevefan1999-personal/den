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
async fn networking_module_exports_socket_classes() -> eyre::Result<()> { run("exports.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn udp_send_to_echoes_on_loopback() -> eyre::Result<()> { run("udp_echo.js").await }
