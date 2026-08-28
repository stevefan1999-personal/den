//! Import maps driven through the real [`Engine`].

use std::path::{Path, PathBuf};

use color_eyre::eyre;
use den_core::engine::{Engine, EngineError};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import_map_engine")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<Engine> {
    let directory = fixture(name);
    let map = std::fs::read_to_string(directory.join("map.json"))?;
    let engine = Engine::new().await;
    engine.set_import_map(&map, &directory).await?;
    engine.run_file(directory.join("main.js")).await?;
    Ok(engine)
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_import_map_match_loads_the_mapped_module() -> eyre::Result<()> {
    let engine = run("exact").await?;
    assert_eq!(engine.eval::<usize>("globalThis.got").await?, 42);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_import_map_match_appends_the_remainder() -> eyre::Result<()> {
    let engine = run("prefix").await?;
    assert_eq!(
        engine.eval::<String>("globalThis.got").await?,
        "from-vendor"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn null_import_map_target_blocks_the_import() -> eyre::Result<()> {
    let engine = run("blocked").await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    assert!(
        got.starts_with("threw:") && got.contains("blocked"),
        "expected a blocked-import error, got {got:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_import_map_overrides_for_matching_parents() -> eyre::Result<()> {
    let engine = run("scopes").await?;
    assert_eq!(engine.eval::<String>("globalThis.got").await?, "top,inner");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unmatched_specifiers_still_resolve_as_files() -> eyre::Result<()> {
    let engine = run("fallthrough").await?;
    assert_eq!(engine.eval::<usize>("globalThis.got").await?, 7);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_import_map_json_is_an_error() -> eyre::Result<()> {
    let map = std::fs::read_to_string(fixture("invalid.json"))?;
    let engine = Engine::new().await;
    let outcome = engine.set_import_map(&map, Path::new("/tmp")).await;
    assert!(
        matches!(outcome, Err(EngineError::ImportMap(_))),
        "expected an import-map parse error, got {outcome:?}"
    );
    Ok(())
}
