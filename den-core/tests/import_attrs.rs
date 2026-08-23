//! Import attributes (`json` / `text` / `bytes`) driven through the real
//! [`Engine`].
//!
//! rquickjs 0.12 already hands `ImportAttributes` to every loader; what is
//! proved here is that den's file loader turns those types into a default
//! export instead of running the JS/TS transpile path.

use std::{env::temp_dir, fs, path::PathBuf, process};

use color_eyre::eyre;
use den_core::engine::{Engine, EngineError};

const DATA_JSON: &str = include_str!("fixtures/data.json");
const HELLO_TXT: &str = include_str!("fixtures/hello.txt");
const BLOB_BIN: &[u8] = include_bytes!("fixtures/blob.bin");

/// One test's scripts on disk. Relative `import` is resolved against the
/// importer, so the JSON/text/bytes fixtures have to sit next to `main.js`.
struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    fn new(test: &str, files: &[(&str, &[u8])]) -> eyre::Result<Self> {
        let directory = temp_dir()
            .join(format!("den-import-attrs-{}", process::id()))
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

    fn entry(&self) -> PathBuf { self.directory.join("main.js") }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.directory); }
}

#[tokio::test(flavor = "multi_thread")]
async fn json_import_attribute_exports_the_parsed_value() -> eyre::Result<()> {
    let fixture = Fixture::new("json", &[
        ("data.json", DATA_JSON.as_bytes()),
        (
            "main.js",
            br#"
              import data from './data.json' with { type: 'json' };
              globalThis.got = `${data.foo}:${data.n}`;
            "#,
        ),
    ])?;

    let engine = Engine::new().await;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<String>("globalThis.got").await?, "bar:42");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn text_import_attribute_exports_the_file_as_a_string() -> eyre::Result<()> {
    let fixture = Fixture::new("text", &[
        ("hello.txt", HELLO_TXT.as_bytes()),
        (
            "main.js",
            br#"
              import text from './hello.txt' with { type: 'text' };
              globalThis.got = typeof text === 'string' ? text : `not-a-string:${typeof text}`;
            "#,
        ),
    ])?;

    let engine = Engine::new().await;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<String>("globalThis.got").await?, HELLO_TXT);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_import_attribute_exports_a_uint8array() -> eyre::Result<()> {
    let fixture = Fixture::new("bytes", &[
        ("blob.bin", BLOB_BIN),
        (
            "main.js",
            br#"
              import bytes from './blob.bin' with { type: 'bytes' };
              const ok = bytes instanceof Uint8Array
                && bytes.length === 4
                && bytes[0] === 0
                && bytes[1] === 1
                && bytes[2] === 255
                && bytes[3] === 10;
              globalThis.got = ok ? 'ok' : `bad:${bytes}`;
            "#,
        ),
    ])?;

    let engine = Engine::new().await;
    engine.run_file::<()>(fixture.entry()).await?;
    assert_eq!(engine.eval::<String>("globalThis.got").await?, "ok");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_import_attribute_type_is_a_loading_error() -> eyre::Result<()> {
    let fixture = Fixture::new("unknown", &[
        ("data.json", DATA_JSON.as_bytes()),
        (
            "main.js",
            br#"
              import data from './data.json' with { type: 'yaml' };
            "#,
        ),
    ])?;

    let engine = Engine::new().await;
    let outcome = engine.run_file::<()>(fixture.entry()).await;
    assert!(
        matches!(outcome, Err(EngineError::Rquickjs(_))),
        "expected a loading error, got {outcome:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_json_module_is_a_loading_error() -> eyre::Result<()> {
    let fixture = Fixture::new("invalid-json", &[
        ("bad.json", b"{ not json"),
        (
            "main.js",
            br#"
              import data from './bad.json' with { type: 'json' };
            "#,
        ),
    ])?;

    let engine = Engine::new().await;
    let outcome = engine.run_file::<()>(fixture.entry()).await;
    assert!(
        matches!(outcome, Err(EngineError::Rquickjs(_))),
        "expected a loading error, got {outcome:?}"
    );
    Ok(())
}
