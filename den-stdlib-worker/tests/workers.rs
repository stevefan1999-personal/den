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
const FIRST_MESSAGE: &str = r#"
  const firstMessage = (target) => new Promise((resolve, reject) => {
    target.addEventListener("message", (event) => resolve(event.data), { once: true });
    target.addEventListener("error", (event) => {
      event.preventDefault();
      reject(new Error(`${event.message} (${event.filename}:${event.lineno})`));
    }, { once: true });
  });
"#;

/// One test's scripts on disk. `Worker` takes a URL, so the integration layer
/// needs real files; they are regenerated from the constants next to each test
/// the way `webassembly.rs` assembles its WAT rather than committing binaries.
struct Fixture {
    directory: PathBuf,
}

impl Fixture {
    /// Write `files` into a directory of this test's own — the process id keeps
    /// two concurrent `cargo nextest run` runs apart, the test name keeps the
    /// tests within one run apart.
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
        timeout(DEADLINE, engine.run_file::<()>(entry)).await??;
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
        .result(
            r#"
            const worker = new Worker("./worker.js");
            worker.postMessage("ping");
            globalThis.result = await firstMessage(worker);
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(echoed, "ping");
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
            r#"
                import { double } from "./lib.js";
                self.onmessage = (event) => postMessage(double(event.data));
                "#,
        ),
    ])?;
    let doubled: i32 = fixture
        .result(
            r#"
            const worker = new Worker("./worker.js", { type: "module" });
            worker.postMessage(21);
            globalThis.result = await firstMessage(worker);
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(doubled, 42);
    Ok(())
}

/// Both worker types go through the same transpiler the entry point does.
#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn a_typescript_worker_is_transpiled_on_both_paths() -> eyre::Result<()> {
    const CLASSIC: &str = r#"
        type Ping = { value: number };
        self.onmessage = (event: MessageEvent) => {
          const ping = event.data as Ping;
          postMessage(`classic:${ping.value}`);
        };
    "#;
    const MODULE: &str = r#"
        import { label } from "./lib.ts";
        enum Kind { Module = "module" }
        self.onmessage = (event: MessageEvent): void => {
          postMessage(`${Kind.Module}:${label(event.data.value)}`);
        };
    "#;
    let fixture = Fixture::new("typescript", &[
        ("classic.ts", CLASSIC),
        ("module.ts", MODULE),
        (
            "lib.ts",
            "export const label = (value: number): string => `${value}`;\n",
        ),
    ])?;
    let both: String = fixture
        .result(
            r#"
            const classic = new Worker("./classic.ts");
            const asModule = new Worker("./module.ts", { type: "module" });
            classic.postMessage({ value: 1 });
            asModule.postMessage({ value: 2 });
            globalThis.result = [
              await firstMessage(classic),
              await firstMessage(asModule),
            ].join(",");
            classic.terminate();
            asModule.terminate();
            "#,
        )
        .await?;
    assert_eq!(both, "classic:1,module:2");
    Ok(())
}

/// The structured clone checklist, now with a thread boundary between the write
/// and the read: the same table the unit tests use, a different pair of
/// runtimes.
#[tokio::test(flavor = "multi_thread")]
async fn every_structured_clone_type_survives_the_thread_boundary() -> eyre::Result<()> {
    const CHECKS: &str = r#"
        const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
        const sent = {
          primitives: [undefined, null, true, -0, NaN, Infinity, 1.5, "text"],
          big: 9007199254740993n,
          date: new Date(86400000),
          regexp: /ab+c/gi,
          map: new Map([["key", { nested: 1 }]]),
          set: new Set([1, "two"]),
          error: new TypeError("typed"),
          domException: new DOMException("denied", "NotAllowedError"),
          buffer,
          view: new Uint16Array(buffer, 2, 1),
          dataView: new DataView(buffer, 1, 2),
          holes: [1, , 3],
          nested: { deep: { deeper: [1, [2, [3]]] } },
        };
        // A cycle: the writer's reference table has to survive the trip.
        sent.self = sent;

        const worker = new Worker("./worker.js");
        worker.postMessage(sent);
        const back = await firstMessage(worker);
        worker.terminate();

        globalThis.result = [
          `primitives:${back.primitives.map((value) => `${value}`).join("|")}`,
          `negativeZero:${Object.is(back.primitives[3], -0)}`,
          `big:${back.big}:${typeof back.big}`,
          `date:${back.date instanceof Date}:${back.date.getTime()}`,
          `regexp:${back.regexp.source}:${back.regexp.flags}:${back.regexp.lastIndex}`,
          `map:${back.map instanceof Map}:${back.map.get("key").nested}`,
          `set:${back.set instanceof Set}:${[...back.set].join("|")}`,
          `error:${back.error instanceof TypeError}:${back.error.message}`,
          `domException:${back.domException.name}:${back.domException.message}`,
          `buffer:${new Uint8Array(back.buffer).join("|")}`,
          `view:${back.view instanceof Uint16Array}:${back.view.byteOffset}:${back.view.length}`,
          `dataView:${back.dataView.byteOffset}:${back.dataView.byteLength}`,
          // v1 divergence (docs/research/10 §4.5): a hole arrives as undefined,
          // i.e. as a present property.
          `holes:${back.holes.length}:${back.holes[1]}:${1 in back.holes}`,
          `nested:${back.nested.deep.deeper[1][1][0]}`,
          `cycle:${back.self === back}`,
          // Aliasing inside one message is preserved; the buffer was cloned
          // rather than transferred, so this side still owns its own.
          `aliased:${back.view.buffer === back.buffer}`,
          `detached:${buffer.detached}`,
        ].join("\n");
    "#;
    let fixture = Fixture::new("clone-table", &[("worker.js", ECHO)])?;
    let report: String = fixture.result(CHECKS).await?;
    assert_eq!(
        report,
        [
            "primitives:undefined|null|true|0|NaN|Infinity|1.5|text",
            "negativeZero:true",
            "big:9007199254740993:bigint",
            "date:true:86400000",
            "regexp:ab+c:gi:0",
            "map:true:1",
            "set:true:1|two",
            "error:true:typed",
            "domException:NotAllowedError:denied",
            "buffer:1|2|3|4",
            "view:true:2:1",
            "dataView:1:2",
            "holes:3:undefined:true",
            "nested:3",
            "cycle:true",
            "aliased:true",
            "detached:false",
        ]
        .join("\n")
    );
    Ok(())
}

/// Transfer, not copy: the parent's buffer is detached before the worker ever
/// sees the bytes, and the bytes still arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_transferred_array_buffer_is_detached_here_and_intact_there() -> eyre::Result<()> {
    let fixture = Fixture::new("transfer-buffer", &[(
        "worker.js",
        r#"
            self.onmessage = (event) => {
              const bytes = new Uint8Array(event.data);
              postMessage(`${event.data.byteLength}:${bytes.join("-")}`);
            };
            "#,
    )])?;
    let report: String = fixture
        .result(
            r#"
            const buffer = new Uint8Array([7, 8, 9]).buffer;
            const worker = new Worker("./worker.js");
            worker.postMessage(buffer, [buffer]);
            const here = `${buffer.detached}:${buffer.byteLength}`;
            globalThis.result = `${here} -> ${await firstMessage(worker)}`;
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(report, "true:0 -> 3:7-8-9");
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
        .result(
            r#"
            const memory = new WebAssembly.Memory({ initial: 1 });
            const buffer = memory.buffer;
            const worker = new Worker("./worker.js");
            const refuse = (attempt) => {
              try { attempt(); return "no throw"; }
              catch (error) {
                return error instanceof DOMException ? error.name : `wrong: ${error}`;
              }
            };
            const posted = refuse(() => worker.postMessage(buffer, [buffer]));
            const cloned = refuse(() => structuredClone(buffer, { transfer: [buffer] }));

            // The memory is still there and still writable: nothing was
            // detached and nothing was freed.
            const bytes = new Uint8Array(memory.buffer);
            bytes[0] = 42;
            globalThis.result = [
              posted,
              cloned,
              `detached:${buffer.detached}`,
              `bytes:${memory.buffer.byteLength}`,
              `readback:${new Uint8Array(memory.buffer)[0]}`,
            ].join("|");
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(
        report,
        "DataCloneError|DataCloneError|detached:false|bytes:65536|readback:42"
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
        r#"
            self.onmessage = (event) => {
              const port = event.data.port;
              const same = port === event.ports[0];
              port.onmessage = (message) => port.postMessage(`${message.data}/pong:${same}`);
              port.postMessage("hello from the worker");
            };
            "#,
    )])?;
    let report: String = fixture
        .result(
            r#"
            const channel = new MessageChannel();
            const worker = new Worker("./worker.js");
            // `addEventListener` does not enable a port's queue (HTML §9.4.4):
            // only `onmessage` or an explicit `start()` does.
            channel.port1.start();
            const greeting = firstMessage(channel.port1);
            worker.postMessage({ port: channel.port2 }, [channel.port2]);
            const first = await greeting;
            const reply = firstMessage(channel.port1);
            channel.port1.postMessage("ping");
            globalThis.result = `${first} | ${await reply}`;
            channel.port1.close();
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(report, "hello from the worker | ping/pong:true");
    Ok(())
}

/// One post, two worker realms — and never the sender's own channel.
#[tokio::test(flavor = "multi_thread")]
async fn a_broadcast_reaches_both_workers_and_not_the_sender() -> eyre::Result<()> {
    const SUBSCRIBER: &str = r#"
        const channel = new BroadcastChannel("integration");
        channel.onmessage = (event) => {
          postMessage(`${name}:${event.data}`);
          channel.close();
        };
        // Only now may the parent broadcast: a channel that is not yet
        // constructed is not yet subscribed, and the fan-out has no backlog.
        postMessage("ready");
    "#;
    let fixture = Fixture::new("broadcast", &[("worker.js", SUBSCRIBER)])?;
    let report: String = fixture
        .result(
            r#"
            const workers = ["first", "second"].map((name) => new Worker("./worker.js", { name }));
            await Promise.all(workers.map(firstMessage));

            const mine = new BroadcastChannel("integration");
            let heardMyself = false;
            mine.onmessage = () => { heardMyself = true; };

            const echoes = Promise.all(workers.map(firstMessage));
            mine.postMessage("hello");
            const heard = (await echoes).sort().join(",");

            mine.close();
            for (const worker of workers) worker.terminate();
            globalThis.result = `${heard} self:${heardMyself}`;
            "#,
        )
        .await?;
    assert_eq!(report, "first:hello,second:hello self:false");
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
        .start(
            r#"
            globalThis.worker = new Worker("./worker.js", { name: "spin-terminate" });
            globalThis.result = await firstMessage(worker);
            "#,
        )
        .await?;
    assert_eq!(
        engine.eval::<String>("globalThis.result").await?,
        "spinning"
    );
    assert!(
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
        .start(
            r#"
            const worker = new Worker("./worker.js", { name: "close-inside" });
            globalThis.result = await firstMessage(worker);
            "#,
        )
        .await?;
    assert_eq!(engine.eval::<String>("globalThis.result").await?, "bye");
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
        .result(
            r#"
            const worker = new Worker("./worker.js");
            await firstMessage(worker);
            const failure = new Promise((resolve) => {
              worker.onerror = (event) => {
                event.preventDefault();
                resolve([
                  event instanceof ErrorEvent,
                  event.type,
                  event.message,
                  event.filename.endsWith("/worker.js"),
                  event.lineno,
                  // v1 divergence (docs/research/08 §1.4): an Error does not
                  // serialise, so only the location crosses the thread.
                  typeof event.error,
                ].join(","));
              };
            });
            worker.postMessage("go");
            globalThis.result = await failure;
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(report, "true,error,boom,true,3,undefined");
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
    const REJECTS: &str = r#"
    self.onunhandledrejection = (event) => {
      event.preventDefault();
      postMessage([
        event instanceof PromiseRejectionEvent,
        event.type,
        event.reason.message,
        // The runtime fired it, so DOM says trusted; a script cannot forge one.
        event.isTrusted,
        event.cancelable,
        typeof event.promise,
      ].join(","));
    };
    Promise.reject(new Error("nobody in here either"));
    "#;
    let fixture = Fixture::new("worker-rejection", &[("worker.js", REJECTS)])?;
    let report: String = fixture
        .result(
            r#"
            const worker = new Worker("./worker.js");
            globalThis.result = await firstMessage(worker);
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(
        report,
        "true,unhandledrejection,nobody in here either,true,true,object"
    );
    Ok(())
}

/// The other half of the chain: `self.onerror` returning `true` cancels the
/// event, so the parent never hears about it — and the worker carries on.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_onerror_returning_true_keeps_the_error_at_home() -> eyre::Result<()> {
    let fixture = Fixture::new("onerror-suppresses", &[(
        "worker.js",
        r#"
            self.onerror = (message, filename, lineno, colno, error) => {
              postMessage(`caught:${message}:${lineno > 0}:${error === undefined}`);
              return true;
            };
            self.onmessage = () => { throw new RangeError("mine"); };
            postMessage("ready");
            "#,
    )])?;
    let report: String = fixture
        .result(
            r#"
            const worker = new Worker("./worker.js");
            await firstMessage(worker);
            let escaped = false;
            worker.onerror = () => { escaped = true; };
            worker.postMessage("go");
            const caught = await firstMessage(worker);
            // Still alive after handling its own error.
            worker.postMessage("again");
            const again = await firstMessage(worker);
            globalThis.result = `${caught} ${again} escaped:${escaped}`;
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(
        report,
        "caught:mine:true:true caught:mine:true:true escaped:false"
    );
    Ok(())
}

/// A payload the far side cannot rebuild is a `messageerror`: not a thrown
/// exception, and not a silently lost message.
#[tokio::test(flavor = "multi_thread")]
async fn a_payload_the_worker_cannot_rebuild_becomes_messageerror() -> eyre::Result<()> {
    let fixture = Fixture::new("messageerror", &[(
        "worker.js",
        r#"
            self.onmessageerror = (event) => postMessage(`${event.type}:${event.data}`);
            self.onmessage = () => postMessage("message");
            "#,
    )])?;
    let report: String = fixture
        .result(
            r#"
            const worker = new Worker("./worker.js");
            // A clone tag whose revival throws on the far side: a DataView
            // cannot be built past the end of its buffer.
            worker.postMessage({
              ["\u0000den:structured-clone"]: "DataView",
              buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
            });
            globalThis.result = await firstMessage(worker);
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(report, "messageerror:null");
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
            r#"
                const inner = new Worker("./inner.js");
                inner.onmessage = (event) => {
                  postMessage(`outer saw: ${event.data}`);
                  inner.terminate();
                };
                inner.onerror = (event) => postMessage(`outer failed: ${event.message}`);
                "#,
        ),
    ])?;
    let relayed: String = fixture
        .result(
            r#"
            const worker = new Worker("./nested/outer.js");
            globalThis.result = await firstMessage(worker);
            worker.terminate();
            "#,
        )
        .await?;
    assert_eq!(relayed, "outer saw: from the inner worker");
    Ok(())
}

/// The process-lifetime rule, which is what makes `den main.js` wait for a
/// worker instead of exiting under it: a live worker keeps `idle()` — the very
/// future `App::run_until_end` awaits — pending, and `terminate()` releases it.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_worker_keeps_idle_pending_and_terminate_releases_it() -> eyre::Result<()> {
    let fixture = Fixture::new("idle-lifetime", &[("worker.js", ECHO)])?;
    let engine = fixture
        .start(
            r#"
            globalThis.worker = new Worker("./worker.js", { name: "idle-lifetime" });
            globalThis.worker.postMessage("ping");
            globalThis.result = await firstMessage(globalThis.worker);
            "#,
        )
        .await?;
    assert_eq!(engine.eval::<String>("globalThis.result").await?, "ping");
    assert!(
        live_worker_threads("idle-lifetime") > 0,
        "the worker whose lifetime is under test should be running"
    );

    // `idle()` holds the runtime lock while it is pending, so the timeout has
    // to drop it before anything else touches the realm (docs/research/09 §3).
    assert!(
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
        .start(
            r#"
            globalThis.spinners = [0, 1, 2]
              .map(() => new Worker("./spin.js", { name: "parallel-echo" }));
            // Each spinner announces itself before entering its loop, so all
            // three are provably running before the echo is asked for.
            await Promise.all(spinners.map(firstMessage));

            const echo = new Worker("./echo.js");
            echo.postMessage("still responsive");
            globalThis.result = await firstMessage(echo);
            echo.terminate();
            "#,
        )
        .await?;
    assert_eq!(
        engine.eval::<String>("globalThis.result").await?,
        "still responsive"
    );
    // One OS thread each, plus each worker's own single-threaded tokio runtime:
    // three spinners cannot be fewer than three threads however they are
    // scheduled. The exact count is the runtime's business, the floor is not.
    assert!(
        live_worker_threads("parallel-echo") >= 3,
        "three spinning workers should hold at least three threads, got {}",
        live_worker_threads("parallel-echo")
    );

    // Nobody terminated the spinners: only `shutdown` can reach them, and when
    // it returns it has joined every thread each of them had.
    Fixture::finish(engine).await?;
    no_worker_threads("parallel-echo").await;
    Ok(())
}

/// Ctrl-C's path: the token cancels the main realm, and `shutdown` is what
/// reaches the worker threads — including one that is parked with nothing to
/// interrupt, which only a join can reclaim.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_terminates_and_joins_workers_after_the_token_is_cancelled() -> eyre::Result<()> {
    let fixture = Fixture::new("shutdown", &[("spin.js", SPIN), ("echo.js", ECHO)])?;
    let engine = fixture
        .start(
            r#"
            const spinner = new Worker("./spin.js", { name: "shutdown-join" });
            // A parked worker: started, idle, waiting for a message that never
            // comes. It has no interrupt to observe, so only the join ends it.
            const parked = new Worker("./echo.js", { name: "shutdown-join" });
            parked.onmessage = () => {};
            globalThis.result = await firstMessage(spinner);
            "#,
        )
        .await?;
    assert_eq!(
        engine.eval::<String>("globalThis.result").await?,
        "spinning"
    );
    assert!(live_worker_threads("shutdown-join") > 0, "the workers ran");

    engine.stop();
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
        "stderr::an_unclaimed_worker_error_is_reported_and_leaves_the_parent_running";
    const REJECTION: &str = "stderr::an_unhandled_rejection_in_the_main_script_is_reported";

    /// Re-run `test` in a child process and give back everything it printed to
    /// stderr.
    fn stderr_of(test: &str) -> eyre::Result<String> {
        let output = Command::new(std::env::current_exe()?)
            .args(["--exact", "--nocapture", "--test-threads=1", test])
            .env(CHILD, "1")
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "the child test failed: {stderr}");
        Ok(stderr)
    }

    fn is_child() -> bool { std::env::var_os(CHILD).is_some() }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unclaimed_worker_error_is_reported_and_leaves_the_parent_running()
    -> eyre::Result<()> {
        if !is_child() {
            let reported = stderr_of(UNCLAIMED)?;
            assert!(
                reported.contains("unclaimed worker failure"),
                "the worker's error never reached stderr: {reported}"
            );
            return Ok(());
        }

        let fixture = Fixture::new("unclaimed-error", &[(
            "worker.js",
            r#"
                self.onmessage = (event) => {
                  if (event.data === "throw") throw new Error("unclaimed worker failure");
                  postMessage("alive");
                };
                "#,
        )])?;
        // No `onerror` on either side, and deliberately not `firstMessage`
        // either: its `error` listener would claim the event and there would be
        // nothing left to report. The default action is what has to run.
        let alive: String = fixture
            .result(
                r#"
                const worker = new Worker("./worker.js");
                const alive = new Promise((resolve) => {
                  worker.onmessage = (event) => resolve(event.data);
                });
                worker.postMessage("throw");
                worker.postMessage("ping");
                globalThis.result = await alive;
                worker.terminate();
                "#,
            )
            .await?;
        assert_eq!(alive, "alive");
        Ok(())
    }

    /// Regression test for the runtime-wide rejection tracker `Engine`
    /// installs: a rejection the main script never handles is reported
    /// rather than swallowed.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unhandled_rejection_in_the_main_script_is_reported() -> eyre::Result<()> {
        if !is_child() {
            let reported = stderr_of(REJECTION)?;
            assert!(
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
