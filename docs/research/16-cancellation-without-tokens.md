# Cancellation without tokens — why den's `stop_token` exists, why Node/Deno/Bun have no equivalent, and how to delete it

Status: research, 2026-08-27. Snapshot of the den working tree (branch `master`, after `0c6ce89`),
rquickjs 0.12.2 (`full-async, rust-alloc, parallel, indexmap, either`), tokio 1.53.1, tokio-util 0.7.19,
Node v26.5.0, Deno 2.9.4, Bun 1.3.9, all on Linux x86_64. **Not a living document.** Every claim carries a
`file:line` into the working tree or a vendored source, or a line quoted verbatim from a probe run. Nothing is
from memory. For the current truth read [ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.

The question this note answers, in the user's words: *"I want to remove the need to attach too much
cancellation token (things like timer is fine), but right now all of our async code needs cancellation to make
it work, and I don't want to associate global cancellation states with the engine. I want to make it lean and
clean, while keeping the same behaviour as closing the process with Ctrl-C in Node.js/Deno/Bun."*

## Sources read

| What | Path |
|---|---|
| den (working tree) | `den-core/src/engine.rs`, `src/app.rs`, `src/main.rs`, `src/repl.rs`, `den-stdlib-timer/src/lib.rs`, `den-stdlib-worker/src/{worker,host,port,broadcast}.rs`, `den-stdlib-process/src/{signal,lib}.rs`, `den-stdlib-whatwg-fetch/src/{lib,fetch_op}.rs`, `den-stdlib-networking/src/*.rs`, `ARCHITECTURE.md` §2 and §7.5, `docs/research/09` |
| rquickjs-core 0.12.2 | `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/src/` (`core/` below) |
| rquickjs-sys 0.12.2 + quickjs-ng | `.../rquickjs-sys-0.12.2/quickjs/quickjs.c`, `build.rs` (`qjs/`) |
| tokio 1.53.1 / tokio-util 0.7.19 / signal-hook-registry 1.4.2 | `.../tokio-1.53.1/src/` (`tokio/`), `.../tokio-util-0.7.19/src/` (`tutil/`), `.../signal-hook-registry-1.4.2/src/lib.rs` (`shr`) |
| Rust std (1.98) | `$(rustc --print sysroot)/lib/rustlib/src/rust/library/std/src/` (`std/`) |
| Deno 2.9.4 source | fetched `ext/signals/lib.rs`, `ext/os/ops/signal.rs`, `ext/os/40_signals.js`, `libs/core/runtime/jsruntime.rs`, `libs/core/async_cancel.rs`, `runtime/web_worker.rs`, `runtime/ops/worker_host.rs`, `cli/worker.rs` (under `/tmp/parity/scratch/cancel/U4-deno/src/`) |
| Node docs | `nodejs/node/main/doc/api/{process,timers,net}.md` (saved under `/tmp/parity/scratch/cancel/U3-node/`) |
| Probes | `/tmp/parity/scratch/cancel/{u1,U2-drop-probe,U3-node,U4-deno,U5-bun,u6,angle-a,angle-b,angle-c,judge-parity,judge-soundness,u7-refute,refute-doc16,doc16-revise}/` — see the table at the end |

There is no Deno checkout on this machine (`~/.cargo/registry/src/*/` holds no deno crate). The fetched files are
stored flat under `U4-deno/src/` with the path folded into the name: `runtime/web_worker.rs` is
`U4-deno/src/runtime_web_worker.rs`, `runtime/ops/worker_host.rs` is `U4-deno/src/runtime_ops_worker_host.rs`, and
so on. Deno line numbers below refer to those files.

---

## 0. TL;DR — the facts an implementer must not get wrong

1. **`idle()` returns only when the spawner is empty.** `core/runtime/async.rs:347-352`: `SchedularPoll::Empty`
   is the only `Poll::Ready` exit. Every `#[rquickjs::function] async fn` is a `Promised` that ends in
   `ctx.spawn(future)` (`core/value/promise.rs:77`). So *every* async stdlib call pins `idle()` open until it
   resolves. This is the whole reason the token exists: `src/app.rs:54` waits for `idle()` to *return* on Ctrl-C.
2. **Only timers and workers cooperate today; everything else hangs Ctrl-C.** Token readers:
   `den-core/src/engine.rs:376-381` (interrupt handler, `runtime.set_interrupt_handler({..}).await` — the range
   includes the trailing `.await;` at `:381`), `:428` (`StopToken` → `den-stdlib-timer/src/lib.rs:92,118,123,167,206`),
   `:463` (`RealmStop` → `den-stdlib-worker/src/worker.rs:529-530`). Non-readers that can pend forever: `fetch`
   (`den-stdlib-whatwg-fetch/src/lib.rs:1204-1210`), `accept`/`read` (`den-stdlib-networking/src/socket.rs:62`),
   the signal pump (`den-stdlib-process/src/signal.rs:207-209,258`). Probe u1: `c … exit=137 (hang) 10005 ms`,
   `d … 137`, `e … 137`, `e2 … 137`, versus `a … exit=0 1009 ms` for the timer.
3. **Node, Deno and Bun do not unwind on Ctrl-C; they die of the signal.** Node strace:
   `rt_sigaction(SIGINT, {sa_handler=0x954c60, …, SA_RESETHAND})` then `tgkill(…, SIGINT)` then
   `+++ killed by SIGINT +++` (U3 m). Deno: `rt_sigaction(SIGINT, SIG_DFL)` then `tgkill` (`U4 m_a_timer.js.trace:16-17`;
   `ext/signals/lib.rs:62-69`). Bun: *zero* `rt_sigaction(SIGINT…)` calls, zero `exit_group` (U5 m). All three:
   pending fetch/accept abandoned, `beforeExit`/`unload` never fire, exit status is signal death (shell 130).
4. **Dropping an rquickjs runtime with pending `ctx.spawn` futures is sound.** `RawRuntime::drop`
   (`core/runtime/raw.rs:123-133`) calls `opaque.clear()` → `self.spawner.take()` (`core/runtime/opaque.rs:290`)
   → `Schedular::clear` (`core/runtime/schedular.rs:236-240`) → every future's destructor, **before**
   `JS_FreeRuntime` (`raw.rs:129`). Probe U2-A: `A pending future dropped; f()=42 globalThis.marker=7 (JS runtime
   still alive at drop time)` then `A AsyncRuntime dropped` — no abort, no leak assertion.
5. **Dropping `idle()` mid-await cancels nothing and corrupts nothing.** It owns only the mutex guard
   (`core/runtime/async.rs:314-359`); spawned futures stay in the spawner and are re-polled on re-entry. Proof is
   probe R1 (`U2-drop-probe/src/bin/refute.rs`, `u7-refute/all.log`), whose listener busy-loops in JS for 200 ms so
   the 500 ms timer wake demonstrably lands while `idle()` is dropped: `R1 SIGINT #2 entered JS at 350.146259ms,
   left at 550.129081ms (hits=2)` / `R1 idle() returned at 751.474462ms: hits=2 fired=1 late=1 delivered=2`. The
   older angle-a E run (`hits=2 fired=1`) does **not** prove this: its listener returns at once, so every drop
   window is sub-millisecond and the 600 ms timer never wakes inside one (§3.2).
6. **A tight JS loop can only be stopped by a polled flag.** Interrupt handler type
   `Box<dyn FnMut() -> bool + Send + 'static>` (`core/runtime.rs:51-52`), polled every 10 000 back-edges
   (`qjs/quickjs.c:479, 8215-8240, 18664-18677`); no `JS_TerminateExecution` anywhere (U2 §3 grep). An
   `Arc<AtomicBool>` is enough: probe U2-D `tight loop: err=true uncatchable=true message=Some("interrupted")
   js_caught=None after 300.145283ms`. And with no JS signal listener the CLI does not need it at all — the kernel
   kills the loop (Deno b: `m_b_loop.js.trace` has no `rt_sigaction(SIGINT…)`; Bun b `exit=130 after_signal_ms=1`).
7. **tokio's SIGINT handler is permanent.** First `ctrl_c()`/`signal()` anywhere in the process installs it via
   `OnceLock::get_or_init` (`tokio/signal/unix.rs:283-290`) and it is never removed (`:334-345`; `shr:632-641`
   says unregister cannot restore `SIG_DFL`). den today: `src/app.rs:93` consumes one Ctrl-C; a second does nothing
   (u6 `[trap] … still alive after 2nd SIGINT with no listener`). den's *other* subscriber is only
   `den-stdlib-process/src/signal.rs:256-257` (`let mut stream = signal(kind)…`; `:240` is just the `use`) and
   `:267,274` (windows `ctrl_c()`/`ctrl_break()`), reached from `SignalHub::add`.
8. **Install nothing until JS listens.** On the hook-free path den already touches no SIGINT disposition:
   angle-c P1 `strace -f -e trace=rt_sigaction den --repl </dev/null` → `SIGINT sigaction lines: 0`. Sending the
   signal den does *not* handle (SIGTERM) to today's binary shows exactly the proposed Ctrl-C behaviour:
   angle-b `a: exit=143 after_signal_ms=2`, `c: 143/2`, `d: 143/3`, `i: 143/2`, `f: 143/3 stdout_lines=5000 last=line 4999`.
9. **Output already logged survives signal death.** `console.log` → `tracing::info!`
   (`den-stdlib-console/src/lib.rs:228-231`) → `io::stdout()` `LineWriter`, flushed per `\n`. angle-c P3b after
   `+++ killed by SIGINT +++`: `lines=5000 last=line 4999` — **5000 is the event count**, not the physical line
   count: the CLI's `tracing_subscriber::fmt()…pretty()` (`src/main.rs:65-72`) emits three physical lines per
   `console.log`, so the same script measures `{"label":"den.g.lines","code":0,"stdout_lines":15000}` and
   `{"label":"den.g.SIGTERM.drained","code":143,"signal":"SIGTERM","stdout_lines":15000}` (refute-doc16). The
   guarantee holds only for a *drained* pipe: with a stalled reader den blocks in `write` and only the events that
   fit the 64 KiB pipe buffer land — `{"label":"g.den.SIGTERM.drained","code":143,"signal":"SIGTERM","stdout_lines":1008}`
   = 336 of 5000 events (refute-doc16 `stall.ts`). Node loses more (U3 g-big: 43 344 queued async pipe lines dropped).
10. **`std::process::exit` from any thread is safe while the JS thread spins inside `idle()`.**
    `std/process.rs:2537-2539` (`rt::cleanup` flushes stdout with `try_lock`, `std/io/stdio.rs:727-743`; no
    destructors); `unique_thread_exit` (`std/sys/exit.rs:3-20`). Probe U2-C: `pipestatus=130` with every stdout line
    present while `while(true){}` ran under `idle()`. Do **not** drop the tokio runtime on the exit path: it waits for
    the blocking pool (u6 `[rt-drop] returned after 1.400152662s` vs `[exit-from-task-while-block-in-place] … 100 ms`).
11. **A JS signal listener must not be a `ctx.spawn` future.** Today it is (`signal.rs:207`), so a listener keeps
    den alive for ever (u1 e2 `137` vs node/deno/bun `exit=0`). Node: "a listener does not ref the loop"
    (`process.md:655-656`, U3 e2 `exit=0 24 ms`); Deno unrefs the op (`ext/os/40_signals.js:18-22`;
    `libs/core/runtime/jsruntime.rs:3039-3041`). The forwarder has to be a tokio task feeding a mailbox drained by the
    root loop — proven by angle-a probe E and the judge's `dw-probe`: `SIGINT#1 delivered via context.with at 101ms
    while entry pending; hits=1`, `entry resolved with 42 at 300ms`.
12. **Test harness rule.** A non-interactive shell's `&` gives the child `SIGINT = SIG_IGN` (U5 gotcha 1:
    `SigIgn: 0000000001001006`, Bun survived `kill -INT` for 120 s). Spawn den from Rust `Command` or under `set -m`,
    and assert `ExitStatusExt::signal() == Some(SIGINT)`, not `$? == 130` (u6: `[exit130] code=130 signal=null` vs
    `[sigdeath] code=130 signal=SIGINT`).

---

## 1. The mental model (teaching section)

### 1.1 Liveness: "what keeps den alive" is "what is spawned"

den has one event loop, `AsyncRuntime::idle()`. It takes the runtime mutex (`core/runtime/async.rs:314`), runs
pending JS jobs, polls the spawner, and returns `Ready` in exactly one case — the spawner reports `Empty`
(`:347-352`). A `ctx.spawn`-ed future is not a tokio task; it lives inside the QuickJS opaque
(`core/context/ctx.rs:418-424` → `core/runtime/opaque.rs:158-163` → `spawner.rs:29-35` → `schedular.rs:67-86`) and is
polled only by whoever holds the mutex. Every `#[rquickjs::function] pub async fn` becomes a `Promised`, and
`Promise::wrap_future` ends in `ctx.spawn(future)` (`core/value/promise.rs:48-78`, line 77).

So the liveness rule is mechanical: **the process is alive while any spawned future is pending.** That is the same
rule as libuv's ref'd-handle count (Node `timers.md:152-155` `unref()` "will not require the Node.js event loop to
remain active", `net.md:929-931`) and deno_core's `has_pending_refed_ops` (`jsruntime.rs:3074-3082`:
`num_pending_ops > num_unrefed_ops`). ARCHITECTURE.md §7.5 already states it as "what keeps den alive is exactly
what is spawned". Nothing in this note changes that rule. What changes is who is allowed to *end* a spawned future
early — and the answer is going to be "its owner, or the process", never "a realm-wide flag".

A useful consequence to keep in mind: a spawned future that awaits a channel or socket ends by itself when the
*other end* goes away (`recv()` → `None`, read → 0). The worker port pump already relies on this
(`den-stdlib-worker/src/port.rs:235-238`; research 09 §2.2 "the parent's pump ends exactly when the worker runtime is
torn down"). Closing the far end *is* cancellation.

### 1.2 Cancellation in Rust is drop, not a flag

A Rust future has no "cancel" method. A future you stop polling and drop is cancelled: "it will stop running at
whichever `.await` it has yielded at. All local variables are destroyed by running their destructor"
(`tokio/task/mod.rs:129-130`). That is the *entire* mechanism. `tokio::select!` only wraps it: the losing arm is
dropped (`tokio/macros/select.rs:141-151`, which also says cancelling a non-cancel-safe future "is not necessarily
wrong … if you are cancelling a task because the application is shutting down").

A `CancellationToken` is therefore not a cancellation primitive. It is a *broadcast flag* (`Arc<TreeNode>` with a
`Mutex<Inner>` + `Notify`, `tutil/sync/cancellation_token.rs:57-59`, `tree_node.rs:46-49`; `is_cancelled()` takes the
mutex every call, `tree_node.rs:82-84`) that a future must voluntarily poll. You need one only when **somebody
refuses to drop the future**. den's refuser is `App::run_until_end`: it wants `idle()` to *return* so that
`shutdown()` can run (`src/app.rs:54-55`), and `idle()` will not return while a fetch is parked. Hence the token,
hence "all of our async code needs cancellation".

Two things can drop a `ctx.spawn` future: the runtime going away — `RawRuntime::drop` clears the spawner before
`JS_FreeRuntime` (`core/runtime/raw.rs:128-129`, `opaque.rs:290`, `schedular.rs:236-240`, `vtable.rs:51-54`) — or the
process going away. Neither needs the future's consent. §3 shows both are sound.

When *is* a shared flag unavoidable? Exactly when you must reach a future (or a bytecode loop) that you do not
own, from a thread you are not on, without ending the process. In den that is one place: `worker.terminate()`
(§1.4). Everything else that holds a token today is either the realm root (to be deleted) or a per-object
close handle (`clearTimeout`, `port.close()`, `channel.close()`) — the class the user already accepts.

### 1.3 The process is the outermost scope

Node, Deno and Bun never make their loop drain on Ctrl-C. With no JS listener the process dies *of* SIGINT:

| Scenario, SIGINT at 1 s | Node 26.5.0 | Deno 2.9.4 | Bun 1.3.9 | den today |
|---|---|---|---|---|
| `setTimeout(…,1e9)` | `exit=130` (U3 a) | `exit=130` (U4 a) | `[a] exit=130 after_signal_ms=3` (U5) | `exit=0 1009 ms` (u1 a) |
| `while(true){}` | `exit=130` (U3 b) | `exit=130`, no handler installed yet (U4 b) | `[b] exit=130 after_signal_ms=1` | `exit=0` + `Error: interrupted` (u1 b) |
| pending `fetch` to a black hole | `exit=130`, request abandoned (U3 c) | `exit=130`, nothing cancelled (U4 c) | `[c] exit=130 after_signal_ms=4` | **hang, 137** (u1 c) |
| listen + pending `accept()` | `exit=130` (U3 d) | `exit=130` (U4 d) | `[d] exit=130 after_signal_ms=1` | **hang, 137** (u1 d) |
| worker in `while(true){}` | `exit=130` (U3 i) | `exit=130` (U4 i) | `[i] exit=130 after_signal_ms=1` | `exit=0` (u1 i) |

Mechanism, from strace: Node installs a C handler with `SA_RESETHAND`, resets the terminal, re-raises, and the
kernel kills every thread (`U3 m_default.strace`; `process.md:669-672` "exiting with code 128 + signal number").
Deno's signal thread does `restore_default(signal)` + raise (`ext/signals/lib.rs:62-69`; `signal-hook
low_level/signal_details.rs:182-183`). Bun installs nothing and lets `SIG_DFL` act. None of them cancels an op,
joins a thread, or runs `exit`/`beforeExit`/`unload` (U3 h `h.out` empty; U4 h neither event; U5 h neither).

The lesson: "same behaviour as Ctrl-C in Node/Deno/Bun" is **not** cooperative shutdown. It is *no* shutdown. The
kernel reclaims sockets, threads and memory; the runtime's state is discarded mid-flight. Once den adopts that,
the fetch/accept/read/child-wait futures need no token, because nobody is waiting for `idle()` any more.

### 1.4 The one poll-based exception: QuickJS interrupt for tight loops

A `while(true){}` never reaches an `.await`, so drop cannot touch it. QuickJS offers exactly one hook: the
interrupt handler, a synchronous callback polled from the bytecode loop every 10 000 back-edges/calls
(`qjs/quickjs.c:479, 8215-8240, 17590, 18664-18677`); it throws an uncatchable `InternalError("interrupted")`
(`try/catch` cannot catch it: probe U2-D `js_caught=None`). rquickjs exposes it as
`AsyncRuntime::set_interrupt_handler(Option<Box<dyn FnMut() -> bool + Send + 'static>>)`
(`core/runtime.rs:51-52`, `core/runtime/async.rs:185-193`) — and installing takes the runtime mutex (`:187-189`), so
it must be installed *before* `idle()` runs and flipped from outside afterwards.

That hook needs a **flag**, not a **token**: the handler returns a `bool`, nothing awaits it. `Arc<AtomicBool>`
with `Ordering::Relaxed` is the minimum (U2-D; the research-09 probe used the same). den keeps the hook in one place
only: inside each worker, for `terminate()`, where the per-worker `CancellationToken` doubles as this flag
(`is_cancelled()`) *and* as the async wake for a parked worker (`cancelled()` → drops the worker's `idle()`,
`worker.rs:673`). That dual use is why the worker keeps a token rather than an `AtomicBool + Notify` pair. The main
realm loses its handler entirely: with no JS listener the kernel kills the loop (§1.3 row b), and with a listener
all three runtimes also fail to run it during a sync loop (U3 f, U4 f, refute-doc16 `f.deno.thenTERM`, U5 f — all
died of SIGTERM, 143).

### 1.5 Signals are a disposition owned by the process module

libuv/Node's rule (`process.md:669-672`): *no listener* → the default handler exits with `128+n`; *a listener* →
"its default behavior will be removed (Node.js will no longer exit)", the signal becomes an ordinary loop event,
a second SIGINT is just another event (U3 e survives two, U5 e survives three), and exiting is the listener's job
(`process.exit()`, U3 e2 `exit=3`). Deno is the same with `prevent_default` (`ext/os/ops/signal.rs:58-64`) and
`Deno.exit` (U4 e2 `exit_group(7)`). Bun installs the handler lazily at `process.on` time (U5 m'
`rt_sigaction(SIGINT, {sa_handler=0x42bb450…})` only with a listener).

What happens once the *last* listener is removed differs, and strace settles it (refute-doc16 `strace_one.sh`,
`m_*_removed.strace`). Node restores the default disposition **at removal time** and lets the kernel kill:
`rt_sigaction(SIGINT, {sa_handler=SIG_DFL, sa_mask=[], sa_flags=SA_RESTORER, …}) = 0` at `removeListener`, then
`--- SIGINT {si_signo=SIGINT, si_code=SI_USER, si_pid=3057378} ---` / `+++ killed by SIGINT +++` with zero
`kill`/`tgkill` lines (`m_node_removed.strace`). Bun does the same: `rt_sigaction(SIGINT, {sa_handler=SIG_DFL,
sa_mask=[INT], …}, {sa_handler=0x42bb450, …}, 8) = 0` at removal, then `+++ killed by SIGINT +++`
(`m_bun_removed.strace`). Deno is the outlier: it keeps its handler and does `rt_sigaction(SIGINT,
{sa_handler=SIG_DFL…})` + `tgkill(2970459, 2970467, SIGINT)` from a helper thread **at delivery time**
(`m_deno_removed.strace`). The practical difference: after removal, a Node/Bun process spinning in `while(true){}`
still dies of SIGINT in ≤2 ms because no runtime code has to run; Deno's helper thread achieves the same off the JS
thread.

den's mapping: `den-stdlib-process` owns the disposition. `SignalHub::add` is the first and only place tokio's
handler gets installed (already true: `signal.rs:256-257` is reached only from `add`). Delivery happens at the root
loop, not inside `idle()` (§4.1 case 2). den adopts **Node/Bun's** mechanism: `SignalHub::remove`, when the last
listener for `n` goes, flushes stdout/stderr and calls `libc::signal(n, SIG_DFL)` itself — because tokio cannot
(fact 7) and because the restore must not depend on the JS thread being free to run `deliver` (§4.1 case 3').
den-core and the CLI know nothing about signals beyond calling `run_event_loop()`.

### 1.6 What changes for a stdlib author

Rule: **an async op never takes a realm token.** It is correct on Ctrl-C because the process dies; on `close()`
because its owner drops the far end or its own handle; on `worker.terminate()` because the worker's runtime is
dropped and takes the spawner with it (§3.1). Three shapes cover every op in den.

**(a) A one-shot op — `fetch`, `child.wait()`, `socket.read()`.** Just await it. Nothing else.

```rust
#[rquickjs::function]
pub async fn wait(this: This<Class<'_, Child>>) -> Result<i32> {
  let status = this.borrow_mut().inner.wait().await?;   // pends until the child exits, or the process dies
  Ok(status.code().unwrap_or_default())
}
```

Why it is correct: Ctrl-C = kernel death (§1.3); `drop(engine)` = destructor runs with JS still alive (§3.1); the
`tokio::process::Child` future is cancel-safe by construction because it is dropped exactly once. Per-request
abort (`AbortSignal`) stays what it is today — an `Arc<AtomicBool>` owned by the request
(`den-stdlib-whatwg-fetch/src/fetch_op.rs:29-66`), never realm state.

**(b) A pump with a JS-visible `close()` — WebSocket, EventSource, MessagePort.** The resource owns the far end;
`close()` drops it; the pump ends on `None`.

```rust
struct Socket { tx: Option<mpsc::UnboundedSender<Frame>> }
impl Socket { fn close(&mut self) { self.tx = None } }          // this IS the cancellation

ctx.spawn(async move {
  while let Some(frame) = rx.recv().await { deliver(&ctx, frame) }  // ends when every sender is gone
});
```

Where the queue must survive a pause and resume later (`port.start()`/`pause()`, `port.rs:364-378`), or must reject
messages while the registry still holds a sender (`broadcast.rs:154-158`), a per-object token is the honest
handle — it is scoped to the object, not the engine, and it is untouched by this note.

**(c) A cross-thread stop — `worker.terminate()`.** The only place a shared flag is legitimate, and it is owned by
the `Worker` object:

```rust
// in WorkerThread::serve_engine, right after build_engine(base) and before any JS runs
engine.runtime.set_interrupt_handler(Some(Box::new({
  let stop = self.stop.clone();
  move || stop.is_cancelled()                                    // the polled flag (§1.4)
}))).await;
let closing = self.stop.child_token();
closing.run_until_cancelled(engine.runtime.idle()).await;        // the async wake for a parked worker
drop(engine.context); drop(engine.runtime);                      // spawner cleared before JS_FreeRuntime
```

The parent side needs nothing: its fault pump `stop.run_until_cancelled(inbox.recv())` (`worker.rs:483`) is the
same per-worker token, and would end anyway when the worker thread drops its sender.

What an author must **not** do: read a realm-wide userdata token, `select!` an op against "the engine stopping",
or `tokio::spawn` a helper that keeps state the realm needs — a detached tokio task does not pin `idle()`
(`den-stdlib-whatwg/src/local_http.rs:53-65` is test support and shows the difference), so it is the right tool
only for things that must *not* keep the process alive, such as the signal forwarder (§4.1).

---

## 2. What Node/Deno/Bun actually do, and the rule den adopts

Harness for all rows: script started, signal sent at ~1 s, watchdog SIGKILL (137) marks a hang. Node/Deno/Bun
outputs are verbatim from `/tmp/parity/scratch/cancel/U3-node`, `U4-deno`, `U5-bun`; den-today from `u1`; cells
written as `{"label":…,"code":…,"signal":…}` are from the later `refute-doc16/harness.ts` (a `Deno.Command`
harness that reports `code` and `signal` separately, per fact 12). Two environment preconditions, both learnt the
hard way: (1) **`HTTP_PROXY`/`HTTPS_PROXY` must be unset.** Bun ignores CIDR entries in `NO_PROXY`, so the row-c
black hole `http://10.255.255.1:81/` answers through the proxy and the row collapses to
`{"label":"c.bun","code":0,"signal":null,"sent":"SIGINT@1000ms(gone)","stdout_last":"ok 403"}`; with the
variables deleted it reproduces (`{"label":"c.bun.noproxy","code":130,"signal":"SIGINT"}`). (2) Every probe here
runs with **piped stdio**; nothing in any probe directory drives a pty, so the REPL row's "den today" cell is
unverified (rustyline needs a tty for raw mode, `rustyline-18.0.1/src/tty/unix.rs:1631`).

| # | Scenario | Node | Deno | Bun | den today | den adopts | Why, if they differ |
|---|---|---|---|---|---|---|---|
| a | timer 1e9 + SIGINT | 130 signal death | 130 signal death | 130, 3 ms | 0 via token | **130 signal death** (kernel `SIG_DFL`) | unanimous |
| b/b2 | `while(true){}` (top level / in timer cb) + SIGINT | 130 | 130 (no handler yet) | 130, 1 ms | 0 + `Error: interrupted` | **130 signal death**; no main-realm interrupt handler | unanimous; den today is the odd one out |
| c/c2 | pending fetch + SIGINT | 130, abandoned | 130, nothing cancelled | 130, 4 ms | **hang 137** | **130** | unanimous |
| d | listen + `accept()` + SIGINT | 130 | 130 | 130, 1 ms | **hang 137** | **130** | unanimous |
| e | listener + timer, SIGINT ×2 | survives both, `exit=0` when timer ends | survives both, alive for ever | survives 3, only SIGTERM ends it | listener runs on **every** SIGINT (`SignalHub`'s own `signal(SIGINT)` stream, `signal.rs:205-209,256-260`); the first Ctrl-C cancels the stop token (`app.rs:88-96`) so the timer never fires; **hang** — stdout `e: armed` / `e: SIGINT #1` / `e: SIGINT #2`, `exit=137` (refute-doc16 `den_e.out`) | **survives every SIGINT, listener runs each time, exits when the timer ends** | Node's outcome (Deno/Bun only differ because their probe timers were 1e9) |
| e2 | listener only, no signal | `exit=0` 24 ms | `exit=0` 15 ms | `exit=0` 10 ms | **hang 137** | **exit 0** | unanimous; needs the forwarder to be a tokio task (fact 11) |
| e3/n | last listener removed, then SIGINT | 130 signal death; `SIG_DFL` restored **at removal**, no re-raise (`m_node_removed.strace`; `{"label":"n.node","code":130,"signal":"SIGINT","ms_after_last_signal":4,"stdout_last":"n: removed"}`)¹ | 130 signal death; handler kept, `SIG_DFL` + `tgkill` **at delivery** from a helper thread (`m_deno_removed.strace`; `op_signal_unbind`) | 130 signal death; `SIG_DFL` restored at removal (`m_bun_removed.strace`; `{"label":"n.bun","code":130,"signal":"SIGINT","ms_after_last_signal":1,"stdout_last":"n: removed"}`) | n/a | **130 signal death**: `SignalHub::remove` restores `SIG_DFL` when the last listener goes (Node/Bun's mechanism, §1.5) | unanimous 3/3 on the outcome; Node+Bun agree on the mechanism (restore at removal), Deno restores at delivery. den takes the majority because it also covers the next row |
| n' | listener added, removed, then `while(true){}` + SIGINT | `{"label":"rmspin.node","code":130,"signal":"SIGINT","ms_after_last_signal":2,"stdout_last":"removed; spinning"}` | `{"label":"rmspin.deno","code":130,"signal":"SIGINT","ms_after_last_signal":2}` | `{"label":"rmspin.bun","code":130,"signal":"SIGINT","ms_after_last_signal":1}` | n/a | **130 signal death** — only reachable with restore-at-removal: a delivery-time `deliver` → `default_action` cannot run while the JS thread is in a sync loop (§4.1 case 3') | unanimous; `refute-doc16/spin.ts`, `rm_then_spin_{node,deno,bun}.js` |
| e4 | listener + pending `accept()` + SIGINT | survives | survives, accept still pending | survives | hang | **survives; accept untouched** | unanimous |
| f | listener + `while(true){}` + SIGINT | hang; SIGTERM 143 | hang; SIGTERM 143 (`{"label":"f.deno.thenTERM#0","code":143,"signal":"SIGTERM","ms_after_last_signal":2}`, identical on #1/#2; SIGINT alone: `{"label":"f.deno.INTonly","code":137,"signal":"SIGKILL","watchdogged":true}`) | hang; SIGTERM 143 | interrupt kills loop, exit 0 | **hang; SIGTERM 143** | unanimous; den gives up its "better" behaviour for parity and simplicity |
| g | 5000 `console.log` **events** to a pipe + SIGINT | 5000 (async pipe loses lines when stalled) | 5000, sync write | 5000, sync write | 5000 events = 15 000 physical lines under `.pretty()` (fact 9) | **5000 events** (LineWriter, fact 9); drained pipe only | Deno/Bun shape |
| h | `beforeExit`/`exit`/`unload` on default SIGINT | neither | neither | neither | n/a | **nothing runs** | unanimous |
| h' | listener calls `process.exit(7)` | 7, `exit` fires | 7 (`e2_sig_timer_exit.js`, no `unload` listener: `{"label":"hp.deno.exit7","code":7,"signal":null,"stdout_last":"e2: handler -> Deno.exit(7)"}`); `unload` fires — shown by the separate `h2_unload_exit.js`, which calls `Deno.exit(5)`: `{"label":"hp.deno.exit5","code":5,"signal":null,"stderr_head":"h2: handler -> Deno.exit(5) ¶ h2: unload fired"}` | 7 | 7 | **7** (`den-stdlib-process/src/lib.rs:100` = `std::process::exit`, called on the JS thread with the runtime mutex held — §3.4, R5) | unanimous |
| i | worker `while(true){}`, parent SIGINT | 130 | 130 | 130 | 0 via `RealmStop` | **130 signal death, no join** | unanimous |
| j | `worker.terminate()` on a spinning worker | returns in 1 ms, exit 0 | **hangs**, worker at 99 % CPU (`{"label":"j.deno","code":137,"signal":"SIGKILL","watchdogged":true,"stdout_last":"j: terminated"}`; `U4-deno/src/runtime_web_worker.rs:321-331` `terminate()` only swaps `termination_signal` and wakes the loop, never `terminate_execution`) | 0 ms, exit 0 | works (interrupt) | **Node/Bun**: per-worker token → interrupt handler | Deno's own comment, `U4-deno/src/runtime_ops_worker_host.rs:147-148`: "`terminate()` alone … can't interrupt synchronous JS already in flight" |
| k | uncaught top-level throw | 1 | 1 | 1 | 0 | **1** (CLI `exit(1)` after printing) | unanimous |
| k2 | throw in timer cb / unhandled rejection | 1 | 1, later timer never fires | 1 immediately | 0, continues | **optional commit 7**: 1 via CLI-installed policy | unanimous; kept optional because den-core must stay embedder-neutral |
| k' | top-level `await new Promise(()=>{})`, no signal | 13 + warning | 1 + "never resolved" | hangs | hangs (0 on Ctrl-C) | **hangs like Bun; Ctrl-C → 130** | out of scope (not cancellation) |
| l | SIGTERM default | 143 | 143 | 143 | 143 | **143**, untouched | unanimous |
| m | exit-status mechanism | own handler + re-raise; `SIG_DFL` at listener removal | `SIG_DFL` + `tgkill` at delivery | no handler until a listener; `SIG_DFL` at listener removal | `exit_group(0)` | **Bun's** until a listener exists, **Node/Bun's** once listeners are removed; never `exit(130)` | signal death is what `make`/`systemd`/`child_process` observe |
| REPL | Ctrl-D / Ctrl-C ×2 with a pending timer | exits at once | exits at once | n/a | *unverified* — every harness here uses piped stdio and rustyline needs a pty (`rustyline-18.0.1/src/tty/unix.rs:1631`); the earlier "0 via token, 26 ms" figure has no probe behind it | **`std::process::exit(0)` after history close** | same visible result |

¹ The earlier Node cell cited `SIG_DFL + kill(pid, SIGINT)` from `U3-node/e3_handler_reraise.js`. That script's own
body is `process.removeAllListeners('SIGINT'); process.kill(process.pid,'SIGINT')` — the *script* re-raises, not
Node. `refute-doc16/node_n_remove.js` (add, `removeListener` at 300 ms, external SIGINT at 1 s) is the clean run.

---

## 3. rquickjs facts that make cancellation-by-drop sound

Probe crate: `/tmp/parity/scratch/cancel/U2-drop-probe/` (rquickjs `=0.12.2`, den's feature set, tokio `=1.53.1`,
multi-thread runtime, rustc 1.98.0). Probes A-D live in `src/main.rs` (full output in U2 §5); the later R1-R9
series is `src/bin/refute.rs`, run as `target/debug/refute R<n>`, full log at
`/tmp/parity/scratch/cancel/u7-refute/all.log`. R6 (SIGTERM into a select! stuck under a tight loop) must be
driven by `u7-refute/r6.sh` (`refute R6 & … kill -TERM $pid; wait $pid`) — `timeout(1)` reports 124 whatever the
child died of; the saved run is `doc16-revise/r6.log`: `R6 entering the select! with a tight JS loop under idle();
expect no delivery` / `[helper] raised SIGINT at 150ms` / `R6 wait status=143`.

### 3.1 Drop order with pending spawned futures

1. Last `Arc` of `AsyncRuntime` (`core/runtime/async.rs:77-82`) → `InnerRuntime::drop` (`:44-48`) runs
   `drop_pending()` (`:36-41`, frees contexts other threads deferred) then drops `RawRuntime`.
2. `RawRuntime::drop` (`core/runtime/raw.rs:123-133`): `Box::from_raw(JS_GetRuntimeOpaque)` → `opaque.clear()`
   (`:128`) → `JS_FreeRuntime` (`:129`) → `drop(opaque)` (`:130`).
3. `Opaque::clear` (`core/runtime/opaque.rs:284-292`): rejection tracker, interrupt handler, prototypes,
   **`spawner.take()` (`:290`)**, userdata (`:291`).
4. `Drop for Schedular` → `clear()` (`schedular.rs:236-240`) → `pop_task_all` (`:214-218`) → `task_drop` (`:109`) →
   `ManuallyDrop::drop(&mut future)` (`schedular/vtable.rs:51-54`). The future's destructor runs with the `JSRuntime`
   fully alive; its `Function`/`Value` → `JS_FreeValue`, its `Ctx` clone → `JS_FreeContext` (`core/context/ctx.rs:97-101`).
5. Only then `JS_FreeRuntime` (`qjs/quickjs.c:2288`). Its `assert(list_empty(&rt->gc_obj_list))` (`:2348`, compiled
   in — den does not set `disable-assertions`, `rquickjs-sys build.rs:147-148`) fires only if a JS ref leaked past
   step 4 (a `static`/`mem::forget`), which is a bug regardless. `JS_ABORT_ON_LEAKS` is never set (`raw.rs:38-103`).

Verbatim: `A idle() resolved=false after 200.21377ms (idle future dropped by select)` / `A AsyncContext dropped` /
`A pending future dropped; f()=42 globalThis.marker=7 (JS runtime still alive at drop time)` / `A AsyncRuntime dropped`
/ `A ok`. Note the future dies at `drop(rt)`, not `drop(ctx)`, because its `Ctx` clone holds a `JSContext` ref
(`core/context/owner.rs:93-100`, `core/context/async.rs:80-115`). angle-c P4: `dropped context+runtime with a
pending 60s spawned future in 161.765µs`.

### 3.2 Dropping `idle()` mid-await

`idle()` takes the mutex at `core/runtime/async.rs:314`; the guard is borrowed by the `ManualPoll` closure
(`:318-354`) and released when the future is dropped. Nothing inside owns a JS value across polls. Spawned futures
are untouched; on re-entry `Schedular::poll` re-registers the waker (`schedular.rs:145`) and pops from the intrusive
`should_poll` queue (`:152`), so a wake that lands while `idle()` is dropped is not lost. The probe that actually
shows this is R1: its JS listener busy-loops for 200 ms inside the deliver arm, so the 500 ms timer's wake lands
squarely inside a drop window — `R1 SIGINT #2 entered JS at 350.146259ms, left at 550.129081ms (hits=2)` /
`R1 idle() returned at 751.474462ms: hits=2 fired=1 late=1 delivered=2`. angle-a E (`hits=2 fired=1`) proves
nothing here: its listener is a single synchronous `onsig()` (`angle-a/probe/src/main.rs:58`), signals fire at
200/400 ms and the timer at 600 ms (`:9-11`), so every drop window is sub-millisecond and no wake ever falls in one.
A panic inside a spawned future is contained by the `Defer` at `schedular.rs:185`.

Two more properties R1 and R7 pin down, both consequences of `idle()` holding the mutex for its whole parked
lifetime (`async.rs:314`; research 09 fact 4):

- **A listener that starts async work keeps den alive until it finishes.** `late=1` above is a 200 ms `ctx.spawn`
  issued from inside the deliver arm at ~350 ms; the re-entered `idle()` picked it up and only returned at 751 ms.
  Async cleanup handlers (`addSignalListener("SIGINT", async () => { await db.close(); process.exit(0) })`) rely on
  exactly this, and nothing else in this note proved it.
- **Only the root `select!` may touch the realm from outside.** Any `context.with`/`async_with` issued by another
  task parks on the mutex until the loop yields: `R7 outsider task calls context.with at 101.281397ms` /
  `R7 idle() returned at 600.558059ms` / `R7 outsider's context.with returned at 600.652382ms`. Rule: every
  in-realm pump stays a `ctx.spawn` future (as `NativeWorker::pump_faults` already is,
  `den-stdlib-worker/src/worker.rs:479-490`), and no `Engine::eval`/`run_module` may be called from a second task
  while `run_event_loop` is parked. The REPL pump (§4.4) obeys this today by being `ctx.spawn`-ed (`src/app.rs:49`).
Mid-poll drop of the runtime is impossible on one thread (`task_drive` at `:189` is synchronous under the lock) and
another thread cannot drop the last `AsyncRuntime` while `idle(&self)` borrows it (`:313`).

`async_with` releases the lock at every `Pending` (`core/context/async/future.rs:154`, `mem::drop(lock)` on both
exits) and polls the spawner between polls (`:113-114`). That is what lets the root loop `context.with(...)` a
listener while `run_module`'s entry promise is pending (judge `dw-probe`: `SIGINT#1 delivered via context.with at
101ms while entry pending; hits=1`, `#2 at 202ms`, `entry resolved with 42 at 300ms`).

### 3.3 The interrupt handler

Type `Box<dyn FnMut() -> bool + Send + 'static>` (`core/runtime.rs:51-52`); trampoline wraps in `catch_unwind`
(`core/runtime/raw.rs:390-423`). `set_interrupt_handler` takes the runtime mutex (`async.rs:187-189`) — install once
before running, flip a flag afterwards. Probe D: `flag still true, handler still armed -> counted loop =
Err(Some("interrupted"))`, `flag cleared -> same context keeps running: counted loop = 19999900000`,
`handler removed at runtime, flag=true -> counted loop = 19999900000`. Two consequences: while the flag is true every
later eval dies (clear it if JS must run again), and `Arc<AtomicBool>` is sufficient.

### 3.4 `process::exit` while another thread is inside `idle()`

`std/process.rs:2537-2540` = `rt::cleanup()` then `libc::exit`. `rt::cleanup` flushes stdout via `try_lock`
(`std/io/stdio.rs:727-743`, cannot deadlock on a held lock). `libc::exit` runs `atexit` handlers; `quickjs.c`
registers none and `quickjs-libc.c` is not compiled (`rquickjs-sys build.rs:143`). No Rust destructors run on any
thread. Probe C: `C second thread ThreadId(18) calling std::process::exit(130) while JS thread spins in idle()` →
`pipestatus=130` with all four stdout lines. `raise(SIGINT)` under `SIG_DFL` runs *nothing* — flush explicitly first.

den's own `process.exit()` is a different shape from probe C: `den-stdlib-process/src/lib.rs:100` is
`std::process::exit` called **on the JS thread, inside a JS call, with the runtime mutex held**. Probe R5 runs that
exact shape through a pipe: `R5 stdout line written before exit (must survive)` / `R5 calling std::process::exit(7)
from inside context.with (runtime mutex held) on ThreadId(1)` / `exit=7` — both stdout lines intact, exit code
honoured. So row h' needs no special casing.

### 3.5 Limits

- A spawned future cannot be dropped *individually* from outside; only the whole spawner (runtime drop) or the
  owner's far end (§1.6 b) ends it. That is why the process is the scope for the CLI.
- A thread blocked inside `JS_CallInternal` cannot be dropped; only the polled flag (§3.3) or the kernel ends it.
- `set_interrupt_handler` cannot be installed while `idle()` runs (mutex). Install at build time.
- The one `block_in_place` loader left in the tree — `den-core/src/loader/http.rs:73`
  `tokio::task::block_in_place(move || Handle::current().block_on(task))`; `mmap_script.rs` no longer has one —
  blocks a tokio *runtime drop* until it returns (u6 `[rt-drop] 1.4 s`); `process::exit` does not wait for it.

---

## 4. The chosen design

Winner: Angle A ("the process is the scope; SIGINT owned by den-stdlib-process; root loop = `select!(signal
inbox, idle())` + `deliver_while`"), both judges 33/40. Grafts from B: land the deletion first with zero new code;
`impl Drop for WorkerHandle`; the SIGTERM-proxy demonstration. Grafts from C: inbox as
`RefCell<Option<UnboundedReceiver>>` in `SignalHub` userdata (no `Arc<tokio::Mutex>`) — but *borrowed* by each
loop phase and put back, not consumed (§4.1 case 2, R2); this ADR; the self-signalling e2e test; optional
`FatalUncaught` exit-1 policy. Two corrections the R-series probes forced: `SignalHub::install` moves to
`Engine::build` (R9), and the last-listener restore moves from `deliver` to `remove` (§1.5, row n').

### 4.1 Ctrl-C flow

**Case 1 — no JS SIGINT listener (default; probes a, b, c, d, g, i, k).** den installs nothing
(`hook_ctrlc_handler` deleted; only remaining subscriber is `SignalHub::add`). Ctrl-C → kernel `SIG_DFL` → every
thread dies wherever it is: inside `JS_CallInternal`, inside `idle()` holding the mutex, inside `reqwest`, inside
`accept`, a worker mid-loop. No Rust runs; shell sees 130 as signal death (angle-a `[E2] code=130 signal=SIGINT`
with a spawned future pending; angle-b SIGTERM proxy on today's binary `a: exit=143 after_signal_ms=2`,
`f: … stdout_lines=5000 last=line 4999`). Pending I/O is abandoned exactly as U3 c / U4 c / U5 c.

**Case 2 — listener present, JS parked on I/O or a timer (e, e3, e4).** `SignalHub::add` for the first listener
of a signal → `tokio::spawn(forward(signal(kind) → inbox_tx))` — a tokio task, not `ctx.spawn`, so it does not pin
`idle()`. tokio's libc handler is installed lazily here (Bun m'). SIGINT → pipe byte → forward task →
`inbox_tx.send("SIGINT")` → root loop:

```rust
// den-stdlib-process/src/signal.rs
// Never a ready `None`: with no hub (engine built without den:process) the arm must sleep for ever,
// otherwise `select!` disables it on the first poll and no signal is ever delivered.
async fn recv(inbox: &mut Option<UnboundedReceiver<String>>) -> String {
  match inbox {
    Some(rx) => rx.recv().await.unwrap_or_else(|| std::future::pending().await),  // all senders gone: also never
    None => std::future::pending().await,
  }
}

// Engine::run_event_loop (cfg stdlib-process) == SignalHub::drive
let mut inbox = context.with(|ctx| SignalHub::take_inbox(&ctx)).await;   // RefCell::take → Option<UnboundedReceiver<String>>
loop {
  tokio::select! {
    biased;
    sig = recv(&mut inbox) => context.with(|ctx| SignalHub::deliver(&ctx, &sig)).await,  // idle() dropped, lock free
    () = runtime.idle() => break,                                                       // spawner empty
  }
}
context.with(|ctx| SignalHub::put_inbox(&ctx, inbox)).await;             // RefCell::replace: back for the next phase
```

`deliver_while` has the same first and last line around its own `select!` (the entry arm may `?` out — put the
receiver back before propagating). Ownership rule, stated once: **the receiver lives in `SignalHub`'s
`RefCell<Option<UnboundedReceiver<String>>>`; `deliver_while` and `drive` each take it on entry and put it back on
exit; nothing consumes it.** The rule exists because a once-only take serves exactly one of the two phases the CLI
runs in sequence (`src/main.rs:79-96` → `run_module`, then `:103` → `run_until_end` → `run_event_loop`): probe R2
ran the `deliver_while` shape and then asked for the receiver a second time, as `run_event_loop` would —
`R2 SIGINT delivered via context.with at 100.31893ms while entry pending; hits=1` / `R2 … at 200.405242ms …; hits=2` /
`R2 entry resolved with 42 at 300.602086ms` / `R2 second take_inbox (what run_event_loop would get after
deliver_while) -> None`. With a consuming take, every SIGINT after the entry module returns is dropped while tokio's
handler stays installed (fact 7): Ctrl-C would do nothing at all. Signals that arrive in the gap between the two
phases sit in the unbounded channel and are delivered by the next take.

The losing `idle()` is dropped (§3.2), listeners run under `context.with`, `idle()` is re-entered. A second Ctrl-C is
another message (U3 e, U5 e). The process ends when JS calls `process.exit` or the spawner drains. Listener-only
script: nothing spawned → `idle()` returns → exit 0 (e2). Last listener removed → `SignalHub::remove` flushes
stdout/stderr and calls `libc::signal(n, SIG_DFL)` *right there* (Node/Bun, §1.5); the kernel does the killing on
the next SIGINT and no runtime code is needed. `deliver` reaching an empty list can still happen for a signal that
was already queued before removal; it runs `default_action` (flush, `SIG_DFL`, `libc::raise(n)`; fallback
`std::process::exit(128 + n)` on non-unix — angle-a E tail `[E] code=130 signal=SIGINT`) as a second line of
defence, never as the primary mechanism. Both paths leave the tokio forwarder and the `watching` entry alone
(commit 6 note).

**Case 3 — listener + tight sync loop (f).** `idle()`'s poll is inside `JS_CallInternal`; the `select!` never runs;
the signal queues; SIGTERM is untouched → 143. angle-a `[E3] code=137 signal=SIGKILL` (watchdog) = Node f / Deno f /
Bun f; R6 reproduces it with the exact `select!` shape: `R6 wait status=143` under `r6.sh`.

**Case 3' — listener added, removed, then a tight sync loop (row n').** Same mechanics as case 3, so a
*delivery-time* restore (`deliver` → `default_action`) could never run and den would hang where Node/Deno/Bun all
die in ≤2 ms (`rmspin.{node,deno,bun}` `code=130,signal=SIGINT`). That is why the restore happens in `remove`
(case 2): by the time the loop spins the disposition is already `SIG_DFL`, and the kernel needs no lock.

**Case 4 — signal while the entry module's top-level await is pending** (a server: `addSignalListener` then
`await serve()`). `Engine::run_module` wraps its `async_with` in `SignalHub::deliver_while(context, pin!(entry))`
— the same `select!` with `out = &mut entry => break out`; the entry is polled by `&mut`, never dropped, and
`async_with` frees the lock at each `Pending` (§3.2). Without this arm (Angle C) Ctrl-C would be silently queued
for any `for await (const conn of listener)` server — a verified gap.

This arm is only useful if the inbox *exists* before the entry module runs. **It does** — corrected by
[17](17-graceful-shutdown-and-external-stop.md) §4.1: `SignalHub::install`
(`den-stdlib-process/src/lib.rs:133-134`) is reached from the module's evaluate hook
(`lib.rs:196-198`, `#[qjs(evaluate)] … crate::install(ctx, exports)`), and that hook runs for **every** realm at
context creation, because `den:process` is one of the `Module::evaluate_def`'d modules (`engine.rs:439` →
`:408-411`; ARCHITECTURE.md §3) — not only when the entry module imports it. Verified on the built binary with an
import-free script: `has addSignalListener =, function`.

Probe R9 (`R9 take_inbox before the entry module -> None` / `R9 entry returned 1 at 800.994669ms; hits=0; process
still alive after 2 SIGINTs` / `R9 signals that were queued in the untaken inbox: 2`) ran in the bare rquickjs probe
crate, which installs no hub of its own; it demonstrates the *failure mode* of a missing hub, not den's behaviour.
The commit-6 relocation of `SignalHub::install` into `Engine::build` is therefore **not required** — the receiver is
already `Some` from the first take on, and an engine built without the feature falls into the `None` →
`pending()` branch above. What commit 6 must still guarantee is that the take is shared across both phases
(`deliver_while` and `drive`), per R2.

**Case 5 — REPL.** See §4.4.

**Case 6 — workers.** Parent Ctrl-C = process-wide death (i). `terminate()` = per-worker token (§4.5).

### 4.2 Residual shared state (each with why it is unavoidable)

| State | Owner | Why unavoidable |
|---|---|---|
| POSIX disposition + tokio's process-wide signal registry | the OS; touched lazily by `SignalHub::add` only | `sigaction` is per-process; tokio's handler is permanent (fact 7). Bun/Node also install at listener time (U5 m', U3 strace). Restored only immediately before `raise` |
| Per-realm signal inbox (`UnboundedSender` in the forward tasks, `RefCell<Option<UnboundedReceiver>>` in `SignalHub` userdata, created in `Engine::build`; **taken on entry and put back on exit by each of `deliver_while` and `drive`**, never consumed — R2 `second take_inbox … -> None` is what a one-shot take gives the second phase) | `den-stdlib-process` | `idle()` holds the mutex while pending (research 09 fact 4; R7 `outsider's context.with returned at 600.652382ms` after a 101 ms call), so a signal enters JS only by dropping `idle()` from the root `select!`, and that select must be woken by something that is *not* a spawned future (fact 11). A mailbox, not cancellation. Corollary rule (§3.2): only the root select touches the realm from outside; everything else is `ctx.spawn` |
| Detached `Handle::spawn` tasks in `den-stdlib-networking/src/websocket.rs:241,396` (handshake / frame pump) | the `WebSocket` object via its channel ends | Not cancellation state — they hold no token and do not pin `idle()`; listed so this table is an actual inventory. They end when the socket or the `event_tx` receiver goes (§1.6 b) or when the process dies |
| Per-worker `CancellationToken` `stop` (+ child `closing`) — `worker.rs:392,471,564,624` | the `Worker` object / the parent's `WorkerRegistry` | (a) bytecode on another thread stops only via a polled `Send` flag (§1.4); (b) a parked worker needs an async wake to drop its `idle()` (`:673`); (c) `close()` must not arm the interrupt (research 09 fact 14), so wake and flag must be separable — `child_token()` gives that. One handle, no parent link, no tree. Node j 1 ms / Bun j 0 ms parity |
| `WorkerRegistry { Vec<WorkerHandle { stop, join }> }` userdata (`worker.rs:386-428`) | the parent realm | Ownership, not cancellation: `impl Drop for WorkerHandle { self.stop.cancel() }` makes drop = cancel one level down; `worker::shutdown` (`:435-461`) joins for tests. Dropped with the context (`opaque.rs:291`) |
| Per-timer token (`den-stdlib-timer/src/lib.rs:13,81,90`) | `clearTimeout` | The user's accepted case; nothing else reads it |
| Per-port `run`/`stop` (`port.rs:54,71`), per-channel `stop` (`broadcast.rs:69`) | `close()`/`pause()` | Resource handles; `pause()` must end the pump but keep the queue (`port.rs:364-378`); never connected to a realm |
| `AbortSignal` `Arc<AtomicBool>` (`fetch_op.rs:29-66`) | the request | deno_core `FetchCancelHandle` shape (`ext/fetch/lib.rs:705-716`) |

Deleted: `Engine.stop_token` (`engine.rs:122`), `new_with_stop_token` (`:172`, zero callers), the
`CancellationToken::new()` arguments in `Engine::new` (`:153`) and `new_with_bundle` (`:163`), the main-realm
interrupt handler (`:376-381`, through the trailing `.await;`), timer `StopToken` (`:428`, `timer/lib.rs:45,92`), `RealmStop` (`:463`,
`worker.rs:382,529-530,658`), `Engine::stop()` (`:857`), the cancel + 500 ms idle in `shutdown` (`:597,600-601`),
`hook_ctrlc_handler` (`app.rs:88-96`), `run_until_cancelled(run_file)` (`main.rs:81-86`), the REPL cancel
(`app.rs:32`), the pump `select!` (`app.rs:64-72`), `tokio-util` from `den-core/Cargo.toml:33`, `"signal"` from
`Cargo.toml:688`.

### 4.3 Engine API

```rust
#[derive(Clone)]
pub struct Engine { pub runtime: AsyncRuntime, pub context: AsyncContext }

Engine::new() / Engine::new_with_bundle(bundle)      // private build(bundle)
engine.run_file(path) / run_module(name) / eval(..)  // unchanged; run_module wrapped in deliver_while under cfg(stdlib-process)
engine.run_event_loop().await                        // cfg(stdlib-process): SignalHub::drive; else runtime.idle()
engine.runtime.idle().await                          // still public, for tests wanting the raw loop
engine.shutdown().await                              // tests/embedders: worker::shutdown (cancel + bounded join) + rejection clear + gc; no cancel, no 500 ms idle
drop(engine)                                         // = cancel this realm (§3.1); WorkerHandle::drop cancels children
// PRECONDITION: `Engine` is `Clone` (engine.rs:118) and drop only cancels when the LAST clone dies. Never move an
// `Engine` clone into a `ctx.spawn`ed future (the src/app.rs:45-49 REPL-pump shape) — that is a cycle: the future
// keeps the runtime alive, the runtime keeps the future alive, and `drop(engine)` is a no-op. [17] §4.1.
// removed: stop_token, stop(), new_with_stop_token
// embedder kill switch: engine.runtime.set_interrupt_handler(Some(Box::new(move || flag.load(Relaxed)))).await before running
```

`WorkerHost::build_engine(&self, base: BaseUrl) -> Result<WorkerEngine, WorkerHostError>` (`host.rs:60-62` loses
the token); the worker crate installs the interrupt handler on the returned runtime itself. Drop order is safe
either way: `ContextOwner` holds its own `rt: AsyncRuntime` (`core/context/owner.rs:87-100`).

### 4.4 REPL

`start_repl_session` (`src/app.rs:23-37`): `tokio::spawn(async move { repl::run_repl(repl_tx).await;
std::process::exit(0) })`. `run_repl` closes the SurrealKV history before returning (`src/repl.rs:63-66`), so the one
thing needing an orderly close is done; `exit` flushes stdout (§3.4) and does not wait for the blocking pool. Ctrl-C
at the prompt is a key, not a signal: rustyline raw mode clears ISIG (`rustyline-18.0.1/src/tty/unix.rs:1631`), so it
is `ReadlineError::Interrupted` (`repl.rs:44-49`): first press hints, second exits. Because `run_repl` re-enters
`readline` immediately after `send` (`repl.rs:41-50`), Ctrl-C twice also ends a REPL line stuck in `while(true){}`
— no interrupt handler needed. `repl_pump` becomes `while let Some(source) = repl_rx.recv().await`. History
durability under signal death: each accepted entry is committed synchronously (`src/history.rs:166-167`) and
SurrealKV's directory lock is an `fs2` flock the kernel releases (`surrealkv-0.21.4/src/lockfile.rs:79`).
`kill -INT` from outside with no listener = `SIG_DFL` death, as for any script.

### 4.5 Workers

`terminate()` → `NativeWorker.stop.cancel()` (`worker.rs:499`), unchanged. Observers: the worker runtime's
interrupt handler, now installed by `WorkerThread::serve_engine` right after `build_engine(base)` (lock is free,
nothing has run) → uncatchable `interrupted` at the next back-edge; the child `closing` (`:624`) → drops the parked
worker's `idle()` (`:673`); the parent's fault pump (`:483`) ends at once. The thread runs `shutdown` for its own
children (`:675`), drops context then runtime (`:683-684`), the port end closes, the parent pump sees `Close`. New:
`impl Drop for WorkerHandle { fn drop(&mut self) { self.stop.cancel() } }` so an embedder dropping a parent
`Engine` stops spinning children (today `:391-394` has no `Drop`; a dropped parent leaks a spinner). Parent Ctrl-C:
kernel death, no join (i). Nested workers: no `RealmStop`; each realm's registry, torn down bottom-up as today.

---

## 5. Migration plan

Ordered, each commit independently green under `cargo test --workspace`. Harness rule: CLI tests spawn
`env!("CARGO_BIN_EXE_den")` via `std::process::Command` (inherits `SIG_DFL`; fact 12).

| # | Commit | Deleted | Added | Proof |
|---|---|---|---|---|
| 1 | `refactor(cli): let SIGINT keep its default disposition` — `src/app.rs`, `src/main.rs`, `Cargo.toml:688` | `hook_ctrlc_handler` (`app.rs:88-96`), `signal` import (`:3`), REPL `stop_token.cancel()` (`:28-34`), pump `select!` (`:64-72`), `run_until_cancelled` wrapper (`main.rs:80-86`), `"signal"` feature | `std::process::exit(0)` after `run_repl`; `while let Some(source) = repl_rx.recv().await`; plain `match app.engine.run_file(x).await` | new `tests/ctrlc.rs` (sketch below) cases 1-2: `status.signal() == Some(SIGINT)` for timer and `while(true){}` |
| 2 | `fix(cli): exit 1 on an uncaught top-level error` — `src/main.rs` | the `den --repl broken.js` fall-through: today the REPL block at `main.rs:98` runs *after* the `match` at `:81-95`, so a failed file still drops into the REPL. Exiting 1 from the `Err` arms removes that — **intended**: Node and Deno exit on a failed entry file and never open a REPL; nothing in den's tests relies on the fall-through | `std::process::exit(1)` in both `Err` arms after printing | `tests/ctrlc.rs` case 3: `code() == Some(1)` (U3/U4/U5 k); add case 3b: `den --repl broken.js </dev/null` exits 1, no "Welcome to den" banner |
| 3 | `refactor(timer): timers keep only their clearTimeout handle` — `den-stdlib-timer/src/lib.rs`, `den-core/src/engine.rs:424-428` | `StopToken` (`:45`), import (`:60`), lookup (`:91-95`), inner `stop.run_until_cancelled` + `.flatten()` at `:117-127,166-170,205-209`; the `store_userdata(StopToken)` line | `arm` returns `(u32, CancellationToken)`; `token.run_until_cancelled(X).await.is_some()` | `den-core/tests/unit/engine.rs:279-293` → `dropping_an_engine_with_a_pending_timer_returns_promptly`: `setTimeout(()=>{},60000)`, `drop(engine)`, `elapsed < 1 s` (§3.1) |
| 4 | `refactor(worker): a worker owns its stop token; dropping the parent cancels it` — `den-stdlib-worker/src/{worker,host,lib}.rs`, `den-core/src/engine.rs:84-106,455-463`, tests, **`ARCHITECTURE.md` §2 tail (`:80-81` "Workers still take a child of the same token (`RealmStop`)") and §7.2 (the `build_engine(&self, stop: CancellationToken, base: BaseUrl)` signature at `:441-444`, the `RealmStop` paragraph at `:452-454`)** — this commit is what deletes the code they describe | `RealmStop` (`worker.rs:371-382`, store `:651-660`, `lib.rs:26`), `child_token` lookup (`:525-530`), token param of `build_engine` (`host.rs:60-62`, `engine.rs:95-106`), `use tokio_util` in `host.rs:12`; **`BareHost`'s `WorkerHost::build_engine` impl `tests/unit/worker.rs:66-72` (`fn build_engine(&self, stop: CancellationToken, base: BaseUrl)` → `Self::build(stop, base)` at `:72`) loses its `stop` param or the crate does not compile**; `BareHost` `stop`/handler/`RealmStop` (`:98-127,167-187`). `tests/unit/worker.rs:12` `use tokio_util::sync::CancellationToken;` stays only because `Fixture` keeps a token | `let stop = CancellationToken::new()`; `set_interrupt_handler` in `serve_engine` after `build_engine`; `impl Drop for WorkerHandle` | `tests/unit/worker.rs:719-736` → `dropping_the_parent_realm_stops_a_spinning_worker` (drop context+runtime, `no_threads_named`); `den-core/tests/unit/engine.rs:121-141` deleted; `workers.rs:494-513` loses `engine.stop()` (`:509`), renamed `shutdown_terminates_and_joins_every_worker`; `terminate_stops_a_worker_that_never_yields` (`:358`) stays green |
| 5 | `refactor(core): Engine has no stop token` — `den-core/src/engine.rs`, `den-core/Cargo.toml:33`, `ARCHITECTURE.md` §1 tail and the §2 token paragraphs (§7.2 already done in commit 4) | `use tokio_util` (`:23`), `stop_token` field (`:122`) + init (`:481`), `new_with_stop_token` (`:166-174`), `build`'s token param (`:176`) **and both constructions of it — `Engine::new` `:153` `Self::build(CancellationToken::new(), Self::EMPTY_BUNDLE)` and `new_with_bundle` `:163` `Self::build(CancellationToken::new(), bundle)`**, interrupt handler (`:376-381`, including the `.await;` at `:381` — cutting `:375-380` leaves an orphan `.await;`), `shutdown`'s cancel + 500 ms idle (`:597,600-601`), `stop()` (`:853-857`), `tokio-util` dep | `build(bundle)`; `shutdown` doc: "reap workers; drop the Engine afterwards" | full `cargo test --workspace`; every `shutdown()` caller (`whatwg.rs:15,23`, `js.rs:15,23`, `wpt.rs:2131`, `e2e.rs`, `workers.rs:91`) unchanged |
| 6 | `feat(process): signal listeners are delivered by the event loop and never keep den alive` — `den-stdlib-process/src/{signal,lib}.rs`, `Cargo.toml` (`macros`), `den-core/src/engine.rs` (`build`, `run_module`, `run_event_loop`), `src/app.rs:54` | `watch`'s `ctx.spawn(listen_loop)` (`:205-211`); `Ctx` from `listen_loop`; `SignalHub::install` call in `lib.rs:134` (moves to `Engine::build`) | `inbox_tx`/`inbox` in `SignalHub`, created in **`Engine::build`** next to the other `store_userdata` calls (R9: installed from the evaluate hook, the inbox does not exist before the entry module); `tokio::spawn` forwarder; `take_inbox` **and `put_inbox`** — each of `deliver_while` and `drive` takes on entry and puts back on exit (R2); `recv` helper that is `pending()` on `None`; `deliver` (+ `default_action` as the queued-signal fallback); **`remove`: flush + `libc::signal(n, SIG_DFL)` when the last listener for `n` goes** (row n'); `drive`, `deliver_while`; `Engine::run_event_loop`; `run_module` under `deliver_while`; `App::run_until_end` calls `run_event_loop`. **Invariant to keep: `remove` (`signal.rs:185-203`) never clears `watching` (`:132,175,178`), and the tokio forwarder is deliberately never torn down** — tokio's handler cannot be uninstalled anyway (fact 7), the forwarder is what carries a later re-added listener's signals and any signal queued before removal (the `deliver` → `default_action` fallback in §4.1 case 2 depends on it still running), and an implementer who "cleans up" `watching` in `remove` gets a second forwarder on the next `add` (duplicate deliveries) and, in a delivery-time design, silently reverts SIGINT to tokio's permanent no-op handler. Re-adding a listener after `SIG_DFL` was restored needs the handler back: `add` must re-`sigaction` the disposition tokio installed when `watching` already contains `n` (save it in `remove` before overwriting; angle-b `sigprobe` showed save/restore working around tokio's `OnceLock`; not probed on this exact path → §7 q2) | `den-stdlib-process/tests/process.rs`: `a_signal_listener_does_not_keep_the_realm_alive` (`timeout(1s, idle())` succeeds; today never returns); `tests/ctrlc.rs` cases 4-7 and 9 |
| 6b | `fix(worker): signal listeners throw in worker realms` — `den-stdlib-process/src/signal.rs` (`add`), `den-stdlib-worker/tests` | — | `add` throws `TypeError("signal listeners are not available in workers")` when the realm has no root loop: `SignalHub::install` runs in every realm (`engine.rs:439` → `lib.rs:134`, or `Engine::build` after commit 6) but a worker's loop is `closing.run_until_cancelled(engine.runtime.idle())` (`worker.rs:673`) and never calls `drive`, so a worker's `addSignalListener` today registers a forwarder whose inbox nobody drains, silently. Node: "Signals are not available on `Worker` threads" (`process.md:618`). One line: the worker's `serve_engine` stores a marker userdata (or simply omits the hub) before any script runs | `tests/unit/worker.rs`: `add_signal_listener_in_a_worker_throws` |
| 7 | *(optional)* `feat(cli): uncaught errors in callbacks and unhandled rejections exit 1` — `den-stdlib-core/src/exceptions.rs:87-95`, `engine.rs` rejection report, `src/main.rs` | — | `FatalUncaught` userdata marker stored by den-cli; `print_exception`/rejection report end in `flush; exit(1)` when present | `tests/ctrlc.rs` case 8: throw in timer cb → `code() == Some(1)`, later timer never fires (U4 k2 / U5 k') |
| 8 | `docs: cancellation without tokens` — this file, `docs/research/README.md`, `README.md:198` checkbox, `ARCHITECTURE.md` §7.5: add rule 5 ("a signal listener does not keep den alive") **and rewrite `ARCHITECTURE.md:518` "Four rules, and they are the whole of it:" to "Five rules, and they are the whole of it:"** | — | — | — |

### 5.1 Integration test sketch — `tests/ctrlc.rs`

```rust
//! Signal-death parity with Node/Deno/Bun. Spawned via Command so the child inherits SIG_DFL, never a shell `&`.
#![cfg(unix)]
use std::{io::{BufRead, BufReader}, os::unix::process::ExitStatusExt, process::{Command, Stdio}, time::{Duration, Instant}};

const READY_TIMEOUT: Duration = Duration::from_secs(5);

fn den(script: &str) -> (std::process::Child, BufReader<std::process::ChildStdout>) {
  let dir = tempfile::tempdir().unwrap().keep();
  let path = dir.join("case.js");
  std::fs::write(&path, script).unwrap();
  let mut child = Command::new(env!("CARGO_BIN_EXE_den")).arg(&path).stdout(Stdio::piped()).spawn().unwrap();
  let out = BufReader::new(child.stdout.take().unwrap());
  (child, out)
}

fn wait_for_line(out: &mut impl BufRead, needle: &str) {
  let mut line = String::new();
  let start = Instant::now();
  while start.elapsed() < READY_TIMEOUT { line.clear(); out.read_line(&mut line).unwrap(); if line.contains(needle) { return } }
  panic!("never saw {needle:?}")
}

fn signal(child: &std::process::Child, sig: i32) { unsafe { libc::kill(child.id() as i32, sig) }; }

#[test]
fn timer_pending_sigint_is_signal_death() {
  let (mut child, mut out) = den(r#"setTimeout(() => {}, 1e9); console.log("armed")"#);
  wait_for_line(&mut out, "armed");
  signal(&child, libc::SIGINT);
  assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGINT));
}

#[test]
fn tight_loop_sigint_is_signal_death() {
  let (mut child, mut out) = den(r#"console.log("start"); while (true) {}"#);
  wait_for_line(&mut out, "start");
  signal(&child, libc::SIGINT);
  assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGINT));
}

#[test]
fn uncaught_top_level_throw_exits_1() {
  let (mut child, _) = den(r#"throw new Error("boom")"#);
  assert_eq!(child.wait().unwrap().code(), Some(1));
}

#[test]
fn listener_survives_two_sigints_then_dies_of_sigterm() {           // commit 6
  let (mut child, mut out) = den(r#"process.addSignalListener("SIGINT", () => console.log("caught")); setTimeout(() => {}, 1e9); console.log("armed")"#);
  wait_for_line(&mut out, "armed");
  signal(&child, libc::SIGINT); wait_for_line(&mut out, "caught");
  signal(&child, libc::SIGINT); wait_for_line(&mut out, "caught");
  signal(&child, libc::SIGTERM);
  assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
}

#[test]
fn listener_alone_does_not_keep_den_alive() {                       // commit 6; today hangs (u1 e2)
  let (mut child, _) = den(r#"process.addSignalListener("SIGUSR1", () => {})"#);
  assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn self_signal_reaches_the_listener() {                              // commit 6; deterministic, no external kill
  let (mut child, mut out) = den(r#"process.addSignalListener("SIGUSR1", () => { console.log("got it"); process.exit(0) }); process.kill(process.pid, "SIGUSR1"); await new Promise(() => {})"#);
  wait_for_line(&mut out, "got it");
  assert_eq!(child.wait().unwrap().code(), Some(0));
}

#[test]
fn signal_is_delivered_during_top_level_await_and_after_the_module_returns() {   // commit 6; R2 + R9 in one process
  // Phase 1: entry module is parked on a top-level await -> deliver_while must see it.
  // Phase 2: module has returned, run_event_loop owns the loop -> drive must see it with the SAME receiver.
  let (mut child, mut out) = den(r#"
    let n = 0;
    process.addSignalListener("SIGINT", () => console.log(`caught ${++n}`));
    setTimeout(() => {}, 1e9);
    console.log("armed");
    await new Promise(resolve => setTimeout(resolve, 1500));
    console.log("module done");
  "#);
  wait_for_line(&mut out, "armed");
  signal(&child, libc::SIGINT); wait_for_line(&mut out, "caught 1");   // inside deliver_while
  wait_for_line(&mut out, "module done");
  signal(&child, libc::SIGINT); wait_for_line(&mut out, "caught 2");   // inside drive
  signal(&child, libc::SIGTERM);
  assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
}
```

The `5000` in row g is an **event** count; if a test asserts on den's stdout it must count `console.log` events
(or set a non-pretty formatter), because `.pretty()` writes three physical lines per event (fact 9).

Add `libc` and `tempfile` to the root crate's `[dev-dependencies]` (both already workspace deps) and a
`[[test]] name = "ctrlc"` entry if the crate does not auto-discover `tests/`. Cases 4-6 and the last one are
`#[ignore]` until commit 6 lands. Note that case 4 (`listener_survives_two_sigints_then_dies_of_sigterm`) sends both
SIGINTs *after* the top-level `armed` print, i.e. entirely inside `run_event_loop`; with a consuming
`take_inbox` it would have failed against the original design (R2), which is why the last case exists.

---

## 6. Rejected designs

- **Angle B — install nothing, keep the signal pump as `ctx.spawn`.** Leanest diff and identical no-listener path,
  but a listener still pins `idle()` for ever (u1 e2: 137 vs 0 on all three), `e` never exits naturally, and its
  optional `sigaction` save/restore behind tokio's `OnceLock` is exactly the U6-idiom-7 subtlety (works in
  `sigprobe`, but edits state tokio believes it owns). Grafted: delete-first commit, `WorkerHandle::drop`, SIGTERM proxy.
- **Angle C — root `select!` in `App` after `run_file` returns, no `deliver_while`.** For `addSignalListener(..);
  await serve()` the loop never runs, tokio's handler is now installed, and Ctrl-C is inert until SIGTERM — worse
  than today. Also mis-stated the REPL path (tty is raw during evaluation, `repl.rs:41-50`). Grafted: inbox
  ownership, ADR, self-signal test, `FatalUncaught`.
- **`select!{idle(), ctrl_c()}` then drop the engine (u6 idiom 1 literal).** Sound (§3.1-3.2) but strictly worse
  for the CLI: turns signal death into `exit_group(130)` (`[exit130] … signal=null`) and returning from
  `#[tokio::main]` waits on a `block_in_place` loader (`[rt-drop] 1.4 s`). It is what an *embedder* does.
- **`TaskTracker` / graceful drain with timeout.** The token pattern renamed (`tutil/task/task_tracker.rs:22-23`
  "usually used together with `CancellationToken`"); Node does not do it either.
- **Keeping the main-realm interrupt handler for Ctrl-C.** Strictly more than any reference runtime offers, costs
  a global flag, and is unreachable once a listener exists (f). Deleted; embedders install their own (§4.3).
- **`Arc<AtomicBool> + Notify` per worker instead of a token.** Two objects for what one `CancellationToken` already
  gives (`is_cancelled()` + `cancelled()` + `child_token()` for `closing`), and the type stays in the tree for
  `clearTimeout` anyway.

## 7. Open questions / limits

1. **Windows.** No `SIG_DFL`; "install nothing" means the console's default Ctrl-C terminates. `deliver`'s default
   action falls back to `process::exit(128 + n)`; a removed listener leaves Ctrl-C at tokio's handler. Unprobed.
2. **Re-adding a listener after the last one was removed.** Commit 6 restores `SIG_DFL` in `remove`; a later
   `add` for the same signal must put tokio's handler back (`sigaction` save/restore, angle-b `sigprobe`), because
   tokio's `OnceLock` will not re-install it. Unprobed on this exact path; the fallback is to document "once
   removed, a SIGINT listener cannot be re-armed" and throw. (Signals in *worker* realms are no longer open: commit
   6b throws there.)
3. **Unhandled rejection / throw in a callback still exit 0** without optional commit 7. Node/Deno/Bun exit 1
   immediately; den-core must not `process::exit` on an embedder's behalf, hence the CLI-installed marker.
4. **Unsettled top-level await with nothing spawned** parks for ever (Bun does too; Node 13 + warning, Deno 1).
   Pre-existing, not cancellation; Deno's diagnostic is the best of the three.
5. **A dependency calling `tokio::signal` eagerly** would silently demote SIGINT process-wide (fact 7). Guard: CI
   grep keyed on the *call sites*, not imports — `signal::ctrl_c\(|signal\(SignalKind|SignalKind::interrupt\(\)` —
   must match only `den-stdlib-process/src/signal.rs:256-257,267,274` (the `use` at `:240` is not a subscription),
   plus `tests/ctrlc.rs` asserting `signal() == Some(SIGINT)`.
6. **Inherited `SIG_IGN`** (`nohup`, non-interactive `&`) makes Ctrl-C inert with no handler of den's own — POSIX
   convention and Bun's behaviour (U5 gotcha 1); Node/Deno override it because they install a handler. Deliberate.
7. **Worker stuck in a blocking loader** ignores `terminate()` until the load returns; bounded by `JOIN_TIMEOUT`
   (`worker.rs:62`). Pre-existing; moot on the Ctrl-C path.
8. **Partial stdout line** (no trailing `\n`) at signal death is lost; nothing in den emits one. The listener path
   flushes before `raise`.

## Probe directories

| Dir (`/tmp/parity/scratch/cancel/`) | What |
|---|---|
| `u1/` | den today: `run.sh`, `ref.sh`, scenarios a-k, `*.out/*.err`, reference runs |
| `U2-drop-probe/` | rquickjs drop/idle/interrupt/exit probes A, B, C, D (`src/main.rs`, `Cargo.toml`); `src/bin/refute.rs` = R1 (wake inside a held-open drop window + listener-spawned async work), R2 (one `take_inbox` cannot serve both loop phases), R3 (`set_interrupt_handler` blocks on the parked mutex), R4 (drop on another thread with pending futures), R5 (`process::exit` from inside `context.with`), R6 (SIGTERM into a select! under a tight loop), R7 (outsider `context.with` blocked for the whole parked `idle()`), R8 (flag stops a spawned loop), R9 (inbox taken before the entry module is `None`) |
| `u7-refute/` | `all.log` — full R1-R9 output; `r6.sh` — the `&`/`kill -TERM`/`wait` driver R6 needs |
| `doc16-revise/` | `r6.log` — the R6 run cited in §3 (`R6 wait status=143`) |
| `refute-doc16/` | `harness.ts` (`Deno.Command` harness reporting `code` **and** `signal`, fact 12); per-row runners `rows_abcd.ts`, `rows_e.ts`, `rows_e34.ts`, `rows_fgh.ts`, `rows_ijkl.ts`, `rows_kprime.ts`, `rows_den.ts`, `spin.ts` (row n'), `stall.ts` (row g stalled pipe), `rest.ts` (f.deno.thenTERM, h'); `strace_one.sh` + `m_{node,bun,deno}_{default,removed,…}.strace` (row e3/n mechanism); scripts with no U3/U4/U5 equivalent: `node_e2_listener_only.js`, `node_n_remove.js`, `bun_n_remove.js`, `node_e4_listener_accept.js`, `bun_e4_listener_accept.js`, `toplevel_throw.js`, `kprime.{mjs,js}`, `rm_then_spin_{node,deno,bun}.js`; den runs `den_{a_timer,b_loop,c_fetch,e2_listener,e_listener_timer,g_lines}.js`, `den_e.out` (row e today) |
| `U3-node/` | Node scripts, `run.sh`, straces (`m_default.strace`), saved `process.md`/`timers.md`/`net.md` |
| `U4-deno/` | Deno scripts, `batch*.sh`, straces (`m_*.trace`), fetched sources under `src/` |
| `U5-bun/` | Bun scripts, `out_*.txt`, `strace_*.txt`, `run.sh` (with `set -m`) |
| `u6/` | tokio idiom probes (`probe/src/main.rs`, `run.ts`, `den-ctrlc.ts`): exit130 vs sigdeath, trap, multi, rt-drop |
| `angle-a/` | probe E/E2/E3 (`select!(inbox, idle())`, SIG_DFL death, listener + tight loop), `sigterm-lines.sh` |
| `angle-b/` | SIGTERM proxy `run.sh` on today's binary; `sigprobe` (sigaction save/restore) |
| `angle-c/` | `p1.strace` (0 SIGINT sigaction lines), `p2.strace` (killed inside idle), `p3.out` (5000/5000), P4 (drop with pending future, re-entered idle) |
| `judge-parity/dw-probe/` | `deliver_while` shape: `context.with` between `async_with` polls |
| `judge-soundness/` | re-runs of angle-a E and angle-b sigprobe |

Intentional stop, graceful Ctrl-C and write-path guarantees: [17](17-graceful-shutdown-and-external-stop.md).
