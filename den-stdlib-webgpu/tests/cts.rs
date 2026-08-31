//! Official WebGPU CTS (`vendor/cts/src/webgpu/**/*.spec.ts`).
//!
//! Each official `.spec.ts` file is one cargo/nextest test. Sources are
//! `fs::read` from the submodule. This harness never rewrites `vendor/cts`.
//! TypeScript is transpiled into `target/cts-js/` so relative `.js` imports
//! in the suite resolve without touching the vendor tree.
//!
//! ```text
//! cargo nextest run -p den-stdlib-webgpu --test cts
//! cargo nextest run -p den-stdlib-webgpu --test cts -E 'test(api/operation/buffers)'
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use den_core::EngineBuilder;
use den_transpiler_oxc::{SourceType, transpile_with_source_map};
use libtest_mimic::{Arguments, Failed, Trial};

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir
}

fn cts_root() -> PathBuf { workspace_root().join("vendor/cts") }

fn suite_root() -> PathBuf { cts_root().join("src/webgpu") }

fn js_out_root() -> PathBuf { workspace_root().join("target").join("cts-js") }

fn runner_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js/cts_run.js")
}

fn skip_reason(relative: &str) -> Option<&'static str> {
    if relative.starts_with("web_platform/") {
        return Some("needs-document");
    }
    if relative.starts_with("webworker/") {
        return Some("needs-gpu-in-worker");
    }
    if relative.starts_with("compat/") {
        return Some("needs-compat-mode");
    }
    if relative.contains("idl/exposed") {
        return Some("needs-document");
    }
    if relative.contains("gpu_external_texture") || relative.contains("external_texture") {
        return Some("needs-external-texture");
    }
    if relative.contains("CopyExternalImageToTexture") {
        return Some("needs-image-bitmap");
    }
    None
}

fn collect_spec_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_spec_files(&path));
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
            && path.file_stem().is_some_and(|stem| {
                Path::new(stem)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("spec"))
            })
        {
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

fn file_path_parts(relative: &str) -> Vec<String> {
    let without_ext = relative.strip_suffix(".spec.ts").unwrap_or(relative);
    without_ext
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn transpile_tree(src_root: &Path, out_root: &Path) -> Result<(), String> {
    let stamp = out_root.join(".stamp");
    let head = format!(
        "{}\ncopy-js-json",
        fs::read_to_string(cts_root().join(".git")).unwrap_or_default()
    );
    if stamp.is_file()
        && fs::read_to_string(&stamp).unwrap_or_default() == head
        && out_root.join("webgpu").is_dir()
    {
        return Ok(());
    }
    let _ = fs::remove_dir_all(out_root);
    walk_transpile(src_root, src_root, out_root)?;
    fs::create_dir_all(out_root).map_err(|error| error.to_string())?;
    fs::write(&stamp, head).map_err(|error| error.to_string())?;
    Ok(())
}

fn walk_transpile(src_root: &Path, dir: &Path, out_root: &Path) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let relative = path.strip_prefix(src_root).unwrap_or(&path);
        let dest = out_root.join(relative);
        if path.is_dir() {
            fs::create_dir_all(&dest).map_err(|error| error.to_string())?;
            walk_transpile(src_root, &path, out_root)?;
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_typescript = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
            && Path::new(name).file_stem().is_some_and(|stem| {
                Path::new(stem)
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("d"))
            });
        let copy_as_is = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("js") || ext.eq_ignore_ascii_case("json"));
        if !is_typescript && !copy_as_is {
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if copy_as_is {
            fs::copy(&path, dest).map_err(|error| error.to_string())?;
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let source_type = SourceType::ts().with_module(true);
        let output = transpile_with_source_map(&source, source_type, &path.to_string_lossy())
            .map_err(|error| format!("{}: {error}", relative.display()))?;
        let js_dest = dest.with_extension("js");
        fs::write(js_dest, output.code).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn spec_js_path(relative: &str) -> PathBuf {
    js_out_root()
        .join("webgpu")
        .join(relative.replace(".spec.ts", ".spec.js"))
}

static TRANSPILE: OnceLock<Mutex<Option<Result<(), String>>>> = OnceLock::new();

fn ensure_transpiled() -> Result<(), String> {
    let slot = TRANSPILE.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(result) = guard.as_ref() {
        return result.clone();
    }
    let result = transpile_tree(&cts_root().join("src"), &js_out_root());
    *guard = Some(result.clone());
    result
}

fn run_spec(relative: String) -> Result<(), Failed> {
    ensure_transpiled().map_err(Failed::from)?;
    let spec = spec_js_path(&relative);
    if !spec.is_file() {
        return Err(format!("transpiled spec missing: {}", spec.display()).into());
    }
    let parts = file_path_parts(&relative);
    let parts_json = serde_json::to_string(&parts).map_err(|error| error.to_string())?;
    let cts_js = js_out_root().to_string_lossy().replace('\\', "/");
    let spec_path = spec.to_string_lossy().replace('\\', "/");
    let runner = runner_path();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let engine = EngineBuilder::new()
            .argv(vec![
                "den".into(),
                runner.display().to_string(),
                cts_js,
                spec_path,
                parts_json,
            ])
            .build()
            .await;
        let result = engine.run_file(runner).await;
        engine.shutdown().await;
        result.map_err(|error| error.to_string())
    })?;
    Ok(())
}

fn assert_harness_classification() {
    assert!(
        cts_root().ends_with("vendor/cts"),
        "CTS root is the vendored submodule"
    );
    assert_eq!(
        skip_reason("web_platform/canvas/context_creation.spec.ts"),
        Some("needs-document"),
        "canvas tests need a document"
    );
    assert_eq!(
        skip_reason("api/validation/queue/copyToTexture/CopyExternalImageToTexture.spec.ts"),
        Some("needs-image-bitmap"),
        "copyExternalImageToTexture needs ImageBitmap"
    );
    assert_eq!(
        skip_reason("api/operation/buffers/map.spec.ts"),
        None,
        "buffer mapping is a headless GPU test"
    );
    assert_eq!(
        file_path_parts("api/operation/buffers/map.spec.ts"),
        ["api", "operation", "buffers", "map"],
        "CTS file path parts drop the .spec.ts suffix"
    );
}

fn main() {
    assert_harness_classification();
    let mut tests = Vec::new();
    let suite = suite_root();
    if !suite.is_dir() {
        tests.push(Trial::test("vendor/cts", || {
            Err(
                "vendor/cts is missing src/webgpu; run `git submodule update --init --depth 1 \
                 vendor/cts`"
                    .into(),
            )
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let mut files = collect_spec_files(&suite);
    files.sort();
    if files.is_empty() {
        tests.push(Trial::test("vendor/cts", || {
            Err("walker found no tests under vendor/cts/src/webgpu".into())
        }));
    }
    for file in files {
        let relative = relative_to(&suite, &file);
        let ignored = skip_reason(&relative).is_some();
        tests.push(
            Trial::test(relative.clone(), move || run_spec(relative)).with_ignored_flag(ignored),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
