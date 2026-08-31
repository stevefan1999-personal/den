use std::path::{Path, PathBuf};

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(directory: &Path, name: &str, store: &Path) -> eyre::Result<PathBuf> {
    let source = match name {
        "crud" => include_str!("js/crud.js"),
        "transactions" => include_str!("js/transactions.js"),
        "batch_limit" => include_str!("js/batch_limit.js"),
        "shutdown_write" => include_str!("js/shutdown_write.js"),
        "shutdown_read" => include_str!("js/shutdown_read.js"),
        "worker_main" => include_str!("js/worker_main.js"),
        "worker_child" => include_str!("js/worker_child.js"),
        _ => return Err(eyre::eyre!("unknown den:kv test case {name}")),
    };
    let path = directory.join(format!("{name}.js"));
    let store = serde_json::to_string(&store.to_string_lossy())?;
    std::fs::write(&path, source.replace("__STORE__", &store))?;
    Ok(path)
}

async fn run(path: PathBuf) -> eyre::Result<()> {
    let engine = Engine::new().await;
    let result = engine.run_file(path).await;
    if result.is_err() {
        engine
            .context
            .with(|ctx| {
                let caught = ctx.catch();
                let message: Option<String> = caught
                    .as_object()
                    .and_then(|object| object.get("message").ok());
                eprintln!(
                    "den:kv fixture: {}",
                    message.unwrap_or_else(|| format!("{caught:?}"))
                );
            })
            .await;
    }
    engine.shutdown().await;
    result?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn durable_crud_and_boundaries() -> eyre::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = directory.path().join("crud.surrealkv");
    run(case(directory.path(), "crud", &store)?).await
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_transactions_and_conflicts() -> eyre::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = directory.path().join("transactions.surrealkv");
    run(case(directory.path(), "transactions", &store)?).await
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_transaction_is_rejected_before_commit_and_store_reopens() -> eyre::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = directory.path().join("batch-limit.surrealkv");
    run(case(directory.path(), "batch_limit", &store)?).await
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_shutdown_closes_and_reopen_recovers_data() -> eyre::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = directory.path().join("shutdown.surrealkv");
    run(case(directory.path(), "shutdown_write", &store)?).await?;
    run(case(directory.path(), "shutdown_read", &store)?).await
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_shutdown_closes_kv_before_join_returns() -> eyre::Result<()> {
    let directory = tempfile::tempdir()?;
    let store = directory.path().join("worker.surrealkv");
    let _ = case(directory.path(), "worker_child", &store)?;
    run(case(directory.path(), "worker_main", &store)?).await?;
    run(case(directory.path(), "shutdown_read", &store)?).await
}
