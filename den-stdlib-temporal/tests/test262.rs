//! test262 Temporal walker.
//!
//! Walks `vendor/test262/test/built-ins/Temporal/{Instant,Duration}/**`,
//! loads the harness files a test `$INCLUDE`s, evals in an Engine-free realm
//! with `den:temporal` installed, and prints pass / skip / fail. The runner
//! itself is not `#[ignore]`; individual files that are not yet in the green
//! slice are counted as fail without panicking the process.

use std::fs;
use std::path::{Path, PathBuf};

use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Module, context::EvalOptions};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives in the workspace")
        .to_path_buf()
}

fn test262_root() -> PathBuf {
    workspace_root().join("vendor/test262")
}

#[derive(Debug, Default)]
struct Frontmatter {
    features: Vec<String>,
    includes: Vec<String>,
    flags: Vec<String>,
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
        flags: yaml_list(block, "flags"),
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
    entries.sort_by_key(|entry| entry.path());
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

async fn run_one(
    runtime: &AsyncRuntime,
    harness: &str,
    includes: &[(String, String)],
    test: &Path,
    source: &str,
    meta: &Frontmatter,
) -> Result<(), String> {
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
    script.push_str("try {\n");
    script.push_str(source);
    script.push_str(
        "\n} catch (error) {\n  throw new Error((error && error.name) ? (error.name + ': ' + error.message) : String(error));\n}\n",
    );

    let context = AsyncContext::full(runtime)
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
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await
}

#[derive(Default)]
struct Counts {
    pass: usize,
    skip: usize,
    fail: usize,
}

/// The constructor / from / toString / valueOf slice we expect to keep green.
fn is_green_slice(relative: &str) -> bool {
    let path = relative.replace('\\', "/");
    const PREFIXES: &[&str] = &[
        "Instant/basic.js",
        "Instant/constructor.js",
        "Instant/from/argument-string.js",
        "Instant/from/argument-instant.js",
        "Instant/from/basic.js",
        "Instant/prototype/toString/basic.js",
        "Instant/prototype/valueOf/basic.js",
        "Instant/fromEpochNanoseconds/basic.js",
        "Duration/basic.js",
        "Duration/constructor.js",
        "Duration/from/argument-string.js",
        "Duration/from/argument-duration.js",
        "Duration/from/argument-propertybag.js",
        "Duration/prototype/toString/options-undefined.js",
        "Duration/prototype/toString/precision.js",
        "Duration/prototype/valueOf/basic.js",
    ];
    PREFIXES
        .iter()
        .any(|prefix| path.ends_with(prefix) || path.contains(prefix))
}

#[tokio::test]
async fn temporal_instant_and_duration_test262() {
    let root = test262_root();
    let harness = root.join("harness");
    let tests = root.join("test/built-ins/Temporal");
    assert!(
        tests.is_dir(),
        "vendor/test262 is missing Temporal tests; run `git submodule update --init vendor/test262`"
    );

    let mut files = collect_js_files(&tests.join("Instant"));
    files.extend(collect_js_files(&tests.join("Duration")));
    files.sort();

    let mut core_harness = String::from(host_prelude());
    for name in ["assert.js", "sta.js"] {
        core_harness.push('\n');
        core_harness
            .push_str(&fs::read_to_string(harness.join(name)).expect("test262 harness file"));
    }
    let extra_includes: Vec<(String, String)> = fs::read_dir(&harness)
        .expect("harness")
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

    let runtime = AsyncRuntime::new().expect("runtime");
    let mut counts = Counts::default();
    let mut slice_fail = Vec::new();
    let mut failures = Vec::new();

    for file in &files {
        let relative = file
            .strip_prefix(&tests)
            .unwrap_or(file)
            .display()
            .to_string();
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(error) => {
                counts.fail += 1;
                failures.push(format!("{relative}: read: {error}"));
                continue;
            }
        };
        let meta = parse_frontmatter(&source);
        if let Some(reason) = should_skip(&meta) {
            counts.skip += 1;
            println!("skip  {relative} ({reason})");
            continue;
        }
        match run_one(
            &runtime,
            &core_harness,
            &extra_includes,
            file,
            &source,
            &meta,
        )
        .await
        {
            Ok(()) => {
                counts.pass += 1;
                println!("pass  {relative}");
            }
            Err(error) => {
                counts.fail += 1;
                let line = format!("{relative}: {error}");
                println!("fail  {line}");
                if is_green_slice(&relative) {
                    slice_fail.push(line.clone());
                }
                failures.push(line);
            }
        }
    }

    println!(
        "test262 Temporal Instant+Duration: {} pass, {} skip, {} fail ({} files)",
        counts.pass,
        counts.skip,
        counts.fail,
        files.len()
    );

    assert!(
        !files.is_empty(),
        "walker found no tests under vendor/test262/test/built-ins/Temporal"
    );
    assert!(
        slice_fail.is_empty(),
        "green slice failed:\n{}",
        slice_fail.join("\n")
    );
}
