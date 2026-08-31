use color_eyre::eyre;
use den_capabilities::{Capability, Decision, Policy};

use super::{DEFAULT_GC_THRESHOLD, DEFAULT_MAX_STACK_SIZE, EngineBuilder};
use crate::engine::EngineError;

#[test]
fn builder_defaults_are_bounded_and_deny_by_default() {
    let builder = EngineBuilder::new();
    assert_eq!(
        builder.settings.max_stack_size, DEFAULT_MAX_STACK_SIZE,
        "the default stack size must be explicit"
    );
    assert_ne!(
        builder.settings.max_stack_size, 0,
        "the default stack must not be unlimited"
    );
    assert_eq!(
        builder.settings.gc_threshold, DEFAULT_GC_THRESHOLD,
        "the QuickJS GC default must remain explicit"
    );
    assert_eq!(
        builder.settings.heap_limit, None,
        "hosts opt in to a heap ceiling"
    );
    assert!(
        builder.settings.import_map.is_none(),
        "hosts opt in to an import map"
    );
    assert_eq!(
        builder.settings.policy.query_all(Capability::Read),
        Decision::Denied,
        "default authority must deny access"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn builder_installs_the_import_map_before_evaluation() -> eyre::Result<()> {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import_map_engine");
    let engine = EngineBuilder::new()
        .import_map(
            r#"{ "imports": { "answer": "./exact/lib.js" } }"#,
            &directory,
        )?
        .build()
        .await;
    let answer = engine
        .eval::<i32>("await import('answer').then(module => module.x)")
        .await?;
    eyre::ensure!(answer == 42);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn configured_stack_limit_stops_recursion() -> eyre::Result<()> {
    let engine = EngineBuilder::new().max_stack_size(64 * 1024).build().await;
    let outcome = engine
        .eval::<()>("(function recurse() { recurse(); })();")
        .await;
    let Err(EngineError::JavaScript(error)) = outcome else {
        return Err(eyre::eyre!("expected stack overflow, got {outcome:?}"));
    };
    eyre::ensure!(
        error.name() == Some("RangeError") && error.message().contains("stack"),
        "expected stack overflow, got {error}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn builder_stores_the_policy_as_context_userdata() -> eyre::Result<()> {
    let policy = Policy::allow_all([Capability::Read]);
    let engine = EngineBuilder::new().policy(policy.clone()).build().await;
    let stored = engine
        .context
        .with(|ctx| {
            ctx.userdata::<Policy>()
                .map(|stored| Policy::clone(&stored))
        })
        .await;
    eyre::ensure!(stored.as_ref() == Some(&policy), "policy was not stored");
    eyre::ensure!(engine.policy().await == policy, "policy query disagrees");
    Ok(())
}

#[cfg(feature = "stdlib-process")]
#[tokio::test(flavor = "multi_thread")]
async fn builder_installs_host_owned_process_arguments() -> eyre::Result<()> {
    let engine = EngineBuilder::new()
        .argv(vec!["den".into(), "main.ts".into(), "--port".into()])
        .build()
        .await;
    let argv = engine
        .eval::<String>("JSON.stringify(process.argv)")
        .await?;
    eyre::ensure!(
        argv == r#"["den","main.ts","--port"]"#,
        "unexpected argv: {argv}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn heap_limit_stops_javascript_allocation() -> eyre::Result<()> {
    let engine = EngineBuilder::new()
        .heap_limit(16 * 1024 * 1024)
        .build()
        .await;
    let outcome = engine
        .eval::<()>("globalThis.large = new ArrayBuffer(32 * 1024 * 1024);")
        .await;
    eyre::ensure!(
        outcome.is_err(),
        "allocation above the heap limit succeeded"
    );
    Ok(())
}
