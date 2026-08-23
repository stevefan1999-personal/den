# rquickjs 0.12 / quickjs-ng / tokio 1.53 — one QuickJS runtime per OS thread, and messages between them

Status: research, 2026-08-22. Supporting doc for the Web Workers feature (dedicated `Worker`,
one `std::thread` + one tokio runtime + one `AsyncRuntime` per worker). Every claim below carries a
`file:line` into the vendored sources or a probe line quoted verbatim from a run. Nothing is from memory.

A **compile-and-run-verified probe** (Appendix A) exercised every mechanism in this document on the exact
crate versions den uses: rquickjs 0.12.2 (`full-async, rust-alloc, parallel, indexmap, either, macro, futures`),
tokio 1.53.1, tokio-util 0.7.19, rustc 1.97.1, edition 2024. The probe lives in the session scratchpad
(`<scratchpad>/threads-probe/`), not in the repo.

## Sources read

| What | Path |
|---|---|
| rquickjs-core 0.12.2 | `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/src/` (abbreviated `core/` below) |
| rquickjs 0.12.2 facade | `.../rquickjs-0.12.2/Cargo.toml` |
| rquickjs-sys 0.12.2 + vendored quickjs-ng | `.../rquickjs-sys-0.12.2/quickjs/quickjs.c`, `quickjs.h`, `.../rquickjs-sys-0.12.2/src/bindings/x86_64-unknown-linux-gnu.rs`, `build.rs` (abbreviated `qjs/`, `sys/`) |
| tokio 1.53.1 | `.../tokio-1.53.1/src/` (abbreviated `tokio/`) |
| den | `den-core/src/engine.rs`, `src/app.rs`, `src/main.rs`, `den-core/src/loader/{http,mmap_script}.rs`, `den-core/src/resolver/http.rs`, `den-stdlib-timer/src/lib.rs`, `den-stdlib-core/src/cancellation.rs`, `den-stdlib-wasm/src/error.rs`, `ARCHITECTURE.md` |
| Specs (WebFetch) | MDN `Worker()` constructor, MDN `WorkerGlobalScope.importScripts()`, HTML Living Standard §10.2 (workers.html; the page is truncated by the fetcher after "terminate a worker", so constructor/importScripts steps are cited from MDN) |

---

Companion notes: [08](08-web-workers-spec.md) (spec conformance), [10](10-structured-clone-strategy.md)
(message format = `JS_WriteObject2` bytes + supplement), [11](11-workers-den-integration-and-tests.md)
(den integration, tokens, joining, tests). This note owns the rquickjs/quickjs-ng/tokio *mechanics*; where
the three disagree, the verification log at the end says which one to follow.

## TL;DR — the fifteen facts the implementer must not get wrong

1. **`AsyncRuntime` and `AsyncContext` are `Send + Sync` under `parallel`** (`core/runtime/async.rs:86-97`,
   `core/context/async.rs:244-251`); everything else that touches a `JSValue` — `Value`, `Object`,
   `Function`, `Persistent<_>` — is **`!Send`** (probe "NEGATIVE" compile: `*mut c_void cannot be sent between
   threads safely … within rquickjs::Value<'static>`). `Ctx<'js>` is `Send` but `!Sync` (`core/context/ctx.rs:103`;
   probe: `NonNull<JSContext> cannot be shared between threads safely`). **Nothing crosses a worker boundary but
   plain Rust data.** See [§1](#1-a-second-runtime-on-another-thread).
2. **`Persistent` cannot hop runtimes even on one thread**: it records the `*mut JSRuntime` it came from and
   `restore()` returns `Error::UnrelatedRuntime` for any other runtime (`core/persistent.rs:37-40, 102-111`). It is
   also `!Send` (probe). It is irrelevant to workers. See [§1.3](#13-persistent-is-neither-send-nor-cross-runtime).
3. **`idle()` does NOT resolve while a `ctx.spawn`-ed future is pending** (`core/runtime/async.rs:347-352`:
   `SchedularPoll::Pending => return Poll::Pending`, only `Empty` is `Ready`). Probe T4:
   `idle() pending while pump alive=true; idle resolved after sender dropped`. So "a live Worker keeps the process
   alive" **falls out for free** by spawning the parent-side inbound pump with `ctx.spawn` in the main context, which
   `App::run_until_end` already awaits via `idle()` (`src/app.rs:111-114`). See [§2](#2-the-event-loop-idle-drive-spawn).
4. **`idle()` holds the runtime mutex for its entire duration, including while pending**
   (`core/runtime/async.rs:314` acquires; the guard is borrowed by the `ManualPoll` closure until `f.await` returns
   at `:359`). Probe T12: `with() blocked while idle() pending=true`. Consequence: **the only way to get JS work into
   a context while `idle()` runs is a `ctx.spawn`-ed future** — a separate tokio task calling `async_with`/`with`
   blocks until `idle()` finishes. `drive()` is the opposite: it locks, polls, and **releases** on every poll
   (`core/runtime/spawner.rs:85-132`; probe T12: `with() while drive() running -> Ok(3)`). See [§2.3](#23-the-lock-who-may-touch-the-context-and-when).
5. **`async_with` releases the lock on every `Pending`** (`core/context/async/future.rs:106-156`: `mem::drop(lock)` at
   `:154` runs for both the `Ready` and the `Pending` exits). It also drives the spawner and runs pending jobs
   between polls of its own future (`:113-144`). See [§3](#3-dispatching-into-a-context-from-an-async-task).
6. **Interrupt = uncatchable `InternalError("interrupted")`** (`qjs/quickjs.c:8215-8219`). It fires on every
   backward jump (`OP_goto*`, `qjs/quickjs.c:18664-18677`), every function call (`:17590`), inside microtask jobs, and
   inside regex matching (`lre_check_timeout`, `:48453-48458`), once per 10 000 polls (`JS_INTERRUPT_COUNTER_INIT`,
   `:479`). `try/catch` cannot catch it (`:20328`). Probe T1: tight `while(true){}` returned `Err(Exception)` with
   `uncatchable=true message="interrupted"`, `js-caught=false`, 300 ms after the flag flipped. Probe T2: a
   `while(true) await Promise.resolve()` loop driven by `idle()` was also killed. **A script parked on an external
   future cannot be interrupted; cancel `idle()` instead** (probe T3). See [§4](#4-terminating-a-running-script).
7. **`set_memory_limit` works with `rust-alloc`** despite the stale doc comment at `core/runtime/async.rs:221-222`:
   quickjs-ng enforces the limit in `js_malloc_rt` *before* calling the custom allocator (`qjs/quickjs.c:1645-1646`),
   and sizes are tracked via the allocator's `usable_size` hook. Probe T6: `js-caught=Some("InternalError: out of
   memory")`, `malloc_limit=16777216`. OOM **is catchable** by JS. See [§5](#5-memory-and-stack-limits-per-worker).
8. **`block_in_place` panics on `current_thread`** (`tokio/runtime/scheduler/multi_thread/worker.rs:430-435`;
   probe P0: `panicked: "can call blocking only when running on the multi-threaded runtime"`). The worker runtime
   must be `Builder::new_multi_thread().worker_threads(1)`. With that, `block_in_place` works both inside
   `Runtime::block_on`'s root future (the `allow_block_in_place: true` path, `worker.rs:418-430`,
   `tokio/runtime/scheduler/multi_thread/mod.rs:91`) and inside spawned tasks (core hand-off, `worker.rs:480-492`).
   Probe T5: `under async_with=42, under idle()=42`. See [§6](#6-tokio-on-the-worker-thread).
9. **Classic script = `eval_with_options` with `global: true, strict: false, promise: false, filename: Some(url)`**
   (`core/context/ctx.rs:29-78, 184-205`). `importScripts` = the same eval per URL after loading the text
   **synchronously**; den's loaders already block via `block_in_place` (`den-core/src/loader/mmap_script.rs:92`,
   `http.rs:116`) but they return a *declared module*, so the worker needs the text-fetching halves factored out.
   Module worker = **`Module::import(&ctx, absolute_url)`**, *not* the `await import(url)` string `Engine::run_file`
   uses (`den-core/src/engine.rs:325-337`): `JS_LoadModule` resolves, links and runs the module's top level up to
   its first `await` **synchronously inside the call** (`qjs/quickjs.c:30999-31040`), whereas `import()` enqueues
   `js_dynamic_import_job` (`:31182`) which `async_with` only runs *after* it has polled the spawner
   (`future.rs:113-144`) — i.e. after a pre-spawned inbound pump has already dispatched any message the parent
   posted right after `new Worker()`. See [§7.3](#73-module-worker) and [§3.4](#34-when-to-spawn-the-inbound-pump).
10. **`Ctx::script_or_module_name(0)` gives the calling script's filename** (`core/context/ctx.rs:455-466` wraps
    `JS_GetScriptOrModuleName`, `qjs/quickjs.c:30890-30913`) — **if** the script was evaluated with
    `EvalOptions.filename` / a module name. Probe T8: `script(filename set, level 0)=Some("file:///tmp/main.js")`,
    `module=Some("file:///tmp/mod.mjs")`, but `eval without filename=Some("eval_script")` and
    `level 1=None`. `Engine::run_file` evaluates `await import(...)` *without* a filename, so the top-level frame is
    `eval_script`; the imported module frames do carry their path. See [§8](#8-base-url-for-new-workerwjs).
11. **Uncaught errors**: sync throw → `Err(Error::Exception)` + `ctx.catch()`; `Exception::message()/stack()`
    (`core/value/exception.rs:73-91`); quickjs-ng puts file:line:col **only in `stack`** (probe T9 own props =
    `message`, `stack`; stack = `"    at f (/tmp/x.js:2:26)\n    at <eval> (/tmp/x.js:3:1)\n"`). Module top-level
    throws and failed `Module::import` come back as **rejected promises**, not `Err` (probe T9). Unhandled rejections:
    `AsyncRuntime::set_host_promise_rejection_tracker` exists (`core/runtime/async.rs:163-171`, type at
    `core/runtime.rs:44-45`); it fires `handled=false` at rejection time and again `handled=true` if a handler is
    attached later (probe T7). **`idle()` prints job errors to stdout and swallows them** (`core/runtime/async.rs:329-342`;
    probe: `error executing job: Error: interrupted`); `drive()` ignores them (`core/runtime/spawner.rs:117-122`).
    See [§9](#9-uncaught-errors-and-unhandled-rejections).
12. **`ArrayBuffer::new(ctx, vec)` + `detach()` is UB in rquickjs 0.12.2** — `JS_DetachArrayBuffer` calls the
    free hook once (`qjs/quickjs.c:58037-58038`), the finalizer calls it again with `data == NULL`
    (`:57934-57936`), and rquickjs's hook does `Vec::from_raw_parts(NULL, …)` (`core/value/array_buffer.rs:100-103`).
    The probe aborted on it (`unsafe precondition(s) violated: NonNull::new_unchecked requires that the pointer is
    non-null` inside `ArrayBuffer::new::drop_raw`). **Use `ArrayBuffer::new_copy` for anything that may be
    transferred.** Also: `BigInt::to_i64()` silently returns `Ok(0)` for `2n**70n` (probe T10) — stringify anything
    outside i64. See [§10](#10-data-crossing-threads-the-structured-clone-raw-material).
13. **The message format is `JS_WriteObject2` → `Vec<u8>` → `JS_ReadObject2`** (decided in doc 10; bindings
    `sys/bindings/x86_64-unknown-linux-gnu.rs:1683-1715`, nothing in rquickjs-core wraps them). In this quickjs-ng
    (0.15.1, `qjs/quickjs.h:1410-1412`) the writer handles primitives, BigInt, boxed primitives, Date, RegExp,
    Array, plain Object, ArrayBuffer, every typed array, **Map and Set** (`BC_TAG_MAP/SET`, `qjs/quickjs.c:37651-37652`,
    writer `:38334-38341`) and object references/cycles (`JS_WRITE_OBJ_REFERENCE`), but **refuses `Error`, `DataView`,
    `DOMException`, functions** (`default:` → `TypeError "unsupported object class"`, `:38342-38348`), **throws on
    accessor properties** (`"only value properties are supported"`, `JS_WriteObjectTag`) and **accepts `Symbol`**
    (`:38361-38372`) which the spec must reject. §10's table is the raw material for the *supplement* that handles
    those cases, not a competing format. See [§10.0](#100-the-serializer-quickjs-ng-already-has).
14. **`close()` is not an interrupt.** It must let the current task finish (HTML "close a worker", doc 08 §2.8), so it
    cancels a separate `closing` token that the inbound pump and `run_until_cancelled(idle())` observe — never the
    interrupt token. `terminate()` cancels both. Because den-stdlib-timer timers hold their *own* tokens
    (`den-stdlib-timer/src/lib.rs:26-29, 55-58`), a pending `setTimeout` would otherwise keep the worker's `idle()`
    pending forever after `close()`. See [§4.4](#44-four-situations).
15. **quickjs-ng's default JS stack limit is 1 MiB, not 256 KiB** (`qjs/quickjs.h:423-424`, `qjs/quickjs.c:2014`;
    the rquickjs doc comment at `core/runtime/async.rs:231` is stale). den disables the check (`set_max_stack_size(0)`,
    `engine.rs:40`); a worker on a default 2 MiB `std::thread` stack therefore segfaults on deep recursion unless the
    thread gets `stack_size(..)` or the worker sets a real limit. See [§5](#5-memory-and-stack-limits-per-worker).

---

## 1. A second runtime on another thread

### 1.1 What `parallel` actually does

`AsyncRuntime` is `Arc<async_lock::Mutex<InnerRuntime>>` plus (under `parallel`) an mpsc sender used to defer
context frees (`core/runtime/async.rs:77-82`). The feature adds four `unsafe impl`s:

```rust
// core/runtime/async.rs:84-97
#[cfg(feature = "parallel")] unsafe impl Send for AsyncRuntime {}
#[cfg(feature = "parallel")] unsafe impl Send for AsyncWeakRuntime {}
#[cfg(feature = "parallel")] unsafe impl Sync for AsyncRuntime {}
#[cfg(feature = "parallel")] unsafe impl Sync for AsyncWeakRuntime {}
// core/runtime/async.rs:50-51
#[cfg(feature = "parallel")] unsafe impl Send for InnerRuntime {}
// core/context/async.rs:244-251
#[cfg(feature = "parallel")] unsafe impl Send for AsyncContext {}
#[cfg(feature = "parallel")] unsafe impl Sync for AsyncContext {}
// core/context/owner.rs:14-28: ContextOwner<R: Send> is Send; CtxPtr (Arc<NonNull<JSContext>>) is Send + Sync
```

The justification in the comments is "all functions which use runtime are behind a mutex" — every entry point
(`with`, `async_with`, `idle`, `execute_pending_job`, `drive`, `set_*`, `AsyncContext::full`) takes
`inner.lock()` first. `parallel` also makes the closure bounds `Send` via `ParallelSend` (`core/markers.rs:27-34`),
makes `InterruptHandler`/`RejectionTracker`/`PromiseHook` require `Send` (`core/runtime.rs:41-52`), calls
`JS_UpdateStackTop` on every lock acquisition so quickjs's stack-overflow check follows the thread
(`core/runtime/raw.rs:194-199`; used at `async.rs:315`, `future.rs:90`, `context/async.rs:236`,
`spawner.rs:112`), and routes `AsyncContext` drops that cannot take the lock through the mpsc channel
(`core/context/async.rs:101-107`), drained on the next locked operation (`drop_pending`, `async.rs:36-41`).

### 1.2 Create on the worker thread, or create and move?

Both work. `AsyncRuntime::new()` (`core/runtime/async.rs:108-124`) is a plain constructor with no thread affinity;
the probe created one on the parent thread, moved it into the `std::thread`, built a context and evaluated on it
(T0: `runtime-created-on-parent-thread usable on worker thread: 1+1=2`). **Recommendation: create it on the worker
thread anyway**, because

* the tokio-facing setup (`set_loader`, `set_interrupt_handler`, `AsyncContext::full`, stdlib `evaluate_def`) is all
  `async` and must run inside the worker's tokio runtime;
* `AsyncRuntime` drop must happen after every `AsyncContext` and every `Value` derived from it has been dropped
  (`JS_FreeRuntime` asserts `list_empty(&rt->gc_obj_list)`, `qjs/quickjs.c:2348`; assertions are compiled in unless
  rquickjs's `disable-assertions` feature is on, `sys/build.rs:147-148`). Keeping runtime, context, and every
  future that captures a `Ctx` inside one `block_on` scope on one thread makes that ordering structural.

What can be handed *to* the worker thread at spawn time: the worker's `CancellationToken`s, the
`mpsc` halves, the script URL/text, `WorkerOptions` (name, type), and the `Arc<EasyOxcTranspiler>`
(`den-core/src/engine.rs:27`, already `Arc`). What the parent keeps: a `std::thread::JoinHandle`, the tokens,
and its channel halves. No `AsyncContext` clone needs to cross at all, and it must not be used cross-thread in practice
even though it is `Sync` — see [§2.3](#23-the-lock-who-may-touch-the-context-and-when).

### 1.3 `Persistent` is neither `Send` nor cross-runtime

```rust
// core/persistent.rs:37-40
pub struct Persistent<T> {
    pub(crate) rt: *mut qjs::JSRuntime,
    pub(crate) value: T,
}
// core/persistent.rs:102-111
pub fn restore<'js>(self, ctx: &Ctx<'js>) -> Result<T::Changed<'js>> {
    let ctx_runtime_ptr = unsafe { qjs::JS_GetRuntime(ctx.as_ptr()) };
    if self.rt != ctx_runtime_ptr { return Err(Error::UnrelatedRuntime); }
    ...
}
```

There is no `unsafe impl Send` anywhere in `persistent.rs` (grep: none), so `Persistent<Object<'static>>` is
`!Send` (probe: `*mut JSRuntime cannot be sent between threads safely … within Persistent<rquickjs::Object<'static>>`).
Restoring into another runtime is not UB — it is a checked `Error::UnrelatedRuntime` — but that is a hard error, so
**`Persistent` has no role in worker messaging.** Its only legitimate use is on one thread/runtime, e.g. parking a
`Function<'js>` (the `onmessage` handler) outside a `with` scope; in this design even that is unnecessary because the
pump future captures the `Ctx` directly (§2.2).

### 1.4 What must never cross threads

| Type | Send? | Why / where |
|---|---|---|
| `AsyncRuntime`, `AsyncWeakRuntime` | Send + Sync | `core/runtime/async.rs:86-97` |
| `AsyncContext` | Send + Sync | `core/context/async.rs:244-251` |
| `Ctx<'js>` | Send, **not Sync** | `core/context/ctx.rs:103`; probe NEGATIVE. Moving one is still pointless: its `'js` is invariant and tied to a lock scope (`ctx.rs:426-450` safety text) |
| `Value`, `Object`, `Function`, `ArrayBuffer`, `TypedArray`, `Promise`, … | **!Send** | contain `JSValue` (`*mut c_void`); probe NEGATIVE |
| `Persistent<T>` | **!Send** | raw `*mut JSRuntime`; probe NEGATIVE |
| `DriveFuture`, `idle()`/`execute_pending_job()` futures | Send + Sync (asserted) | `core/runtime/spawner.rs:64-67`, `async.rs:305-306, 356-357` |
| `InterruptHandler`, `RejectionTracker` | must be `Send` | `core/runtime.rs:44-45, 51-52` |

Message payloads therefore have to be owned, `Send` Rust data built inside one context and re-materialised inside
the other. The chosen shape (doc 10) is the `Vec<u8>` that `JS_WriteObject2` produces, plus whatever the
supplement adds for the types the writer refuses (§10.0); `Vec<u8>` per transferred buffer is already inside that
stream (copy-then-detach, doc 10 §4.2). §10 lists the rquickjs API for each type the supplement has to touch.

---

## 2. The event loop: `idle`, `drive`, `spawn`

### 2.1 `Ctx::spawn` and the spawner

```rust
// core/context/ctx.rs:418-424
pub fn spawn<F>(&self, future: F) where F: Future<Output = ()> + 'js {
    unsafe { self.get_opaque().push(future) }
}
```

The future goes into the runtime-wide `Spawner` (`core/runtime/opaque.rs:158-163` → `spawner.rs:29-35`), whose
`push` also wakes whoever called `listen()` (`spawner.rs:34`, used by `drive()` at `:114`). The futures are
polled **only** by someone holding the runtime lock — `idle()` (`async.rs:347`), `drive()` (`spawner.rs:123`),
`execute_pending_job()` (`async.rs:297`) and `async_with` (`future.rs:114`). That is what makes it sound for the
future to capture a `Ctx<'js>` (den's timers do exactly this: `den-stdlib-timer/src/lib.rs:28-37, 57-68`, the
future owns `func: Function<'js>` and calls it from inside the poll).

### 2.2 `idle()` — read in full

```rust
// core/runtime/async.rs:313-360
pub async fn idle(&self) {
    let mut lock = self.inner.lock().await;           // :314  lock taken …
    lock.runtime.update_stack_top();
    lock.drop_pending();
    let f = ManualPoll::new(|cx| loop {
        let pending = lock.runtime.execute_pending_job().map_err(...);   // :320 run one microtask
        match pending {
            Err(e) => { /* :329-342 prints "error executing job: {}" to STDOUT via println!, continues */ }
            Ok(true) => continue,
            Ok(false) => {}
        }
        match lock.runtime.get_opaque().poll(cx) {     // :347 poll every ctx.spawn-ed future
            SchedularPoll::ShouldYield => return Poll::Pending,
            SchedularPoll::Empty => return Poll::Ready(()),   // :349 ONLY exit
            SchedularPoll::Pending => return Poll::Pending,   // :350 spawned futures alive -> stay pending
            SchedularPoll::PendingProgress => {}
        }
    });
    f.await                                            // :359 … held until here
}
```

`SchedularPoll::Empty` is returned only when the spawner's task list is empty (`core/runtime/schedular.rs:59-62,
140-143`). So **`idle()` resolves exactly when (a) no microtask is pending and (b) no `ctx.spawn`-ed future is
alive.** A pending future that is waiting on a tokio channel (`Pending` at `schedular.rs:157`) keeps `idle()`
pending; when the channel wakes, the spawner's `AtomicWaker` wakes the `idle()` task (`schedular.rs:145`).

Probe T4 (pump = `ctx.spawn(async move { while let Some(m) = inbox.recv().await { onmessage(m) } })`):

```
T4 ctx.spawn pump: idle() pending while pump alive=true; idle resolved after sender dropped in 393.204999ms; got=["hello", "world"]
```

**Design consequence (process lifetime).** `App::run_until_end` already does
`tokio::spawn(runtime.drive())` then `stop_token.run_until_cancelled(runtime.idle()).await` (`src/app.rs:106-114`).
If `new Worker(...)` on the main context registers its *parent-side* receive pump with `ctx.spawn`, `idle()` on the main
runtime stays pending until that pump ends — i.e. until the worker's outbound sender is dropped, which happens when
the worker thread's runtime/context are dropped (worker `close()`d, errored out, or `terminate()`d). Node-style
"a live worker keeps the process alive" is therefore **free**; no reference counting of workers is needed. Conversely
a `terminate()` must drop the worker-side sender (or close the channel) or the main `idle()` never returns.
The worker-side sender lives in the worker context's userdata (doc 11 §2.1), which `Opaque::clear` drops before
`JS_FreeRuntime` (`core/runtime/opaque.rs:284-292`, order: rejection tracker, interrupt handler, prototypes,
**spawner, then userdata**) — so the parent's pump ends exactly when the worker runtime is torn down. Joining the
OS thread afterwards is a separate concern (doc 11 §4 keeps `JoinHandle`s in a `WorkerRegistry`); `idle()` alone
only guarantees the *channel* is closed, not that the thread has exited.

### 2.3 The lock: who may touch the context, and when

`idle()` keeps the `async_lock::Mutex` guard alive across `Poll::Pending` (the guard `lock` at `async.rs:314` is
borrowed by the closure and only dropped when `f.await` returns). Probe T12:

```
T12 idle(): with() blocked while idle() pending=true; after idle resolved with()=2
T12 drive(): with() while drive() running -> Ok(3); drive task finished before runtime drop=false
T12 drive() completes once the runtime is dropped: false
```

Hence, while the worker thread (or den's main thread) is inside `idle()`:

* a **separate tokio task** calling `context.with(...)` / `async_with(...)` parks on the mutex until `idle()` returns —
  it is not a deadlock, but it is a livelock for as long as any spawned future is alive (i.e. forever for a worker
  with a live inbox). **Every "dispatch into the context" path must therefore be a `ctx.spawn`-ed future**, never a
  tokio task that calls `with`.
* `drive()` is different (`core/runtime/spawner.rs:81-133`): each poll takes `lock_arc`, runs jobs, polls the
  spawner, then the guard goes out of scope (`:131-132`) and it returns `Pending` after registering its waker via
  `listen()` (`:114`). Other `with`/`async_with` callers interleave freely. But `drive()` alone does **not** give the
  "wait until everything is done" signal; and it is **never woken when the runtime is dropped** (T12 last line: the
  task stayed pending 300 ms after `drop(rt)`): `DriveFuture` only returns `Ready` if it is polled and `try_ref()`
  fails (`spawner.rs:87-89`). Do not `await` it as a shutdown signal — den already just `tokio::spawn`s it and forgets it.

The recommended worker main loop is `stop.run_until_cancelled(rt.idle()).await` (§4.4), with the inbound pump and
all timers as `ctx.spawn`-ed futures, and **no other task ever calling `with` on that context** after evaluation
starts. (den's REPL works today only because it evaluates before `run_until_end`'s `idle()` — `src/app.rs:108-114`.)

### 2.4 `execute_pending_job` / `is_job_pending`

`is_job_pending()` (`core/runtime/async.rs:268-272`) is `JS_IsJobPending || !spawner_is_empty()`.
`execute_pending_job()` (`:278-309`) runs one job *or* one spawner poll and then returns — it is not waker-driven,
so a hand-rolled loop around it spins. Use `idle()`.

---

## 3. Dispatching INTO a context from an async task

### 3.1 The 0.12 signatures

```rust
// core/context/async.rs:218-224  (async_with! macro at :69-78 is #[deprecated])
pub fn async_with<F, R>(&self, f: F) -> WithFuture<F, R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> R + ParallelSend,   // ParallelSend = Send under `parallel`
    R: ParallelSend,
// core/context/async.rs:230-241
pub async fn with<F, R>(&self, f: F) -> R
where
    F: for<'js> FnOnce(Ctx<'js>) -> R + ParallelSend,
    R: ParallelSend,
```

`R` must also be `'static` for `WithFuture: Future` (`future.rs:63`), so **return owned Rust data, never a
`Value<'js>`** — den's `Engine::eval` returns `U: FromJs + Send + Sync + 'static` for exactly that reason
(`den-core/src/engine.rs:350`).

The `async |ctx|` form, as den uses it (`den-core/src/engine.rs:314-337`):

```rust
self.context.async_with(async |ctx| {
    ctx.eval_with_options::<Promise, _>(src, options)?
        .into_future::<Object>()
        .await?
        .get("value")
}).await?
```

`WithFuture::poll` (`core/context/async/future.rs:66-157`) = lock (`:70-88`) → create `Ctx` (`:97`) → poll the
user future; if pending, poll the spawner (`:113-133`) and run microtasks (`:135-144`); if no progress, return
`Pending` **after dropping the lock** (`:147-154`). So awaiting a JS promise from inside `async_with` is correct and
does not starve the spawner, and does not block other `with` callers while it waits.

### 3.2 How den-stdlib-timer does it today (the pattern to reuse)

```rust
// den-stdlib-timer/src/lib.rs:57-69
ctx.spawn({
    let token = token.child_token();
    async move {
        if token.run_until_cancelled(time::sleep(duration)).await.is_some() {
            let _ = func.call::<_, ()>(());   // Function<'js> captured by the future; called under the lock
        }
    }
});
```

The worker's inbound pump is the same shape with an `mpsc::Receiver` instead of a `sleep`:

```rust
// inside AsyncContext::with / evaluate hook, worker side
let pump_ctx = ctx.clone();
ctx.spawn(async move {
    while let Some(envelope) = inbox.recv().await {          // tokio::sync::mpsc — Send, wakes the spawner
        let event = envelope.into_js(&pump_ctx)?;            // rebuild MessageEvent in THIS context
        dispatch_message_event(&pump_ctx, event);            // self.dispatchEvent / onmessage
    }
    // channel closed => parent terminated us or dropped the Worker: fall out, idle() may now resolve
});
```

Probe T4 is literally this loop (Appendix A, lines 197-231). The parent side is symmetric: `new Worker()` does
`ctx.spawn` of a loop over the worker→parent receiver that dispatches onto the `Worker` object.

### 3.3 `Func` / `Async` wrappers

Sync host functions: `globals.set("name", Func::from(fn))` (`core/lib.rs:83-85` prelude). Promise-returning host
functions: `Func::from(Async(async fn ... -> Result<T>))` (prelude `:92`; probe `fresh()` line 85 uses both). A
`Ctx<'js>` parameter can appear anywhere in the argument list (`den-stdlib-timer/src/lib.rs:17-21`).

Host functions are **class objects with a `call` slot**, not C functions: rquickjs registers
`call: Some(class::ffi::call)` in its class definition (`core/runtime/opaque.rs:123`), and `JS_CallInternal`
dispatches non-bytecode callables straight through `rt->class_array[p->class_id].call` **without pushing a
`JSStackFrame`** (`qjs/quickjs.c:17619-17627`). That is why `ctx.script_or_module_name(0)` inside a host function
names the *JS caller* (§8) and why a host function's `ctx.eval_with_options` (re-entrant `importScripts`, §7.2) sees
the caller's frame as its parent.

### 3.4 When to spawn the inbound pump

HTML enables the worker's inside port message queue only after the script's synchronous part has run (doc 08 §2.8
step 5: after the classic script returns, or after a module has evaluated up to its first `await`). A message the
parent posts immediately after `new Worker()` is sitting in the mpsc channel by then. Whether a pump that was
`ctx.spawn`-ed **before** evaluation can dispatch it too early is decided by `WithFuture::poll`'s order
(`core/context/async/future.rs:106-144`): *user future first*, then the spawner, then pending jobs.

| Entry script evaluated via | Where the synchronous top level runs | Pump spawned before eval is … |
|---|---|---|
| classic: `ctx.eval_with_options` (§7.1) | inside the first poll of the user future, before any spawner poll | safe |
| module: `Module::import(&ctx, url)` → `JS_LoadModule` | inside the call: resolve → link → `JS_EvalFunction` up to the first `await`, all synchronous (`qjs/quickjs.c:30999-31040`) | safe |
| module: `eval("await import(url)")` as in `Engine::run_file` | in `js_dynamic_import_job`, a **job** (`qjs/quickjs.c:31182`) that `WithFuture::poll` executes *after* polling the spawner | **wrong** — `onmessage` is dispatched before the module has set it |

So either use `Module::import` for module workers (recommended, §7.3) and keep doc 11 §2.1's "install pump, then
run script" order, or spawn the pump only after the initial evaluation returns. Do not use the `await import()`
string with a pre-spawned pump.

---

## 4. Terminating a running script

### 4.1 The handler and what den already wires

```rust
// core/runtime.rs:51-52 (parallel)
pub type InterruptHandler = Box<dyn FnMut() -> bool + Send + 'static>;
// core/runtime/async.rs:185-193
pub async fn set_interrupt_handler(&self, handler: Option<InterruptHandler>)
// den-core/src/engine.rs:227-232
runtime.set_interrupt_handler({
    let world_end = stop_token.child_token();
    Some(Box::new(move || world_end.is_cancelled()))
}).await;
```

The trampoline (`core/runtime/raw.rs:390-423`) runs the closure under `catch_unwind`; a panic inside it is stored and
converted into an interrupt (`:404-411`). A per-worker `CancellationToken` checked with `is_cancelled()` is the right
shape; the probe used an `AtomicBool` with identical semantics.

### 4.2 Where quickjs-ng polls, and what it throws

```c
// qjs/quickjs.c:479
#define JS_INTERRUPT_COUNTER_INIT 10000
// qjs/quickjs.c:8215-8240
static void JS_ThrowInterrupted(JSContext *ctx) {
    JS_ThrowInternalError(ctx, "interrupted");
    JS_SetUncatchableError(ctx, ctx->rt->current_exception);
}
static no_inline __exception int __js_poll_interrupts(JSContext *ctx) {
    ctx->interrupt_counter = JS_INTERRUPT_COUNTER_INIT;
    if (rt->interrupt_handler && rt->interrupt_handler(rt, rt->interrupt_opaque)) { JS_ThrowInterrupted(ctx); return -1; }
    return 0;
}
static inline __exception int js_poll_interrupts(JSContext *ctx) {
    if (unlikely(--ctx->interrupt_counter <= 0)) return __js_poll_interrupts(ctx); else return 0;
}
```

Poll sites: every `OP_goto`/`OP_goto16`/`OP_goto8` (loop back-edges, `:18664-18677`), the remaining jump opcodes
(`:18695-18755`), entry of `JS_CallInternal` (`:17590`), `instanceof` prototype walks (`:8457`), property-enumeration
loops (`:16348`), and the regex engine via `lre_check_timeout` (`:48453-48458`, so a catastrophic-backtracking regex
is interruptible too). So the handler runs roughly every 10 000 back-edges/calls — the probe saw ~0.02 ms latency
(T1: `300.024012ms` after a 300 ms timer). `JS_SetInterruptHandler` itself is just two stores (`:2120-2124`).

### 4.3 Is it catchable? What does the engine do afterwards?

* Interpreter `exception:` path skips catch handlers for uncatchable errors
  (`qjs/quickjs.c:20328`: `if (!JS_IsUncatchableError(rt->current_exception)) { /* unwind to catch */ }`).
  Probe T1: `js-caught=false` with a `try { while(true){} } catch {}` wrapper.
* Async functions: `js_async_function_resume` sees the uncatchable error and **does not reject the promise**, it just
  terminates the function (`:20900-20903`), same for promise reactions (`:54316-54318`). So `.catch()` cannot observe
  it either.
* Rust side: the eval returns `Err(Error::Exception)`; `ctx.catch()` yields the error object;
  `Value::is_uncatchable_error()` (`core/value.rs:394-396`, wraps `JS_IsUncatchableError` `qjs/quickjs.h:821`) lets
  the host tell "terminated" from "script threw". Probe T1:
  `catch: uncatchable=true message=Some("interrupted") stack=Some("    at <eval> (eval_script:1:1)\n")`.
* The runtime is left in a normal state: the probe's T1 runtime was dropped cleanly afterwards, and rquickjs's own test
  `interrupt_handler_idle` (`core/runtime/async.rs:617-641`) covers interrupt + `idle()`.

**Caveat — the handler stays armed.** After a terminate, *every* later bytecode poll throws again while the token
is cancelled, including finalisers or `onerror` dispatch you might attempt. That is what we want for
`terminate()` (nothing in the worker may run again); just do not try to run JS "cleanup" in the worker after
cancelling the token.

### 4.4 Four situations

| Script state | What stops it | Probe |
|---|---|---|
| Tight sync loop (`while(true){}`) in the initial eval | interrupt handler → eval returns `Err`, `is_uncatchable_error()` | T1 |
| Microtask-only loop (`while(true) await Promise.resolve()`) driven by `idle()` | interrupt fires inside the job; `idle()` prints `error executing job: Error: interrupted` to **stdout** and then resolves because no job/future remains | T2: `idle() after kill resolved=true in 302ms` |
| Parked on an external future (`await sleepMs(10000)`, a pending message pump, a long timer) | **nothing to interrupt**; cancel `idle()` with the token instead, then drop context + runtime | T3: `idle() pending after 500ms=true; run_until_cancelled(idle) -> None in 101ms` … `dropped ctx+rt with a pending ctx.spawn future alive: no abort` |
| `self.close()` called from JS (HTML "close a worker": finish the current task, discard queued ones, stop timers) | **not** the interrupt token. `close()` runs under the lock inside one `idle()` poll; cancelling a separate `closing` token there is only *observed* by `closing.run_until_cancelled(idle())` after that poll returns — so the current task and its microtasks complete (`idle()`'s inner loop drains jobs before it yields, `async.rs:320-345`), then the `idle()` future is dropped and the runtime teardown drops every still-pending `ctx.spawn` future (timers, the pump) with the spawner | same mechanism as T3; not separately probed |

So `terminate()` = cancel the interrupt token **and** `closing`, then drop the parent's receiver; `close()` = cancel
`closing` only. With either, `run_until_cancelled(idle())` drops the `idle()` future (releasing the lock) so the
worker's `block_on` returns and the drop sequence below runs. Dropping a runtime with pending spawned futures is
safe because `Opaque::clear()` takes the spawner (and therefore every captured `Function<'js>`/`Ctx`) *before*
`JS_FreeRuntime` (`core/runtime/opaque.rs:284-292`, called from `RawRuntime::drop` `core/runtime/raw.rs:124-133`).

**Why `closing` must drive `run_until_cancelled(idle())`, not just the pump.** den-stdlib-timer gives every
`setTimeout`/`setInterval` its own `CancellationToken` (`den-stdlib-timer/src/lib.rs:26-29, 55-58`) that nothing
but `clearTimeout` cancels. After `close()` a worker with a pending 30 s timer would otherwise sit in `idle()` for
30 s (or forever for an interval) even though its pump has ended — `idle()` stays pending while *any* spawned
future is alive (§2.2). Doc 11 §9 currently writes `boot.stop_token.run_until_cancelled(idle())`; use `closing`
there (it is cancelled by both `close()` and `terminate()`, and should be a child of `stop_token` so Ctrl-C still
propagates).

Worker main future skeleton (the probe ran this shape end-to-end; `closing` added per the row above):

```rust
fn worker_thread(stop: CancellationToken, closing: CancellationToken, inbox: mpsc::UnboundedReceiver<Envelope>, outbox: mpsc::UnboundedSender<Envelope>, script: ScriptSource) {
  let tokio_rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(1)                 // block_in_place needs the multi-thread scheduler (§6)
    .thread_name(format!("den-worker-{name}"))
    .enable_all()
    .build()
    .expect("worker tokio runtime");
  tokio_rt.block_on(async move {
    let runtime = AsyncRuntime::new()?;                      // created HERE, on the worker thread (§1.2)
    runtime.set_max_stack_size(0).await;                     // mirror Engine::new (engine.rs:40) or a real limit (§5)
    runtime.set_loader(resolver, loader).await;              // same tuple shape as Engine::new (engine.rs:43-222)
    runtime.set_interrupt_handler(Some(Box::new({ let t = stop.child_token(); move || t.is_cancelled() }))).await;
    runtime.set_host_promise_rejection_tracker(Some(Box::new(report_unhandled))).await;   // §9
    let context = AsyncContext::full(&runtime).await?;
    context.with(|ctx| install_worker_global_scope(&ctx, inbox, outbox.clone())).await;     // ctx.spawn pumps inside
    let eval = context.async_with(async |ctx| evaluate_worker_script(ctx, &script).await).await;   // §7
    report_initial_error(eval, &outbox);
    closing.run_until_cancelled(runtime.idle()).await;       // None => close()/terminate(); Some(()) => script finished with nothing pending
    drop(context);                                           // all Values/futures already gone: spawner cleared on runtime drop
    drop(runtime);
  });
  drop(tokio_rt);                                            // joins the 1 worker + blocking threads (§6.3)
}
```

---

## 5. Memory and stack limits per worker

| API | Wraps | Notes |
|---|---|---|
| `AsyncRuntime::set_memory_limit(usize)` (`core/runtime/async.rs:223-227` → `raw.rs:246-249` → `JS_SetMemoryLimit`) | `rt->malloc_state.malloc_limit` (`qjs/quickjs.c:2087`) | 0 = unlimited. **Works with `rust-alloc`**: checked in `js_malloc_rt`/`js_calloc_rt`/`js_realloc_rt` before the allocator is called (`:1622-1623, 1645-1646, 1692-1693`), tracked with `mf.js_malloc_usable_size` which `RustAllocator` implements via a size header (`core/allocator/rust.rs:113-117`). The rquickjs doc comment saying "Noop when a custom allocator is used" (`async.rs:221-222`) is stale for this quickjs-ng. Probe T6: `js-caught=Some("InternalError: out of memory") … malloc_limit=16777216`. OOM is a normal catchable `InternalError` (`JS_ThrowOutOfMemory`, `:8127-8136`); the runtime stays usable. |
| `set_max_stack_size(usize)` (`async.rs:232-236` → `raw.rs:256-264`) | `JS_SetMaxStackSize` | **Default 1 MiB** in this quickjs-ng (`JS_DEFAULT_STACK_SIZE (1024 * 1024)`, `qjs/quickjs.h:423-424`, applied at `qjs/quickjs.c:2014`); the "256x1024" in rquickjs's doc comment (`async.rs:231`) is stale. `0` disables the check; values above 16 MiB are clamped to 0 (`raw.rs:257-263`). den's `Engine::new` passes 0 (`engine.rs:40`), so **den has no JS stack check at all today**. For a worker on a `std::thread` with Rust's default 2 MiB stack, either keep 0 and give the thread a bigger stack (`thread::Builder::stack_size`) or set a limit comfortably below the thread stack (e.g. 1 MiB on a 2 MiB thread; the interpreter needs headroom above the limit for the frame that trips it) so deep recursion is a `RangeError` rather than a segfault. The check uses `JS_UpdateStackTop` on each lock acquisition (§1.1), so it is correct on the worker thread — the QuickJS stack *is* the `block_on` root future's stack, i.e. the `std::thread`'s, not a tokio worker's. |
| `set_gc_threshold(usize)` (`async.rs:239-243`) | `JS_SetGCThreshold` | Bytes of allocation between automatic cycle collections. Leave default unless measured. |
| `memory_usage()` (`async.rs:260-262`) | `JS_ComputeMemoryUsage` | `MemoryUsage = JSMemoryUsage` (`core/runtime.rs:55`): `malloc_size`, `malloc_limit`, … Used in probe T6. |
| `run_gc()` (`async.rs:251-257`) | `JS_RunGC` | Drains pending context frees first. |

`WorkerOptions` has no standard limit knobs; expose them as den-specific options or env (12-factor) if at all.

---

## 6. tokio on the worker thread

### 6.1 `block_in_place` flavours — from source

`tokio::task::block_in_place` → `runtime::scheduler::block_in_place` → `multi_thread::worker::block_in_place`
(`tokio/task/blocking.rs:74-79`, `tokio/runtime/scheduler/block_in_place.rs:4-9`). The decision table in
`tokio/runtime/scheduler/multi_thread/worker.rs:408-448`:

| `current_enter_context()` | on a pool worker thread? | Result |
|---|---|---|
| `Entered {..}` | yes | proceed; take the core, hand it to a fresh blocking thread `run(worker)` (`:455-492`), run `f` via `exit_runtime` (`:505`), steal the core back on drop (`:380-403`) |
| `Entered { allow_block_in_place: true }` | no | proceed — **this is `Runtime::block_on` on the std thread** (`multi_thread/mod.rs:87-94` passes `true` to `enter_runtime`) |
| `Entered { allow_block_in_place: false }` | no | `panic!("can call blocking only when running on the multi-threaded runtime")` (`:430-435`) — current_thread runtime or `LocalSet` |
| `NotEntered` | any | plain call |

Probe P0 reproduced the panic on a current_thread runtime; probe T5 shows it working on
`new_multi_thread().worker_threads(1)` both from the `block_on` root future (`under async_with=42`) and from the
single worker thread while `idle()` runs a spawned future (`under idle()=42` — the core was handed off to a blocking
thread while `syncBlock` waited, then reclaimed).

`Handle::current()` inside the loader works because `block_on` enters the runtime context (`tokio/runtime/runtime.rs:371`).

### 6.2 Exact builder config

```rust
tokio::runtime::Builder::new_multi_thread()
    .worker_threads(1)          // one async worker; the blocking pool (default max 512) is created regardless
    .thread_name("den-worker")  // optional; blocking-pool threads inherit it
    .enable_all()               // timers (den-stdlib-timer), I/O (reqwest in HttpLoader)
    .build()
```

`rt-multi-thread` is already a dependency of `den-core` for this very reason
(`den-core/Cargo.toml` comment: "`block_in_place` + `Handle::current` in the loaders need these"). Run the worker
future with `runtime.block_on(fut)` from the `std::thread` closure (probe `worker_thread`, Appendix A line 108).
Do **not** spawn the per-worker tokio runtime from inside the parent's tokio runtime's worker threads — creating a
runtime inside another runtime's `block_on` is fine, but *dropping* one is not (see 6.3); the `std::thread` gives it
its own stack, its own `block_on`, and a clean drop point. The parent joins the thread from `spawn_blocking`
(probe `main`, line 538); doc 11 §4 is the decided design (a `WorkerRegistry` of `JoinHandle`s in the spawning
context's userdata, joined bottom-up), so do not detach.

Thread spawn: `std::thread::Builder::new().name(..).stack_size(..).spawn(..)` — set `stack_size` explicitly
if the worker keeps den's `set_max_stack_size(0)` (§5); the default 2 MiB is what `RUST_MIN_STACK` gives a
spawned thread, and QuickJS recursion runs on exactly that stack.

### 6.3 Clean shutdown

`Drop for Runtime` (`tokio/runtime/runtime.rs:500-514`) shuts the scheduler down, then the `BlockingPool` drop calls
`shutdown(None)` which **waits indefinitely** for outstanding blocking tasks (`tokio/runtime/blocking/pool.rs:282-286,
244-275`). For a worker that is fine — nothing long-lived runs there — and it happens on the worker's own
`std::thread`, never inside the parent runtime (that would trip tokio's "cannot drop a runtime in a context where
blocking is not allowed" panic). If a loader is mid-`block_in_place` (a `reqwest::get` in flight) when we terminate,
the drop blocks until that request finishes or times out; `shutdown_timeout` (`runtime.rs:452-456`) /
`shutdown_background` (`:489-491`) are the escape hatches. Doc 11 §9 picks `tokio.shutdown_background()` at the end
of the thread body — fine, because by then `block_on` has returned and everything QuickJS-related is already
dropped; the only thing a stray blocking task could still hold is a loader's `reqwest` future.

Drop order inside `block_on` (probe T3/T11 verified, no abort):
1. Stop the pumps (cancel/drop channel ends) and let `run_until_cancelled(idle())` return.
2. Drop every remaining `Value`/`AsyncContext` — for den this means the `block_on` future's locals.
3. Drop `AsyncRuntime` last; `RawRuntime::drop` clears the spawner (dropping any still-pending futures and the
   `Ctx`/`Function`s they captured), then `JS_FreeRuntime` (`core/runtime/raw.rs:124-133`, `opaque.rs:284-292`).
4. `block_on` returns; drop the tokio `Runtime`; the `std::thread` closure returns and the OS thread exits
   (probe T11: `worker tokio runtime dropped; OS thread exiting`, P1: `worker thread joined ok=true`).

If a `Value` is leaked past step 3 (e.g. stashed in a `static`), `JS_FreeRuntime` hits `assert(list_empty(&rt->gc_obj_list))`
(`qjs/quickjs.c:2348`) and aborts the process — assertions are on unless `disable-assertions` (`sys/build.rs:147-148`).

---

## 7. Evaluating classic and module worker scripts

### 7.1 Classic script

```rust
// core/context/ctx.rs:29-41 (non_exhaustive) and Default at :67-78 => global: true, strict: true, promise: false
let mut options = EvalOptions::default();
options.global = true;        // JS_EVAL_TYPE_GLOBAL (qjs/quickjs.h:428)
options.strict = false;       // classic scripts are sloppy unless they opt in; den's Engine::eval forces strict (engine.rs:371) — do NOT copy that
options.promise = false;      // top-level await is a module-only feature; keeping JS_EVAL_FLAG_ASYNC off makes `await` a SyntaxError as in browsers
options.filename = Some(script_url.clone());   // => JS_GetScriptOrModuleName and `stack` frames carry the URL (§8, §9)
ctx.eval_with_options::<(), _>(transpiled_source, options)?;
```

`eval_with_options` (`ctx.rs:184-205`) passes `filename` to `eval_raw`, defaulting to `"eval_script"` (`:194`).
The source must already be loaded and transpiled: the transpiler call is `transpiler.transpile(src, syntax,
IsModule::Bool(false), false)` — note `IsModule::Bool(false)` (a classic script), whereas both den loaders pass
`IsModule::Bool(true)` (`mmap_script.rs:78`, `http.rs:95`).

### 7.2 `importScripts(...urls)`

Semantics (MDN `importScripts`): "synchronously imports one or more scripts into the scope of a classic worker";
throws `TypeError` "if the current WorkerGlobalScope is a module"; `NetworkError` for a non-JS MIME type;
`SyntaxError` "if any URL cannot be resolved"; relative URLs "relative to the worker entry script's URL".

Implementation = for each URL in order: resolve against the worker's script URL, load the text **synchronously**,
transpile as classic, `eval_with_options` with the options of §7.1 and `filename = resolved_url`. den's loaders are
already synchronous via `block_in_place` (`den-core/src/loader/mmap_script.rs:92`, `http.rs:116`), which §6 shows
works on the worker runtime, **but** they return `Module<'js, Declared>` (the `Loader` trait, `core/loader.rs:96-104`)
and always transpile as module — so `importScripts` cannot call them as-is. Factor the "fetch bytes + sniff
extension/MIME" half of `HttpLoader::load` (`http.rs:78-87`) and the mmap half of `MmapScriptLoader::load`
(`mmap_script.rs:50-69`) into reusable `fn fetch_script_text(url) -> Result<(String, &'static str)>` helpers that
both the `Loader` impls and `importScripts` call. Errors map to: load failure → `NetworkError`-shaped `DOMException`
(den has no `DOMException` yet; the JS-land class-building pattern of `den-stdlib-wasm/src/error.rs:22-43` applies),
bad URL → `SyntaxError` via `Exception::throw_syntax` (`core/value/exception.rs:111`), module worker →
`Exception::throw_type` (`:131`).

A Rust `importScripts` host function runs *inside* an `eval` (under the lock, on the JS stack); calling
`ctx.eval_with_options` re-entrantly from a host function is normal in QuickJS (quickjs's own `eval` does it) and
den already does it in `WebAssemblyErrors::install` (`den-stdlib-wasm/src/error.rs:92`).

### 7.3 Module worker

Reuse `Engine::run_file`'s approach verbatim (`den-core/src/engine.rs:308-338`):

```rust
let src = format!(r#"await import(`{}`)"#, url);
ctx.eval_with_options::<Promise, _>(src, { global: true, promise: true, strict: true, filename: Some(url) })?
    .into_future::<Object>().await?   // {"value": namespace}
```

**Do not copy that for workers.** Use `Module::import(&ctx, resolved_url)` (`core/value/module.rs:426-444`):

```rust
// core/value/module.rs:426-444 — JS_LoadModule(ctx, base = script_or_module_name(1) or "", specifier)
pub fn import<S: Into<Vec<u8>>>(ctx: &Ctx<'js>, specifier: S) -> Result<Promise<'js>>
```

* **Synchronous up to the first `await`.** `JS_LoadModule` → `JS_LoadModuleInternal` (`qjs/quickjs.c:30999-31040`)
  calls the resolver/loader (`js_host_resolve_imported_module`), links, and `JS_EvalFunction`s the module **inside
  the call**; only the settlement of the returned promise is deferred. The `await import()` string instead
  enqueues `js_dynamic_import_job` (`qjs/quickjs.c:31182`), so the module does not even *load* until the job loop
  runs — which, inside `async_with`, is after the spawner poll (§3.4: a pre-spawned inbound pump would dispatch
  queued messages before `onmessage` exists). `Module::import` also keeps the loader's `block_in_place` inside the
  root future, where §6.1 proved it works.
* **Base name.** `Module::import` resolves relative to `ctx.script_or_module_name(1)` — the *caller's caller* frame —
  falling back to `""`. Called from Rust inside `async_with` there is no JS frame, so the base is `""` and
  `FileResolver` resolves relative to its `with_path("./")` (CWD). Therefore **pass the already-resolved absolute
  path/URL** (§8 computed it in the parent) as the specifier; never a relative one.
* Both loaders transpile with `IsModule::Bool(true)` — correct for module workers. Probe T9: module-level throws and
  unresolvable specifiers surface as **rejected promises** (`Ok(promise) -> awaited: Err(Exception)`, then
  `ctx.catch()` = `"could not load module 'does-not-exist.js'"`), so always `.into_future::<()>().await` and then
  `ctx.catch()`. Per doc 08 §2.8 step 5 the pump may run concurrently with that await (top-level `await` does not
  delay message delivery); the module's own frames carry the resolved path as `filename` (`Module::declare(ctx,
  name, ..)` in both loaders), so nested `new Worker("./x.js")` from the entry module resolves correctly (§8).

---

## 8. Base URL for `new Worker("./w.js")`

**Spec**: the URL "is resolved relative to the current HTML page's location" (MDN `Worker()`; HTML: encoding-parse
relative to the outside settings' API base URL). For a runtime without documents that means *the URL of the script
that called `new Worker`*.

**Reachable from Rust: yes, partially.** `JS_GetScriptOrModuleName` is declared at `qjs/quickjs.h:1245`, bound at
`sys/bindings/x86_64-unknown-linux-gnu.rs:1723`, and wrapped as `Ctx::script_or_module_name(stack_level) ->
Option<Atom>` (`core/context/ctx.rs:455-466`). The C side walks `rt->current_stack_frame` up `n` levels and returns
the `filename` atom of that frame's bytecode (`qjs/quickjs.c:30890-30913`; comment: "does not work for eval()" — it
returns the *enclosing function's* filename, which for `eval`ed code is whatever `EvalOptions.filename` was).

Probe T8, calling a Rust `whereAmI(level)` host function:

```
script(filename set, level 0)=Some("file:///tmp/main.js") | nested-fn(level 0)=Some("file:///tmp/nested.js")
| level 1=None | eval without filename=Some("eval_script") | module=Some("file:///tmp/mod.mjs")
| from Rust with no JS frame=None
```

* Level **0** is the JS frame that called the host function, because rquickjs host functions are class objects
  dispatched through `rt->class_array[..].call` with **no `JSStackFrame` of their own** (`qjs/quickjs.c:17619-17627`;
  `core/runtime/opaque.rs:123`; §3.3). Level 1 walked past the top-level script to nothing (`None`), because these
  tests had one JS frame. **Doc 11 §9 writes `ctx.script_or_module_name(1)` for the parent-side base — that is
  wrong for a `new Worker` at a script's top level (returns `None`) and returns the grand-caller's file otherwise;
  use level 0.** (`Module::import` uses level 1 internally, `module.rs:431`, which is why §7.3 tells you to pass an
  absolute specifier.)
* Inside a module loaded through den's resolver, the name is the **resolved path** the loader was given
  (`Module::declare(ctx, name, …)` at `mmap_script.rs:83` / `http.rs:102`) — i.e. a filesystem-relative path like
  `./w.js`/`sub/w.js` for `FileResolver` (`core/loader/file_resolver.rs:130-154` joins relative to the importing
  module's *directory*) or an absolute `http(s)://` URL for `HttpResolver` (`den-core/src/resolver/http.rs:25-40`).
* den's `Engine::run_file` evaluates the `await import(...)` wrapper **without** a filename (`engine.rs:326-332`), so
  code in that wrapper would see `eval_script`; but the actual program is the imported module, whose frames carry
  their path. Calls to `new Worker` from the program therefore *do* see a usable base — unless the call happens
  from REPL input (`Engine::eval`, also filename-less) or from a `Function` constructor / `eval` string.

**Resolution rule to implement**: `base = ctx.script_or_module_name(0)` (inside the `Worker` constructor's host
function); if it is `None`/`"eval_script"`/not parseable, fall back to the process CWD exactly like `den main.js`
does (`FileResolver::with_path("./")`, `engine.rs:93`). Then feed `(base, specifier)` through the same resolver tuple
`Engine::new` builds, so `http(s)://`, `./`, `../`, bare names and den's extension patterns all behave like `import`.
Module workers get the resolved name as their module name; classic workers get it as `EvalOptions.filename` — that is
what makes nested `new Worker`/`importScripts` inside the worker resolve correctly (§7).

---

## 9. Uncaught errors and unhandled rejections

### 9.1 Synchronous throws / failed eval

`ctx.eval*` returns `Err(Error::Exception)` (`core/result.rs:96`), the value is pending in the context;
`ctx.catch()` takes it (`core/context/ctx.rs:257-262`, `JS_GetException`). den's existing reporter is the model
(`src/main.rs:52-66`): `value.as_exception()` → `Exception`, else `Coerced<String>`.

`Exception` API (`core/value/exception.rs`): `message()` `:73-78`, `stack()` `:83-88`, `Display` prints
`Error: <message>\n<stack>` `:217-230`, `from_message`/`throw_*` constructors `:59-211`. There is **no `file()`/`line()`**
method and quickjs-ng does not define `fileName`/`lineNumber` own properties on error instances (probe T9 own string
props = `[("message","bad"), ("stack","    at f (/tmp/x.js:2:26)\n    at <eval> (/tmp/x.js:3:1)\n")]`; the
`lineNumber`/`columnNumber` getters at `qjs/quickjs.c:41842-41844` are on *Function.prototype*, not errors). To fill
`ErrorEvent.filename/lineno/colno`, parse the first `at … (file:line:col)` frame of `stack`. `name` comes from
`exception.get::<_, String>("name")` (T9: `TypeError`). Interrupt vs. real error: `value.is_uncatchable_error()`.

### 9.2 Module evaluation / dynamic import failures

Both `Module::evaluate` (`core/value/module.rs:308-320`) and `Module::import` (`:426-444`) return `Ok(Promise)`; the
failure is the promise rejecting. `Promise::into_future::<T>()` (`core/value/promise.rs:152`) yields
`Err(Error::Exception)` with the reason left as the pending exception (probe T9: `catch: message=Some("module-fail")
stack=Some("    at <anonymous> (file:///tmp/bad.mjs:1:30)\n")`; for a missing specifier the message is
`could not load module 'does-not-exist.js'` with an empty stack).

### 9.3 Unhandled promise rejections

```rust
// core/runtime.rs:44-45 (parallel)
pub type RejectionTracker = Box<dyn for<'a> Fn(Ctx<'a>, Value<'a>, Value<'a>, bool) + Send + 'static>;
// core/runtime/async.rs:163-171
pub async fn set_host_promise_rejection_tracker(&self, tracker: Option<RejectionTracker>)
```

Arguments: `(ctx, promise, reason, is_handled)`. Probe T7 with
`const p = Promise.reject(new Error('boom-later-handled')); Promise.reject(new Error('boom-unhandled')); p.catch(() => {})`:

```
T7 rejection tracker calls: ["boom-later-handled handled=false", "boom-unhandled handled=false", "boom-later-handled handled=true"]
```

I.e. QuickJS calls it *at rejection time* with `false`, then again with `true` when a handler is attached
later — the same two-phase protocol browsers use (`unhandledrejection` / `rejectionhandled`). To implement
"unhandled rejection → worker `error` event" without false positives, record `promise`-keyed entries on `false`,
remove on `true`, and report whatever is still there at a microtask-checkpoint (i.e. when `idle()`'s inner loop finds
no more jobs — in practice, after the initial eval's `async_with` returns and then periodically from a
`ctx.spawn`-ed tick). Keying needs a stable promise identity; `Value` is `!Hash`, but the raw pointer from
`value.as_raw()` (`core/value.rs:427`) is stable while the object is alive — keep a `Persistent<Promise>` (same runtime,
same thread, so allowed) to pin it.

### 9.4 Errors thrown inside jobs/timers

`idle()` catches job errors and `println!`s `error executing job: <Display>` **to stdout** (`core/runtime/async.rs:329-342`);
`drive()` drops them (`spawner.rs:117-122`). Errors thrown by a `Function::call` inside a `ctx.spawn`-ed future (a
timer callback, the message pump's `onmessage` dispatch) come back to *that future* as `Err(Error::Exception)` —
den-stdlib-timer currently discards them (`let _ = func.call::<_, ()>(())`, `lib.rs:34, 65`). For the worker, every
host-initiated call into JS (`onmessage`, timers) must `match` the result, `ctx.catch()`, and route to the `error`
event / parent `ErrorEvent` — that is the only place "uncaught exception in a worker" can be observed for callbacks.

---

## 10. Data crossing threads: the structured-clone raw material

### 10.0 The serializer quickjs-ng already has

Doc 10 decided the wire format: `JS_WriteObject2(ctx, &mut len, value, JS_WRITE_OBJ_REFERENCE, &mut sab_tab)` →
`Vec<u8>` → `JS_ReadObject2(ctx2, ptr, len, JS_READ_OBJ_REFERENCE, &mut sab_tab)` in the receiving runtime.
Facts re-verified here against `qjs/quickjs.c` (quickjs-ng 0.15.1, `quickjs.h:1410-1412`):

| | Where | Verdict for den |
|---|---|---|
| Bindings | `sys/bindings/x86_64-unknown-linux-gnu.rs:1683-1715` (`JS_WriteObject`, `JS_WriteObject2`, `JS_ReadObject`, `JS_ReadObject2`); reachable as `rquickjs::qjs::*`. **rquickjs-core wraps none of them** (grep `JS_WriteObject` in `core/`: nothing) | raw `unsafe` calls under the lock, like `den-stdlib-wasm/src/memory.rs` already does for other `qjs::` entry points |
| Flags | `quickjs.h:1214-1233`: `JS_WRITE_OBJ_BYTECODE (1<<0)`, `_SAB (1<<2)`, `_REFERENCE (1<<3)`, `_STRIP_SOURCE`, `_STRIP_DEBUG`; reader mirrors | set only `_REFERENCE`; never `_BYTECODE` on a message path (it serialises functions into loadable bytecode) |
| Supported tags | `BC_TAG_*` enum `quickjs.c:37631-37653`; writer switch `:38290-38375` — primitives, BigInt, boxed primitives (`OBJECT_VALUE`), Date, RegExp, Array, plain Object, ArrayBuffer, SharedArrayBuffer (gated), every typed array (`is_typed_array`, `:58395`), **Map/Set**, object references, Symbol | everything the spec calls "platform-independent" except the next row |
| Refused | `JS_CLASS_ERROR`, `JS_CLASS_DATAVIEW` (excluded by `is_typed_array`), `DOMException`, functions → `default:` `TypeError "unsupported object class"` (`:38342-38348`); accessor properties → `TypeError "only value properties are supported"` in `JS_WriteObjectTag` | the **supplement** (doc 10 §7) pre-walks the graph and encodes these itself using the rquickjs calls in the table below |
| Over-permissive | `JS_TAG_SYMBOL` is written (`:38361-38372`); the spec requires `DataCloneError` | pre-screen (doc 10 §1.2) |
| Cycles / identity | `JS_WRITE_OBJ_REFERENCE`: `js_object_list_add` + `BC_TAG_OBJECT_REFERENCE` (`:38284-38296`); without the flag a cycle is `TypeError "circular reference"` | use the flag; no hand-rolled `HashMap<*mut c_void, NodeId>` needed for the parts the writer handles |
| Atoms | property names are written as strings and re-interned by the reader, so a blob written in runtime A reads in runtime B (doc 10 §0 probe did exactly this) | cross-thread safe; only `_SAB` carries a raw pointer |

Everything below was exercised in probe T10 (Appendix A lines 303-460). It is the rquickjs-level raw material for
the supplement — the types the writer refuses or mishandles (Error, DataView, Symbol screening, holes, transfer
bookkeeping) — plus the `ArrayBuffer`/`TypedArray` rules that apply to rebuilding transferred buffers. **One
cross-doc hazard**: doc 10 §4.3 floats a later zero-copy path that rebuilds a transferred buffer on the receiver
with `ArrayBuffer::new(ctx, vec)`; per fact 12 such a buffer must *never* be detached again (relay to a nested
worker, `port.postMessage(buf, [buf])`), or the finalizer double-frees. Either keep `new_copy` or fix rquickjs's
free hook first.

| Type | Detect (reader side) | Build (writer side) | Notes / probe evidence |
|---|---|---|---|
| primitives | `Value::type_of()` → `Type::{Undefined,Null,Bool,Int,Float,String,Symbol,BigInt,…}` (`core/value.rs:540-558`); `as_bool/as_int/as_float/as_number` (`:189-263`); `String` via `FromJs` | `Value::new_*` (`:152-250`), `IntoJs` | `Int` vs `Float` is a quickjs tag detail; clone as f64/i32 faithfully. |
| `Symbol` | `Type::Symbol` / `is_symbol()` `:340` | — | Probe: `Symbol type_of=symbol`. Structured clone must throw `DataCloneError`. |
| `ArrayBuffer` | `ArrayBuffer::from_value` (`core/value/array_buffer.rs:276`), `as_bytes() -> Option<&[u8]>` (`:242`, `None` when detached), `len()` `:230` | **`ArrayBuffer::new_copy(ctx, &[u8])`** (`:123`, `JS_NewArrayBufferCopy`) | Transfer = copy bytes out, then `detach()` (`:259`, `JS_DetachArrayBuffer`). **Never `ArrayBuffer::new` (`:91`) on a buffer that may be detached** — the finalizer re-runs the free hook with NULL (`qjs/quickjs.c:58037-58041` + `:57934-57936`) and `drop_raw` does `Vec::from_raw_parts(NULL)` (`:100-103`): the probe aborted with `unsafe precondition(s) violated`. Probe after fix: `bytes=Some([1,2,3,4]) after detach as_bytes=None JS byteLength=0`. `from_source_shared` (`:153`) exists for SAB if ever needed. |
| `TypedArray<T>` | `TypedArray::<T>::from_value` (`core/value/typed_array.rs:175`) — **class-checked per element type**: a `Uint16Array` is `Err` as `TypedArray<u8>` (probe). Element types: `i8 u8 i16 u16 i32 u32 f32 f64 i64 u64` + `U8Clamped` (`:35-48, 57-62`); no `Float16Array` binding (`:43` comment). `as_bytes()` `:208`, `arraybuffer()` `:219`, byte offset/length via `get::<_, usize>("byteOffset")` | `TypedArray::<T>::new_copy(ctx, &[T])` (`:137`) or `from_arraybuffer` (`:232`) for views sharing one buffer | Probe: `TypedArray<u16>: len=2 bytes=Some([1,0,255,255]) buffer-len=4`. Clone must preserve buffer sharing between views of the same buffer — build the buffer once, then views over it. Try each element type in turn to detect. |
| `DataView` | no binding; `obj.is_instance_of(globals.get("DataView"))` (`core/value/object.rs:185`) + `byteOffset`/`byteLength`/`buffer` props | `Constructor::construct` on `globals.get::<_, Constructor>("DataView")` (`core/value/function.rs:275`) | Probe: `DataView: type_of=object is_instance_of(DataView)=true byteOffset=2 byteLength=4`. |
| `Date` | `unsafe { rquickjs::qjs::JS_IsDate(value.as_raw()) }` (`sys/bindings/x86_64-unknown-linux-gnu.rs:1081`, `qjs/quickjs.h:950`) or `is_instance_of(Date)`; `SystemTime::from_js` (`core/value/convert/from.rs:372-385, 388`) → via `getTime()`; for NaN dates read `getTime()` as `f64` yourself | `SystemTime::into_js` (`into.rs:489-493`, `new Date(ms)`) or `JS_NewDate(ctx, f64)` (`bindings:1078`) | Probe: `JS_IsDate=true SystemTime::from_js=1700000000000 SystemTime::into_js is Date=true`. Prefer carrying `f64` millis so `Invalid Date` round-trips. |
| `RegExp` | `is_instance_of(RegExp)`; `source`/`flags` props | `RegExp` constructor `construct((source, flags))` | Probe: `source=a+b flags=gi`. `lastIndex` is *not* cloned per spec. |
| `Map` / `Set` | `is_instance_of(Map|Set)`; enumerate with a JS helper (`Array.from(m)` → `Vec<Array>`) or `JsIterator` (`core/value/iterable.rs:185`) — `Object::keys()` returns **nothing** for a Map (probe: `Object::keys()=[]`) | `Map`/`Set` constructor with an array of entries | Probe: `entries-via-Array.from=2`. **Tuples are not `FromJs`** (`core/value/convert/from.rs`; first probe build failed on `Vec<(Value,Value)>`): read entries as `Array` and index. |
| `BigInt` | `Type::BigInt` (`JS_TAG_BIG_INT \| JS_TAG_SHORT_BIG_INT`); `BigInt::to_i64()` (`core/value/bigint.rs:23-31`) | `BigInt::from_i64/from_u64` (`:9-21`) | **`to_i64` wraps silently**: probe `2^70 to_i64=Ok(0)`. Carry the decimal string (`toString()` → `BigInt(str)` via a JS helper; probe `BigInt(str) type_of=big_int`) or sign+magnitude bytes; only use `i64` for the fast path after a range check via JS `BigInt.asIntN(64, x) === x`. |
| `Error` objects | `value.is_error()` (`core/value.rs:388`), `as_exception()`; `name`/`message`/`stack`/`cause` props | `Exception::from_message` (`exception.rs:59`) sets only `message`; to keep the subclass, `construct` the matching global (`TypeError` etc.) and `set("stack", …)`/`set("cause", …)` | Probe: `is_error=true name=RangeError message=r cause.message=c has stack=true`. Spec-serialisable names: Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError; anything else → `Error`. |
| `Proxy` | `Type::Proxy` / `is_proxy()` (`core/value.rs:406`, `:554`); `Proxy::target/handler` (`core/value/proxy.rs:31, 40`) | — | Probe: `Proxy type_of=proxy is_proxy=true`. Spec: proxies are not cloneable → `DataCloneError`. |
| plain object vs class instance | `Object::get_prototype()` (`object.rs:157`) compared to `Object.prototype` / `None` | `Object::new` (`:19`) or `Object::new_proto(ctx, None)` (`:31`) for null-proto | Probe: class instance `proto==Object.prototype? false`, plain `true`, `Object.create(null)` → `None`. Spec clones class instances as plain objects (own enumerable props only), so no detection is strictly needed except for the platform types above. |
| `Array` + holes | `Value::into_array`/`is_array()`, `Array::len()`, `Array::get::<Value>(i)` returns `undefined` for holes (`core/value/array.rs:46`); distinguish holes with `arr.as_object().contains_key(i)` (`object.rs:51`) | `Array::new` + `set(i, v)` (`:19, 58`) — skipping hole indices preserves holes; set `length` afterwards | Probe: `len=3 get(1) type=undefined contains_key(1)=false contains_key(0)=true`. |
| property enumeration | `Object::keys()`/`props()` = own **enumerable string** keys, integer keys first then insertion order (`object.rs:111-127`, `Filter::default() = string().enum_only()` `:214-218`) | `Object::set` | Probe: `keys() default filter=["2","b","a"]`; `own_keys(Filter::new().string().symbol())` sees all 5 incl. the non-enumerable and the symbol. Default filter is exactly the structured-clone enumeration. |
| `Function`, `Promise`, `WeakMap`, class instances of host classes (`Class<'js, C>`) | `Type::Function/Constructor/Promise`; `Value::as_class::<C>()` (`core/class.rs:427`) | — | `DataCloneError`. `MessagePort` transfer is a host-class special case: detect with `as_class::<MessagePortClass>()`, move its channel halves. |

Cycle detection and identity preservation for the parts the supplement walks itself: key a `HashMap<*mut c_void,
NodeId>` by `value.as_raw()` object pointers (stable for the duration of one `with` call) and emit back-references;
the `JS_WriteObject2` part of the graph gets this from `JS_WRITE_OBJ_REFERENCE` (§10.0).

---

## 11. rquickjs APIs the design depends on that the sections above do not name

| Need | API (0.12.2) | Constraints verified from source |
|---|---|---|
| Per-context state the host functions reach without captures: the worker's outbound sender, `closing` token, name; the parent's `WorkerRegistry`; the `HostSlot` (doc 11 §1.3, §2.1, §4) | `Ctx::store_userdata<U>(&self, U) -> Result<Option<Box<U>>, UserDataError<U>>`, `Ctx::userdata<U>(&self) -> Option<UserDataGuard<U>>`, `Ctx::remove_userdata<U>()` (`core/context/ctx.rs:480-510`) | `U: JsLifetime<'js>` with `U::Changed<'static>: Any` — a struct of `mpsc::UnboundedSender`, `CancellationToken`, `String`, `Mutex<Vec<JoinHandle>>` qualifies with `#[derive(JsLifetime)]`. Insert/remove **fail while any `UserDataGuard` is alive** (`core/runtime/userdata.rs:114-121`, `count > 0`), so store everything at install time and only *read* from host functions. Dropped in `Opaque::clear` after the spawner and before `JS_FreeRuntime` (`opaque.rs:284-292`) — that drop is what closes the worker→parent channel (§2.2). |
| Native handle objects (`WorkerHandle`, `MessagePort` core) that JS-land classes wrap (doc 11 §2.2) | `#[rquickjs::class]` + `#[derive(Trace, JsLifetime)]` + `#[rquickjs::methods]`, with `#[qjs(skip_trace)]` on non-JS fields — exactly `den-stdlib-core/src/cancellation.rs:9-28` (`CancellationTokenWrapper`). `Class::<C>::instance(ctx, value)`, `Class::define(&globals)`, `Class::prototype(&ctx)`, `.borrow()/.borrow_mut()` (`core/class.rs:223, 273, 263, 300-313`); `Value::as_class::<C>()` / `instance_of::<C>()` (`:427, :390`) | `JsClass<'js>: Trace<'js> + JsLifetime<'js>` (`class.rs:87`). `Class<'js, C>` is a `Value` → `!Send`; the *fields* may be `Send` types, which is how channel halves and tokens get in. A class cannot `extends` a JS builtin or be extended by JS `class X extends Y` (den-stdlib-wasm/src/error.rs:5-8) — hence JS-land `EventTarget` hierarchy + native handle. |
| Host functions on the global (`postMessage`, `close`, `importScripts`, `structuredClone`) | `globals.set(name, Func::from(f))`, `Func::from(Async(f))` for promise-returning ones (`core/lib.rs` prelude), `Ctx<'js>` as any parameter, `Rest<Value>` for variadics, `Opt<T>` for optionals | Dispatched via the class `call` slot, no JS frame (§3.3) — `script_or_module_name(0)` is the caller. The closure may capture only `Send + 'static` data under `parallel` (`ParallelSend`); reach `Ctx`-bound state through userdata. |
| Sending from inside a host function (under the lock, not async) | `tokio::sync::mpsc::UnboundedSender::send` is synchronous and never blocks; a bounded `Sender` would need `try_send` (no `.await` is possible inside `with`/a host function) | Unbounded is what the probe and doc 11 use. Back-pressure, if ever wanted, has to be implemented as a JS-visible limit, not an await. |
| Module-side stdlib for the worker context (`console`, timers, `fetch`, …) | the same `BuiltinResolver`/`ModuleLoader` registration plus `Module::evaluate_def::<M, _>(ctx, "den:…")` that `Engine::new` performs inline (`engine.rs:43-222`, `:236-297`) | doc 11 §1 routes this through `WorkerHost::build_engine`, i.e. a second `Engine::new` per worker; the `Arc<EasyOxcTranspiler>` (`engine.rs:27`) is `Send + Sync` and can be shared or rebuilt. The worker's interrupt handler must read a **child** of the parent's `stop_token` (`engine.rs:227-232` already takes a child of whatever token it is given) so Ctrl-C propagates (doc 11 §4). |
| Telling "terminated" from "threw" after any `Function::call` / eval | `Value::is_uncatchable_error()` (`core/value.rs:394-396`) on `ctx.catch()` | After the interrupt token is cancelled **every** later JS entry throws again (§4.3) — do not try to dispatch `error` events in a terminated worker. |
| Raw serializer calls | `rquickjs::qjs::{JS_WriteObject2, JS_ReadObject2, JS_FreeValue, js_free}` (`sys/bindings/x86_64-unknown-linux-gnu.rs:1683-1715`); `value.as_raw()`, `ctx.as_raw().as_ptr()`, `Value::from_js_value(ctx, raw)` / `ctx.handle_exception(raw)` | The writer's output buffer is `js_malloc`'d in the *writer's* runtime — copy it into a `Vec<u8>` and `js_free(ctx, ptr)` before leaving the `with` (doc 10 §7 shows the sequence). |

---

## Appendix A — compile-and-run-verified probe

Location: `<scratchpad>/threads-probe/` (`Cargo.toml` + `src/main.rs`, ~600 lines), built with
`CARGO_TARGET_DIR=<scratchpad>/dentarget cargo run`. Shape: parent `new_multi_thread()` runtime (like `#[tokio::main]`)
→ `std::thread::Builder::spawn` → inside it `Builder::new_multi_thread().worker_threads(1).enable_all().build().block_on(...)`
→ fresh `AsyncRuntime` + `AsyncContext::full` per test, interrupt handler reading an `Arc<AtomicBool>` the parent flips
on a timer, a `tokio::sync::mpsc` inbox/outbox pair across the threads, and a second build with `--features negative`
for the `!Send` proofs.

Full run output (exit code 0):

```
P0 block_in_place on current_thread runtime: panicked: Some("can call blocking only when running on the multi-threaded runtime")
T0 runtime-created-on-parent-thread usable on worker thread: 1+1=2
T1 tight-loop interrupt after 300.024012ms: result=Err(Exception) | catch: uncatchable=true message=Some("interrupted") stack=Some("    at <eval> (eval_script:1:1)\n") | js-caught=false
error executing job: Error: interrupted
T2 microtask-loop: idle() after kill resolved=true in 302.485139ms
T3 external-await: idle() pending after 500ms=true; run_until_cancelled(idle) -> None in 101.313823ms
T3 dropped ctx+rt with a pending ctx.spawn future alive: no abort
T4 ctx.spawn pump: idle() pending while pump alive=true; idle resolved after sender dropped in 393.204999ms; got=["hello", "world"]
T5 block_in_place on worker_threads(1): under async_with=42, under idle()=42
T6 set_memory_limit(16MiB) with rust-alloc: eval=Ok(()) | catch: uncatchable=false non-exception value type=uninitialized | js-caught=Some("InternalError: out of memory") | malloc_size now=106648 limit=16777216
T7 rejection tracker calls: ["boom-later-handled handled=false", "boom-unhandled handled=false", "boom-later-handled handled=true"]
T8 script_or_module_name: script(filename set, level 0)=Some("file:///tmp/main.js") | nested-fn(level 0)=Some("file:///tmp/nested.js") | level 1=None | eval without filename=Some("eval_script") | module=Some("file:///tmp/mod.mjs") | from Rust with no JS frame=None
T9 eval error: result=Err(Exception) name=TypeError own string props=[("message", "bad"), ("stack", "    at f (/tmp/x.js:2:26)\n    at <eval> (/tmp/x.js:3:1)\n")]
T9 module throw at top level: err="Ok(promise) -> awaited: Err(Exception)" catch: uncatchable=false message=Some("module-fail") stack=Some("    at <anonymous> (file:///tmp/bad.mjs:1:30)\n")
T9 Module::import of missing specifier: err="Ok(promise) -> awaited: Err(Exception)" catch: uncatchable=false message=Some("could not load module 'does-not-exist.js'") stack=Some("")
T10 ArrayBuffer: bytes=Some([1, 2, 3, 4]) after detach as_bytes=None JS byteLength=0
T10 TypedArray<u16>: len=2 bytes=Some([1, 0, 255, 255]) buffer-len=4 ; TypedArray<u8> from a Uint16Array -> Err (class check)
T10 DataView: type_of=object is_instance_of(DataView)=true byteOffset=2 byteLength=4
T10 Date: JS_IsDate=true SystemTime::from_js=1700000000000 SystemTime::into_js is Date=true
T10 RegExp: is_instance_of(RegExp)=true source=a+b flags=gi
T10 Map: is Map=true entries-via-Array.from=2 Object::keys()=[]
T10 BigInt: 2^62 to_i64=Ok(4611686018427387904); 2^70 to_i64=Ok(0); toString=1180591620717411303424 ; BigInt(str) type_of=big_int
T10 Error: is_error=true name=RangeError message=r cause.message=c has stack=true
T10 Symbol type_of=symbol Proxy type_of=proxy is_proxy=true | class instance proto==Object.prototype? false | plain proto==Object.prototype? true | Object.create(null) proto="none"
T10 Array holes: len=3 get(1) type=undefined contains_key(1)=false contains_key(0)=true
T10 keys() default filter=["2", "b", "a"] | own_keys(string+symbol, incl non-enumerable) count=5
T12 idle(): with() blocked while idle() pending=true; after idle resolved with()=2
T12 drive(): with() while drive() running -> Ok(3); drive task finished before runtime drop=false
T12 drive() completes once the runtime is dropped: false
T11 worker main future finished; tokio runtime dropping
T11 worker tokio runtime dropped; OS thread exiting
P1 worker thread joined ok=true ; echoes received on parent=["echo:hello", "echo:world"]
```

`--features negative` (the six expected compile errors, abridged):

```
error[E0277]: `*mut JSRuntime` cannot be sent between threads safely   within `Persistent<rquickjs::Object<'static>>`
error[E0277]: `*mut c_void` cannot be sent between threads safely      within `Persistent<rquickjs::Object<'static>>`
error[E0277]: `*mut c_void` cannot be sent between threads safely      within `rquickjs::Value<'static>`
error[E0277]: `*mut c_void` cannot be sent between threads safely      within `rquickjs::Object<'static>`
error[E0277]: `*mut c_void` cannot be sent between threads safely      within `rquickjs::Function<'static>`
error[E0277]: `NonNull<JSContext>` cannot be shared between threads safely   within `Ctx<'static>`
```

Positive `Send`/`Sync` assertions that compiled in the same file: `AsyncRuntime: Send + Sync`,
`AsyncContext: Send + Sync`, `Ctx<'static>: Send`.

Things the first probe run taught (and that the final code reflects):

* `Vec<(Value, Value)>` does not implement `FromJs` — tuples are not convertible; read JS pairs as `Array`.
* `ArrayBuffer::new(ctx, vec![...])` + `detach()` aborts in the finalizer (§10). Switched to `new_copy`.
* `Module::evaluate` / `Module::import` never return `Err` for runtime failures; the promise rejects (§9.2).

---

## Verification log / open questions

* **Not measured**: interrupt latency under a deep regex backtrack (covered by `lre_check_timeout` reading the same
  handler, `qjs/quickjs.c:48453-48458`, so it should be immediate — the handler is called on every regex timeout
  check, not every 10 000 steps).
* **Not probed**: `JS_SetCanBlock` (`qjs/quickjs.c:2126`, bound at `sys/bindings:1554`) for `Atomics.wait` in
  workers; not wrapped by rquickjs; out of scope while `SharedArrayBuffer` is not transferred.
* **stdout noise**: `idle()`'s `println!("error executing job: …")` (`core/runtime/async.rs:337-340`) will print
  `error executing job: Error: interrupted` on every `terminate()` that lands inside a microtask, and for every
  uncaught job error in a worker. Options: accept; or route all host→JS calls through code that catches first so the
  job never errors; or upstream a hook. No rquickjs API exists to silence it.
* **`drive()` does not terminate** after the runtime is dropped (T12) — harmless for den's detached `tokio::spawn`
  usage but must not be awaited for shutdown.
* `Engine::run_file`/`Engine::eval` evaluate without `EvalOptions.filename`; giving them one (the file path /
  `"<repl>"`) costs nothing and makes `script_or_module_name` useful from the entry script (§8).
* The spec pages were only partially fetchable (HTML workers.html is truncated by the fetcher after "terminate a
  worker"); constructor/importScripts rules are cited from MDN, whose text matches the algorithm steps the other
  research doc should quote from the HTML standard directly.

---

## Verification log — completeness review (2026-08-22)

Second pass by a reviewer who re-read every cited line in
`/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{rquickjs-core-0.12.2,rquickjs-sys-0.12.2,tokio-1.53.1,tokio-util-0.7.19}`
and the den sources named in §0. No probe was re-run; claims below are source-confirmed unless marked.

**Confirmed verbatim (no change):**

| Claim | Evidence re-read |
|---|---|
| `idle()` holds the `async_lock` guard across `Poll::Pending`; `SchedularPoll::Empty` is the only `Ready` exit; job errors go to `println!` | `core/runtime/async.rs:313-360` (guard `lock` at `:314` borrowed by the `ManualPoll` closure through `f.await`) |
| `async_with` releases the lock on every `Pending` — both the `ShouldYield` early return and the `!made_progress` exit drop the local guard | `core/context/async/future.rs:66-157` (`lock` is a local of `poll`, `mem::drop(lock)` at `:154`; the `ShouldYield` `return` drops it by scope) |
| `Ctx::spawn` bound is `Future<Output = ()> + 'js` — **no `Send`**, so the future may own `Function<'js>`/`Ctx` | `core/context/ctx.rs:418-424` |
| `Send`/`Sync` impls for `AsyncRuntime`, `AsyncWeakRuntime`, `InnerRuntime`, `AsyncContext`, `Ctx` (Send only), `DriveFuture` | `async.rs:50-51, 86-97`; `context/async.rs:244-251`; `ctx.rs:103`; `spawner.rs:64-67` |
| `Persistent { rt: *mut JSRuntime, value }`, `restore` → `Error::UnrelatedRuntime`; no `unsafe impl Send` in the file | `core/persistent.rs:37-40, 102-111` |
| `drive()` takes `lock_arc` per poll, registers via `listen()`, returns `Pending` after each pass; only `Ready` when `try_ref()` fails | `core/runtime/spawner.rs:81-133` |
| `set_interrupt_handler`, `set_host_promise_rejection_tracker`, `set_loader<R: Resolver + 'static, L: Loader + 'static>` (no `Send` bound on loaders), `set_memory_limit`, `set_max_stack_size` (clamps > 16 MiB to 0), `memory_usage`, `run_gc` signatures | `async.rs:163-262`; `raw.rs:246-264` |
| `InterruptHandler`/`RejectionTracker` are `+ Send + 'static` under `parallel`; tracker args `(Ctx, promise, reason, is_handled)` | `core/runtime.rs:41-52` |
| Interrupt: `JS_INTERRUPT_COUNTER_INIT 10000`, `JS_ThrowInterrupted` sets uncatchable, `JS_IsUncatchableError` checks at the interpreter `exception:` label, in `js_async_function_resume`, and in promise reactions | `qjs/quickjs.c:479, 8215-8226, 20328, 20900-20912, 54316` |
| `set_memory_limit` works with `rust-alloc`: limit checked in `js_malloc_rt`/`js_calloc_rt`/`js_realloc_rt` before the allocator | `qjs/quickjs.c:1622-1623, 1645-1646, 1692-1693, 2087` |
| `ArrayBuffer::new` + `detach` UB: `JS_DetachArrayBuffer` calls `free_func` then sets `data = NULL` but leaves `free_func` set; finalizer calls `free_func(rt, opaque, NULL)` again; `drop_raw` does `Vec::from_raw_parts(NULL, cap, cap)` | `qjs/quickjs.c:58030-58041, 57908-57937`; `core/value/array_buffer.rs:91-119` |
| `BigInt::to_i64` uses `JS_ToInt64Ext` with no range error | `core/value/bigint.rs:23-31` |
| `JS_FreeRuntime` asserts `list_empty(&rt->gc_obj_list)`; assertions on unless `disable-assertions` defines `NDEBUG`; den's feature list does not enable it | `qjs/quickjs.c:2348`; `sys/build.rs:147-148`; root `Cargo.toml:49-55` |
| `RawRuntime::drop` → `Opaque::clear()` (tracker, interrupt, panic, prototypes, spawner, userdata) → `JS_FreeRuntime` | `core/runtime/raw.rs:124-133`; `opaque.rs:284-292` |
| `block_in_place` decision table; `Runtime::block_on` on multi-thread enters with `allow_block_in_place = true`; `current_thread` panics with "can call blocking only when running on the multi-threaded runtime" | `tokio/runtime/scheduler/multi_thread/worker.rs:408-448`; `multi_thread/mod.rs:87-94` |
| Dropping a tokio `Runtime` inside an async context panics "Cannot drop a runtime in a context where blocking is not allowed" (from `BlockingPool` shutdown wait) | `tokio/runtime/blocking/shutdown.rs:44-56`; `blocking/pool.rs:244-275`; `runtime/runtime.rs:452-514` |
| `CancellationToken::run_until_cancelled(&self, fut) -> Option<F::Output>`, returns `None` immediately if already cancelled | `tokio-util/src/sync/cancellation_token.rs:300-334` |
| `EvalOptions` fields/defaults (`global: true, strict: true, backtrace_barrier: false, promise: false, filename: None`), `eval_with_options` defaults the name to `"eval_script"` | `core/context/ctx.rs:29-78, 184-205` |
| `script_or_module_name(isize) -> Option<Atom>` wraps `JS_GetScriptOrModuleName`, which walks `current_stack_frame` and returns the bytecode's `filename` | `ctx.rs:455-466`; `qjs/quickjs.c:30890-30913` |
| den: `Engine::new` inline loader tuple and `evaluate_def` calls, `set_max_stack_size(0)`, interrupt handler on a child token, `run_file`/`eval` without `filename`; `App::run_until_end` = `spawn(drive())` + `run_until_cancelled(idle())`; both loaders `block_in_place` and transpile `IsModule::Bool(true)`; timers use per-timer tokens | `den-core/src/engine.rs:39-40, 43-222, 227-232, 236-297, 325-337, 368-374`; `src/app.rs:98-115`; `loader/mmap_script.rs:78-92`, `loader/http.rs:95-116`; `den-stdlib-timer/src/lib.rs:26-37, 55-69` |

**Corrected in place:**

1. §5 / fact 15 — default JS stack limit is **1 MiB** (`JS_DEFAULT_STACK_SIZE (1024 * 1024)`, `qjs/quickjs.h:423-424`),
   not 256 KiB; the rquickjs doc comment is stale for quickjs-ng 0.15.1.
2. §7.3 / fact 9 — "either `Module::import` or the `await import()` string" was unsafe advice: the string path runs
   the module in a *job* (`js_dynamic_import_job`, `qjs/quickjs.c:31182`) that `async_with` executes after polling
   the spawner, so a pre-spawned pump dispatches messages before the module has run. `JS_LoadModule` is synchronous
   up to the first `await` (`qjs/quickjs.c:30999-31040`). Now: `Module::import` with an absolute specifier (its base
   is `script_or_module_name(1)`, `module.rs:431`, i.e. `""` when called from Rust).
3. §1.4 / §10 — the doc presented a hand-rolled `SerializedValue` tree as *the* payload; doc 10 had already decided
   `JS_WriteObject2` bytes. Rewritten as "bytes + supplement", with the serializer's verified support/refusal list in
   the new §10.0 (Map/Set supported; Error/DataView/DOMException/accessors refused; Symbol over-accepted).
4. §4.4 skeleton — `stop.run_until_cancelled(idle())` replaced by `closing.run_until_cancelled(idle())`, with the
   reason (per-timer tokens keep `idle()` pending after `close()`).
5. §6.2 — "or simply let it be detached" removed; doc 11 §4 decided joining.

**Added:** §3.3 paragraph on the class-`call` dispatch (no JS frame for host functions); §3.4 (pump spawn timing vs.
`WithFuture::poll` order); §4.4 `close()` row; §10.0; §11 (userdata, class, `Func`, mpsc-under-lock, stdlib
reuse, raw serializer entry points); TL;DR facts 13-15.

**Cross-doc inconsistencies for the maintainer (doc 09 is the source of truth for the mechanism):**

* Doc 11 §9 `WorkerBoot.base = ctx.script_or_module_name(1)` — must be level **0** (probe T8; `qjs/quickjs.c:17619-17627`).
* Doc 11 §9 `boot.stop_token.run_until_cancelled(parts.runtime.idle())` — must be the `closing` token (§4.4), or a
  worker with a pending timer never exits after `close()`.
* Doc 10 §4.3's future zero-copy rebuild via `ArrayBuffer::new` is UB if that buffer is ever detached again (fact 12).
* Doc 11 §2.1 spawns the pump before `script::run`; that is only correct together with correction 2 above
  (`Module::import`, not an `await import()` string) or with a pump gated until evaluation returns.

**Unconfirmable from source alone / not re-probed:**

* `block_in_place` from a *spawned* task on the 1-worker runtime (core hand-off path, `worker.rs:455-492`) — probe T5
  exercised only the `block_on` root-future path (both T5 cases ran inside the root future). Source says it works;
  the worker design never needs it because all JS runs in the root future.
* `JS_ReadObject2` in a second runtime for a blob containing `BC_TAG_SYMBOL` registered symbols, and the Map/Set
  zombie-record desync (doc 10 §4.4) — owned by doc 10's probe, not re-run here.
* HTML spec text for "run a worker" steps 2.12-2.13 and "close a worker" is taken from doc 08 §2.8, which quotes the
  standard; this reviewer did not re-fetch workers.html.
