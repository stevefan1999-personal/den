//! Official WPT testharness runner (`vendor/wpt`).
//!
//! `vendor/wpt` is a shallow git submodule with a cone sparse-checkout.
//! Materialized trees: `resources/` (testharness.js), `common/`,
//! `websockets/`, `wasm/jsapi/`, `url/`, `fetch/`, `FileAPI/`, `streams/`,
//! `xhr/`, and the `tools/` modules required by wptserve.
//!
//! Each official testharness source file is one cargo/nextest test. The file
//! is `fs::read` from the submodule. This harness never rewrites files on
//! disk. `{{host}}` / `{{ports[ws][0]}}` substitution is harness-side, the
//! same job wpt-serve does, and does not edit the vendor tree.
//!
//! testharness subtests are discovered only after the file runs, so nextest
//! lists the official file. The file fails if any testharness case FAILs.
//!
//! ```text
//! cargo nextest run -p den-core --test wpt --features stdlib,wasm,jit
//! cargo nextest run -p den-core --test wpt --features stdlib -E 'test(FileAPI)'
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use den_core::engine::Engine;
use libtest_mimic::{Arguments, Completion, Failed, Trial};
use rquickjs::{CatchResultExt as _, Object, Promise, context::EvalOptions};

const ADAPTER: &str = include_str!("wpt_adapter.js");
const BOOTSTRAP: &str = include_str!("wpt_bootstrap.js");

const STATUS_PASS: i32 = 0;
const STATUS_FAIL: i32 = 1;
const STATUS_TIMEOUT: i32 = 2;
const HARNESS_ERROR: i32 = 1;
const HARNESS_TIMEOUT: i32 = 2;

const WPT_TREES: &[&str] = &[
    "websockets",
    "FileAPI",
    "url",
    "fetch",
    "wasm/jsapi",
    "streams",
];

fn wpt_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(workspace) = manifest.parent() else {
        panic!("den-core lives in the workspace");
    };
    workspace.join("vendor/wpt")
}

fn relative_to(root: &Path, case_path: &Path) -> String {
    case_path
        .strip_prefix(root)
        .unwrap_or(case_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_cases(dir: &Path) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return collected;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collected.extend(collect_cases(&entry_path));
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let runnable = name.ends_with(".any.js") || name.ends_with(".window.js");
        if runnable {
            collected.push(entry_path);
        }
    }
    collected
}

fn skip_reason(relative: &str) -> Option<&'static str> {
    if relative.starts_with("wasm/") && !cfg!(feature = "wasm") {
        return Some("needs-wasm-feature");
    }
    if relative.contains(".window.") {
        return Some("needs-document");
    }
    if relative.starts_with("streams/readable-byte-streams/")
        || relative == "streams/readable-streams/crashtests/garbage-collection.any.js"
    {
        return Some("needs-byte-streams");
    }
    if relative.contains("streams/readable-streams/owning-type") {
        return Some("needs-owning-streams");
    }
    if relative.starts_with("fetch/") {
        const FETCH_ONLY: &[(&str, &str)] = &[
            ("fetch-later", "needs-fetch-later"),
            ("api/basic/referrer.any.js", "needs-referrer-policy"),
            ("api/basic/request-referrer.any.js", "needs-referrer-policy"),
            (
                "api/cors/cors-preflight-referrer.any.js",
                "needs-referrer-policy",
            ),
            (
                "api/redirect/redirect-referrer.any.js",
                "needs-referrer-policy",
            ),
            (
                "api/redirect/redirect-referrer-override.any.js",
                "needs-referrer-policy",
            ),
            ("mediasource", "needs-mediasource"),
            ("response-clone-iframe", "needs-document"),
            ("response-arraybuffer-realm", "needs-document"),
            ("response-blob-realm", "needs-document"),
            ("data-urls/navigate", "needs-navigation"),
            ("local-network-access", "needs-lna"),
            ("scheme-blob", "needs-blob-url"),
            ("range/blob", "needs-blob-url"),
            ("nosniff/parsing-nosniff", "needs-document"),
            ("content-type/script", "needs-document"),
            ("content-type/response", "needs-document"),
            ("content-type/multipart.window", "needs-document"),
            ("h1-parsing/", "needs-raw-http-document"),
            ("origin/assorted", "needs-document"),
            ("range/general.window", "needs-document"),
            ("redirects/data.window", "needs-navigation"),
            ("keepalive.any.js", "needs-document"),
            ("cors-keepalive", "needs-document"),
            ("redirect-keepalive", "needs-document"),
            (".h2.any.js", "needs-http2"),
            ("request-headers-case", "needs-http-header-name-case"),
            ("stale-while-revalidate/", "needs-swr-timing"),
            ("http-cache/no-vary-search", "needs-no-vary-search"),
            ("http-cache/partial", "needs-http-range-cache"),
            ("orb/tentative/", "needs-orb"),
            ("header-values.any.js", "needs-http-ctl-header-bytes"),
            ("header-values-normalize", "needs-http-ctl-header-bytes"),
            ("header-value-combining", "needs-raw-response-headers"),
            ("content-length/parsing", "needs-raw-http-content-length"),
            ("api-and-duplicate-headers", "needs-raw-http-content-length"),
        ];
        for (needle, reason) in FETCH_ONLY {
            if relative.contains(needle) {
                return Some(*reason);
            }
        }
    }
    let rules: &[(&str, &str)] = &[
        ("idlharness", "needs-idlharness"),
        ("/stream/", "needs-websocketstream"),
        (".https.", "needs-http-wss"),
        ("mixed-content", "needs-http-wss"),
        ("back-forward-cache", "needs-bfcache"),
        ("basic-auth", "needs-credentials-in-websocket-url"),
        ("Create-on-worker-shutdown", "needs-worker-shutdown"),
        ("bufferedAmount-unchanged", "needs-sync-xhr"),
        ("Create-http-urls", "needs-url"),
        ("Create-non-absolute-url", "needs-url"),
        ("target-address-space", "needs-private-network"),
        ("multi-globals", "needs-multi-global"),
        ("/jspi/", "needs-jspi"),
        ("esm-integration", "needs-wasm-esm"),
        ("/gc/", "needs-wasm-gc"),
        ("/js-string/", "needs-wasm-string-builtins"),
        ("/exception/", "needs-wasm-proposal"),
        ("/tag/", "needs-wasm-proposal"),
        ("wasm/jsapi/function/", "needs-wasm-proposal"),
        ("moduleSource", "needs-wasm-module-source"),
        ("resizable", "needs-shared-or-resizable-memory"),
        ("constructor-shared", "needs-shared-or-resizable-memory"),
        (
            "to-fixed-length-buffer-shared",
            "needs-shared-or-resizable-memory",
        ),
        ("FileAPI/url/", "needs-blob-url"),
        ("send-file-formdata", "needs-form-post"),
        ("Blob-textStream", "needs-blob-stream"),
        ("remove-own-iframe", "needs-document"),
        ("send-many-64K-messages", "needs-backpressure"),
        (
            "close-connecting-async",
            "needs-async-websocket-error-event",
        ),
    ];
    for (needle, reason) in rules {
        if relative.contains(needle) {
            return Some(*reason);
        }
    }
    None
}

#[derive(Clone, Copy)]
struct WptPorts {
    http0: u16,
    http1: u16,
    ws:    u16,
}

fn rewrite_tokens(source: &str, ports: WptPorts, host: &str) -> String {
    source
        .replace("{{host}}", host)
        .replace("{{hosts[][www]}}", host)
        .replace("{{hosts[alt][www]}}", "127.0.0.1")
        .replace("{{hosts[alt][]}}", "127.0.0.1")
        .replace("{{hosts[alt][www2]}}", "127.0.0.1")
        .replace("{{domains[www1]}}", "127.0.0.1")
        .replace("{{domains[www2]}}", "127.0.0.1")
        .replace("{{ports[http][0]}}", &ports.http0.to_string())
        .replace("{{ports[http][1]}}", &ports.http1.to_string())
        .replace("{{ports[https][0]}}", &ports.http0.to_string())
        .replace("{{ports[https][1]}}", &ports.http1.to_string())
        .replace("{{ports[ws][0]}}", &ports.ws.to_string())
        .replace("{{ports[wss][0]}}", &ports.ws.to_string())
        .replace("{{ports[h2][0]}}", &ports.http0.to_string())
}

fn location_js(origin: &str, relative: &str) -> String {
    let path = format!("/{}", relative.replace('\\', "/"));
    let href = format!("{origin}{path}");
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let (hostname, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, port),
        None => (host, ""),
    };
    format!(
        r#"
globalThis.location = {{
  href: {href:?},
  protocol: "http:",
  host: {host:?},
  hostname: {hostname:?},
  port: {port:?},
  pathname: {path:?},
  search: "",
  hash: "",
  origin: {origin:?},
}};
"#
    )
}

fn timeout_ms(body: &str) -> u64 {
    if body.contains("timeout=long") || body.contains("name=\"timeout\" content=\"long\"") {
        60_000
    } else {
        15_000
    }
}

fn read_dependency(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn resolve_script(wpt: &Path, case_path: &Path, spec: &str) -> Option<PathBuf> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let no_query = match trimmed.split_once('?') {
        Some((head, _)) => head,
        None => trimmed,
    };
    if no_query.ends_with("testharness.js") || no_query.ends_with("testharnessreport.js") {
        return None;
    }
    if let Some(absolute) = no_query.strip_prefix('/') {
        return Some(wpt.join(absolute));
    }
    let Some(parent) = case_path.parent() else {
        return Some(wpt.join(no_query));
    };
    Some(parent.join(no_query))
}

fn expand_source(wpt: &Path, case_path: &Path, body: &str) -> Result<String, String> {
    let mut source = String::new();
    for line in body.lines() {
        let Some(meta) = line.trim().strip_prefix("// META:") else {
            continue;
        };
        let Some(spec) = meta.trim().strip_prefix("script=") else {
            continue;
        };
        let Some(resolved) = resolve_script(wpt, case_path, spec) else {
            continue;
        };
        source.push_str(&read_dependency(&resolved)?);
        source.push('\n');
    }
    source.push_str(body);
    let encoded = serde_json::to_string(&source)
        .map_err(|error| format!("JavaScript source serialization failed: {error}"))?;
    Ok(format!("(0, eval)({encoded});\n"))
}

struct Report {
    timed_out: bool,
    harness:   i32,
    rows:      Vec<(i32, String, String)>,
}

fn parse_report(encoded: &str) -> serde_json::Result<Report> {
    let (timed_out, harness, rows) = serde_json::from_str(encoded)?;
    Ok(Report {
        timed_out,
        harness,
        rows,
    })
}

enum Verdict {
    Pass,
    Skip,
    Fail,
}

fn classify_verdict(report: &Report) -> Verdict {
    if report.timed_out {
        return Verdict::Fail;
    }
    if report.harness == HARNESS_ERROR || report.harness == HARNESS_TIMEOUT {
        return Verdict::Fail;
    }
    if report.rows.is_empty() {
        return Verdict::Fail;
    }
    let any_fail = report
        .rows
        .iter()
        .any(|row| row.0 == STATUS_FAIL || row.0 == STATUS_TIMEOUT);
    if any_fail {
        return Verdict::Fail;
    }
    let any_pass = report.rows.iter().any(|row| row.0 == STATUS_PASS);
    if any_pass {
        Verdict::Pass
    } else {
        Verdict::Skip
    }
}

fn fail_detail(relative: &str, report: &Report) -> String {
    let fails: Vec<String> = report
        .rows
        .iter()
        .filter(|row| row.0 == STATUS_FAIL || row.0 == STATUS_TIMEOUT)
        .map(|row| format!("{} — {}", row.1, row.2))
        .collect();
    if fails.is_empty() {
        if report.timed_out {
            return format!("{relative}: harness timeout");
        }
        return format!("{relative}: harness status {}", report.harness);
    }
    format!("{relative}: {}", fails.join(" | "))
}

async fn run_case(
    wpt: &Path, testharness_src: &str, case_path: &Path, relative: &str,
) -> Result<Completion, Failed> {
    let needs_http = relative.starts_with("fetch/") || relative.starts_with("url/");
    let ports = WptPorts {
        http0: 8000,
        http1: 8001,
        ws:    8002,
    };
    let body =
        fs::read_to_string(case_path).map_err(|error| format!("{relative}: read: {error}"))?;
    let source = expand_source(wpt, case_path, &body)
        .map_err(|error| format!("{relative}: dependency: {error}"))?;
    let expanded = rewrite_tokens(&source, ports, "localhost");
    let wait_ms = timeout_ms(&body);
    let mut bundle = String::from(BOOTSTRAP);
    bundle.push_str(testharness_src);
    bundle.push('\n');
    if needs_http {
        bundle.push_str(&location_js("http://localhost:8000", relative));
    }
    bundle.push_str(ADAPTER);
    bundle.push_str("\nawait Promise.resolve();\n");
    if !needs_http {
        bundle.push_str("globalThis.location.pathname = \"");
        bundle.push_str(&relative.replace('\\', "/"));
        bundle.push_str("\";\n");
    }
    bundle.push_str(&expanded);
    bundle.push_str("\ndone();\n");
    bundle.push_str("\nawait __denWptWait(");
    bundle.push_str(&wait_ms.to_string());
    bundle.push_str(");\n");
    let engine = Engine::new().await;
    let filename = relative.to_owned();
    let encoded = engine
        .context
        .async_with(async move |ctx| {
            let mut options = EvalOptions::default();
            options.global = true;
            options.promise = true;
            // testharness.js is browser sloppy-mode; Engine::eval_prepared is always
            // strict.
            options.strict = false;
            options.filename = Some(filename);
            let run = async {
                let promise: Promise = ctx.eval_with_options(bundle.as_str(), options)?;
                let object = promise.into_future::<Object>().await?;
                object.get::<_, String>("value")
            };
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await;
    engine.shutdown().await;
    match encoded {
        Ok(text) => {
            let report = parse_report(&text)
                .map_err(|error| format!("{relative}: invalid harness report: {error}"))?;
            match classify_verdict(&report) {
                Verdict::Pass => Ok(Completion::Completed),
                Verdict::Skip => Ok(Completion::ignored_with("testharness precondition")),
                Verdict::Fail => Err(fail_detail(relative, &report).into()),
            }
        }
        Err(error) => Err(format!("{relative}: {error}").into()),
    }
}

fn run_case_sync(
    wpt: &Path, testharness_src: &str, case_path: &Path, relative: &str,
) -> Result<Completion, Failed> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(run_case(wpt, testharness_src, case_path, relative))
}

fn harness_smoke() -> Result<(), Failed> {
    if !wpt_root().ends_with("vendor/wpt") {
        return Err("wpt root is not the vendored submodule".into());
    }
    if skip_reason("url/javascript-urls.window.js") != Some("needs-document") {
        return Err("window tests were not classified as needing a document".into());
    }
    let rewritten = rewrite_tokens(
        "ws://{{host}}:{{ports[ws][0]}}/echo",
        WptPorts {
            http0: 1,
            http1: 2,
            ws:    9,
        },
        "127.0.0.1",
    );
    if rewritten != "ws://127.0.0.1:9/echo" {
        return Err(format!("unexpected WPT token rewrite: {rewritten}").into());
    }
    if timeout_ms("// META: timeout=long\n") != 60_000 {
        return Err("long WPT timeout was not selected".into());
    }
    let report = parse_report(r#"[false,0,[[0,"ok",""]]]"#)?;
    if !matches!(classify_verdict(&report), Verdict::Pass) {
        return Err("a single PASS row was not classified as a file pass".into());
    }
    if collect_official(Path::new("/den-wpt-no-such")).is_ok() {
        return Err("missing official trees did not fail the harness".into());
    }
    Ok(())
}

fn collect_official(wpt: &Path) -> Result<Vec<PathBuf>, String> {
    let mut collected = Vec::new();
    for tree in WPT_TREES {
        let dir = wpt.join(tree);
        if !dir.is_dir() {
            return Err(format!(
                "missing vendor/wpt/{tree}; update the sparse checkout from .gitmodules"
            ));
        }
        collected.extend(collect_cases(&dir));
    }
    collected.sort();
    Ok(collected)
}

fn main() {
    let mut tests = vec![Trial::ignorable_test("harness::smoke", || {
        harness_smoke().map(|()| Completion::Completed)
    })];

    let wpt = wpt_root();
    let testharness = wpt.join("resources").join("testharness.js");
    if !testharness.is_file() {
        tests.push(Trial::ignorable_test("vendor/wpt", || {
            Err("missing testharness.js; run `git submodule update --init vendor/wpt`".into())
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let testharness_src = match fs::read_to_string(&testharness) {
        Ok(text) => Arc::new(text),
        Err(error) => {
            tests.push(Trial::ignorable_test("vendor/wpt", move || {
                Err(format!("read testharness.js: {error}").into())
            }));
            libtest_mimic::run(&Arguments::from_args(), tests).exit();
        }
    };
    let wpt = Arc::new(wpt);
    let collected = match collect_official(&wpt) {
        Ok(collected) => collected,
        Err(message) => {
            tests.push(Trial::ignorable_test("vendor/wpt", move || {
                Err(message.into())
            }));
            libtest_mimic::run(&Arguments::from_args(), tests).exit();
        }
    };
    if collected.is_empty() {
        tests.push(Trial::ignorable_test("vendor/wpt", || {
            Err("walker found no official testharness files under vendor/wpt".into())
        }));
    }
    for case_path in collected {
        let relative = relative_to(&wpt, &case_path);
        let ignored = skip_reason(&relative).is_some();
        let wpt = Arc::clone(&wpt);
        let testharness_src = Arc::clone(&testharness_src);
        tests.push(
            Trial::ignorable_test(relative.clone(), move || {
                run_case_sync(&wpt, &testharness_src, &case_path, &relative)
            })
            .with_ignored_flag(ignored),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
