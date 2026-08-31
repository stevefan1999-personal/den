//! Import attributes (`json` / `text` / `bytes`) driven through the real
//! [`Engine`].

use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::{Engine, EngineError};

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import_attrs")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<Engine> {
    let engine = Engine::new().await;
    engine.run_file(case(name)).await?;
    Ok(engine)
}

#[tokio::test(flavor = "multi_thread")]
async fn json_import_attribute_exports_the_parsed_value() -> eyre::Result<()> {
    let engine = run("json.js").await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    eyre::ensure!(got == "bar:42", "expected \"bar:42\", got {got:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn text_import_attribute_exports_the_file_as_a_string() -> eyre::Result<()> {
    let engine = run("text.js").await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    eyre::ensure!(
        got == include_str!("fixtures/hello.txt"),
        "text import changed contents: {got:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_import_attribute_exports_a_uint8array() -> eyre::Result<()> {
    let engine = run("bytes.js").await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    eyre::ensure!(got == "0,1,255,10", "unexpected byte import: {got:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn json_import_strips_a_bom_and_accepts_a_top_level_primitive() -> eyre::Result<()> {
    let engine = run("json_bom_primitive.js").await?;
    let got = engine.eval::<usize>("globalThis.got").await?;
    eyre::ensure!(got == 42, "expected 42, got {got}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn text_import_preserves_characters_that_need_js_string_escaping() -> eyre::Result<()> {
    let engine = run("text_escaping.js").await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    eyre::ensure!(
        got == include_str!("fixtures/import_attrs/escaped.txt"),
        "escaped text import changed contents: {got:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_import_attribute_type_is_a_loading_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.run_file(case("unknown.js")).await;
    eyre::ensure!(
        matches!(&outcome, Err(EngineError::JavaScript(_))),
        "expected a loading error, got {outcome:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_json_module_is_a_loading_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.run_file(case("invalid_json.js")).await;
    eyre::ensure!(
        matches!(&outcome, Err(EngineError::JavaScript(_))),
        "expected a JSON loading error, got {outcome:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_utf8_text_module_is_a_loading_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.run_file(case("invalid_utf8.js")).await;
    eyre::ensure!(
        matches!(&outcome, Err(EngineError::JavaScript(_))),
        "expected a UTF-8 loading error, got {outcome:?}"
    );
    Ok(())
}
