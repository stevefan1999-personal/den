use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js").join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    Engine::new().await.run_file::<()>(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn process_global_exposes_pid_argv_and_env() -> eyre::Result<()> {
    run("process.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn env_get_set_delete_round_trips() -> eyre::Result<()> {
    run("env_roundtrip.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn cwd_round_trips_with_chdir_in_a_temp_dir() -> eyre::Result<()> {
    run("cwd.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn lookup_localhost_returns_loopback() -> eyre::Result<()> {
    run("lookup.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_echo_exits_zero_and_reads_stdout() -> eyre::Result<()> {
    run("spawn.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_and_remove_signal_listener_do_not_throw() -> eyre::Result<()> {
    run("signals.js").await
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn kill_terminates_a_spawned_sleep() -> eyre::Result<()> {
    run("kill.js").await
}
