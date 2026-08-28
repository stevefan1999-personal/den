//! The transpiler in front of `Engine::eval`.
//!
//! `Engine::eval` transpiles with the widest syntax the build enables
//! (`get_best_transpiling`) and then evaluates the result as *global script
//! code*, not as a module — so what is proved here is that TypeScript and JSX
//! survive that path and reach QuickJS as plain script.
#![cfg(feature = "transpile")]

use color_eyre::eyre;
use den_core::engine::Engine;

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "typescript")]
async fn eval_strips_typescript_type_annotations() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let greeting: String = engine
        .eval(include_str!("fixtures/transpile/typescript.ts"))
        .await?;
    assert_eq!(greeting, "hello den");
    Ok(())
}

/// A TypeScript `enum` is the one construct that has to be *lowered* rather
/// than erased, and its lowering reads pre-computed member values out of the
/// semantic pass — so it is the annotation-stripping test's more demanding
/// sibling.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "typescript")]
async fn eval_lowers_a_typescript_enum_to_a_runtime_object() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let mapped: String = engine
        .eval(include_str!("fixtures/transpile/enum.ts"))
        .await?;
    assert_eq!(mapped, "2|Warn");
    Ok(())
}

/// den's resolver has no `react` module, so the transpiler is pinned to the
/// classic JSX runtime: the output is `React.createElement` calls, which the
/// script itself has to supply. That contract is exactly what this asserts.
#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "react")]
async fn eval_compiles_jsx_to_classic_create_element_calls() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let rendered: String = engine
        .eval(include_str!("fixtures/transpile/jsx.tsx"))
        .await?;
    assert_eq!(rendered, r#"<ul id="list"><li>den</li></ul>"#);
    Ok(())
}

/// A syntax error has to come back as a transpiler error, not as a panic and
/// not as a silently empty program.
#[tokio::test(flavor = "multi_thread")]
async fn eval_reports_a_syntax_error_as_a_transpiler_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.eval::<()>("const = ;").await;
    assert!(
        matches!(
            outcome,
            Err(den_core::engine::EngineError::EasyOxcTranspiler(_))
        ),
        "expected a transpiler error, got {outcome:?}"
    );
    Ok(())
}
