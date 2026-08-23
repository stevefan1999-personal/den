//! Official test262 Temporal suite
//! (`vendor/test262/test/built-ins/Temporal/**`).
//!
//! Each official `.js` file is one cargo/nextest test. The file is read raw
//! from the submodule and evaluated after the harness files it `$INCLUDE`s.
//! This crate never rewrites files under `vendor/test262`.
//!
//! ```text
//! cargo nextest run -p den-stdlib-temporal --test test262
//! cargo nextest run -p den-stdlib-temporal --test test262 -E 'test(Instant/basic)'
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use libtest_mimic::{Arguments, Failed, Trial};
use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Module, context::EvalOptions,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives in the workspace")
        .to_path_buf()
}

fn test262_root() -> PathBuf { workspace_root().join("vendor/test262") }

#[derive(Debug, Default, Clone)]
struct Frontmatter {
    features: Vec<String>,
    includes: Vec<String>,
    flags:    Vec<String>,
    negative: bool,
}

fn parse_frontmatter(source: &str) -> Frontmatter {
    let Some(start) = source.find("/*---") else {
        return Frontmatter::default();
    };
    let rest = &source[start + 5..];
    let Some(end) = rest.find("---*/") else {
        return Frontmatter::default();
    };
    let block = &rest[..end];
    Frontmatter {
        features: yaml_list(block, "features"),
        includes: yaml_list(block, "includes"),
        flags:    yaml_list(block, "flags"),
        negative: block.contains("\nnegative:") || block.contains("\nnegative :"),
    }
}

fn yaml_list(block: &str, key: &str) -> Vec<String> {
    let header = format!("{key}:");
    let Some(line_start) = block.find(&header) else {
        return Vec::new();
    };
    let after = block[line_start + header.len()..].trim_start();
    if let Some(rest) = after.strip_prefix('[') {
        let end = rest.find(']').unwrap_or(rest.len());
        return rest[..end]
            .split(',')
            .map(|item| item.trim().trim_matches(',').to_string())
            .filter(|item| !item.is_empty())
            .collect();
    }
    after
        .lines()
        .take_while(|line| line.starts_with("  - ") || line.starts_with("\t- "))
        .filter_map(|line| {
            line.split_once("- ")
                .map(|(_, item)| item.trim().to_string())
        })
        .filter(|item| !item.is_empty())
        .collect()
}

const SUPPORTED_FEATURES: &[&str] = &[
    "Temporal",
    "BigInt",
    "Symbol",
    "Symbol.species",
    "Symbol.iterator",
    "Symbol.toStringTag",
    "computed-property-names",
    "arrow-function",
    "let",
    "const",
    "destructuring-binding",
    "rest-parameters",
    "template",
    "default-parameters",
    "class",
    "class-fields-public",
    "Proxy",
];

fn should_skip(meta: &Frontmatter) -> Option<&'static str> {
    if meta.negative {
        return Some("negative frontmatter");
    }
    if meta
        .flags
        .iter()
        .any(|flag| flag == "module" || flag == "async")
    {
        return Some("module/async flag");
    }
    for feature in &meta.features {
        if feature.starts_with("Intl") || !SUPPORTED_FEATURES.contains(&feature.as_str()) {
            return Some("unsupported feature");
        }
    }
    None
}

fn collect_js_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_js_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("js") {
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

fn host_prelude() -> &'static str {
    r#"
      globalThis.print = function print() {};
      globalThis.$262 = {
        global: globalThis,
        evalScript(source) { return eval(source); },
        createRealm() { throw new Test262Error("$262.createRealm is not implemented"); },
        detachArrayBuffer() { throw new Test262Error("$262.detachArrayBuffer is not implemented"); },
        gc() { throw new Test262Error("$262.gc is not implemented"); },
      };
    "#
}

fn load_harness(root: &Path) -> Result<(String, Vec<(String, String)>), Failed> {
    let harness = root.join("harness");
    let mut core = String::from(host_prelude());
    for name in ["assert.js", "sta.js"] {
        core.push('\n');
        core.push_str(
            &fs::read_to_string(harness.join(name))
                .map_err(|error| format!("test262 harness {name}: {error}"))?,
        );
    }
    let extras = fs::read_dir(&harness)
        .map_err(|error| format!("test262 harness: {error}"))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("js") {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            if name == "assert.js" || name == "sta.js" {
                return None;
            }
            fs::read_to_string(&path).ok().map(|body| (name, body))
        })
        .collect();
    Ok((core, extras))
}

fn run_one(
    harness: &str, includes: &[(String, String)], test: &Path, source: &str, meta: &Frontmatter,
) -> Result<(), Failed> {
    let mut script = String::from(harness);
    for include in &meta.includes {
        let body = includes
            .iter()
            .find(|(name, _)| name == include)
            .map(|(_, body)| body.as_str())
            .ok_or_else(|| format!("missing include {include}"))?;
        script.push('\n');
        script.push_str(body);
    }
    script.push('\n');
    script.push_str(source);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let js = AsyncRuntime::new().map_err(|error| error.to_string())?;
        let context = AsyncContext::full(&js)
            .await
            .map_err(|error| error.to_string())?;
        context
            .async_with(async |ctx| {
                let run = async {
                    let (_module, evaluated) = Module::evaluate_def::<
                        den_stdlib_temporal::js_temporal,
                        _,
                    >(ctx.clone(), "den:temporal")?;
                    evaluated.into_future::<()>().await?;
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = false;
                    options.strict = true;
                    options.filename = Some(test.display().to_string());
                    ctx.eval_with_options::<(), _>(script, options)?;
                    Ok::<_, rquickjs::Error>(())
                };
                run.await.catch(&ctx).map_err(|error| match error {
                    CaughtError::Value(value) => {
                        let object = value.as_object();
                        let name = object
                            .and_then(|object| object.get::<_, String>("name").ok())
                            .unwrap_or_else(|| "Error".to_string());
                        let message = object
                            .and_then(|object| object.get::<_, String>("message").ok())
                            .unwrap_or_else(|| format!("{value:?}"));
                        format!("{name}: {message}")
                    }
                    other => other.to_string(),
                })
            })
            .await
    })?;
    Ok(())
}

fn harness_classify() -> Result<(), Failed> {
    assert!(
        test262_root().ends_with("vendor/test262"),
        "test262 root is the vendored submodule"
    );
    assert_eq!(
        relative_to(
            Path::new("/Temporal"),
            Path::new("/Temporal/Instant/basic.js")
        ),
        "Instant/basic.js",
        "trial names are official paths under built-ins/Temporal"
    );
    let meta = parse_frontmatter(
        "/*---\nfeatures: [Temporal, Intl.DateTimeFormat]\nflags: [async]\n---*/\n",
    );
    assert_eq!(
        should_skip(&meta),
        Some("module/async flag"),
        "async flag is a harness skip"
    );
    let intl = parse_frontmatter("/*---\nfeatures: [Temporal, Intl]\n---*/\n");
    assert_eq!(
        should_skip(&intl),
        Some("unsupported feature"),
        "Intl is not installed"
    );
    let ok = parse_frontmatter("/*---\nfeatures: [Temporal]\n---*/\n");
    assert_eq!(should_skip(&ok), None, "plain Temporal is executable");
    assert!(
        collect_js_files(Path::new("/den-test262-no-such-dir")).is_empty(),
        "missing tree walks to empty"
    );
    if test262_root().join("harness").is_dir() {
        let (core, extras) = load_harness(&test262_root())?;
        assert!(
            run_one(&core, &extras, Path::new("harness::empty"), "", &ok).is_ok(),
            "empty official body still installs den:temporal"
        );
    }
    Ok(())
}

fn main() {
    let mut tests = vec![Trial::test("harness::classify", harness_classify)];
    let root = test262_root();
    let suite = root.join("test/built-ins/Temporal");
    if !suite.is_dir() {
        tests.push(Trial::test("vendor/test262", || {
            Err(
                "vendor/test262 is missing Temporal tests; run `git submodule update --init \
                 vendor/test262`"
                    .into(),
            )
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let (core_harness, extra_includes) = match load_harness(&root) {
        Ok(loaded) => loaded,
        Err(error) => {
            tests.push(Trial::test("vendor/test262", move || Err(error)));
            libtest_mimic::run(&Arguments::from_args(), tests).exit();
        }
    };
    let harness = Arc::new(core_harness);
    let includes = Arc::new(extra_includes);
    let mut files = collect_js_files(&suite);
    files.sort();
    if files.is_empty() {
        tests.push(Trial::test("vendor/test262", || {
            Err("walker found no tests under vendor/test262/test/built-ins/Temporal".into())
        }));
    }
    for file in files {
        let relative = relative_to(&suite, &file);
        let source = match fs::read_to_string(&file) {
            Ok(source) => source,
            Err(error) => {
                tests.push(Trial::test(relative, move || {
                    Err(format!("read: {error}").into())
                }));
                continue;
            }
        };
        let meta = parse_frontmatter(&source);
        let ignored = should_skip(&meta).is_some();
        let harness = Arc::clone(&harness);
        let includes = Arc::clone(&includes);
        tests.push(
            Trial::test(relative, move || {
                run_one(&harness, &includes, &file, &source, &meta)
            })
            .with_ignored_flag(ignored),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
