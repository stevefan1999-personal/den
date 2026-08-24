//! Engine teardown.
//!
//! `den-stdlib-wasm` parks an `OwnedCtx` — a refcounted `Ctx<'static>` — inside
//! the `Store`, and the `Store` inside the userdata of that very context
//! (den-stdlib-wasm/src/backend.rs, src/store.rs). That is a cycle
//! context → userdata → Store → OwnedCtx → context, so if the runtime did not
//! drop its userdata before freeing itself, the `JSContext` refcount would
//! never reach zero and every engine would leak or abort at teardown.
//!
//! These tests churn engines in a loop so that a leak grows and an abort is not
//! a one-in-a-hundred flake. They assert on a value each time round so the
//! engine is genuinely used and not optimised into nothing.

use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

/// Enough repetitions that a per-engine leak is visible in RSS and a teardown
/// abort is certain rather than lucky; still under a second in total.
const ENGINE_CHURN: usize = 25;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/webassembly")
        .join(name)
}

/// Control: `Engine::new` evaluates `den:wasm` (and so installs the store, the
/// engine and the error classes) even when no script touches `WebAssembly`. If
/// this one aborts, registration alone is at fault.
#[tokio::test(flavor = "multi_thread")]
async fn engines_that_never_touch_webassembly_survive_repeated_teardown() -> eyre::Result<()> {
    for round in 0..ENGINE_CHURN {
        let engine = Engine::new().await;
        assert_eq!(
            engine.eval::<usize>(&format!("{round} + 1")).await?,
            round + 1
        );
        drop(engine);
    }
    Ok(())
}

/// The real exercise: instantiate a module, hand out an `ArrayBuffer` aliasing
/// the linear memory, then drop everything — including the `Store` that owns
/// the `OwnedCtx` handle on the context being freed.
#[tokio::test(flavor = "multi_thread")]
async fn engines_that_instantiate_and_touch_memory_survive_repeated_teardown() -> eyre::Result<()> {
    for round in 0..ENGINE_CHURN {
        Engine::new()
            .await
            .run_file::<()>(case("lifetime_memory.js"))
            .await
            .map_err(|error| eyre::eyre!("round {round}: {error}"))?;
    }
    Ok(())
}

/// A JS closure imported into wasm is held by the store's import registry, and
/// the store is held by the context that owns the closure — the second half of
/// the same cycle. Dropping the engine has to break it from the runtime side.
#[tokio::test(flavor = "multi_thread")]
async fn engines_holding_an_imported_js_closure_survive_repeated_teardown() -> eyre::Result<()> {
    for round in 0..ENGINE_CHURN {
        Engine::new()
            .await
            .run_file::<()>(case("lifetime_import.js"))
            .await
            .map_err(|error| eyre::eyre!("round {round}: {error}"))?;
    }
    Ok(())
}
