//! The Web Workers API driven through the real [`Engine`], exactly as `den`
//! drives it.
//!
//! `den-stdlib-worker` proves the semantics against bare `AsyncContext`s; what
//! is proved here is that a *user* gets them: `den:worker`'s globals installed
//! in every realm, the module loaders and the transpiler in front of a worker
//! script, `BaseUrl` following the entry point, and `Engine::shutdown` reaping
//! the threads. Every assertion travels back into Rust, and every cross-thread
//! wait is a promise settled by an event under a timeout — never a sleep.

use std::{env::temp_dir, fs, path::PathBuf, process, time::Duration};

use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;
use tokio::time::timeout;

/// The bound on every cross-thread wait. Generous, because it is a failure
/// detector and not a schedule: a healthy round trip takes microseconds.
const DEADLINE: Duration = Duration::from_secs(10);

/// The window a worker is given to prove it is *not* going to let the runtime
/// go idle. Unlike [`DEADLINE`] this one is expected to elapse, so it is short.
const STILL_BUSY: Duration = Duration::from_millis(250);

/// Awaiting one event as a promise, prelude to every main script below. The
/// `error` listener is what turns a broken worker into a failed assertion
/// instead of a timeout.
const FIRST_MESSAGE: &str = include_str!("fixtures/workers/first_message.js");

/// One test's scripts on disk. `Worker` takes a URL, so committed fixture
/// sources are copied into an isolated directory before each test.
struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    /// Copy `files` into a directory of this test's own — the process id keeps
    /// concurrent `cargo nextest run` invocations apart, and the test name
    /// separates fixtures within one run.
    fn new(test: &str, files: &[(&str, &str)]) -> eyre::Result<Self> {
        let directory = temp_dir()
            .join(format!("den-workers-{}", process::id()))
            .join(test);
        // A directory left by an aborted run would let a fixture that has since
        // been deleted keep answering.
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

    /// A fresh engine with `main` already run to completion as its entry point.
    ///
    /// Going through `run_file` rather than `eval` is the point: it is what
    /// gives the realm the fixture directory as its `BaseUrl`, so `./worker.js`
    /// in a main script means what a user expects. It doubles as the regression
    /// test for absolute entry points (`den /abs/path.js`), which every test
    /// here is, because `temp_dir` is nowhere near the working directory.
    async fn start(&self, main: &str) -> eyre::Result<Engine> {
        let entry = self.directory.join("main.js");
        fs::write(&entry, format!("{FIRST_MESSAGE}\n{main}"))?;
        let engine = Engine::new().await;
        timeout(DEADLINE, engine.run_file(entry)).await??;
        Ok(engine)
    }

    /// Run `main`, read back what it left in `globalThis.result`, and tear the
    /// realm down. The main script does the waiting, so a worker that never
    /// answers fails as a timeout on `run_file`.
    async fn result<T>(&self, main: &str) -> eyre::Result<T>
    where
        T: for<'js> FromJs<'js> + Send + Sync + 'static,
    {
        let engine = self.start(main).await?;
        let value = timeout(DEADLINE, engine.eval::<T>("globalThis.result")).await??;
        Self::finish(engine).await?;
        Ok(value)
    }

    /// Shut the realm down and prove nothing survived it: after `shutdown` the
    /// worker threads are joined and every parent-side pump they kept alive is
    /// gone, so the runtime must be able to reach idle.
    async fn finish(engine: Engine) -> eyre::Result<()> {
        timeout(DEADLINE, engine.shutdown()).await?;
        timeout(DEADLINE, engine.runtime.idle()).await?;
        Ok(())
    }
}

impl Drop for Fixture {
    /// The scripts are this test's litter, not the user's: without this every
    /// run leaves a tree under `temp_dir()` that is only ever cleaned up by a
    /// later run that happens to draw the same pid.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
        // The per-process parent goes with the last fixture in the run; while
        // other fixtures are still there it is not empty and this does nothing.
        if let Some(parent) = self.directory.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

/// How many worker OS threads named `name` this process still has.
///
/// `worker::shutdown` warns and *detaches* a thread it cannot join, so an empty
/// registry is not by itself proof that nothing is left running; the thread
/// names are. Every test that asserts on this names its workers, because the
/// whole test binary shares one process — and because Linux truncates
/// `/proc/<pid>/task/<tid>/comm` to 15 bytes, of which `den-worker:` already
/// takes 11, those names have to differ within their first four characters.
#[cfg(target_os = "linux")]
fn live_worker_threads(name: &str) -> usize {
    const COMM_LIMIT: usize = 15;
    let wanted: String = format!("den-worker:{name}")
        .chars()
        .take(COMM_LIMIT)
        .collect();
    fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|thread| {
            fs::read_to_string(thread.path().join("comm")).is_ok_and(|comm| comm.trim() == wanted)
        })
        .count()
}

/// Elsewhere there is no `/proc`, so the leak assertions degrade to nothing
/// rather than to a lie. `shutdown`'s own join is still exercised.
#[cfg(not(target_os = "linux"))]
fn live_worker_threads(_name: &str) -> usize { 0 }

/// Await every worker thread named `name` going away, bounded by [`DEADLINE`].
///
/// This is the exact assertion, and an immediate `live_worker_threads(…) == 0`
/// is not — even straight after a `shutdown` that has joined every thread.
/// `pthread_join` returns as soon as the kernel clears the child's
/// `CLONE_CHILD_CLEARTID` futex, which happens *before* the task is released
/// and its `/proc/self/task/<tid>` entry unlinked; measured here, the entry
/// outlives the join by well under a millisecond, about three runs in a
/// hundred. So `/proc` is a lagging observer of a fact that is already true,
/// and the honest way to read it is to wait for it to catch up.
///
/// A thread ending is not a future anyone can await, which is why this is the
/// one place that polls; it returns the instant the condition holds, so a
/// healthy run pays a single tick and a leak still fails.
async fn no_worker_threads(name: &str) {
    const TICK: Duration = Duration::from_millis(1);
    timeout(DEADLINE, async {
        while live_worker_threads(name) > 0 {
            tokio::time::sleep(TICK).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{} thread(s) of worker {name:?} outlived its terminate()",
            live_worker_threads(name)
        )
    });
}

const ECHO: &str = "self.onmessage = (event) => postMessage(event.data);\n";

/// Announces itself, then never yields again: the only way out is the interrupt
/// handler that `terminate()` and `shutdown()` trip.
const SPIN: &str = "postMessage(\"spinning\");\nfor (;;) {}\n";

#[tokio::test(flavor = "multi_thread")]
async fn a_classic_worker_echoes_a_message_through_the_engines_loaders() -> eyre::Result<()> {
    let fixture = Fixture::new("classic-echo", &[("worker.js", ECHO)])?;
    let echoed: String = fixture
        .result(include_str!("fixtures/workers/classic-echo/main.js"))
        .await?;
    eyre::ensure!(echoed == "ping", "expected echo \"ping\", got {echoed:?}");
    Ok(())
}

/// A module worker gets den's loader chain, so a static import of a sibling
/// file has to resolve against the *worker's* directory.
#[tokio::test(flavor = "multi_thread")]
async fn a_module_worker_resolves_a_static_import_of_a_sibling() -> eyre::Result<()> {
    let fixture = Fixture::new("module-import", &[
        ("lib.js", "export const double = (value) => value * 2;\n"),
        (
            "worker.js",
            include_str!("fixtures/workers/module-import/worker.js"),
        ),
    ])?;
    let doubled: i32 = fixture
        .result(include_str!("fixtures/workers/module-import/main.js"))
        .await?;
    eyre::ensure!(doubled == 42, "expected 42, got {doubled}");
    Ok(())
}

/// Both worker types go through the same transpiler the entry point does.
#[tokio::test(flavor = "multi_thread")]
async fn a_typescript_worker_is_transpiled_on_both_paths() -> eyre::Result<()> {
    const CLASSIC: &str = include_str!("fixtures/workers/typescript/classic.ts");
    const MODULE: &str = include_str!("fixtures/workers/typescript/module.ts");
    let fixture = Fixture::new("typescript", &[
        ("classic.ts", CLASSIC),
        ("module.ts", MODULE),
        (
            "lib.ts",
            "export const label = (value: number): string => `${value}`;\n",
        ),
    ])?;
    let both: String = fixture
        .result(include_str!("fixtures/workers/typescript/main.js"))
        .await?;
    eyre::ensure!(
        both == "classic:1,module:2",
        "unexpected transpiler result: {both:?}"
    );
    Ok(())
}

/// The structured clone checklist, now with a thread boundary between the write
/// and the read: the same table the unit tests use, a different pair of
/// runtimes.
#[tokio::test(flavor = "multi_thread")]
async fn every_structured_clone_type_survives_the_thread_boundary() -> eyre::Result<()> {
    const CHECKS: &str = include_str!("fixtures/workers/clone-table/main.js");
    let fixture = Fixture::new("clone-table", &[("worker.js", ECHO)])?;
    let report: String = fixture.result(CHECKS).await?;
    insta::assert_snapshot!(report);
    Ok(())
}

/// Transfer, not copy: the parent's buffer is detached before the worker ever
/// sees the bytes, and the bytes still arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_transferred_array_buffer_is_detached_here_and_intact_there() -> eyre::Result<()> {
    let fixture = Fixture::new("transfer-buffer", &[(
        "worker.js",
        include_str!("fixtures/workers/transfer-buffer/worker.js"),
    )])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/transfer-buffer/main.js"))
        .await?;
    eyre::ensure!(
        report == "true:0 -> 3:7-8-9",
        "unexpected transfer report: {report:?}"
    );
    Ok(())
}

/// The `[[ArrayBufferDetachKey]]` guard, on the only buffer in den that has
/// one: `WebAssembly.Memory#buffer`.
///
/// Transferring it would detach the wasm instance's linear memory — quickjs's
/// `JS_DetachArrayBuffer` runs the buffer's free hook and knows nothing about
/// detach keys — and the instance would go on running against freed pages.
/// The refusal has to be a `DataCloneError` *and* leave the memory untouched,
/// which is asserted by using it afterwards.
#[cfg(feature = "wasm")]
#[tokio::test(flavor = "multi_thread")]
async fn a_webassembly_memory_buffer_refuses_to_be_transferred() -> eyre::Result<()> {
    let fixture = Fixture::new("wasm-detach-key", &[("worker.js", ECHO)])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/wasm-detach-key/main.js"))
        .await?;
    eyre::ensure!(
        report == "DataCloneError|DataCloneError|detached:false|bytes:65536|readback:42",
        "unexpected wasm transfer report: {report:?}"
    );
    Ok(())
}

/// A `MessagePort` inside a message graph: the clone pre-pass turns it into a
/// placeholder, the far side revives it as the *same* wrapper it puts in
/// `event.ports`, and the channel then carries traffic both ways without the
/// worker's own port taking part.
#[tokio::test(flavor = "multi_thread")]
async fn a_transferred_message_port_carries_traffic_both_ways() -> eyre::Result<()> {
    let fixture = Fixture::new("transfer-port", &[(
        "worker.js",
        include_str!("fixtures/workers/transfer-port/worker.js"),
    )])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/transfer-port/main.js"))
        .await?;
    eyre::ensure!(
        report == "hello from the worker | ping/pong:true",
        "unexpected port report: {report:?}"
    );
    Ok(())
}

/// One post, two worker realms — and never the sender's own channel.
#[tokio::test(flavor = "multi_thread")]
async fn a_broadcast_reaches_both_workers_and_not_the_sender() -> eyre::Result<()> {
    const SUBSCRIBER: &str = include_str!("fixtures/workers/broadcast/worker.js");
    let fixture = Fixture::new("broadcast", &[("worker.js", SUBSCRIBER)])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/broadcast/main.js"))
        .await?;
    eyre::ensure!(
        report == "first:hello,second:hello self:false",
        "unexpected broadcast report: {report:?}"
    );
    Ok(())
}

/// The interrupt handler reached from another thread — and nothing else.
///
/// A worker that never yields cannot observe its port being closed, and
/// `shutdown` is deliberately kept out of it until the assertion is already
/// made: what stops this thread is `terminate()` cancelling the token the
/// worker's interrupt handler polls, or nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn terminate_stops_a_worker_that_never_yields() -> eyre::Result<()> {
    let fixture = Fixture::new("terminate", &[("worker.js", SPIN)])?;
    let engine = fixture
        .start(include_str!("fixtures/workers/terminate/main.js"))
        .await?;
    let result = engine.eval::<String>("globalThis.result").await?;
    eyre::ensure!(
        result == "spinning",
        "expected a spinning worker, got {result:?}"
    );
    #[cfg(target_os = "linux")]
    eyre::ensure!(
        live_worker_threads("spin-terminate") > 0,
        "the spinning worker should be running before it is terminated"
    );

    engine.eval::<()>("globalThis.worker.terminate();").await?;
    no_worker_threads("spin-terminate").await;
    Fixture::finish(engine).await?;
    Ok(())
}

/// `close()` inside the worker: the message it posted first still arrives, and
/// the thread ends without anyone terminating it.
#[tokio::test(flavor = "multi_thread")]
async fn close_from_inside_delivers_the_last_message_and_ends_the_thread() -> eyre::Result<()> {
    let fixture = Fixture::new("close-inside", &[(
        "worker.js",
        "postMessage(\"bye\");\nclose();\n",
    )])?;
    let engine = fixture
        .start(include_str!("fixtures/workers/close-inside/main.js"))
        .await?;
    let result = engine.eval::<String>("globalThis.result").await?;
    eyre::ensure!(
        result == "bye",
        "expected final message \"bye\", got {result:?}"
    );
    // Nobody called `terminate()`: the worker ended itself, so the realm can
    // reach idle on its own — which is what lets `den main.js` exit, and the
    // thread is gone before `shutdown` has anything to join.
    timeout(DEADLINE, engine.runtime.idle()).await?;
    no_worker_threads("close-inside").await;
    Fixture::finish(engine).await?;
    Ok(())
}

/// The error chain (HTML §10.2.5): an exception nobody in the worker claimed
/// becomes an `ErrorEvent` on the parent's `Worker`, with a usable location.
#[tokio::test(flavor = "multi_thread")]
async fn an_uncaught_worker_error_reaches_the_parent_with_its_location() -> eyre::Result<()> {
    // The throw is on line 3, which is what `lineno` has to say.
    const THROWS: &str =
        "postMessage(\"ready\");\nself.onmessage = () => {\n  throw new TypeError(\"boom\");\n};\n";
    let fixture = Fixture::new("error-event", &[("worker.js", THROWS)])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/error-event/main.js"))
        .await?;
    eyre::ensure!(
        report == "true,error,boom,true,3,true,true",
        "unexpected error report: {report:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typescript_worker_error_uses_the_authored_stack() -> eyre::Result<()> {
    const THROWS: &str = "type Marker = {\n  value: \
                          number;\n};\npostMessage('ready');\nself.onmessage = () => {\n  const \
                          marker: Marker = { value: 1 };\n  throw new TypeError(`mapped \
                          ${marker.value}`);\n};\n";
    const MAIN: &str = "const worker = new Worker('./worker.ts');\nawait \
                        firstMessage(worker);\nglobalThis.result = await new Promise((resolve) => \
                        {\n  worker.onerror = (event) => {\n    event.preventDefault();\n    \
                        resolve(`${event.lineno}|${event.error instanceof \
                        TypeError}|${event.error.stack.includes('worker.ts:7:')}`);\n  };\n  \
                        worker.postMessage('go');\n});\nworker.terminate();\n";
    let fixture = Fixture::new("mapped-worker-error", &[("worker.ts", THROWS)])?;
    let report: String = fixture.result(MAIN).await?;
    eyre::ensure!(
        report == "7|true|true",
        "unexpected mapped worker stack: {report:?}"
    );
    Ok(())
}

/// HTML §8.1.7.5, through the seam two halves of den meet at: den-core's
/// rejection tracker builds the event and dispatches it at the realm's global,
/// while the class and the `onunhandledrejection` slot come from
/// `den:worker`. A worker scope is where the whole chain is observable end to
/// end — and `preventDefault()` there is what stops the rejection reaching
/// stderr.
#[tokio::test(flavor = "multi_thread")]
async fn an_unhandled_rejection_in_a_worker_fires_at_its_global() -> eyre::Result<()> {
    const REJECTS: &str = include_str!("fixtures/workers/worker-rejection/worker.js");
    let fixture = Fixture::new("worker-rejection", &[("worker.js", REJECTS)])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/worker-rejection/main.js"))
        .await?;
    eyre::ensure!(
        report == "true,unhandledrejection,nobody in here either,true,true,object",
        "unexpected rejection report: {report:?}"
    );
    Ok(())
}

/// The other half of the chain: `self.onerror` returning `true` cancels the
/// event, so the parent never hears about it — and the worker carries on.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_onerror_returning_true_keeps_the_error_at_home() -> eyre::Result<()> {
    let fixture = Fixture::new("onerror-suppresses", &[(
        "worker.js",
        include_str!("fixtures/workers/onerror-suppresses/worker.js"),
    )])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/onerror-suppresses/main.js"))
        .await?;
    eyre::ensure!(
        report == "caught:mine:true:true caught:mine:true:true escaped:false",
        "unexpected onerror report: {report:?}"
    );
    Ok(())
}

/// A payload the far side cannot rebuild is a `messageerror`: not a thrown
/// exception, and not a silently lost message.
#[tokio::test(flavor = "multi_thread")]
async fn a_payload_the_worker_cannot_rebuild_becomes_messageerror() -> eyre::Result<()> {
    let fixture = Fixture::new("messageerror", &[(
        "worker.js",
        include_str!("fixtures/workers/messageerror/worker.js"),
    )])?;
    let report: String = fixture
        .result(include_str!("fixtures/workers/messageerror/main.js"))
        .await?;
    eyre::ensure!(
        report == "messageerror:null",
        "unexpected messageerror report: {report:?}"
    );
    Ok(())
}

/// A worker realm is a full realm: it has `den:worker` too, and a base URL of
/// its own, so `new Worker("./inner.js")` inside a worker means the *worker's*
/// directory.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_can_spawn_a_worker_relative_to_itself() -> eyre::Result<()> {
    let fixture = Fixture::new("nested", &[
        (
            "nested/inner.js",
            "postMessage(\"from the inner worker\");\n",
        ),
        (
            "nested/outer.js",
            include_str!("fixtures/workers/nested/nested/outer.js"),
        ),
    ])?;
    let relayed: String = fixture
        .result(include_str!("fixtures/workers/nested/main.js"))
        .await?;
    eyre::ensure!(
        relayed == "outer saw: from the inner worker",
        "unexpected nested worker report: {relayed:?}"
    );
    Ok(())
}

/// The process-lifetime rule, which is what makes `den main.js` wait for a
/// worker instead of exiting under it: a live worker keeps `idle()` — the very
/// future `App::run_until_end` awaits — pending, and `terminate()` releases it.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_worker_keeps_idle_pending_and_terminate_releases_it() -> eyre::Result<()> {
    let fixture = Fixture::new("idle-lifetime", &[("worker.js", ECHO)])?;
    let engine = fixture
        .start(include_str!("fixtures/workers/idle-lifetime/main.js"))
        .await?;
    let result = engine.eval::<String>("globalThis.result").await?;
    eyre::ensure!(result == "ping", "expected echo \"ping\", got {result:?}");
    #[cfg(target_os = "linux")]
    eyre::ensure!(
        live_worker_threads("idle-lifetime") > 0,
        "the worker whose lifetime is under test should be running"
    );

    // `idle()` holds the runtime lock while it is pending, so the timeout has
    // to drop it before anything else touches the realm (docs/research/09 §3).
    eyre::ensure!(
        timeout(STILL_BUSY, engine.runtime.idle()).await.is_err(),
        "a live worker must keep the runtime busy"
    );

    engine.eval::<()>("globalThis.worker.terminate()").await?;
    timeout(DEADLINE, engine.runtime.idle()).await?;

    // The thread check waits for `shutdown`'s join: `terminate()` only cancels
    // a token, and the parent's `idle()` is released by the worker dropping its
    // port, which happens before the OS thread has finished unwinding.
    Fixture::finish(engine).await?;
    no_worker_threads("idle-lifetime").await;
    Ok(())
}

/// Four workers, four OS threads: three of them never yield and the fourth
/// still answers. No wall clock is involved — a shared thread could not deliver
/// the echo at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_echo_arrives_while_three_workers_spin() -> eyre::Result<()> {
    let fixture = Fixture::new("parallel", &[("spin.js", SPIN), ("echo.js", ECHO)])?;
    let engine = fixture
        .start(include_str!("fixtures/workers/parallel/main.js"))
        .await?;
    let result = engine.eval::<String>("globalThis.result").await?;
    eyre::ensure!(
        result == "still responsive",
        "expected the echo worker to stay responsive, got {result:?}"
    );

    // Nobody terminated the spinners: only `shutdown` can reach them, and when
    // it returns it has joined every thread each of them had.
    Fixture::finish(engine).await?;
    no_worker_threads("parallel-echo").await;
    Ok(())
}

/// The embedder's teardown path: `shutdown` is what reaches the worker
/// threads — including one that is parked with nothing to interrupt, which
/// only a join can reclaim.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_terminates_and_joins_every_worker() -> eyre::Result<()> {
    let fixture = Fixture::new("shutdown", &[("spin.js", SPIN), ("echo.js", ECHO)])?;
    let engine = fixture
        .start(include_str!("fixtures/workers/shutdown/main.js"))
        .await?;
    let result = engine.eval::<String>("globalThis.result").await?;
    eyre::ensure!(
        result == "spinning",
        "expected a spinning worker, got {result:?}"
    );
    #[cfg(target_os = "linux")]
    eyre::ensure!(
        live_worker_threads("shutdown-join") > 0,
        "the workers did not run"
    );

    timeout(DEADLINE, engine.shutdown()).await?;
    no_worker_threads("shutdown-join").await;
    Ok(())
}

/// Two things that are only observable on stderr, so they are checked in a
/// child process: an error no `onerror` claimed, and a promise nobody handled.
///
/// libtest's output capture is per-test-thread and both reports come from
/// somewhere else — a worker thread, and the runtime's rejection tracker — so
/// re-running one test with its stderr piped is the only honest way to read
/// them back.
mod stderr {
    use std::process::Command;

    use super::*;

    /// Tells the re-executed test binary to be the child rather than fork
    /// another one.
    const CHILD: &str = "DEN_WORKERS_STDERR_CHILD";

    const UNCLAIMED: &str =
        "workers::stderr::an_unclaimed_worker_error_is_reported_and_leaves_the_parent_running";
    const REJECTION: &str =
        "workers::stderr::an_unhandled_rejection_in_the_main_script_is_reported";

    /// Re-run `test` in a child process and give back everything it printed to
    /// stderr.
    fn stderr_of(test: &str) -> eyre::Result<String> {
        let output = Command::new(std::env::current_exe()?)
            .args(["--exact", "--nocapture", "--test-threads=1", test])
            .env(CHILD, "1")
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        eyre::ensure!(output.status.success(), "the child test failed: {stderr}");
        Ok(stderr)
    }

    fn is_child() -> bool { std::env::var_os(CHILD).is_some() }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unclaimed_worker_error_is_reported_and_leaves_the_parent_running()
    -> eyre::Result<()> {
        if !is_child() {
            let reported = stderr_of(UNCLAIMED)?;
            eyre::ensure!(
                reported.contains("unclaimed worker failure"),
                "the worker's error never reached stderr: {reported}"
            );
            return Ok(());
        }

        let fixture = Fixture::new("unclaimed-error", &[(
            "worker.js",
            include_str!("fixtures/workers/unclaimed-error/worker.js"),
        )])?;
        // No `onerror` on either side, and deliberately not `firstMessage`
        // either: its `error` listener would claim the event and there would be
        // nothing left to report. The default action is what has to run.
        let alive: String = fixture
            .result(include_str!("fixtures/workers/unclaimed-error/main.js"))
            .await?;
        eyre::ensure!(
            alive == "alive",
            "expected parent to stay alive, got {alive:?}"
        );
        Ok(())
    }

    /// Regression test for the runtime-wide rejection tracker `Engine`
    /// installs: a rejection the main script never handles is reported
    /// rather than swallowed.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unhandled_rejection_in_the_main_script_is_reported() -> eyre::Result<()> {
        if !is_child() {
            let reported = stderr_of(REJECTION)?;
            eyre::ensure!(
                reported.contains("Uncaught (in promise)")
                    && reported.contains("nobody awaited this"),
                "the rejection never reached stderr: {reported}"
            );
            return Ok(());
        }

        let engine = Engine::new().await;
        engine
            .eval::<()>(r#"Promise.reject(new Error("nobody awaited this"));"#)
            .await?;
        Fixture::finish(engine).await
    }
}
