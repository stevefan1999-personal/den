//! Import maps driven through the real [`Engine`].
//!
//! `Engine::set_import_map` installs the map as context userdata; the first
//! resolver in the chain remaps bare specifiers before builtins and files run.

use std::{env::temp_dir, fs, path::PathBuf, process};

use color_eyre::eyre;
use den_core::engine::{Engine, EngineError};

/// One test's scripts on disk. The map's `base_dir` is this directory, so
/// `./lib.js` in the map is a sibling of `main.js`.
struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new(test: &str, files: &[(&str, &[u8])]) -> eyre::Result<Self> {
        let directory = temp_dir()
            .join(format!("den-import-map-{}", process::id()))
            .join(test);
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory)?;
        for (name, body) in files {
            let path = directory.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, body)?;
        }
        Ok(Self { directory })
    }

    fn entry(&self) -> PathBuf {
        self.directory.join("main.js")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_import_map_match_loads_the_mapped_module() -> eyre::Result<()> {
    let fixture = Fixture::new(
        "exact",
        &[
            ("lib.js", b"export const x = 42;\n"),
            (
                "main.js",
                br#"
                  import { x } from "lib";
                  globalThis.got = x;
                "#,
            ),
        ],
    )?;

    let engine = Engine::new().await;
    engine
        .set_import_map(r#"{"imports":{"lib":"./lib.js"}}"#, &fixture.directory)
        .await?;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<usize>("globalThis.got").await?, 42);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_import_map_match_appends_the_remainder() -> eyre::Result<()> {
    let fixture = Fixture::new(
        "prefix",
        &[
            ("vendor/util.js", b"export const x = 'from-vendor';\n"),
            (
                "main.js",
                br#"
                  import { x } from "lib/util.js";
                  globalThis.got = x;
                "#,
            ),
        ],
    )?;

    let engine = Engine::new().await;
    engine
        .set_import_map(r#"{"imports":{"lib/":"./vendor/"}}"#, &fixture.directory)
        .await?;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(
        engine.eval::<String>("globalThis.got").await?,
        "from-vendor"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn null_import_map_target_blocks_the_import() -> eyre::Result<()> {
    let fixture = Fixture::new(
        "blocked",
        &[(
            "main.js",
            br#"
              globalThis.got = await import("blocked").then(
                () => "ok",
                (error) => `threw:${error}`
              );
            "#,
        )],
    )?;

    let engine = Engine::new().await;
    engine
        .set_import_map(r#"{"imports":{"blocked":null}}"#, &fixture.directory)
        .await?;
    engine.run_file::<()>(fixture.entry()).await?;
    let got = engine.eval::<String>("globalThis.got").await?;
    assert!(
        got.starts_with("threw:") && got.contains("blocked"),
        "expected a blocked-import error, got {got:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_import_map_overrides_for_matching_parents() -> eyre::Result<()> {
    let fixture = Fixture::new(
        "scopes",
        &[
            ("top.js", b"export const x = 'top';\n"),
            ("nested/inner.js", b"export const x = 'inner';\n"),
            (
                "nested/mod.js",
                b"import { x } from 'pkg';\nexport { x };\n",
            ),
            (
                "main.js",
                br#"
                  import { x as top } from "pkg";
                  import { x as inner } from "./nested/mod.js";
                  globalThis.got = `${top},${inner}`;
                "#,
            ),
        ],
    )?;

    let engine = Engine::new().await;
    engine
        .set_import_map(
            r#"{
                "imports": {"pkg": "./top.js"},
                "scopes": {"./nested/": {"pkg": "./nested/inner.js"}}
            }"#,
            &fixture.directory,
        )
        .await?;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<String>("globalThis.got").await?, "top,inner");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unmatched_specifiers_still_resolve_as_files() -> eyre::Result<()> {
    let fixture = Fixture::new(
        "fallthrough",
        &[
            ("other.js", b"export const x = 7;\n"),
            (
                "main.js",
                br#"
                  import { x } from "./other.js";
                  globalThis.got = x;
                "#,
            ),
        ],
    )?;

    let engine = Engine::new().await;
    engine
        .set_import_map(r#"{"imports":{"lib":"./lib.js"}}"#, &fixture.directory)
        .await?;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<usize>("globalThis.got").await?, 7);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_import_map_json_is_an_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.set_import_map("not json", "/tmp").await;
    assert!(
        matches!(outcome, Err(EngineError::ImportMap(_))),
        "expected an import-map parse error, got {outcome:?}"
    );
    Ok(())
}
