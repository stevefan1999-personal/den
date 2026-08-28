//! Official WebAssembly core spec suite (`vendor/spec/test/core/**/*.wast`).
//!
//! Each official `.wast` file is one cargo/nextest test. The file is the
//! authentic unit: a script shares one store across `module` / `register` /
//! `invoke` / `assert_*`. Directives are not split into Rust tests.
//!
//! Sources are `fs::read` from `vendor/spec`. This harness never rewrites them.
//!
//! # How a file runs
//!
//! Parsed with the `wast` crate (same wasm-tools major as `wat 1.257.1`).
//! Directives execute on **wasmtime 48** from
//! [`den_stdlib_wasm::backend::new_engine`] — the same proposal set as
//! `WebAssembly.Module` / `validate`. Compiled modules then go through
//! `WebAssembly.validate` in a den JS realm.
//!
//! # Skip policy
//!
//! Registered, then `#[ignore]` (nextest reports skipped, not hidden):
//!
//! - path components `threads`, `shared-everything`, `custom-page-sizes`,
//!   `wide-arithmetic` — proposals den leaves off in `new_engine`
//! - `skip-stack-guard-page.wast` — needs a host-sized stack, not a proposal
//! - `simd/meta/` — Python generators, not tests
//!
//! `test/js-api` is testharness-style and is the WPT copy of
//! `vendor/wpt/wasm/jsapi`. This crate does not walk it.
//!
//! ```text
//! cargo nextest run -p den-stdlib-wasm --test spec_core
//! cargo nextest run -p den-stdlib-wasm --test spec_core -E 'test(i32)'
//! ```

#[path = "spec_core/runner.rs"] mod runner;

use std::{
    fs,
    path::{Path, PathBuf},
};

use den_core::engine::Engine;
use libtest_mimic::{Arguments, Failed, Trial};

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir
}

fn spec_root() -> PathBuf { workspace_root().join("vendor/spec") }

fn skip_reason(relative: &str) -> Option<&'static str> {
    let path = relative.replace('\\', "/");
    if path.contains("simd/meta/") {
        return Some("generator, not a test");
    }
    if path.ends_with("skip-stack-guard-page.wast") {
        return Some("host stack guard; not a proposal den configures");
    }
    path.split('/').find_map(|part| {
        matches!(
            part,
            "threads" | "shared-everything" | "custom-page-sizes" | "wide-arithmetic"
        )
        .then_some("proposal den does not enable")
    })
}

fn collect_wast_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_wast_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("wast") {
            files.push(path);
        }
    }
    files
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

fn run_wast_file(wast_path: &Path, relative: &str) -> Result<(), Failed> {
    let source =
        fs::read_to_string(wast_path).map_err(|error| format!("{relative}: read: {error}"))?;
    let outcome = runner::run_wast(wast_path, &source)?;
    if outcome.compiled.is_empty() {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let engine = Engine::new().await;
        for bytes in &outcome.compiled {
            let literal = bytes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            engine
                .eval::<()>(&format!(
                    "if (!WebAssembly.validate(new Uint8Array([{literal}]))) throw new \
                     Error('WebAssembly.validate returned false');"
                ))
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok::<_, String>(())
    })?;
    Ok(())
}

fn harness_classify() -> Result<(), Failed> {
    assert!(
        spec_root().ends_with("vendor/spec"),
        "spec root is the vendored submodule"
    );
    assert_eq!(
        relative_to(Path::new("/core"), Path::new("/core/simd/a.wast")),
        "simd/a.wast",
        "trial names are official paths under test/core"
    );
    assert_eq!(
        skip_reason("threads/atomic.wast"),
        Some("proposal den does not enable"),
        "threads is a proposal skip"
    );
    assert_eq!(
        skip_reason("skip-stack-guard-page.wast"),
        Some("host stack guard; not a proposal den configures"),
        "stack-guard file is not a proposal"
    );
    assert_eq!(
        skip_reason("simd/meta/gen.wast"),
        Some("generator, not a test"),
        "simd/meta is not executed"
    );
    assert_eq!(
        skip_reason("i32.wast"),
        None,
        "core i32.wast is an official executable script"
    );
    assert!(
        collect_wast_files(Path::new("/den-spec-core-no-such-dir")).is_empty(),
        "missing tree walks to empty"
    );
    let missing = run_wast_file(Path::new("/den-spec-core-no-such.wast"), "missing.wast");
    assert!(missing.is_err(), "missing official file is a fail");
    Ok(())
}

fn main() {
    let mut tests = vec![Trial::test("harness::classify", harness_classify)];
    let core = spec_root().join("test/core");
    if !core.is_dir() {
        tests.push(Trial::test("vendor/spec", || {
            Err(
                "vendor/spec is missing test/core; run `git submodule update --init --depth 1 \
                 vendor/spec`"
                    .into(),
            )
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let mut files = collect_wast_files(&core);
    files.sort();
    if files.is_empty() {
        tests.push(Trial::test("vendor/spec", || {
            Err("walker found no tests under vendor/spec/test/core".into())
        }));
    }
    for wast_path in files {
        let relative = relative_to(&core, &wast_path);
        let ignored = skip_reason(&relative).is_some();
        tests.push(
            Trial::test(relative.clone(), move || {
                run_wast_file(&wast_path, &relative)
            })
            .with_ignored_flag(ignored),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
