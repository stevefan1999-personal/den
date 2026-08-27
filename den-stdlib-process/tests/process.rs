use std::{path::PathBuf, time::Duration};

use color_eyre::eyre;
use den_core::engine::Engine;
use tokio::time::timeout;

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

/// The regression this crate's signal delivery exists for: the forwarder is a
/// `tokio::spawn`ed task, so a realm whose only business is listening for a
/// signal still reaches idle and `den script.js` still exits. A `ctx.spawn`ed
/// pump — what this used to be — never returns from `idle()` at all.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_signal_listener_does_not_keep_the_realm_alive() -> eyre::Result<()> {
    const DEADLINE: Duration = Duration::from_secs(1);

    let engine = Engine::new().await;
    engine.run_file::<()>(case("signal_listener.js")).await?;
    timeout(DEADLINE, engine.runtime.idle()).await?;
    Ok(())
}
