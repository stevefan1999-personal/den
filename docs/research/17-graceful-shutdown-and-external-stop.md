# Graceful shutdown and external stop — kill, interrupt, drain, and why none of them needs a token on `Engine`

Status: research, 2026-08-27. Snapshot of the den working tree (branch `master`, after `0c6ce89`, with the
uncommitted stdlib-test refactor), rquickjs-core 0.12.2 (`full-async, rust-alloc, parallel`), tokio 1.53.1,
tokio-util 0.7.19, tempfile 3.27.0, rusqlite 0.39.0 / libsqlite3-sys 0.37.0 (bundled sqlite 3.51.3), surrealkv 0.21.4,
axum 0.8.9, hyper-util 0.1.20, wasmtime 48.0.0, rustc 1.98.0, Node v26.5.0, Deno 2.9.4, Bun 1.3.9, Linux x86_64.
**Not a living document.** Every claim carries a `file:line` into the working tree or a vendored source, or a line
quoted verbatim from a probe run. Nothing is from memory. For the current truth read
[ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.

Builds on [16](16-cancellation-without-tokens.md), which is the settled baseline and is not re-derived here: no
`Engine` stop token; Ctrl-C with no JS listener is kernel signal death (Node/Deno/Bun parity); with a listener the
signal is a mailbox event drained by the root `select!(recv, idle())`; embedders cancel by `drop(engine)`; the
per-worker token stays for `terminate()`; dropping an rquickjs runtime with pending `ctx.spawn` futures is sound
(16 §3).

The question, in the user's words: *"What if I also want external stopping of a Den instance? Like I want Ctrl-C
bound to stop Den, then we can gracefully shut down all resources and say goodbye to the world — that is why I had
a world-end token before, but I think it is over-engineered. We still need something that does not corrupt, say a
file buffer."* Three needs: (1) **embedder stop** from another task or thread, including a script in a tight loop;
(2) **graceful Ctrl-C**, opt-in, with in-flight work, "goodbye" hooks, a deadline and a second-Ctrl-C escape;
(3) **no corruption** on abrupt death, so graceful is a nicety and never a correctness requirement.

## Sources read

| What | Path |
|---|---|
| den (working tree) | `den-core/src/engine.rs`, `src/{app,main,repl,history}.rs`, `den-stdlib-fs/src/lib.rs`, `den-stdlib-sqlite/src/lib.rs`, `den-stdlib-console/src/lib.rs`, `den-stdlib-networking/src/{io,socket,websocket,tls}.rs`, `den-stdlib-process/src/{signal,lib,spawn}.rs`, `den-stdlib-whatwg/src/compression.rs`, `den-stdlib-wasm/src/backend.rs`, `den-core/src/loader/*.rs`, `ARCHITECTURE.md` §2 |
| rquickjs-core 0.12.2 | `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/src/` (`core/` below); rquickjs-sys `quickjs/quickjs.h` |
| tokio 1.53.1 / tokio-util 0.7.19 / tempfile 3.27.0 / axum 0.8.9 / hyper-util 0.1.20 / wasmtime 48.0.0 / rusqlite 0.39.0 / libsqlite3-sys 0.37.0 / surrealkv 0.21.4 | same registry root (`tokio/`, `tutil/`, `tempfile/`, `axum/`, `hyper-util/`, `wasmtime/`, `rusqlite/`, `sqlite3.c`, `surrealkv/`) |
| Rust std 1.98 | `$(rustc --print sysroot)/lib/rustlib/src/rust/library/std/src/` (`std/`) |
| Deno 2.9.4 source | the files fetched for 16, flat under `/tmp/parity/scratch/cancel/U4-deno/src/` |
| Node docs | `/tmp/parity/scratch/graceful/U2-node/docs/*.md` (main branch) |
| Bun types + docs | `/tmp/parity/bt/node_modules/bun-types/{serve,bun,s3}.d.ts`, `docs/` |
| Probes | `/tmp/parity/scratch/graceful/{u1-writepaths,U2-node,U2-deno,U2-bun,U2-harness,out,U3,angle-a,design,judge}/` — table at the end |

`/tmp` on this machine is tmpfs (`df -T /tmp` → `tmpfs`), so every byte count below is page-cache truth, not
fsync truth; the sqlite/surrealkv fsync calls are no-ops there, which is irrelevant for process-death probes.

---

## 0. TL;DR — the facts an implementer must not get wrong

1. **"Stop" is three different operations owned by three different layers.** *Kill* is the kernel's
   (`SIG_DFL`, 16 §1.3). *Interrupt* of a tight bytecode loop is the QuickJS interrupt flag's
   (`core/runtime/async.rs:185` `set_interrupt_handler`; polled every 10 000 back-edges, `quickjs.c:479, 8215-8240`;
   there is no `JS_TerminateExecution`, `quickjs.h` grep). *Drain* — stop accepting, finish in-flight, close — is
   the owner of `idle()`'s: it dispatches one event into JS and waits with a deadline. None of the three is a
   token on `Engine`.
2. **Embedder stop is the host's flag plus the host's `select!` plus `drop`.** The flag goes into the interrupt
   handler *before* the first run (installing takes the runtime mutex, `async.rs:187-189`; 16 R3). Probed on den's
   pins: a tight loop ended 1.5 ms after `cancel()` and the runtime dropped in 130 µs — `E1 program arm: err=true
   uncatchable=true message=Some("interrupted") at 301.504313ms` / `E1 dropped context+runtime in 130.198µs`
   (`angle-a/embed.log`); an entry module parked on a top-level await was dropped mid-`async_with` with a 60 s op
   pending — `E2 dropped context+runtime with the 60 s op pending in 183.075µs`, exit 0 on both runtime flavours.
   That is `axum::serve(..).with_graceful_shutdown(until)` (`axum/src/serve/mod.rs:151-153, 284-291`) with no
   wrapper; the library owns no token (`hyper-util/src/server/graceful.rs:21-23` is a bare `watch::Sender<()>`).
3. **The program arm may win with `Err(interrupted)`** (E1 above). A host that maps every `Err` to "script failed"
   misreports its own stop: guard the arm with the host's own flag (`if !*stop.borrow() { result? }`, §4.1).
4. **Flip the flag from a thread the JS loop does not block.** On a `current_thread` runtime the cancel task never
   runs: `[E1 current_thread exit=124 (124 = timeout: cancel task never ran)]`. Multi-thread runtime task or
   `std::thread`.
5. **A goodbye `eval` under the same flag dies at its first interrupt poll.** `E5 goodbye outcome=js error
   Exception generated by QuickJS marker=closing at 101.55921ms`. A host that wants a goodbye must give the
   interrupter its own flag (E6: `marker=closed at 214.288512ms`; E7 with a runaway goodbye: `interrupted` at
   402.788759ms). den does not ship that recipe (no second caller); it ships the sentence.
6. **Graceful Ctrl-C is the JS signal listener, nothing else.** Node, Deno and Bun converge on one idiom: a listener
   cancels the default action, then the listener owns termination and calls `exit()` ("If one of these signals has a
   listener installed, its default behavior will be removed (Node.js will no longer exit)", `process.md:669-672`;
   `os-signals.mdx:19-22` "listen for that signal and call `process.exit()`"; U2 b2/b4 all `code:0`). Exit hooks
   (`exit`, `beforeExit`, `unload`) cannot complete **I/O** on any of the three — Node does resume after an `await`
   inside `exit` (`exit after microtask await`, U2 b3) where Deno and Bun do not, but the post-await marker write
   lands nowhere (`marker_written_after_await:false` ×3) and no timer continuation ever runs — so no hook can close
   a socket. den's listener may be async and keeps den alive while its work is pending (16 R1 `late=1`), which is
   strictly more than the hooks give.
7. **Second Ctrl-C: the listener removes itself first.** All three reference runtimes simply re-run the handler on
   SIGINT #2 (U2 b5 `SIGINT #1 / SIGINT #2 / graceful done -> exit(0)` at ~3.1 s, `code:0`) and all three die of the
   second signal if the listener was removed, even mid-loop (16 row n' `rmspin.{node,deno,bun}` `code:130,
   signal:SIGINT`). den's 16-commit-6 `remove` restores `SIG_DFL` at removal; mechanism probed: `S4 child:
   signal(SIGINT, SIG_DFL) -> previous handler was tokio's` / `S4 child status: code=None signal=Some(2)`
   (`design/all.log`). Self-removal inside the handler is safe because `dispatch` clones the list before calling
   (`den-stdlib-process/src/signal.rs:217-218`). A deadline is `setTimeout(() => exit(130), GRACE_MS)` in the
   listener — no runtime grace timer, none of the three has one.
8. **den holds no user-space file buffer.** `grep -rn BufWriter den-stdlib-networking/src den-stdlib-process/src
   den-stdlib-fs/src src` → nothing; `den:fs` has no open-file handle and no `append` (`den-stdlib-fs/src/lib.rs`).
   Once `write(2)` returned the bytes are the kernel's and survive process death (`man 2 write` NOTES; U1 P4: every
   logged event present after SIGTERM, `physicalLines:16656 physicalPerEvent:3 endsWithNewline:true`). There is
   nothing for a shutdown hook to flush.
9. **Three den write paths tear; exactly one gets a fix.** They are `den:fs write` (`lib.rs:221-225`), `den:fs copy`
   (`:120-124`) and `den:assert assertSnapshot` (`den-stdlib-assert/src/lib.rs:669` → `:225`/`:231`); only `write`
   gets the opt-in below. `copy` is `std::fs::copy` = `copy_file_range(2)`, not read-then-write, so it cannot compose
   from the helper (§7 q10); `assertSnapshot` is test-only (§7 q11). For those two the guarantee is coupled to
   shutdown: do not call them while stopping. The `write` tear itself: `tokio::fs::write` =
   `asyncify(std::fs::write)` (`tokio/src/fs/write.rs:96-99`) = `File::create(path)?.write_all(contents)`
   (`std/fs.rs:420-425`), truncate-then-write. U1 P1, 6 of 6 runs `torn:true`, three of them 0 bytes:
   `{"label":"P1.delay1ms",…,"sizeAfterDeath":0,"firstByte":"-","torn":true}`. Node/Deno/Bun tear identically
   (`P1ref.node.delay2ms … sizeAfterDeath:1323008`, `P1ref.bun.delay0ms … 0`). Fix: opt-in
   `write(path, bytes, { atomic: true })` = `NamedTempFile::new_in(parent)` + `write_all` + `persist` (= `rename(2)`,
   `tempfile/src/file/imp/unix.rs:94-96`; `man 2 rename`: "atomically replaced"). Default stays torn for parity and
   because rename changes inode/hard-link/mode semantics.
10. **sqlite, REPL history, stdout and sockets need nothing.** sqlite: DELETE journal + `synchronous=FULL` defaults
    (`#define PAGER_JOURNALMODE_DELETE 0` `sqlite3.c:16463` plus the zeroed `Pager` — *not* `:62283`, which is a
    WAL→DELETE downgrade inside `sqlite3PagerSetJournalMode`; `:18060`), zero-magic journal ignored (`:60411`, `:64104-64122`), P2
    `sqlite3:["ok","0","delete"]` mid-txn and `["ok","5008","delete"]` autocommit, no `Drop` ran. History:
    `Durability::Immediate` fsync per entry (`src/history.rs:119,159,167`), P3 `recalled via Up-arrow: … const
    p3_marker_2018095 = 42` after SIGTERM. stdout: `LineWriter`, event-atomic (P4). Sockets: the peer sees FIN/RST,
    as with a dead Node process; JS already has `flush()`/`shutdown()` (`den-stdlib-networking/src/io.rs:78-83`,
    `:85-91`).
11. **`kill_on_drop(true)` stays** (`den-stdlib-process/src/spawn.rs:111`). `drop(engine)` SIGKILLs children,
    signal death leaves them to the kernel. Leaving a sandboxed script's children running after an embedder's drop
    is the opposite of what need (1) asks; decided, documented, not probed further.
12. **wasm host calls ignore the QuickJS flag.** `{"label":"wasm_spin","sigint_at_ms":1002,"alive_after_ms":4000,
    "watchdogged":true,"code":137}` vs `{"label":"js_spin",…,"ended_after_ms":1018,"code":0}` (`U3/probe.log`).
    `den-stdlib-wasm/src/backend.rs:161-190` sets no `epoch_interruption`/`consume_fuel`. Three lines fix it when a
    sandbox asks (wasmtime `config.rs:784-787`, `store.rs:1084-1086`, `engine.rs:857-859`); not now.

---

## 1. The mental model (teaching section)

### 1.1 Three things people mean by "stop", and who owns each

**Kill.** The process ends now; no code of ours runs. Owner: the kernel. Ctrl-C with no listener is this (16 §1.3,
fact 8 of 16: `SIGINT sigaction lines: 0`; SIGTERM proxy `a: exit=143 after_signal_ms=2`). SIGKILL and power loss are
this too, and they are why §1.5 exists: correctness cannot depend on anything that runs at stop time.

**Interrupt.** A thread is inside `JS_CallInternal` and never reaches an `.await`; the only thing that reaches it is
a flag polled by the bytecode loop, which raises an uncatchable `InternalError("interrupted")` (16 §1.4;
`core/runtime.rs:51-52` `Box<dyn FnMut() -> bool + Send + 'static>`). Owner: whoever installed the handler — the
worker for `terminate()` (16 §4.5), or the embedder for need (1) (§4.1 below). It is a `bool`, nothing awaits it,
and it must be installed before `idle()` takes the mutex (`async.rs:187-189`; 16 R3). Same shape one layer down:
wasmtime's epoch is "a counter … on the `Engine` … compiled code … checks the current epoch against that deadline"
(`wasmtime/src/config.rs:675-683`), incremented by an atomic (`engine.rs:857-859`), "signal-safe" (`:852-854`).

**Drain.** Stop accepting, finish what is in flight, close, then end. Owner: the owner of `idle()`. This is the only
one of the three that is *cooperative*, and cooperation means one thing: an event reaches JS, JS does its closes,
JS calls `exit()`. In den that event is the signal mailbox (16 §4.1 case 2) for the CLI, and for an embedder it is
`engine.eval("await globalThis.onShutdown?.()")` under `tokio::time::timeout` (E3/E4). Drain is never "make every
future stop": a `fetch` that is mid-response is exactly the in-flight work drain is supposed to *finish*.

The world-end token conflated all three: it was the interrupt flag (`engine.rs:376-381`), the drain trigger
(`app.rs:88-96` → `idle()` returns → `shutdown()`), and a substitute for kill (timers selecting on it so `idle()`
could return, `timer/lib.rs:92,118`). Separated, each has a smaller, pre-existing owner.

### 1.2 Why a token is the wrong tool for drain

Drain is a property of **resources**, not of **futures**. "Stop accepting" is `listener` not being polled again;
"finish in-flight" is the handler promise resolving; "close" is `conn.shutdown()`, `db.close()`. Each is a method on
an object JS already holds. A token threaded through every stdlib future can only do the opposite — abort the
in-flight work — which is kill with extra steps, and it costs every async op a `select!` (16 fact 2: the ones that
did not take it hung Ctrl-C; the ones that did were cancelled instead of drained).

The owner of the loop can drain with two primitives it already has: dispatch one event, then wait with a deadline.
For the CLI the dispatch is a signal listener call (`SignalHub::deliver`, 16 §4.1) and the wait is `idle()` re-entered
— the listener's own async work keeps den alive until it finishes (16 R1 `idle() returned at 751.474462ms:
hits=2 fired=1 late=1`), and its own `setTimeout(() => exit(130), GRACE_MS)` is the deadline. For an embedder the
dispatch is `engine.eval(..)` and the wait is `tokio::time::timeout(grace, ..)` (`tokio/src/time/timeout.rs:86`) — Deno's
`run_up_to_duration` is literally that (`runtime_worker.rs:959-975`, `tokio::time::timeout(duration,
self.js_runtime.run_event_loop(..))`; not `cli_worker.rs`, which is 824 lines and has no such symbol). Neither needs
a type.

### 1.3 The axum lesson: take a future, own no token

`axum::serve(..).with_graceful_shutdown(signal)` takes `F: Future<Output = ()> + Send + 'static`
(`axum/src/serve/mod.rs:151-153`) and stores nothing but that future (`:155-159`). Inside, it is converted to a
`watch` channel close at the composition edge (`:275-279`), the accept loop is a root `select!` (`:284-291`), and
"drained" means every per-connection receiver dropped (`:296-303` `close_tx.closed().await`). hyper-util's whole
`GracefulShutdown` type is `{ tx: watch::Sender<()> }` (`graceful.rs:21-23`) whose `shutdown(self)` is
`send(()); tx.closed().await` (`:63-70`). tokio-util's canonical example creates the token in `main`
(`task_tracker.rs:110-111`), and the library function `background_task` takes **no token** (`:99-104`) — the root
wraps it (`:118-119`). The idiom is: the root owns the token, the root owns the select, the root waits. A token
field on the library's main type inverts it, which is what `Engine.stop_token` (`engine.rs:122`) did.

den's translation: `Engine.runtime` is public (`engine.rs:120`), `AsyncRuntime::idle` is the drain (`async.rs:313`),
dropping it mid-await is sound (16 §3.2), so the host writes `tokio::select! { _ = program => .., _ = until => .. };
drop(engine)` and den adds nothing — not even a `run_until(until)` wrapper: the entry module is itself a future the
host must put in the select (a top-level `while(true){}` never returns from `run_file`, E8 `program arm won:
Err(…) at 401.588987ms`), so a wrapper around `run_event_loop` alone would leave it uncovered.

### 1.4 No exit hook can complete I/O anywhere; what den can do better, and why that is still not a token

Node: `process.on('exit')` "Listener functions **must** only perform **synchronous** operations" (`process.md:128`);
`beforeExit` can schedule more loop work but is "_not_ emitted for … `process.exit()`" (`:35,41`). Deno: `unload`
runs on `Deno.exit()` but `beforeunload` does not, and no async continuation after an `await` in `unload` runs,
"not even the microtask one" (U2 b3). Bun: `beforeExit` ×2 then `exit sync`, no microtask continuation (b3).

The three are *not* symmetric, and the doc lines overstate the rule: Node does resume after an `await` inside
`process.on('exit')` — U2 b3 `node.natural` prints `exit after microtask await` (`b3_hooks.mjs:12-13`
`await Promise.resolve()`), which Deno and Bun never print. What holds on all three is the weaker, load-bearing
statement: **no hook can complete I/O.** The post-`await` marker write lands nowhere
(`marker_written_after_await:false` ×3) and no timer continuation scheduled from a hook ever runs. So a hook still
cannot `await conn.shutdown()` — the only place that can is the signal listener, which is why all three
documentations tell you to put cleanup there (fact 6).

den already does better without a new hook name: 16 commit 6 calls listeners synchronously (`signal.rs:223`
`func.call::<_, ()>(())`) but an async listener's spawned work pins `idle()` (16 §1.1) until it resolves. That is a
"shutdown event whose listeners may extend with promises, bounded by the deadline" — the extension is the spawned
work, the bound is the listener's own timer. It is not a token because nothing polls anything: the loop simply has
not drained yet. Adding `waitUntil` or a `shutdown` event would be a second name for what `addSignalListener` with
an `async` function already is; rejected (§6).

### 1.5 Correctness versus politeness

SIGKILL, OOM-kill and power loss run no hook, so anything that is only correct after a hook is a bug waiting for
`kill -9`. The write API has to be safe **by itself**; graceful stop can only add politeness (a clean FIN, a WAL not
needing replay, a "goodbye" line).

Three den paths are not safe by themselves (fact 9); the only one that gets a fix here is whole-file write, because
it is the only one whose shape admits it. The helper, in the same blocking shape
`tokio::fs::write` already uses (`tokio/src/fs/write.rs:96-99`):

```rust
// den-stdlib-fs/src/lib.rs — inside write(), when options.atomic == true
tokio::task::spawn_blocking(move || {
  let parent = Path::new(&path).parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
  let mut tmp = NamedTempFile::new_in(parent)?;      // tempfile/src/file/mod.rs:621 — same filesystem, so persist is one rename(2)
  tmp.write_all(&contents)?;
  tmp.persist(&path).map_err(|e| e.error)           // tempfile/src/file/mod.rs:204 → imp/unix.rs:94-96 rename
}).await?
```

The target is either the complete old file or the complete new file, never a prefix. No fsync: process death is
covered by the page cache; fsync buys power-loss durability only and nobody asked (`sync: true` before `persist` is
the one-line addition when they do). Explicit `close()` is the other half of the rule: resources whose *protocol*
needs an orderly end — `conn.shutdown()` (`io.rs:85-91`; `:84` is `flush`'s closing brace), `db.close()` (`den-stdlib-sqlite/src/lib.rs:71-80`),
`history.close()` (`src/repl.rs:65-67`) — are called by JS or the REPL in the graceful path and are *optional* for
correctness: P2 reopened clean with no `close()`, P3 replayed the WAL with a stale `LOCK`.

Already crash-safe by construction (probed §3): `den:sqlite` (journal + `synchronous=FULL`), REPL history
(SurrealKV `Immediate` fsync per commit), `console.*` (`LineWriter`, one `write_all` per event), every other `den:fs`
op (single kernel-atomic syscalls). Needs the helper — and gets it — `den:fs write`, alone. Two other paths tear and
get nothing: `den:fs copy`, because `std::fs::copy` is `copy_file_range(2)` and not read-then-write, so the helper
does not compose into it; and `den:assert assertSnapshot`, because it is a test-only API (§7 q10-q11). For those two
the only mitigation is not calling them while stopping — the one place in this note where a corruption guarantee is
coupled to graceful shutdown rather than independent of it.

### 1.6 The second-Ctrl-C rule

First Ctrl-C with a listener: mailbox event, listener runs, den stays alive while its async work is pending. Second
Ctrl-C: *by default* another mailbox event — the listener runs again, which is exactly what Node, Deno and Bun do
(U2 b5) and what 16 row e chose. To make the second press an escape, the listener **removes itself before its first
`await`**; `SignalHub::remove` (16 commit 6) restores `SIG_DFL` at removal (Node/Bun mechanism, 16 §1.5), so the next
SIGINT is kernel death even if JS is by then in a tight loop where no listener could run (16 §4.1 case 3'; S4). A
counter guard (`if (closing) exit(130)`) cannot do that — it needs the loop to be free — so it is only the fallback
where `SIG_DFL` does not exist (Windows, 16 §7 q1). Fact 8 makes the escape safe: nothing den owns tears.

---

## 2. What Node/Deno/Bun offer and do (U2)

Harness: `/tmp/parity/scratch/graceful/U2-harness/{h.ts,run.ts,one.ts}` (`Deno.Command`, `clearEnv` + proxy vars
deleted, `code` and `signal` reported separately per 16 fact 12); results `all.log`, `b7.log`, `extra.log`;
artifacts `out/`.

### 2.1 APIs

| Runtime | Graceful primitives | Semantics (doc line) |
|---|---|---|
| Node 26.5.0 | `process.on('SIGINT')`; `process.on('exit'/'beforeExit')`; `process.exit`/`exitCode`; `http.Server.close([cb])`, `closeAllConnections()`, `closeIdleConnections()`; `AbortSignal` in fs/net/timers (16 `signal` options in `fs.md`); `util.aborted`; `worker.terminate()`; `fs.fsync/fdatasync` | listener → "If one of these signals has a listener installed, its default behavior will be removed (Node.js will no longer exit)" `process.md:669-672` (`:654-656` is only the CJS `process.on('SIGINT', …)` code sample); `exit` listeners "must only perform synchronous operations" `:128`; `beforeExit` not on `exit()` `:41`; `server.close` "closes all connections … not sending a request or waiting for a response" `http.md:1771`; `closeAllConnections` "forceful … including active" `:1801`; "Signals are not available on `Worker` threads" `process.md:623` |
| Deno 2.9.4 | `Deno.addSignalListener/removeSignalListener` (`deno.d.ts:3948,3966`); `unload`/`beforeunload` (`:17794,17802`); `Deno.serve({signal})` + `HttpServer.shutdown()/finished/unref()` (`:5749,5857,5876-5879`); `Deno.exit`/`exitCode` (`:1753,1770`); `FsFile.sync()/syncData()` (`:2322,2358`) | `shutdown()` "Gracefully close … pending requests will be allowed to finish" `:5876-5879`; `unload` "primarily for API compatibility with browsers" `:17802` |
| Bun 1.3.9 | `process.on('SIGINT'/'beforeExit'/'exit')`; `server.stop(closeActiveConnections?)` → Promise (`serve.d.ts:909-923`), `unref()` `:1150`, `pendingRequests` `:1155`; `Bun.FileSink` `write/flush/end` (`s3.d.ts:16,22,29`), `writer({highWaterMark})` | "Neither event is emitted when the process is killed by a signal it has no listener for. To run cleanup on a signal, listen for that signal and call `process.exit()`" `os-signals.mdx:19-22`; `ctrl-c.mdx:7` "you must explicitly call `process.exit()`"; FileSink "buffers these chunks internally" `file-io.mdx:176`, auto-flush at hwm `:182` |

### 2.2 Probes (`all.log` / `b7.log` / `extra.log`; exit as `code`/`signal`)

| # | Scenario | Node | Deno | Bun |
|---|---|---|---|---|
| b1 | 200 000 × 20 B buffered writes, self-SIGINT right after the sync loop | `bytes_on_disk:-1` — file never created (`writableLength=4000000 bytesWritten=0`) | `bytes_on_disk:0,bytes_lost:4000000` | `bytes_on_disk:3997500,bytes_lost:2500` (sub-hwm tail) |
| b1 | external SIGINT at ready+0 / +20 / +200 ms | 0 / **race** / 4 000 000 — only `+0 ms` is stable: `+20 ms` was `0` in the saved run and `bytes_on_disk:4000000, bytes_lost:0` on rerun | 0 / 131 040 / 3 996 720 (`bytes_lost:3280` even at +200 ms) | 4 000 000 at all three |
| b2 | SIGINT handler → `end`/`close` → `exit(0)` | `bytes_lost:0`, `code:0` | `bytes_lost:0` | `bytes_lost:0` |
| b7 | `exit(0)` with writes buffered | `-1` (never created) | `bytes_lost:4000000` | `bytes_lost:2500` |
| b7 | natural end, no close | `bytes_lost:0` | **`bytes_lost:3280` with `resolved=200000 rejected=0`** — a resolved `writer.write()` does not mean bytes reached the fd (`extra.log:1-2, 8-10`) | `bytes_lost:0`, exited in 3 ms |
| b3 | exit hooks, natural end (`exitCode=3`) | `beforeExit` → timer runs → `beforeExit` → `exit sync code=3` → **`exit after microtask await`** — Node *does* run the microtask continuation inside `process.on('exit')` (`b3_hooks.mjs:12-13` `await Promise.resolve()`), unlike the other two; the timer-await line still never runs and `marker_written_after_await:false` | `beforeunload cancelable=true` → `unload sync`; no microtask continuation | `beforeExit` ×2 → `exit sync code=3`; no continuation |
| b3 | `exit(4)` | only `exit sync code=4` | only `unload sync` | only `exit sync code=4` |
| b3 | SIGINT, no listener | `code:130,signal:SIGINT`, no hook, 2 ms | same, 2 ms | same, 1 ms |
| b4 | HTTP, 3 s handler in flight, SIGINT +300 ms, graceful call | `server.close()`: `closed after 2711ms`, curl `slow-done 200`, idle keep-alive closed +3 ms | `shutdown()`: `finished after 2705ms`, `slow-done 200`, idle +3 ms | `stop(false)`: `stopped after 2737ms`, `slow-done 200`, idle keep-alive still open until `idle_closed_ms_after_signal:2786` (`all.log:39-40`) — contradicts `serve.d.ts:914-915`; a rerun reproduced the shape (`stopped after 2700ms`, idle `2705`) |
| b4 | forceful | `closeAllConnections()`: `closed after 2ms`, curl `52` | `ac.abort()`: `finished after 1ms`, curl `52` | `stop(true)`: curl `52`, promise resolved only at `2696`/`2702ms` (`all.log:41-42`) — the "306 ms" curl timing of the first draft is in no saved log and is dropped |
| b5 | SIGINT #1 starts a 3 s graceful shutdown, SIGINT #2 at +500 ms | `SIGINT #1` / `SIGINT #2` / `graceful done -> exit(0)` at 3142 ms, `code:0` — no built-in force-exit | identical, 3118 ms | identical, 3122 ms |
| b6 | sqlite, 200 000 inserts in one open tx, self-SIGINT | hot `-journal` left, reopen `ok \| 0` | `ok \| 0` | `ok \| 0` |
| b6 | 1000-row commits in a loop, external SIGINT +400 ms | `ok \| 919000` (whole batches) | `ok \| 487000` | `ok \| 1027000` |
| b6 | same, WAL | `ok \| 966000` | `ok \| 475000` | `ok \| 1016000` |

Summary: nothing is graceful by default; the opt-in is a signal listener that then owns termination; no exit hook
completes I/O; graceful server close means finish in-flight and close idle; a second Ctrl-C just re-runs the handler;
on abrupt death each runtime loses exactly its user-space buffers while SQLite's journal made every reopen
`integrity_check ok` — durability came from the storage layer, never from the runtime.

---

## 3. den's write paths under abrupt death (U1)

Binary `target/debug/den` (matches the working tree: `find src den-* -name '*.rs' -newer target/debug/den` → empty).
SIGTERM is the proxy for `SIG_DFL` death (16 fact 8). Logs under `/tmp/parity/scratch/graceful/u1-writepaths/`.

| Path | Bytes go to | User-space buffer lost | On-disk TORN | Crash-safe by design |
|---|---|---|---|---|
| `den:fs write` `lib.rs:221-225` → `tokio::fs::write` → `File::create` (truncate, `std/fs.rs:638-640`) + `write_all` | regular file | none (one `write(2)`) | **YES** — 0 bytes or a prefix (P1) | **no**; opt-in `{atomic:true}` (§1.5, commit 9) |
| `den:fs copy` `lib.rs:120-124` → `tokio::fs::copy` = `std::fs::copy` = `copy_file_range(2)`/`sendfile(2)` on Linux — *not* read+write, so it cannot reuse the atomic-write helper | regular file | none | **YES** — destination truncated then filled | **no**, and no opt-in (§7 q10) |
| `den:fs rename/createDir(All)/hardLink/removeFile/removeDir(All)/setPermissions` `:126-139,171-212` | metadata | none | no — single kernel-atomic syscalls | yes (kernel) |
| `den:assert assertSnapshot` `den-stdlib-assert/src/lib.rs:669` → `:188` `assert_insta_snapshot` → `:225` `fs::create_dir_all` + `:231` `fs::write` — blocking `std::fs` on the JS thread, not `tokio::fs` | `./snapshots/<name>.snap(.new)`, relative to the CWD | none | **YES** — same `File::create` + `write_all` tear class as `den:fs write` | **no**; test-only API, deliberately unfixed (§7 q11) |
| `den:sqlite` `lib.rs:38-44` `Connection::open` default flags, `:71-80` `close` (`Drop` also closes, `rusqlite/src/inner_connection.rs:404-409`; no `Drop` on signal death) | `.db` + `-journal` | sqlite page cache of the open txn — by design never in the file before commit | no (P2 `integrity_check` = `ok` both ways) | **yes**: DELETE is journal mode 0 and the `Pager` is zeroed — `#define PAGER_JOURNALMODE_DELETE 0` (`sqlite3.c:16463`; `:62283` is a WAL→DELETE downgrade inside `sqlite3PagerSetJournalMode`, not the default), `SQLITE_DEFAULT_SYNCHRONOUS 2` = FULL (`:18060`); journal magic zeroed until commit-time `syncJournal` (`:60405-60411`, `:63205`, `:63250`); zero-magic journal "not hot" (`:64104-64122`) |
| REPL history `src/history.rs:117-128` per-entry txn `Durability::Immediate` (`:119,159`) via `block_in_place` (`:166-168`); `close()` `:171-176` | SurrealKV `wal/`, `manifest/`, `sstables/`, `LOCK` | none: `Immediate` → `should_sync` (`surrealkv/src/transaction.rs:875`) → WAL append + fsync (`lsm.rs:943-944` `if sync { wal_guard.sync()?; }`; there is no vlog on disk — P3's `history.surrealkv/` holds only `LOCK`, `manifest/`, `sstables/`, `wal/`, and the marker lands in `wal/00000000000000000000.wal`) | no (P3) | **yes**: WAL replayed on open (`lsm.rs:25,656,1046`); `LOCK` is an `fs2` flock the kernel releases (`lockfile.rs:79`); `Tree::close` only stops background tasks (`lsm.rs:1355-1378`) |
| `console.*` `den-stdlib-console/src/lib.rs:228-231` → `tracing` → `io::stdout` (`fmt_layer.rs:749`), one `write_all` per event (`:1050`) → `LineWriter` (`std/io/stdio.rs:613,645`; `linewritershim.rs:261-268`) | stdout fd | only a trailing partial line; den emits none | no — event-atomic (P4) | yes for a drained fd (16 fact 9) |
| TCP/Unix/TLS `io.rs:69-91` (`flush` `:78-83`, `shutdown` `:85-91`; `tokio/src/net/tcp/stream.rs:1486-1493`; tokio-native-tls `tls.rs:9`) | kernel socket → peer | bytes not yet accepted by `write(2)`, any TLS record OpenSSL holds | n/a — external; peer sees FIN/RST | n/a; JS has `flush()`/`shutdown()` |
| WebSocket `websocket.rs:236-246, 392-400` (channel + `Handle::spawn` pumps) | peer | queued frames | n/a | n/a |
| `CompressionStream` `compression.rs:45-47, 86-` | JS memory | all (memory) | none durable | n/a |
| `den:process spawn` `spawn.rs:197-199` (stdin dropped at spawn), `:111` `kill_on_drop(true)` | child process | none | n/a | signal death: kernel reparents; `drop(engine)`: SIGKILL (fact 11) |
| workers (`worker.rs:868` is the only `std::fs` use, a read), module loader (`grep 'cache\|write\|std::fs\|tokio::fs' den-core/src/loader/*.rs` → none) | nothing | — | — | — |

Scope of the inventory: every JS-reachable byte-emitting path in the default-feature binary, found by reading each
crate. It is *checked*, not complete by construction — the `den:assert` row was missed by the first pass and added
only after a second sweep, so a new stdlib crate can add a row without anything here failing.

Probes, verbatim:

**P1** — `den:fs write` 64 MiB over a pre-filled 64 MiB file, SIGTERM once the truncate is observed (+delay):
```
{"label":"P1.delay0ms","built":"built 1481ms","truncSeenAfterMs":6813,"sizeAtTrunc":0,"code":143,"signal":"SIGTERM","sizeAfterDeath":7356416,"firstByte":"A","torn":true}
{"label":"P1.delay1ms","built":"built 1389ms","truncSeenAfterMs":6818,"sizeAtTrunc":0,"code":143,"signal":"SIGTERM","sizeAfterDeath":0,"firstByte":"-","torn":true}
{"label":"P1.delay8ms","built":"built 2452ms","truncSeenAfterMs":7735,"sizeAtTrunc":0,"code":143,"signal":"SIGTERM","sizeAfterDeath":29196288,"firstByte":"A","torn":true}
```
(6 of 6 torn; the ~7 s before the truncate is the `Vec<u8>` conversion of a 67 M-element JS array — §7 q4.)
**P1ref** — same shape on `Deno.writeFile` / `fs/promises.writeFile` / `Bun.write`: all torn identically
(`P1ref.deno.delay2ms … sizeAfterDeath:0`, `P1ref.node.delay2ms … 1323008`, `P1ref.bun.delay0ms … 0`).

**P2** — `den:sqlite` 10 000 inserts, SIGTERM after `row 5000`:
```
{"label":"P2.txn","txn":true,"code":143,"signal":"SIGTERM","lastRowLogged":"row 5000","filesAfterDeath":{"p2_txn.db":8192,"p2_txn.db-journal":8720},"sqlite3":["ok","0","delete"],"filesAfterReopen":["p2_txn.db","p2_txn.db-journal"]}
{"label":"P2.autocommit","txn":false,"code":143,"signal":"SIGTERM","lastRowLogged":"row 5000","filesAfterDeath":{"p2_autocommit.db":90112},"sqlite3":["ok","5008","delete"],"filesAfterReopen":["p2_autocommit.db"]}
```
`xxd p2_txn.db-journal | head -1` → `00000000: 0000 0000 0000 0000 0000 0000 c718 d500` (zero magic, ignored).

**P3** — REPL under a pty (`script(1)`), one line typed, `kill -TERM`, second REPL presses Up:
```
P3 first REPL wait status=143 (script(1) forwards child status)
P3 lock file content: 3804192
P3 recalled via Up-arrow: [?2004h[?2026h[K> [2C[?2026l[?2026h[K> const p3_marker_2018095 = 42[30C[?2026l[?2004l
P3 reopened cleanly (no fallback message): 0 occurrences of fallback
```

**P4** — 20 000 `console.log` events to a file, SIGTERM mid-loop / after loop:
```
{"label":"P4.midloop","code":143,"signal":"SIGTERM","bytes":1081530,"events":5552,"lastEvent":5551,"contiguous":true,"physicalLines":16656,"physicalPerEvent":3,"endsWithNewline":true,"allLogged":false}
{"label":"P4.afterloop","code":143,"signal":"SIGTERM","bytes":3909086,"events":20000,"lastEvent":19999,"contiguous":true,"physicalLines":60003,"physicalPerEvent":3,"endsWithNewline":true,"allLogged":true}
```

### 3.1 The minimal guarantee

Rule: whatever returned from `write(2)` before the kernel killed the process is in the page cache and survives
(`man 2 write` NOTES: "A successful return from write() does not make any guarantee that data has been committed to
disk … The only way to be sure is to call fsync(2)" — that is a power-loss caveat, not a process-death one). Graceful
stop adds no durability in den; it only lets a script finish protocol-level goodbyes.

1. `den:fs write`: add `{ atomic: true }` (§1.5 helper), default unchanged.
2. `den:fs copy` (`lib.rs:120-124`) stays torn with **no** atomic option: it is `std::fs::copy` (`copy_file_range(2)`),
   not read-then-write, so it cannot reuse the helper inside Rust, and the JS composition `read` +
   `write({atomic:true})` is impractical until commit 15 (§7 q4: `write` takes only a JS number array, `written
   10464ms` for 64 MiB). `den:assert assertSnapshot` likewise stays torn, as test-only (§7 q11).
3. No `sync`/fsync option until asked.
4. `den:sqlite`, REPL history, stdout/stderr, sockets, compression, workers, loader: nothing.
5. Child processes: `kill_on_drop` stays (fact 11).

Net: for `den:fs write` abrupt death == graceful stop once the caller opts into `{atomic:true}`, which makes
untearability a property of the write call and not of shutdown; for every other path in the table but two it is
equal unconditionally. Those two are `copy` and `assertSnapshot`: they have no in-den remedy, so their only
mitigation is not calling them while stopping — for those two, and only those two, the corruption guarantee *is*
coupled to shutdown (§7 q10-q11).

---

## 4. The chosen design

Winner: "Root-owned stop" (judge 43 vs 40) with five grafts from the axum-shape design (§6). Both designs share the
skeleton — 16 plus a host flag in `set_interrupt_handler` plus `select!` plus `drop`; the JS signal listener as the
only graceful hook; `write(path, bytes, {atomic:true})`. They differed on the second-Ctrl-C recipe, the goodbye
recipe, `kill_on_drop`, and test count; the winner stops at the first rung that holds.

### 4.1 Engine API — nothing added

After 16 §5 commits 1-6, every line below already exists; this note adds a rustdoc comment and no method. The
sketch does **not** depend on 16 commit 6 moving `SignalHub::install` to `Engine::build`, and that premise is wrong
anyway: `engine.rs:439` `evaluate_stdlib_module!(den_stdlib_process::js_process, "den:process")` expands (`:408-411`)
to `Module::evaluate_def(ctx.clone(), "den:process")`, which runs the `#[qjs(evaluate)]` hook
(`den-stdlib-process/src/lib.rs:196-198`) → `crate::install` (`:133`) → `SignalHub::install` (`:134`) at build time,
in **every** realm, whether or not any script imports `den:process`. Probed on the working-tree binary with a script
that imports nothing: `target/debug/den g1.js` → `typeof process =, object` / `has addSignalListener =, function`.
So 16 §4.1 case 4 and commit 6 must drop "a take before the entry module therefore sees no hub" (16's own commit 6b
already states the truth, `engine.rs:439` → `lib.rs:134`); 16's R9 measured the bare rquickjs probe crate, not den.
What commit 6 still has to add is `inbox_tx`/`inbox`, and it can add them inside the existing `install` — the move
itself is a no-op. Correction carried by commit 14.

```rust
// den-core/src/engine.rs
#[derive(Clone)]
pub struct Engine { pub runtime: AsyncRuntime, pub context: AsyncContext }        // :119-121 minus stop_token :122
impl Engine {
  pub async fn new() -> Engine;                                                    // :153
  pub async fn new_with_bundle(bundle: Bundle) -> Engine;                          // :162
  pub async fn run_file(&self, filename: PathBuf) -> Result<(), EngineError>;       // :485; run_module under deliver_while (16 commit 6)
  pub async fn run_module(&self, specifier: &str) -> Result<(), EngineError>;      // :518
  pub async fn eval<U: for<'js> FromJs<'js> + Send + Sync + 'static>(&self, src: &str) -> Result<U, EngineError>; // :580 (:578 is eval_prepared's closing brace)
  pub async fn run_event_loop(&self);                                              // 16 commit 6: SignalHub::drive, else runtime.idle()
  pub async fn shutdown(&self);                                                    // :596 after 16 commit 5: bounded worker join + gc; no cancel
}
// rquickjs-core, the whole embedder kill switch:
//   AsyncRuntime::set_interrupt_handler(&self, Option<Box<dyn FnMut() -> bool + Send + 'static>>)   // core/runtime/async.rs:185
//   AsyncRuntime::idle(&self)                                                                      // :313
// drop(engine) == cancel this realm (16 §3.1); WorkerHandle::drop cancels children (16 commit 4); kill_on_drop SIGKILLs spawned processes
```

Embedder recipe (the rustdoc on `Engine`, commit 10):

```rust
let (stop, mut stopped) = tokio::sync::watch::channel(false);          // host-owned, and literally hyper-util's whole GracefulShutdown (graceful.rs:21-23);
                                                                       // NOT tokio_util::CancellationToken — 16 commit 5 deletes tokio-util from
                                                                       // den-core/Cargo.toml:33 and it is not a dev-dependency, so a doctest would not resolve it
let engine = Engine::new().await;
engine.runtime.set_interrupt_handler(Some(Box::new({                   // BEFORE any JS runs: takes the mutex idle() will hold (async.rs:187-189; 16 R3)
  let flag = stop.subscribe(); move || *flag.borrow()                  // polled every 10 000 back-edges; Arc<AtomicBool> + any awaitable works too
}))).await;
let program = async {                                                  // entry module + event loop are ONE future the host owns
  engine.run_file(entry).await?;                                       // tight loop in the entry: Err(interrupted) ~1.5 ms after cancel (E1)
  engine.run_event_loop().await;                                       // parked fetch/accept: the arm below drops it (E2)
  Ok::<_, EngineError>(())
};
tokio::select! {                                                       // == axum with_graceful_shutdown (serve/mod.rs:284-291)
  result = program => if !*stop.borrow() { result? },                  // the program arm can win with Err(interrupted) (E1) — not a script failure
  _ = stopped.changed() => {}                                          // losing arm dropped mid-await (16 §3.2; E2 for an in-flight async_with)
}
drop(engine);                                                          // the cancel: 130 µs (E1), 183 µs with a 60 s op pending (E2)
```

Rules carried by the same rustdoc, each with its probe line:
- Install the handler before the first run (16 R3).
- Flip the flag from a multi-thread runtime task or a `std::thread` — `[E1 current_thread exit=124 (cancel task
  never ran)]`.
- Once the flag is true every later eval in that runtime dies (16 §3.3 probe D); the engine is single-use after a
  hard stop.
- A goodbye `eval` under the same flag dies at its first interrupt poll (`E5 … js error … marker=closing at
  101.55921ms`); give the interrupter its own flag if you want one. Recipe not shipped.
- `Engine` derives `Clone` (`engine.rs:118`), so `drop(engine)` is the cancel only when it drops the **last** clone —
  and never move a clone into a `ctx.spawn` future: runtime → spawner → future → Engine → runtime is a cycle drop
  cannot break. The tree's only clone is `src/app.rs:45` (moved into `ctx.spawn` at `:49`, held across the pump's
  loop at `:61-63`), which is saved only by `process::exit`. Cheap to state, easy to violate.
- Deadline variant: `let _ = tokio::time::timeout(grace, engine.run_event_loop()).await; stop.send_replace(true); drop(engine);`
  (Deno `run_up_to_duration`).

CLI composition root (after 16 commits 1-2): no select, no token, no `ctrl_c()`, no `--grace`:
`match app.engine.run_file(x).await { Err(e) => { print; std::process::exit(1) } _ => {} }` then `run_event_loop().await`.

### 4.2 Ctrl-C: default and opt-in

**Default (no JS SIGINT listener).** den installs nothing; Ctrl-C is `SIG_DFL` death of every thread, no Rust runs,
`status.signal() == Some(SIGINT)` (16 §4.1 case 1). Nothing to flush (fact 8); nothing torn except one of fact 9's
three write paths caught mid-write, which no hook could have saved either (P1). Parity: U2 b3 `code:130,signal:SIGINT`, no hook,
1-2 ms.

**Opt-in graceful = `process.addSignalListener("SIGINT", fn)`** (`den-stdlib-process/src/lib.rs:114-116`),
delivered by the 16 root `select!` (case 2) or `deliver_while` (case 4). The listener owns termination:

```js
import { addSignalListener, removeSignalListener, exit } from "den:process";
const GRACE_MS = 5000;
const goodbye = async () => {
  removeSignalListener("SIGINT", goodbye);   // FIRST, before any await: second Ctrl-C = kernel death, even mid-loop (S4; 16 row n')
  setTimeout(() => exit(130), GRACE_MS);     // deadline; exit is std::process::exit on the JS thread, proven safe (16 R5)
  closing = true;                            // stops the NEXT accept() only — it does not drain: the accept() already
                                             // outstanding is a Promised -> ctx.spawn future (16 fact 1) that pins
                                             // idle() for ever, and TcpListener has no close() (socket.rs:40-72)
  await Promise.allSettled(inFlight);        // in-flight handlers finish; den stays alive while they are pending (16 R1)
  await conn.shutdown(); db.close();         // politeness: safe without them (P2/P3)
  exit(0);                                   // MANDATORY, not politeness: the pinned accept() is why (§7 q7)
};
addSignalListener("SIGINT", goodbye);
```

Flow: SIGINT → tokio's handler (installed lazily by `SignalHub::add`) → forwarder task → inbox → root select drops
`idle()` → listener runs under `context.with` → `idle()` re-entered; the pending `accept()`/`write_all` are untouched
(16 fact 5). Listener stuck in a sync loop: the select never runs, SIGTERM ends it (16 case 3, row f: 143 on all
three) — unless the listener removed itself, in which case SIGINT #2 is kernel death. Windows: no `SIG_DFL`; use the
counter guard (16 §7 q1).

**JS-visible hooks: exactly one, the signal listener.** No `unload`/`beforeExit`, no `shutdown` event, no `waitUntil`,
no runtime grace timer, no runtime second-Ctrl-C force-exit (§6). One hook is enough for the *dispatch* half of
drain; it is not enough for the *resource* half, and den is short there: with no `serve()` and no `TcpListener
.close()`, a den script cannot do what U2 b4 does on all three (`server.close()` / `HttpServer.shutdown()` /
`server.stop()`) — it can only stop issuing new `accept()`s and then `exit()`. Stated once: what den is missing for
graceful servers is a **resource method, not another hook** (§7 q7).

**REPL.** Ctrl-C is a key, not a signal (rustyline raw mode; `src/repl.rs:44-49` first press hints, second exits).
Ctrl-D → `run_repl` returns after `history.close()` (`:65-67`, saves a WAL replay) → `std::process::exit(0)` (16 §4.4).

### 4.3 Residual shared state

| State | Owner | Why unavoidable |
|---|---|---|
| POSIX disposition + tokio's permanent signal registry (16 fact 7) | OS; touched lazily by `SignalHub::add`, restored by `remove` | `sigaction` is per-process; it is what makes the second-Ctrl-C escape kernel-side (S4 `signal=Some(2)`) |
| Per-realm signal inbox (16 §4.2 row 2) | `den-stdlib-process` | `idle()` holds the mutex while parked (16 R7); a mailbox, not cancellation. Also the seam a host would use for a synthetic "wind down" — deliberately unexposed |
| The host's own flag/token, read by the closure the host installed | the embedder's composition root | bytecode stops only via a polled `Send` flag (16 fact 6); zero callers inside den (the CLI never installs one) |
| Per-worker `CancellationToken` `stop` + child `closing` (`worker.rs:392,471,564,624`) | the `Worker` object | cross-thread `terminate()` needs a `Send` flag and an async wake (16 §1.4) |
| Per-timer / per-port / per-channel / `AbortSignal` handles (16 §4.2 rows 6-8) | the resource | close handles, never realm state |
| `kill_on_drop(true)` (`spawn.rs:111`) | the `Child` | the one place embedder drop (SIGKILL) and signal death (reparent) diverge; kept (fact 11) |
| Detached WebSocket pumps (`websocket.rs:241,396`) | the socket's channel ends | hold no token; end with the channel or the process |
| The kernel page cache | the kernel | the durability boundary for every den write path (fact 8) |

---

## 5. Migration plan

Continues 16 §5 (commits 1-8). Each commit independently green under `cargo test --workspace`; CLI tests spawn
`env!("CARGO_BIN_EXE_den")` via `Command` (16 fact 12).

| # | Commit | Files | Deleted | Added | Proof |
|---|---|---|---|---|---|
| 9 | `feat(fs): write(path, bytes, { atomic })` | `den-stdlib-fs/src/lib.rs:221-225`, `den-stdlib-fs/Cargo.toml:23`, `den-stdlib-fs/tests/fs.rs`, `den-stdlib-fs/tests/js/write_atomic.js` | `tempfile` from `[dev-dependencies]` | `Opt<Object>` options on `write`; the six-line `spawn_blocking` body of §1.5 (`NamedTempFile::new_in(parent)` → `write_all` → `persist`); `tempfile.workspace = true` under `[dependencies]` (workspace already pins it, `Cargo.toml:112`). No `sync`; default path untouched; **`copy` (`lib.rs:120-124`) stays torn with no atomic option** — `tokio::fs::copy` is `std::fs::copy` (`copy_file_range(2)`/`sendfile(2)`), not read-then-write, so it cannot reuse this helper in Rust, and a JS caller can only compose `read` + `write({atomic:true})` after commit 15 (§7 q10) | `atomic_write_replaces_the_target_and_leaves_no_temp_file`: pre-fill target, write `{atomic:true}`, assert bytes equal and `readDir(parent)` shows only the target. Atomicity itself is `rename(2)`'s, not re-proven by a signal probe |
| 10 | `docs(core): embedder stop recipe` | `den-core/src/engine.rs` rustdoc on `pub struct Engine` | — | the §4.1 recipe and its six rules, verbatim. The recipe's stop signal is a `tokio::sync::watch` pair (tokio is already a den-core dependency; `Arc<AtomicBool>` plus any awaitable the host already holds is equivalent), **not** `tokio_util::sync::CancellationToken`: 16 commit 5 deletes `tokio-util` from `den-core/Cargo.toml:33` and den-core's `[dev-dependencies]` (color-eyre, futures, libtest-mimic, rquickjs, tokio, tokio-tungstenite, wat) has none, so a `tokio_util` snippet would not resolve and the doctest below would fail to build. No `run_until`, no `interrupter()`, no grace parameter | rustdoc compiles. To make it a `cargo test --doc -p den-core` doctest it must also gain a `?`-carrier — `#[tokio::main(flavor = "multi_thread")] async fn main() -> Result<(), EngineError>` — and an `entry` binding, because `no_run` still compiles the body; everything else in the fragment compiles as printed |
| 11 | `test(core): dropping an engine parked on a top-level await is sound` (graft from D1's E2) | `den-core/tests/unit/engine.rs` | — | `hosts_token_drops_an_engine_parked_on_a_top_level_await`: entry module `await`s a 60 s spawned op, a spawned task cancels at 100 ms, `select!{ run_file, cancelled }`, assert the cancelled arm wins and `drop(engine)` completes, all in < 1 s; `#[tokio::test(flavor = "multi_thread")]` (fact 4). Guards the one invariant the recipe depends on that 16 did not probe (16 §3.2 covers `idle()`; E2 covers `async_with`) | the test itself; an rquickjs bump that breaks it fails here, not in a host |
| 12 | `test(cli): graceful Ctrl-C recipe` | `tests/ctrlc.rs` (root crate, created by 16 §5.1) | — | (a) `async_listener_finishes_its_close_then_exits_0`: listener `await`s a 300 ms timer, prints `closed`, `exit(0)`; assert `closed` seen and `code()==Some(0)` (16 R1 on the den path). (b) `listener_that_removes_itself_dies_of_the_second_sigint`: handler calls `removeSignalListener` then `while(true){}`; SIGINT #1 → `caught`, SIGINT #2 → `status.signal()==Some(SIGINT)` within 2 s (S4 on the real `remove` path; also proves 16 row n'). Both `#[ignore]` until 16 commit 6 lands | the two cases |
| 13 | `docs: graceful Ctrl-C is the script's listener` | `README.md:197-198`, `ARCHITECTURE.md` §2 (`:47-82`) | — (nothing left to delete: 16 commit 4 removes `ARCHITECTURE.md:80-81` "Workers still take a child of the same token (`RealmStop`)", 16 commit 5 the `stop_token` paragraphs — which run `:55-64`, not `:55-63`; `:64` is the orphan `spawned futures.` — and 16 commit 8 touches only §7.5 and `:518`. This commit is purely additive) | README: the 9-line `goodbye` recipe; `:198` is **already** `- [x] Remove the need for the global state …`, so the edit is not a tick but making the line true (`Engine.stop_token` still exists at `engine.rs:122` until 16 commit 5) plus a pointer to 16/17 — commit 13 owns `:198`, 16 commit 8 must drop its claim on it. ARCHITECTURE §2: default Ctrl-C is kernel death; a listener turns it into a mailbox event; second Ctrl-C after self-removal is kernel death; den holds no user-space buffers (fact 8), but three write paths tear (fact 9); the §3 crash-safety table condensed to 6 rows (`write`, `copy`, `assertSnapshot`, sqlite, history, stdout); one subsection "Embedding: stopping an Engine" with the recipe | — |
| 14 | `docs(research): graceful shutdown and external stop` | this file; `docs/research/README.md` (index entry after 16's — `:38` has 16's, `grep '17-'` in that file returns nothing); `docs/research/16-cancellation-without-tokens.md` (the three corrections listed at the end of §6) | — | the index entry, and in 16: delete "a take before the entry module therefore sees no hub" from §4.1 case 4 and commit 6 (keep 6b's parenthetical), repoint `process.md:653-656` → `:669-672` in §1.5 and fact 11, and add the `Engine: Clone` / never-clone-into-`ctx.spawn` caveat to §4.3's `drop(engine)` line. The one-line pointer at the end of 16 is **not** work: it already exists at `16-cancellation-without-tokens.md:780` | — |
| 15 | *(optional, separate)* `fix(fs): write accepts string and Uint8Array` | `den-stdlib-fs/src/lib.rs:222`, hoist `JsByteBuf`/`js_bytes` (`den-stdlib-networking/src/io.rs:13-25`) to den-stdlib-core | `contents: Vec<u8>` | `contents: JsByteBuf` | today `write(path, new Uint8Array([65,66,67,68]))` → `Error converting from js 'object' into type 'array'`; 64 MiB costs `written 10464ms` for a ~15 ms write (U1 side finding 1). Not part of the graceful story; listed because commit 9 touches the same signature and the two must not merge |

Deletion audit: 16 commits 1-6 already remove the entire cancellation surface (`stop_token`, `stop()`,
`new_with_stop_token`, main-realm interrupt handler, `StopToken`, `RealmStop`, `hook_ctrlc_handler`,
`run_until_cancelled`, REPL cancel, pump `select!`, `tokio-util` from den-core, `"signal"` feature). This note adds no
CLI flag, no engine deadline, no `Stop` handle, no hook name, no `waitUntil`, no synthetic-signal API, no `sync`
option, no wasm epoch. Total code added: one fs option (~8 lines), one rustdoc, two CLI tests, one core test.

---

## 6. Rejected designs and graft notes

- **Angle B — `Engine::new() -> (Engine, Stop)` with `stop(grace)`/`interrupt()`.** The world-end token renamed:
  `interrupt()` is an `Arc<AtomicBool>` `Engine::build` must install in the handler (`engine.rs:377-379` today),
  same lifetime, same per-realm flag; `stop(grace)` is a select + timeout the caller already writes. Cost: a new pub
  type, a tuple constructor touching 67 `Engine::new()` call sites in 39 files (`grep -rn "Engine::new()"
  --include='*.rs'`, target excluded), and a handle the CLI holds and never uses. Conceded before judging.
- **`Engine::run_until(until)` / `run_event_loop(until)`.** Degenerates to `select!` and leaves the entry module
  uncovered (E8: a top-level `while(true){}` never returns from `run_file`). Three lines if a second caller appears.
- **Soft/hard token split + escalation task as the shipped embedder goodbye (D1).** Works (E6/E7) but is two tokens
  and a task in the recipe — the pattern the user called over-engineered, now at the host. Host-owned, so not a den
  global; still not shipped. **Grafted**: the one-sentence E5 warning in the rustdoc (fact 5).
- **D1's three den-core recipe tests.** Speculative for an embedder that does not exist; **grafted** only the E2 test
  (commit 11), because it guards a den invariant (dropping an in-flight `async_with`) that 16 did not probe and
  that the winner's own S2 (`design/stop.rs:77-79`) does not cover — S2 drops `idle()`.
- **Deleting `kill_on_drop(true)` (D1).** A behaviour change decided with no probe, against U1's "decide once when Q1
  is designed"; it would make an embedder's drop leave a sandboxed script's children running — the opposite of need
  (1). Kept (fact 11).
- **`if (closing) exit(130)` as the second-Ctrl-C guard (D1).** Cannot run while JS is in a tight loop (16 case 3).
  The remove-first recipe is kernel-side. Note the winner overclaimed "strictly better than Node/Deno/Bun": all
  three support the same remove idiom (16 row n'); b5 merely did not use it. Parity via the same idiom.
- **Deno `unload`/`beforeunload`, Node `exit`/`beforeExit`, a den `shutdown` event with `waitUntil`.** No hook
  completes I/O on any of the three (U2 b3: Node *does* resume after an `await` inside `exit`, Deno and Bun do not,
  and the post-await marker lands on none of them), so they cannot close a socket; a second hook name for what the
  async listener already does.
- **Runtime grace timer / runtime second-Ctrl-C force-exit / `--grace` flag.** No reference runtime has one (U2 b5).
- **`sync: true` / fsync by default.** Process death is covered by the page cache; fsync is a power-loss feature at
  milliseconds per call; add behind the same options object when asked.
- **Atomic write by default.** All three references tear identically (P1ref); rename replaces the inode (hard
  links keep old bytes, a symlink at `path` becomes a regular file, mode becomes the tempfile's 0600) — an
  unrequested semantic change.
- **wasmtime epoch interruption now.** Three lines (`config.epoch_interruption(true)`, `store.set_epoch_deadline(1)`,
  `engine.increment_epoch()` in the stop closure) — deferred until a sandbox requirement exists (fact 12).
- **`TaskTracker` drain.** "usually used together with `CancellationToken`" (`task_tracker.rs:22-24`) — the token
  pattern renamed; already rejected in 16 §6.

Corrections applied to the input designs: D1 cited `src/app.rs:158-162` for the REPL pump cycle; the file is 108
lines and the `ctx.spawn(Self::repl_pump(.., engine, ..))` is at `:45-49` (`judge/verify.log`). D2's S1-S3 use the
16 probe crate's runtime, not a den `Engine`; the composed den-binary run is commit 12's job.

Corrections this note carries **back into 16** (all three land in commit 14, since 16 is otherwise settled):
(a) 16 §4.1 case 4 and commit 6 claim "a take before the entry module therefore sees no hub" — false in den
(§4.1 above); 16 commit 6b's parenthetical (`engine.rs:439` → `lib.rs:134`, "runs in every realm") is the correct
statement and the contradicting sentence in commit 6 goes. (b) 16 §1.5 and its fact 11 cite `process.md:653-656`
for Node's default-handler rule; the sentence is at `:669-672` (§2.1 above). (c) 16 §4.3's `drop(engine) // = cancel
this realm` is unconditional, but `Engine` is `Clone` (`engine.rs:118`) and the "never move an `Engine` clone into a
`ctx.spawn` future" rule of §4.1 is its precondition — an embedder reading only 16 can build the `src/app.rs:45-49`
cycle and never see the drop take effect.

---

## 7. Open questions / limits

1. **Self-removal inside the SIGINT handler restoring `SIG_DFL`** is proven at the mechanism level (S4, bare tokio
   process) and by `dispatch`'s clone-before-call (`signal.rs:217-218`); the den-path proof is commit 12 (b), which
   cannot run until 16 commit 6 lands. If commit 6's `deliver` iterates the live `RefCell` instead of a clone,
   self-removal panics on borrow.
2. **Windows**: no `SIG_DFL`; the escape degrades to the counter guard, which cannot run mid-loop — same as
   Node/Deno/Bun there. Unprobed.
3. **Embedder "graceful" without a kernel signal** (tell JS to wind down): only `libc::raise(SIGINT)` when a listener
   exists (process-wide side effect on the host's other tokio signal subscribers) or `timeout(grace,
   run_event_loop())` then the hard flag then `eval` under a second flag. The inbox is the seam; unexposed.
4. **`den:fs write` accepts only a JS number array** (`lib.rs:222` `Vec<u8>` via rquickjs `from.rs:332-337`):
   `Uint8Array` and strings are rejected, 64 MiB costs 10 s of conversion. Commit 15, separate.
5. **Atomic write and directory permissions**: `NamedTempFile::new_in(parent)` needs write permission on the
   directory, not just the file; mode/owner become the tempfile's unless copied. Document; copy the old mode in the
   closure if a user asks.
6. **A goodbye that blocks** (sync loop, blocking loader `den-core/src/loader/http.rs:73`) defers the listener's
   `setTimeout` deadline until it yields — same limit as Node's timers; the removed-listener escape still works.
7. **den has no HTTP server and `TcpListener` has no `close()`.** `den-stdlib-networking/src/socket.rs:40-72`
   exposes exactly three members — `local_addr` `:59-60`, `accept` `:62-66`, static `listen` `:68-72` — and no
   `close()`, no `Drop`. An already-outstanding `accept()` is an async `#[rquickjs::methods]` fn, i.e. a `Promised`
   → `ctx.spawn` future (16 fact 1), so it pins `idle()` for ever no matter what the script stops calling: a
   graceful script cannot drain, and `exit()` in the listener is mandatory rather than polite. This is a missing
   *resource method*, not a missing hook — until a `serve()` or a listener `close()` exists, den's graceful story is
   strictly weaker than U2 b4 (`server.close()` / `HttpServer.shutdown()` / `server.stop()`). A future `serve()`
   needs `shutdown()`/`finished` like `Deno.HttpServer` (`deno.d.ts:5857,5876-5879`).
8. **Re-adding a listener after `SIG_DFL` was restored** is 16 §7 q2, unchanged.
9. **wasm spin under embedder stop** hangs until a sandbox requirement pays for the epoch trio (fact 12).
10. **`den:fs copy` stays torn and gets no opt-in.** `lib.rs:120-124` is `tokio::fs::copy` = `std::fs::copy` =
    `copy_file_range(2)`/`sendfile(2)` on Linux, not read-then-write, so it cannot be composed out of the commit-9
    helper inside Rust; the JS composition (`read` + `write(.., {atomic:true})`) is blocked by q4 until commit 15.
    For this one path the no-corruption guarantee *is* coupled to shutdown: the only remedy available today is not
    calling `copy` while stopping. §1.5's "correctness never depends on a hook" therefore holds for every den write
    path except this one and q11.
11. **`den:assert assertSnapshot` writes torn** (`den-stdlib-assert/src/lib.rs:669` → `:188` → `:225`
    `fs::create_dir_all` + `:231` `fs::write`): blocking `std::fs` truncate-then-write on the JS thread, same tear
    class as `den:fs write`. Probed on the working-tree binary: `import { assertSnapshot } from "den:assert";
    assertSnapshot("refuter-value-1234", "refuter_probe")` created `snapshots/refuter_probe.snap.new` (64 bytes) and
    printed `threw:, AssertionError: Snapshot "refuter_probe" failed: snapshot differs; review the generated
    .snap.new file`. It ships in the default binary and is JS-reachable, but it is a test-only API writing under the
    CWD; deliberately unfixed, listed so the §3 table covers it.

## Probe directories

| Dir (`/tmp/parity/scratch/graceful/`) | What |
|---|---|
| `u1-writepaths/` | `harness.ts`; `p1_run.ts`/`p1.js` (den 64 MiB write torn), `p1_ref_run.ts` + `p1_ref_{deno.ts,node.mjs,bun.js}`, `p2_run.ts`/`p2.js` (sqlite txn/autocommit), `p3_run.sh` (REPL history under a pty, `typescript{1,2}.log`), `p4_run.ts`/`p4.js` (console events to a file); logs `p1.log`, `p1_ref.log`, `p2.log`, `p3.log`, `p4.log`; artifacts `p2_txn.db{,-journal}`, `p2_autocommit.db`, `p4_{midloop,afterloop}.out` |
| `U2-node/`, `U2-deno/`, `U2-bun/` | `b1_ws` (buffered writes + SIGINT), `b2_ws_flush` (handler closes then exits), `b3_hooks` (exit hooks), `b4_http` (graceful/forceful server close), `b5_double` (second SIGINT mid-grace), `b6_sqlite` (open tx / batched / WAL); Deno also `b7_buffer.ts`, `b7_natural_debug.ts`, `b7_variants.ts`; Node/Deno `docs/` |
| `U2-harness/` | `h.ts`, `run.ts`, `one.ts` (`Deno.Command`, `clearEnv`, code+signal); `all.log`, `b7.log`, `extra.log` |
| `out/` | `b1_*.bin`, `b2_*.bin`, `b7_*.bin` byte-count artifacts; `b6_*_{tx,batch,wal}.db` (+ leftover `-journal`) |
| `U3/` | `harness.ts`, `js_spin.js`, `wasm_spin.js`, `probe.log` (QuickJS interrupt reaches JS in 5 ms, never a wasm `(loop br 0)`) |
| `angle-a/` | `embed.rs` (E1 tight loop in entry, E2 in-flight `async_with` dropped, E3/E4 goodbye resolved / deadline, E5 goodbye under one token, E6/E7 soft-hard split, E8 entry loop under the split; multi-thread and `current_thread` runs), `embed.log`, `patch*.ts` |
| `design/` | `stop.rs` (S1 spawned tight loop, S2 `idle()` dropped with a 60 s future pending, S3 entry eval interrupted, S4 `signal(SIGINT, SIG_DFL)` over tokio's handler → `signal=Some(2)`), `all.log`, `S{1,2,3,4}.log` |
| `judge/` | `verify.log` — working-tree line verification of both designs' citations (`Engine::new()` 67/39, `app.rs:45-49`, `signal.rs:217-218`, `spawn.rs:111`, `fs lib.rs:221-225`) |

Baseline probes referenced by label (R1-R9, U2-A/C/D, angle-b/c, refute-doc16 rows) live under
`/tmp/parity/scratch/cancel/` and are indexed at the end of [16](16-cancellation-without-tokens.md).
