//! Official Test262 suite: every `vendor/test262/test/**/*.js` file that is
//! not a `_FIXTURE`.
//!
//! The runner follows [INTERPRETING.md](../../vendor/test262/INTERPRETING.md)
//! the same way QuickJS's `run-test262.c` does: a fresh realm per trial,
//! harness files evaluated as global scripts, `$262` host functions, strict
//! and sloppy passes, modules via `JS_EVAL_TYPE_MODULE`, async via `print`,
//! and negative tests that expect a named exception. Sources are `fs::read`
//! from the submodule and never rewritten.
//!
//! **No trial is ignored.** Unsupported host pieces (`$262.createRealm`,
//! `$262.agent.start`) throw at the call site so the official file still
//! runs and fails rather than disappearing from the tally.
//!
//! ```text
//! cargo nextest run -p den-core --test test262
//! cargo nextest run -p den-core --test test262 -E 'test(language/types/undefined)'
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use den_core::engine::Engine;
use den_util::stack::JsError;
use libtest_mimic::{Arguments, Failed, Trial};
use rquickjs::{
    ArrayBuffer, CatchResultExt as _, CaughtError, Coerced, Ctx, Exception, FromJs as _, Function,
    Object, Promise, Value, context::EvalOptions, function::Opt, object::Property, qjs,
};

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest.parent() else {
        panic!("crate lives in the workspace");
    };
    root.to_path_buf()
}

fn test262_root() -> PathBuf { workspace_root().join("vendor/test262") }

fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

#[derive(Debug, Default, Clone)]
struct Frontmatter {
    includes:      Vec<String>,
    flags:         Vec<String>,
    negative_type: Option<String>,
    timeout_long:  bool,
}

impl Frontmatter {
    fn has_flag(&self, name: &str) -> bool { self.flags.iter().any(|flag| flag == name) }

    fn raw(&self) -> bool { self.has_flag("raw") }

    fn module(&self) -> bool { self.has_flag("module") }

    fn async_test(&self) -> bool { self.has_flag("async") }

    fn can_block(&self) -> bool { !self.has_flag("CanBlockIsFalse") }

    fn negative(&self) -> bool { self.negative_type.is_some() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mode {
    strict: bool,
    module: bool,
    raw:    bool,
}

#[derive(Clone)]
struct Harness {
    assert_js: Arc<str>,
    sta_js:    Arc<str>,
    extras:    Arc<[(String, String)]>,
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
    let negative_type = block
        .find("negative:")
        .and_then(|start| yaml_scalar(block.get(start..).unwrap_or_default(), "type"));
    Frontmatter {
        includes: yaml_list(block, "includes"),
        flags: yaml_list(block, "flags"),
        negative_type,
        timeout_long: yaml_scalar(block, "timeout").as_deref() == Some("long"),
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
        .map(str::trim)
        .take_while(|line| line.starts_with("- "))
        .map(|line| line.trim_start_matches("- ").trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn yaml_scalar(block: &str, key: &str) -> Option<String> {
    let header = format!("{key}:");
    let line_start = block.find(&header)?;
    let after = block
        .get(line_start + header.len()..)
        .unwrap_or_default()
        .trim_start();
    let value = after
        .split(['\n', '\r', ' ', '#'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(['\'', '"']);
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn modes(meta: &Frontmatter) -> Vec<Mode> {
    if meta.raw() {
        return vec![Mode {
            strict: false,
            module: false,
            raw:    true,
        }];
    }
    if meta.module() {
        return vec![Mode {
            strict: false,
            module: true,
            raw:    false,
        }];
    }
    if meta.has_flag("noStrict") {
        return vec![Mode {
            strict: false,
            module: false,
            raw:    false,
        }];
    }
    if meta.has_flag("onlyStrict") {
        return vec![Mode {
            strict: true,
            module: false,
            raw:    false,
        }];
    }
    vec![
        Mode {
            strict: false,
            module: false,
            raw:    false,
        },
        Mode {
            strict: true,
            module: false,
            raw:    false,
        },
    ]
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
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("js") {
            continue;
        }
        let fixture = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("_FIXTURE"));
        if !fixture {
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

fn load_harness(root: &Path) -> Result<Harness, Failed> {
    let harness = root.join("harness");
    let assert_js = fs::read_to_string(harness.join("assert.js"))
        .map_err(|error| format!("test262 harness assert.js: {error}"))?;
    let sta_js = fs::read_to_string(harness.join("sta.js"))
        .map_err(|error| format!("test262 harness sta.js: {error}"))?;
    let extras: Vec<(String, String)> = fs::read_dir(&harness)
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
    Ok(Harness {
        assert_js: Arc::<str>::from(assert_js),
        sta_js:    Arc::<str>::from(sta_js),
        extras:    extras.into(),
    })
}

fn include_body<'a>(includes: &'a [(String, String)], name: &str) -> Result<&'a str, Failed> {
    includes
        .iter()
        .find(|(file, _)| file == name)
        .map(|(_, body)| body.as_str())
        .ok_or_else(|| format!("missing harness include {name}").into())
}

fn set_can_block(ctx: &Ctx<'_>, can_block: bool) {
    // SAFETY: `ctx` is the live realm; JS_SetCanBlock only stores a flag the
    // Atomics.wait path reads, the same call QuickJS's runner makes per file.
    unsafe {
        let runtime = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
        qjs::JS_SetCanBlock(runtime, can_block);
    }
}

const SLOPPY_SCRIPT: Mode = Mode {
    strict: false,
    module: false,
    raw:    false,
};

fn eval_options(filename: String, mode: Mode) -> EvalOptions {
    let mut options = EvalOptions::default();
    options.global = !mode.module;
    options.strict = mode.strict;
    options.promise = false;
    options.filename = Some(filename);
    options
}

fn throw_unimplemented(ctx: Ctx<'_>, what: &'static str) -> rquickjs::Result<()> {
    Err(Exception::throw_type(
        &ctx,
        &format!("{what} is not implemented"),
    ))
}

fn js_eval_script(ctx: Ctx<'_>, source: String) -> rquickjs::Result<Value<'_>> {
    ctx.eval_with_options(source, eval_options("<evalScript>".into(), SLOPPY_SCRIPT))
}

fn js_detach(ctx: Ctx<'_>, value: Value<'_>) -> rquickjs::Result<()> {
    let Some(mut buffer) = ArrayBuffer::from_value(value) else {
        return Err(Exception::throw_type(&ctx, "expected an ArrayBuffer"));
    };
    buffer.detach();
    Ok(())
}

fn js_gc(ctx: Ctx<'_>) { ctx.run_gc(); }

fn js_null(ctx: Ctx<'_>) -> Value<'_> { Value::new_null(ctx) }

fn js_create_realm(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.createRealm")
}

fn js_agent_start(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.agent.start")
}

fn js_agent_broadcast(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.agent.broadcast")
}

fn js_agent_receive(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.agent.receiveBroadcast")
}

fn js_agent_report(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.agent.report")
}

fn js_agent_leaving(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    throw_unimplemented(ctx, "$262.agent.leaving")
}

fn js_sleep(millis: Coerced<f64>) {
    let millis = millis.0.max(0.0) as u64;
    std::thread::sleep(Duration::from_millis(millis));
}

fn snapshot(printed: &Mutex<Vec<String>>) -> Vec<String> {
    printed.lock().map_or_else(
        |poisoned| poisoned.into_inner().clone(),
        |lines| lines.clone(),
    )
}

fn install_host(ctx: &Ctx<'_>, printed: Arc<Mutex<Vec<String>>>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let print = Function::new(ctx.clone(), move |message: Opt<Coerced<String>>| {
        if let Ok(mut lines) = printed.lock() {
            lines.push(message.0.map(|Coerced(text)| text).unwrap_or_default());
        }
    })?
    .with_name("print")?;
    globals.prop("print", Property::from(print).writable().configurable())?;

    let host = Object::new(ctx.clone())?;
    host.set(
        "evalScript",
        Function::new(ctx.clone(), js_eval_script)?.with_name("evalScript")?,
    )?;
    host.set(
        "detachArrayBuffer",
        Function::new(ctx.clone(), js_detach)?.with_name("detachArrayBuffer")?,
    )?;
    host.set("gc", Function::new(ctx.clone(), js_gc)?.with_name("gc")?)?;
    host.set("global", globals.clone())?;
    host.set(
        "createRealm",
        Function::new(ctx.clone(), js_create_realm)?.with_name("createRealm")?,
    )?;

    let is_html_dda = Function::new(ctx.clone(), js_null)?.with_name("IsHTMLDDA")?;
    // SAFETY: JS_SetIsHTMLDDA only sets the [[IsHTMLDDA]] slot on this live
    // function object; it does not take ownership. QuickJS's runner does the
    // same so IsHTMLDDA-tagged tests have a document.all stand-in.
    unsafe {
        qjs::JS_SetIsHTMLDDA(ctx.as_raw().as_ptr(), is_html_dda.as_value().as_raw());
    }
    host.set("IsHTMLDDA", is_html_dda)?;

    let origin = Instant::now();
    let agent = Object::new(ctx.clone())?;
    agent.set(
        "start",
        Function::new(ctx.clone(), js_agent_start)?.with_name("start")?,
    )?;
    agent.set(
        "broadcast",
        Function::new(ctx.clone(), js_agent_broadcast)?.with_name("broadcast")?,
    )?;
    agent.set(
        "receiveBroadcast",
        Function::new(ctx.clone(), js_agent_receive)?.with_name("receiveBroadcast")?,
    )?;
    agent.set(
        "report",
        Function::new(ctx.clone(), js_agent_report)?.with_name("report")?,
    )?;
    agent.set(
        "leaving",
        Function::new(ctx.clone(), js_agent_leaving)?.with_name("leaving")?,
    )?;
    agent.set(
        "getReport",
        Function::new(ctx.clone(), js_null)?.with_name("getReport")?,
    )?;
    agent.set(
        "sleep",
        Function::new(ctx.clone(), js_sleep)?.with_name("sleep")?,
    )?;
    agent.set(
        "monotonicNow",
        Function::new(ctx.clone(), move || origin.elapsed().as_secs_f64() * 1000.0)?
            .with_name("monotonicNow")?,
    )?;
    host.set("agent", agent)?;

    globals.prop("$262", Property::from(host).writable().configurable())?;
    Ok(())
}

fn eval_source<'js>(
    ctx: &Ctx<'js>, source: &str, filename: &str, mode: Mode,
) -> rquickjs::Result<Value<'js>> {
    ctx.eval_with_options(source, eval_options(filename.to_owned(), mode))
}

fn js_error<'js>(ctx: &Ctx<'js>, error: CaughtError<'js>) -> JsError {
    match error {
        CaughtError::Exception(exception) => JsError::from_value(ctx, &exception.into_value()),
        CaughtError::Value(value) => JsError::from_value(ctx, &value),
        CaughtError::Error(error) => {
            let quoted =
                serde_json::to_string(&error.to_string()).unwrap_or_else(|_| "\"error\"".into());
            let value = ctx
                .eval::<Value<'_>, _>(format!("new Error({quoted})"))
                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
            JsError::from_value(ctx, &value)
        }
    }
}

fn matches_error(error: &JsError, expected: &str) -> bool {
    error.name() == Some(expected)
        || error.to_string().starts_with(expected)
        || error.message().starts_with(expected)
}

fn timeout(meta: &Frontmatter) -> Duration {
    if meta.timeout_long {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    }
}

fn async_verdict(printed: &[String]) -> Result<(), Failed> {
    let mut complete = false;
    let mut failure = None;
    for line in printed {
        if line == "Test262:AsyncTestComplete" {
            complete = true;
        } else if let Some(reason) = line.strip_prefix("Test262:AsyncTestFailure:") {
            failure = Some(reason.trim().to_owned());
        }
    }
    if let Some(reason) = failure {
        return Err(format!("async failure: {reason}").into());
    }
    if complete {
        Ok(())
    } else {
        Err("$DONE() not called".into())
    }
}

fn finish(meta: &Frontmatter, printed: &[String], error: Option<JsError>) -> Result<(), Failed> {
    match (meta.negative(), error) {
        (true, Some(error)) => {
            let Some(expected) = meta.negative_type.as_deref() else {
                return Ok(());
            };
            if matches_error(&error, expected) {
                Ok(())
            } else {
                Err(format!(
                    "unexpected error type: expected {expected}, got {}",
                    error.name().unwrap_or("Error")
                )
                .into())
            }
        }
        (true, None) => Err("expected error".into()),
        (false, Some(error)) => Err(error.to_string().into()),
        (false, None) if meta.async_test() => async_verdict(printed),
        (false, None) => Ok(()),
    }
}

fn eval_harness(ctx: &Ctx<'_>, harness: &Harness, meta: &Frontmatter) -> Result<(), Failed> {
    eval_source(ctx, &harness.assert_js, "harness/assert.js", Mode {
        strict: false,
        module: false,
        raw:    false,
    })
    .catch(ctx)
    .map_err(|error| format!("assert.js: {error}"))?;
    eval_source(ctx, &harness.sta_js, "harness/sta.js", Mode {
        strict: false,
        module: false,
        raw:    false,
    })
    .catch(ctx)
    .map_err(|error| format!("sta.js: {error}"))?;
    if meta.async_test() {
        let body = include_body(&harness.extras, "doneprintHandle.js")?;
        eval_source(ctx, body, "harness/doneprintHandle.js", Mode {
            strict: false,
            module: false,
            raw:    false,
        })
        .catch(ctx)
        .map_err(|error| format!("doneprintHandle.js: {error}"))?;
    }
    for name in &meta.includes {
        let body = include_body(&harness.extras, name)?;
        eval_source(ctx, body, &format!("harness/{name}"), Mode {
            strict: false,
            module: false,
            raw:    false,
        })
        .catch(ctx)
        .map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(())
}

async fn run_script_body(
    ctx: &Ctx<'_>, source: &str, filename: &str, mode: Mode,
) -> Result<Option<JsError>, Failed> {
    match eval_source(ctx, source, filename, mode).catch(ctx) {
        Ok(value) => {
            if value.is_promise() {
                let promise = Promise::from_js(ctx, value).map_err(|error| error.to_string())?;
                match promise.into_future::<Value<'_>>().await.catch(ctx) {
                    Ok(_) => Ok(None),
                    Err(error) => Ok(Some(js_error(ctx, error))),
                }
            } else {
                Ok(None)
            }
        }
        Err(error) => Ok(Some(js_error(ctx, error))),
    }
}

async fn run_one(
    test: PathBuf, source: String, meta: Frontmatter, mode: Mode, harness: Harness,
) -> Result<(), Failed> {
    let printed = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::new().await;
    let filename = dunce::canonicalize(&test)
        .unwrap_or(test)
        .to_string_lossy()
        .replace('\\', "/");
    let wait = timeout(&meta);
    let host_printed = Arc::clone(&printed);
    let outcome = tokio::time::timeout(wait, async {
        let eval_error = engine
            .context
            .async_with({
                let inner_meta = meta.clone();
                async move |ctx| {
                    set_can_block(&ctx, inner_meta.can_block());
                    install_host(&ctx, host_printed)
                        .map_err(|error| Failed::from(error.to_string()))?;
                    if !mode.raw {
                        eval_harness(&ctx, &harness, &inner_meta)?;
                    }
                    run_script_body(&ctx, &source, &filename, mode).await
                }
            })
            .await?;
        if let Some(error) = eval_error {
            engine.run_event_loop().await;
            return finish(&meta, &snapshot(&printed), Some(error));
        }
        engine.run_event_loop().await;
        finish(&meta, &snapshot(&printed), None)
    })
    .await;
    engine.shutdown().await;
    match outcome {
        Ok(result) => result,
        Err(_elapsed) => Err(format!("timed out after {wait:?}").into()),
    }
}

fn run_one_sync(
    test: PathBuf, meta: Frontmatter, mode: Mode, harness: Harness,
) -> Result<(), Failed> {
    let source = fs::read_to_string(&test).map_err(|error| format!("read: {error}"))?;
    tokio_runtime().block_on(run_one(test, source, meta, mode, harness))
}

fn assert_harness_classification() -> Result<(), Failed> {
    if !test262_root().ends_with("vendor/test262") {
        return Err("test262 root is the vendored submodule".into());
    }
    let both = parse_frontmatter("/*---\nflags: [generated]\n---*/\n");
    if modes(&both).len() != 2 {
        return Err("unrestricted tests must run sloppy and strict".into());
    }
    let module = parse_frontmatter("/*---\nflags: [module]\n---*/\n");
    if modes(&module)
        != [Mode {
            strict: false,
            module: true,
            raw:    false,
        }]
    {
        return Err("module tests must run once as a module".into());
    }
    let negative =
        parse_frontmatter("/*---\nnegative:\n  phase: runtime\n  type: TypeError\n---*/\n");
    if negative.negative_type.as_deref() != Some("TypeError") {
        return Err("negative type was not parsed".into());
    }
    let dashed = parse_frontmatter("/*---\nflags:\n  - onlyStrict\n  - async\n---*/\n");
    if !dashed.has_flag("onlyStrict") || !dashed.async_test() {
        return Err("dashed YAML lists must parse".into());
    }
    let fixture = Path::new("dep_FIXTURE.js");
    if !fixture
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("_FIXTURE"))
    {
        return Err("fixture names must be excluded from the trial list".into());
    }
    Ok(())
}

fn trial_name(relative: &str, mode: Mode, modes: &[Mode]) -> String {
    if modes.len() == 2 && mode.strict {
        format!("{relative} [strict]")
    } else {
        relative.to_owned()
    }
}

fn main() {
    let mut tests = vec![Trial::test(
        "harness::classify",
        assert_harness_classification,
    )];
    let root = test262_root();
    let suite = root.join("test");
    if !suite.is_dir() {
        tests.push(Trial::test("vendor/test262", || {
            Err(
                "vendor/test262 is missing tests; run `git submodule update --init vendor/test262`"
                    .into(),
            )
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let harness = match load_harness(&root) {
        Ok(loaded) => loaded,
        Err(error) => {
            tests.push(Trial::test("vendor/test262", move || Err(error)));
            libtest_mimic::run(&Arguments::from_args(), tests).exit();
        }
    };
    let mut files = collect_js_files(&suite);
    files.sort();
    if files.is_empty() {
        tests.push(Trial::test("vendor/test262", || {
            Err("walker found no tests under vendor/test262/test".into())
        }));
    }
    for file in files {
        let relative = relative_to(&suite, &file);
        let meta = match fs::read_to_string(&file) {
            Ok(source) => parse_frontmatter(&source),
            Err(error) => {
                tests.push(Trial::test(relative, move || {
                    Err(format!("read: {error}").into())
                }));
                continue;
            }
        };
        let selected = modes(&meta);
        for mode in selected.iter().copied() {
            let name = trial_name(&relative, mode, &selected);
            let file = file.clone();
            let meta = meta.clone();
            let harness = harness.clone();
            tests.push(Trial::test(name, move || {
                run_one_sync(file, meta, mode, harness)
            }));
        }
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
