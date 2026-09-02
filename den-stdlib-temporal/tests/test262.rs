//! Official test262 Temporal suite: `test/built-ins/Temporal/**` plus
//! `test/intl402/**`.
//!
//! Most of `intl402` is Temporal: 2029 of its 3357 files exercise non-ISO
//! calendars, and all but 45 of those never touch an `Intl` object, so they
//! score den's calendar support today. Files that do need a real `Intl`
//! constructor are ignored by feature, not by name.
//!
//! Each official `.js` file is one cargo/nextest test, named by its path under
//! `test/`, so the subtree is part of the name. The file is read raw from the
//! submodule and evaluated after the harness files it `$INCLUDE`s. This crate
//! never rewrites files under `vendor/test262`.
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

use den_core::engine::Engine;
use libtest_mimic::{Arguments, Failed, Trial};

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest.parent() else {
        panic!("crate lives in the workspace");
    };
    root.to_path_buf()
}

fn test262_root() -> PathBuf { workspace_root().join("vendor/test262") }

const TEST262_TREES: &[&str] = &["built-ins/Temporal", "intl402"];

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
    let Some(rest) = source.get(start + 5..) else {
        return Frontmatter::default();
    };
    let Some(end) = rest.find("---*/") else {
        return Frontmatter::default();
    };
    let Some(block) = rest.get(..end) else {
        return Frontmatter::default();
    };
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
    let after = block
        .get(line_start + header.len()..)
        .unwrap_or_default()
        .trim_start();
    if let Some(rest) = after.strip_prefix('[') {
        let end = rest.find(']').unwrap_or(rest.len());
        return rest
            .get(..end)
            .unwrap_or_default()
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
    // A calendar feature (eras and non-ISO month codes), not an Intl object:
    // 1556 intl402 files carry it and run fine without any Intl constructor.
    "Intl.Era-monthcode",
    // Intl phase 1: the namespace, `getCanonicalLocales` and `Intl.Locale`
    // (including its locale-info accessors) are installed by den-stdlib-intl.
    "Intl.Locale",
    "Intl.Locale-info",
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
        if SUPPORTED_FEATURES.contains(&feature.as_str()) {
            continue;
        }
        return Some(if feature.starts_with("Intl") {
            "needs an Intl object"
        } else {
            "unsupported feature"
        });
    }
    None
}

fn collect_js_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
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

const fn host_prelude() -> &'static str {
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
        .filter_map(std::result::Result::ok)
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
        let engine = Engine::new().await;
        engine
            .eval::<()>(&format!("{script}\nundefined"))
            .await
            .map_err(|error| format!("{}: {error}", test.display()))
    })?;
    Ok(())
}

fn assert_harness_classification() -> Frontmatter {
    assert!(
        test262_root().ends_with("vendor/test262"),
        "test262 root is the vendored submodule"
    );
    assert_eq!(
        relative_to(
            Path::new("/test"),
            Path::new("/test/intl402/Temporal/basic.js")
        ),
        "intl402/Temporal/basic.js",
        "trial names are official paths under test/, so the subtree is visible"
    );
    let meta = parse_frontmatter(
        "/*---\nfeatures: [Temporal, Intl.DateTimeFormat]\nflags: [async]\n---*/\n",
    );
    assert_eq!(
        should_skip(&meta),
        Some("module/async flag"),
        "async flag is a harness skip"
    );
    let intl = parse_frontmatter("/*---\nfeatures: [Temporal, Intl.DateTimeFormat]\n---*/\n");
    assert_eq!(
        should_skip(&intl),
        Some("needs an Intl object"),
        "Intl constructors are not installed"
    );
    let eras = parse_frontmatter("/*---\nfeatures: [Temporal, Intl.Era-monthcode]\n---*/\n");
    assert_eq!(
        should_skip(&eras),
        None,
        "era/month-code calendars are plain Temporal"
    );
    let ok = parse_frontmatter("/*---\nfeatures: [Temporal]\n---*/\n");
    assert_eq!(should_skip(&ok), None, "plain Temporal is executable");
    assert!(
        collect_js_files(Path::new("/den-test262-no-such-dir")).is_empty(),
        "missing tree walks to empty"
    );
    ok
}

fn assert_empty_harness(core: &str, extras: &[(String, String)], ok: &Frontmatter) {
    assert!(
        run_one(core, extras, Path::new("harness::empty"), "", ok).is_ok(),
        "empty official body still installs den:temporal"
    );
}

fn harness_classify() -> Result<(), Failed> {
    let ok = assert_harness_classification();
    if test262_root().join("harness").is_dir() {
        let (core, extras) = load_harness(&test262_root())?;
        assert_empty_harness(&core, &extras, &ok);
    }
    Ok(())
}

fn main() {
    let mut tests = vec![Trial::test("harness::classify", harness_classify)];
    let root = test262_root();
    let suite = root.join("test");
    if !TEST262_TREES.iter().all(|tree| suite.join(tree).is_dir()) {
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
    let mut files: Vec<PathBuf> = TEST262_TREES
        .iter()
        .flat_map(|tree| collect_js_files(&suite.join(tree)))
        .collect();
    files.sort();
    if files.is_empty() {
        tests.push(Trial::test("vendor/test262", || {
            Err("walker found no tests under vendor/test262/test".into())
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
