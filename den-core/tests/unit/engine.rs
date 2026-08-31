use std::{env::temp_dir, fs, path::PathBuf, process};

use color_eyre::eyre;
use rquickjs::{embed, loader::Bundle};

use crate::engine::{Engine, EngineError, PendingRejections};

static EMBEDDED_BUNDLE: Bundle = {
    // Keep Cargo's file dependencies in the same item as the proc macro, which
    // reads the files itself without registering them with rustc.
    const _: &[&[u8]] = &[
        include_bytes!("../fixtures/engine/embedded/answer.js"),
        include_bytes!("../fixtures/engine/embedded/main.js"),
        include_bytes!("../fixtures/engine/embedded/worker.js"),
        include_bytes!("../fixtures/engine/embedded/worker_double.js"),
        include_bytes!("../fixtures/engine/embedded/worker_parent.js"),
    ];
    embed! {
        "den-embed:/answer.js": "tests/fixtures/engine/embedded/answer.js",
        // Dependencies precede their importers so QuickJS can resolve static imports.
        "den-embed:/main.js": "tests/fixtures/engine/embedded/main.js",
        "den-embed:/worker_double.js": "tests/fixtures/engine/embedded/worker_double.js",
        "den-embed:/worker.js": "tests/fixtures/engine/embedded/worker.js",
        "den-embed:/worker_parent.js": "tests/fixtures/engine/embedded/worker_parent.js",
    }
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/engine")
        .join(name)
}

fn write_special_script(name: &str, source: &str) -> PathBuf {
    let directory = temp_dir().join(format!("den-engine-{}-special", process::id()));
    fs::create_dir_all(&directory).expect("the temporary directory is writable");
    let path = directory.join(name);
    fs::write(&path, source).expect("the temporary directory is writable");
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn an_embedded_entry_imports_its_embedded_sibling() -> eyre::Result<()> {
    let engine = Engine::new_with_bundle(EMBEDDED_BUNDLE).await;
    engine.run_module("den-embed:/main.js").await?;
    let answer = engine.eval::<usize>("globalThis.embeddedAnswer").await?;
    eyre::ensure!(answer == 42, "expected 42, got {answer}");
    Ok(())
}

#[cfg(feature = "stdlib-worker")]
#[tokio::test(flavor = "multi_thread")]
async fn embedded_modules_are_available_inside_module_workers() -> eyre::Result<()> {
    let engine = Engine::new_with_bundle(EMBEDDED_BUNDLE).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.run_module("den-embed:/worker_parent.js"),
    )
    .await??;
    let answer = engine
        .eval::<usize>("globalThis.embeddedWorkerAnswer")
        .await?;
    eyre::ensure!(answer == 42, "expected worker answer 42, got {answer}");
    engine.shutdown().await;
    Ok(())
}

/// Let every spawned task run to a standstill, then ask the realm how many
/// unhandled rejections it decided to report. Reporting itself goes to
/// stderr, which this process cannot read back.
async fn reported_rejections(engine: &Engine) -> usize {
    engine.runtime.idle().await;
    engine
        .context
        .with(|ctx| {
            ctx.userdata::<PendingRejections>()
                .map_or(0, |pending| pending.reported.get())
        })
        .await
}

/// The other half of the absolute entry point: everything it imports is
/// named relative to *it*, extension optional, exactly as it would be for
/// an entry point named relative to the working directory.
#[tokio::test(flavor = "multi_thread")]
async fn an_absolute_entry_point_resolves_its_relative_siblings() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file(fixture("sibling/main.js")).await?;
    let answer = engine.eval::<usize>("globalThis.siblingAnswer").await?;
    eyre::ensure!(answer == 42, "expected sibling answer 42, got {answer}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn run_file_accepts_an_absolute_path() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file(fixture("absolute.js")).await?;
    let got = engine.eval::<usize>("globalThis.absoluteRan").await?;
    eyre::ensure!(got == 7, "expected 7, got {got}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn run_file_accepts_a_file_url_with_spaces_and_a_backtick() -> eyre::Result<()> {
    let path = write_special_script(
        "entry with spaces and `tick.js",
        "globalThis.specialFilenameRan = 9;\n",
    );
    let url = url::Url::from_file_path(&path).expect("an absolute file URL");
    let engine = Engine::new().await;
    engine.run_file(PathBuf::from(url.as_str())).await?;
    let got = engine
        .eval::<usize>("globalThis.specialFilenameRan")
        .await?;
    eyre::ensure!(got == 9, "expected special-name entry result 9, got {got}");
    // Only this file: the directory is shared with every other special-name
    // fixture in the binary, and tests run concurrently.
    fs::remove_file(&path)?;
    Ok(())
}

/// The base URL is what `new Worker("./child.js")` resolves against, so it
/// has to follow the entry point rather than the working directory.
#[cfg(feature = "stdlib-worker")]
#[tokio::test(flavor = "multi_thread")]
async fn run_file_points_the_base_url_at_the_entry_points_directory() -> eyre::Result<()> {
    use den_stdlib_worker::BaseUrl;
    use url::Url;

    let path = fixture("base_url.js");
    let engine = Engine::new().await;
    engine.run_file(path.clone()).await?;

    let directory = path.canonicalize()?;
    let directory = directory.parent().expect("a file has a parent").to_owned();
    let expected = Url::from_directory_path(directory).expect("an absolute directory");
    let actual = engine
        .context
        .with(|ctx| ctx.userdata::<BaseUrl>().map(|base| base.0.clone()))
        .await;

    eyre::ensure!(
        actual.as_deref() == Some(expected.as_str()),
        "expected base URL {expected}, got {actual:?}"
    );
    Ok(())
}

/// How long a worker's error is given to climb back to its parent before
/// the test calls the chain broken. Generous: it is a thread spawn, an
/// engine build and a module load, on whatever the CI box is doing.
#[cfg(all(feature = "stdlib-worker", feature = "stdlib-timer"))]
const WORKER_FAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `den:worker` already made the main global an EventTarget, so the tests
/// that want the event listen — they do not stand up a JS Event class.
#[cfg(feature = "stdlib-worker")]
const REJECTION_HARNESS: &str = include_str!("../fixtures/engine/rejection_harness.js");

/// An uncaught error in the main script used to print twice: once from
/// `main.rs`, which is handed the failure, and once from the rejection
/// tracker, which sees the promise QuickJS rejects for the module body and
/// then frees without ever attaching a handler to it.
#[tokio::test(flavor = "multi_thread")]
async fn a_top_level_throw_is_not_also_an_unhandled_rejection() -> eyre::Result<()> {
    let path = fixture("top_level_throw.js");
    let engine = Engine::new().await;
    let outcome = engine.run_file(path.clone()).await;

    eyre::ensure!(
        matches!(&outcome, Err(EngineError::JavaScript(_))),
        "expected the top-level throw to fail, got {outcome:?}"
    );
    let reported = reported_rejections(&engine).await;
    eyre::ensure!(
        reported == 0,
        "expected no duplicate rejection, got {reported}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn eval_returns_an_owned_source_mapped_javascript_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine
        .eval::<()>(
            "type Marker = {\n  value: number;\n};\nfunction fail(): never {\n  throw new \
             TypeError('eval boom');\n}\nfail();",
        )
        .await;
    let Err(EngineError::JavaScript(error)) = outcome else {
        return Err(eyre::eyre!(
            "expected an owned JavaScript error, got {outcome:?}"
        ));
    };
    eyre::ensure!(error.name() == Some("TypeError"), "wrong subtype: {error}");
    let location = error
        .location()
        .ok_or_else(|| eyre::eyre!("missing location: {error}"))?;
    eyre::ensure!(
        location.filename == "<eval:1>" && location.line == 5,
        "{error}"
    );
    let rendered = error.to_string();
    let _: i32 = engine.eval("1").await?;
    eyre::ensure!(
        error.to_string() == rendered,
        "owned error changed after re-entering the realm"
    );
    Ok(())
}

/// The other direction of the same fix: suppressing the module's own
/// duplicate must not suppress a rejection the script really did leave
/// lying around.
#[tokio::test(flavor = "multi_thread")]
async fn a_rejection_the_entry_point_leaves_behind_is_still_reported() -> eyre::Result<()> {
    let path = fixture("entry_point_rejection.js");
    let engine = Engine::new().await;
    engine.run_file(path.clone()).await?;

    let reported = reported_rejections(&engine).await;
    eyre::ensure!(reported == 1, "expected one rejection, got {reported}");
    Ok(())
}

#[cfg(feature = "stdlib-worker")]
#[tokio::test(flavor = "multi_thread")]
async fn a_realm_that_cancels_unhandledrejection_stops_the_report() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>(&format!(
            "{REJECTION_HARNESS}\n{}",
            include_str!(
                "../fixtures/engine/a_realm_that_cancels_unhandledrejection_stops_the_report.js"
            )
        ))
        .await?;

    let reported = reported_rejections(&engine).await;
    eyre::ensure!(
        reported == 0,
        "expected no rejection report, got {reported}"
    );
    let seen = engine.eval::<String>("globalThis.seen.join(',')").await?;
    eyre::ensure!(
        seen == "unhandledrejection:claimed by the realm",
        "unexpected event sequence: {seen:?}"
    );
    Ok(())
}

/// And a realm that hears the event out without cancelling it gets the
/// print anyway, plus the `rejectionhandled` that a handler arriving after
/// the report owes it (HTML §8.1.7.5).
#[cfg(feature = "stdlib-worker")]
#[tokio::test(flavor = "multi_thread")]
async fn a_handler_attached_after_the_report_fires_rejectionhandled() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>(&format!(
            "{REJECTION_HARNESS}
             globalThis.late = Promise.reject(new Error('late'));
             undefined;"
        ))
        .await?;
    let reported = reported_rejections(&engine).await;
    eyre::ensure!(reported == 1, "expected one rejection, got {reported}");

    engine
        .eval::<()>("globalThis.late.catch(() => {});\nundefined;")
        .await?;
    engine.runtime.idle().await;

    let seen = engine.eval::<String>("globalThis.seen.join(',')").await?;
    eyre::ensure!(
        seen == "unhandledrejection:late,rejectionhandled:late",
        "unexpected event sequence: {seen:?}"
    );
    Ok(())
}

/// The whole error chain, end to end: an exception thrown from a timer
/// callback inside a worker is reported by *Rust*, and used to stop there —
/// on stderr — because only the JS-side reporters went through the worker
/// scope's escalation. Now every reporter resolves the realm's sink, so it
/// fires the worker's own `error` event and, uncancelled, the parent's.
#[cfg(all(feature = "stdlib-worker", feature = "stdlib-timer"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_throwing_timer_in_a_worker_reaches_the_parents_onerror() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file(fixture("worker_timer/parent.js")).await?;
    // The worker's fault travels over a channel into a task this realm
    // spawned, so draining the realm is what delivers it. Bounded, because
    // a chain that stays broken would otherwise hang the suite.
    let drained = tokio::time::timeout(WORKER_FAULT_TIMEOUT, engine.runtime.idle()).await;
    eyre::ensure!(drained.is_ok(), "the worker never finished");

    let seen = engine.eval::<String>("globalThis.seen").await?;
    eyre::ensure!(
        seen == "from the timer",
        "unexpected worker error: {seen:?}"
    );
    engine.shutdown().await;
    Ok(())
}

/// Cancellation of a pending timer is drop, not a flag: the runtime clears its
/// spawner before `JS_FreeRuntime`, so an embedder that drops the `Engine`
/// never waits out a 60-second `setTimeout`.
#[cfg(feature = "stdlib-timer")]
#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_engine_with_a_pending_timer_returns_promptly() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>("setTimeout(() => {}, 60000);\nundefined;")
        .await?;

    let started = std::time::Instant::now();
    drop(engine);
    let elapsed = started.elapsed();
    eyre::ensure!(
        elapsed < std::time::Duration::from_secs(1),
        "dropping the engine waited {elapsed:?} for the timer"
    );
    Ok(())
}

/// A server spends its life inside the entry module's top-level await. The
/// host drops that program future, then reacquires the runtime through
/// `shutdown` before dropping the engine so deferred QuickJS values are freed
/// under the runtime lock.
/// `multi_thread` so the stop can arrive while the JS loop owns the runtime.
#[cfg(feature = "stdlib-timer")]
#[tokio::test(flavor = "multi_thread")]
async fn hosts_token_shuts_down_an_engine_parked_on_a_top_level_await() -> eyre::Result<()> {
    let entry = write_special_script(
        "parked_on_a_long_await.js",
        "await new Promise((resolve) => setTimeout(resolve, 60000));\n",
    );
    let engine = Engine::new().await;

    // Stands in for the host's stop signal (a `watch` flip, a Ctrl-C, an
    // admin endpoint): whatever it is, it only ever races the program future.
    let started = std::time::Instant::now();
    let stopped_the_program = tokio::select! {
        _ = engine.run_file(entry) => false,
        () = tokio::time::sleep(std::time::Duration::from_millis(100)) => true,
    };
    engine.shutdown().await;
    drop(engine);
    let elapsed = started.elapsed();

    eyre::ensure!(
        stopped_the_program,
        "the entry module returned instead of staying parked"
    );
    eyre::ensure!(
        elapsed < std::time::Duration::from_secs(1),
        "stopping a parked engine took {elapsed:?}"
    );
    Ok(())
}

#[cfg(feature = "stdlib-timer")]
#[tokio::test(flavor = "multi_thread")]
async fn set_timeout_returns_a_number_and_clear_timeout_is_a_function() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let report: String = engine
        .eval(include_str!("../fixtures/engine/timer_handles.js"))
        .await?;
    eyre::ensure!(
        report == "function,number",
        "unexpected timer handles: {report:?}"
    );
    Ok(())
}

/// `setInterval(f, 0)` reached `tokio::time::interval`, which panics on a
/// zero period — on the main thread, that is the whole process.
#[cfg(feature = "stdlib-timer")]
#[tokio::test(flavor = "multi_thread")]
async fn a_zero_delay_timer_is_clamped_instead_of_panicking() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>(include_str!("../fixtures/engine/zero_delay_timers.js"))
        .await?;
    engine.runtime.idle().await;

    let ticks = engine.eval::<usize>("globalThis.ticks").await?;
    eyre::ensure!(ticks == 2, "expected two ticks, got {ticks}");
    let timed_out = engine.eval::<bool>("globalThis.timedOut === true").await?;
    eyre::ensure!(timed_out, "the timer did not finish");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unhandled_rejection_is_reported_after_the_turn_ends() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>("Promise.reject(new Error('nobody claims this'));\nundefined;")
        .await?;
    let reported = reported_rejections(&engine).await;
    eyre::ensure!(reported == 1, "expected one rejection, got {reported}");
    Ok(())
}

/// The other half of the same decision: QuickJS reports the rejection
/// first and the handler second, so reporting eagerly would make every
/// `const p = Promise.reject(…); p.catch(…)` a false alarm.
#[tokio::test(flavor = "multi_thread")]
async fn a_rejection_handled_later_in_the_turn_is_not_reported() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .eval::<()>(
            "const p = Promise.reject(new Error('claimed')); p.catch(() => {});\nundefined;",
        )
        .await?;
    let reported = reported_rejections(&engine).await;
    eyre::ensure!(reported == 0, "expected no rejection, got {reported}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn eval_runs_script_and_converts_result_to_rust_type() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let text = engine.eval::<String>(r#"null ?? "123""#).await?;
    eyre::ensure!(text == "123", "expected \"123\", got {text:?}");
    let number = engine.eval::<usize>("null ?? 123").await?;
    eyre::ensure!(number == 123, "expected 123, got {number}");
    Ok(())
}

// `Engine::eval` deliberately evaluates as global script code (that is what a
// REPL line is), so module syntax is a QuickJS error rather than a panic.
// Loading a module goes through `Engine::run_file`, which imports instead
// of evaluating.
#[tokio::test(flavor = "multi_thread")]
async fn eval_rejects_module_syntax_as_a_recoverable_error() -> eyre::Result<()> {
    let engine = Engine::new().await;
    let outcome = engine.eval::<()>("export const hello = 'world'").await;
    eyre::ensure!(
        matches!(&outcome, Err(EngineError::JavaScript(_))),
        "expected a script syntax error, got {outcome:?}"
    );
    Ok(())
}
