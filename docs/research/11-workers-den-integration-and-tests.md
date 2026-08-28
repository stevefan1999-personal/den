# 11 — Web Workers: how the feature fits den, and how it is tested

Companion to the Worker research series. This note answers one question only:
*where does a dedicated `Worker` live in den's crate graph, engine, lifecycle
and test suite?* Structured clone, EventTarget semantics and the spec surface
are other notes' business; they are mentioned here only where they constrain
the integration.

Every claim is pinned to a file:line read on 2026-08-22 against
rquickjs 0.12.2, tokio 1.53.1, quickjs-ng as vendored in rquickjs-sys 0.12.2,
and den at commit `4975f63`. Companion notes that *do* exist and are assumed
read: `08-web-workers-spec.md` (the spec surface), `09-rquickjs-threads-and-event-loop.md`
(compile-and-run probes T1–T12 of the event loop; every event-loop claim below
agrees with it) and `10-structured-clone-strategy.md` (the serializer).

Decisions taken by the maintainer and *not* re-opened here: one OS thread per
worker with its own tokio runtime and its own QuickJS runtime; `terminate()`
must stop a running script; classic is the default script type; a live worker
keeps the process alive; no SharedWorker / ServiceWorker.

---

## 0. The substrate, verified

| Fact | Where |
|---|---|
| `Engine` is `Clone` and owns `transpiler: Arc<EasyOxcTranspiler>`, `runtime: AsyncRuntime`, `context: AsyncContext`, `stop_token: CancellationToken` | `den-core/src/engine.rs:24-31` |
| `Engine::new` is the composition root: one `BuiltinResolver`, one `HttpResolver`, one `FileResolver` rooted at `./`; one `BuiltinLoader`, `ModuleLoader`, `HttpLoader`, `MmapScriptLoader`; interrupt handler = `stop_token.child_token().is_cancelled()`; then `evaluate_def` of the seven global-installing modules | `den-core/src/engine.rs:35-306`, interrupt at `:227-232` |
| `Engine::new` makes a *fresh* `CancellationToken` — nothing lets a caller hand one in | `den-core/src/engine.rs:225` |
| `run_file` evals `` await import(`{path}`) `` as a **global script** with the default filename (`eval_script`) | `den-core/src/engine.rs:325-332`; default filename `rquickjs-core/src/context/ctx.rs:199` |
| Both file/URL loaders end in `tokio::task::block_in_place(|| Handle::current().block_on(task))` | `den-core/src/loader/mmap_script.rs:92`, `den-core/src/loader/http.rs` (ARCHITECTURE §3) |
| `block_in_place` panics on a `current_thread` runtime: *"can call blocking only when running on the multi-threaded runtime"*; it is allowed from inside a multi-thread runtime's `block_on` | `tokio/src/runtime/scheduler/multi_thread/worker.rs:420-435` |
| `Builder::worker_threads(0)` asserts | `tokio/src/runtime/builder.rs:519-521` |
| `AsyncRuntime` / `AsyncContext` are `Send + Sync` under `parallel`; every method takes the runtime `Mutex` | `rquickjs-core/src/runtime/async.rs:84-97`, `context/async.rs:246-251` |
| `AsyncRuntime::idle()` returns `Ready` **only** when the spawner reports `SchedularPoll::Empty`, i.e. no spawned future is left; a spawned future that is merely pending yields `Poll::Pending` with the waker registered | `runtime/async.rs:313-360`, `runtime/schedular.rs:140-143` (`is_empty` = task list empty), `:152-158` |
| `idle()` **holds the runtime lock for as long as it is pending** (the `MutexGuard` is moved into the `ManualPoll` closure) | `runtime/async.rs:314-318` |
| `drive()` takes the lock per poll and releases it (`lock` is a loop local) | `runtime/spawner.rs:85-133` |
| `AsyncContext::async_with` also polls the spawner while the root future is pending | `context/async/future.rs:109-136` |
| Spawned-task wakers are cross-thread safe: `schedular_wake` pushes to an atomic queue and wakes an `AtomicWaker` | `runtime/schedular/waker.rs:23-48`, `schedular/queue.rs:23,53` |
| `Ctx::spawn` takes `F: Future<Output = ()> + 'js` — **no `Send` bound**, so a spawned future may hold `Ctx`/`Function` | `context/ctx.rs:418-423` |
| `InterruptHandler = Box<dyn FnMut() -> bool + Send + 'static>` (parallel) | `rquickjs-core/src/runtime.rs:52` |
| A `true` from the interrupt handler throws `InternalError: interrupted` marked **uncatchable**; polled from loop back-edges, so `while(true){}` is interruptible | `quickjs.c:8215-8240`, back-edge polls at `:18666-18755` |
| `PromiseFuture::poll` bails out with `Err(Error::Exception)` on an uncatchable pending exception | `rquickjs-core/src/value/promise.rs:193-200` |
| `RejectionTracker = Fn(Ctx, promise, reason, is_handled: bool) + Send`; QuickJS calls it with `false` at rejection and `true` when a handler is attached later | `rquickjs-core/src/runtime.rs:44-45`, `quickjs.c:54371-54375`, `:55165-55169` |
| den sets **no** rejection tracker today — unhandled rejections are silently dropped in the main context | `den-core/src/engine.rs` (grep: no `set_host_promise_rejection_tracker`) |
| `idle()` prints job errors with `println!` (stdout), marked `TODO` | `runtime/async.rs:335-341` |
| `ctx.store_userdata` fails with `UserDataError` if any `UserDataGuard` is alive | `runtime/userdata.rs:114-121` |
| Runtime drop clears userdata **before** `JS_FreeRuntime` | `runtime/raw.rs:123-132`, `runtime/opaque.rs:284-292` |
| `Ctx::script_or_module_name(n)` = `JS_GetScriptOrModuleName(ctx, n)`: walk `n` frames up `rt->current_stack_frame`, return that frame's `JSFunctionBytecode.filename` (or `None` if the frame is not bytecode). **Level 0, not 1, from an rquickjs-defined function**: rquickjs functions are class objects dispatched through the class `call` hook (`runtime/opaque.rs:123`, `quickjs.c:17621-17625`), which pushes **no** `JSStackFrame`, so level 0 is already the JS caller (doc 09 probe T8 confirms). quickjs-libc's `os.Worker` and rquickjs's own `Module::import` use level 1 because `JS_NewCFunction` natives *do* push a frame (`js_call_c_function`, `quickjs.c:17373-17379`) — do not copy that `1`. Called with no JS frame at all → `None` | `context/ctx.rs:455-464`, `quickjs.c:30890-30913`, `value/module.rs:426-431`, `quickjs-libc.c:4160-4166` |
| `EvalOptions` is `#[non_exhaustive]`: a struct literal (`EvalOptions { global: true, ..Default::default() }`) does **not compile** outside rquickjs; build it as den does — `let mut options = EvalOptions::default(); options.global = true; …` | `context/ctx.rs:28-40`, `den-core/src/engine.rs:327-331` |
| `qjs::JS_LoadModule(ctx, basename, filename) -> JSValue` (a promise) is bound; it resolves `filename` against `basename` through the installed resolver, loads, instantiates and **starts evaluating synchronously** before returning (`JS_LoadModuleInternal`), unlike `import()` which only enqueues a job (`js_dynamic_import` → `js_dynamic_import_job`) | `quickjs.h:1247`, `quickjs.c` (`JS_LoadModule`, `js_dynamic_import_job`), `rquickjs-sys/src/bindings/x86_64-unknown-linux-gnu.rs:1729` |
| `DOMException` (with the `DataCloneError`/`InvalidStateError`/… name table) is **already a global in every den context**: `AsyncContext::full` → `JS_NewContext` → `JS_AddIntrinsicAToB` → `JS_AddIntrinsicDOMException`. No JS-land class is needed for it (doc 10 §6) | `context/async.rs:161-163`, `quickjs.c:63338-63343`, `:62150-62175`, `:62329-62356` |
| `Persistent<T>` is `!Send` (raw `*mut JSRuntime` + raw value) and cannot cross runtimes (`UnrelatedRuntime`) — it must not be captured by a `Send` closure such as the rejection tracker (§5.2) | `persistent.rs:88-100`, doc 09 fact 2 |
| `FileResolver::resolve`: names starting with `.` are joined onto `parent(base)`; anything else is joined onto the search paths (`./`) — which is why absolute paths fail (ARCHITECTURE §6) | `rquickjs-core/src/loader/file_resolver.rs:130-152` |
| `EvalOptions.filename` sets the script name that `script_or_module_name` will later report | `context/ctx.rs:38-40`, `:184-204` |
| The global object is a plain `JS_CLASS_OBJECT` (so `Object.setPrototypeOf(globalThis, …)` works) | `quickjs.c:57357` |
| QuickJS-ng 0.15.1 ships a graph serializer: `JS_WriteObject2(ctx, &mut size, obj, flags, *mut JSSABTab)` / `JS_ReadObject2(ctx, buf, len, flags, *mut JSSABTab)` with `JS_WRITE_OBJ_REFERENCE` / `JS_READ_OBJ_REFERENCE` for cycles and shared references (without the flag a cycle is `TypeError: circular reference`); supports null/undefined/bool/number/string/BigInt, arrays, plain objects, ArrayBuffer, typed arrays, RegExp, Date, boxed primitives, Map, Set — **and `Symbol`** (`BC_TAG_SYMBOL`, both plain and `Symbol.for` symbols are written, `quickjs.c:38362-38371`), so a spec-correct `structuredClone(Symbol())` → `DataCloneError` must be enforced by a pre-walk, not by the serializer. `Error` objects, functions, Promises, Proxies, class-private state are `TypeError: unsupported object class` (`:38344-38347`) | `quickjs.h:1214-1236`, `quickjs.c:38218-38376`; bindings `rquickjs-sys/src/bindings/x86_64-unknown-linux-gnu.rs:1671` (`JSSABTab`), `:1691-1715`; `rquickjs::qjs` re-exports the sys crate (`rquickjs-core/src/lib.rs:65-67`); den already calls raw `qjs::` in `den-stdlib-wasm/src/memory.rs:9` |
| quickjs-libc's own `os.Worker` is the prior art for exactly this design: a detached thread, a fresh `JSRuntime`, `JS_LoadModule(basename, filename)`, and messages as `JS_WriteObject2` blobs over a mutex-protected queue | `quickjs-libc.c:4058-4113` (thread body), `:4150-4224` (ctor), `:4226-4275` (postMessage), `:2675-2725` (receive) |
| `EasyOxcTranspiler` is a ZST, `Send + Sync`, and proven shareable across 8 threads | `den-transpiler-oxc/src/lib.rs:375-400`, ARCHITECTURE §4 |
| quickjs-ng `Error` objects carry `message` and `stack` only — no `fileName`/`lineNumber` own properties; frames are formatted `    at name (file:line:col)`; `Error.prepareStackTrace` CallSites expose `getFileName/getLineNumber/getColumnNumber` | `quickjs.c:7878-7893`, `:7956`, `:62109-62115` |
| `tempfile` is in `Cargo.lock` only transitively via wasmtime; no workspace crate depends on it | `cargo tree -i tempfile` |
| CI runs `cargo nextest run --workspace --no-default-features --features stdlib,typescript,react,<backend>` for both wasm backends | `.github/workflows/lint.yml:44-55` |

---

## 1. Dependency direction — recommendation: **(b)**, with the narrowest possible host trait

### 1.1 Why (a) and (c) lose

**(c) "move engine construction into a leaf crate below both"** is not an
option at all. Engine construction *is* the thing that depends on every
`den-stdlib-*` crate (`den-core/Cargo.toml:42-53`, `engine.rs:48-292`). Any
crate that builds an engine sits above the stdlib crates by definition, so a
"leaf below the worker crate" that builds engines would have to depend on the
worker crate's module — the same cycle, one directory over. (c) collapses into
(a) or (b).

**(a) "implement it inside den-core"** compiles, and it is the ponytail reflex
answer — but it fails the repository's own stated boundary. ARCHITECTURE §1:
*"`den-core` owns everything about how JavaScript gets in … Each `den-stdlib-*`
crate owns one JS-visible API and knows nothing about the loader chain."*
`EventTarget`, `Event`, `MessageEvent`, `ErrorEvent`, `MessageChannel`,
`MessagePort`, `BroadcastChannel` and `structuredClone` are JS-visible APIs
with zero loader knowledge; they are `den-stdlib-*` material by the rule every
other global follows. Only one thing in the whole feature needs den-core: "give
me a fully built engine". Putting ~1500 lines into den-core to get at one
`async fn` inverts the ratio.

### 1.2 Why (b) is not "DI abuse"

The maintainer's rule: an interface with one implementation and no testing
seam is a smell. Here the seam is real and it is the *only* way the worker
crate can test the cross-thread path without den-core:

- The trait has one production implementation (`den-core`) and **one test
  implementation** (`den-stdlib-worker`'s own `BareHost`, §10.2) that builds a
  bare `AsyncRuntime` + `AsyncContext::full` + `evaluate_def::<js_worker>` —
  the exact harness precedent of `den-stdlib-wasm/src/lib.rs:354-385`.
- The trait surface is a single method. It is not a strategy, it is a
  constructor pointer. That is the "factory methods" case the guidance names.

Keep it that small. Everything the worker crate can do itself — spawning the
OS thread, building the per-thread tokio runtime, installing the worker scope,
loading the script, running the loop, reporting errors, joining — it does
itself, in rquickjs terms, with no den-core in sight.

### 1.3 The contract

```rust
// den-stdlib-worker/src/host.rs

/// What a worker thread needs from the embedder: a freshly built engine.
///
/// Lifetime: one `Arc<dyn WorkerHost>` per process (singleton), cloned into the
/// userdata of every context that may call `new Worker` — which includes the
/// worker contexts themselves, so nesting is free. The `EngineParts` it
/// returns are scoped to one worker thread and dropped when that thread ends.
pub trait WorkerHost: Send + Sync + 'static {
  /// Build a runtime + context with the full stdlib installed, whose interrupt
  /// handler observes `stop_token` (so cancelling it stops a running script).
  fn build_engine(
    &self,
    stop_token: CancellationToken,
  ) -> Pin<Box<dyn Future<Output = EngineParts>>>;
}
```

No `Send` on the boxed future: it is only ever awaited by `Runtime::block_on`
on the worker's own OS thread (§9), which needs no `Send`, and `Engine::new`'s
future holds rquickjs lock guards across `.await`s — proving it `Send` buys
nothing and may not compile.

```rust

/// The subset of `den_core::engine::Engine` a worker thread actually touches.
pub struct EngineParts {
  pub runtime: AsyncRuntime,
  pub context: AsyncContext,
  #[cfg(feature = "transpile")]
  pub transpiler: Arc<EasyOxcTranspiler>,
}

/// Userdata slot the module reads when `new Worker` runs.
#[derive(JsLifetime)]
pub struct HostSlot(pub Arc<dyn WorkerHost>);
```

den-core's side is a dozen lines:

```rust
// den-core/src/engine.rs  (AFTER)
#[cfg(feature = "stdlib-worker")]
struct DenWorkerHost;

#[cfg(feature = "stdlib-worker")]
impl den_stdlib_worker::WorkerHost for DenWorkerHost {
  fn build_engine(&self, stop_token: CancellationToken)
    -> Pin<Box<dyn Future<Output = den_stdlib_worker::EngineParts>>>
  {
    Box::pin(async move { Engine::new_with_stop_token(stop_token).await.into_parts() })
  }
}

impl Engine {
  pub async fn new() -> Engine { Self::new_with_stop_token(CancellationToken::new()).await }

  pub async fn new_with_stop_token(stop_token: CancellationToken) -> Engine {
    // … the existing body of `new`, with `let stop_token = CancellationToken::new();`
    //   (engine.rs:225) replaced by the parameter …
    context.with(|ctx| {
      // … existing evaluate_defs …
      #[cfg(feature = "stdlib-worker")]
      {
        let _ = Module::evaluate_def::<den_stdlib_worker::js_worker, _>(ctx.clone(), "den:worker")?;
        ctx.store_userdata(den_stdlib_worker::HostSlot(Arc::new(DenWorkerHost)))
          .map_err(|_| rquickjs::Error::Unknown)?;
      }
      Ok::<_, rquickjs::Error>(())
    }).await.unwrap();
    // …
  }
}
```

`Engine::new_with_stop_token` is the only new public API den-core grows, and
it is independently useful (the App could pass its own token instead of
reaching into `engine.stop_token`).

What the worker crate depends on: `rquickjs` (`macro`, `futures`,
`array-buffer` — same list as `den-stdlib-wasm/Cargo.toml:18`), `tokio`
(`rt`, `rt-multi-thread`, `sync`), `tokio-util`, `relative-path` (already in
den-core's tree, `den-core/Cargo.toml:19`) and, optionally, `den-transpiler-oxc`
behind a `transpile` feature mirroring `den-core/Cargo.toml:52,69-72` — needed
only to transpile *classic* worker scripts, which cannot go through the module
loader (§7).

---

## 2. The worker global scope: who installs it, and how it gets the ports

### 2.1 Two installs, two moments

den's three-phase registration (ARCHITECTURE §2: `BuiltinResolver` →
`ModuleLoader` → `evaluate_def`) runs **inside `Engine::new`** and is identical
for the main context and for every worker context. That is the right place for
the things *every* context has: `Worker`, `MessageChannel`, `MessagePort`,
`BroadcastChannel`, `EventTarget`, `Event`, `MessageEvent`, `ErrorEvent`,
`structuredClone`. They go in `js_worker`'s `#[qjs(evaluate)]` hook exactly the
way `den:wasm` installs `WebAssembly` (`den-stdlib-wasm/src/lib.rs:206-257`)
and `den:core` both exports and sets globals (`den-stdlib-core/src/lib.rs:65-75`).
Nested workers therefore work with no extra code: the child's `Engine::new`
installs `Worker` and the `HostSlot` just like the parent's.

The worker-only surface — `self`, `postMessage`, `onmessage`,
`onmessageerror`, `onerror`, `close`, `importScripts`, `name`, and the global
*being* an `EventTarget` — is **not** a module concern and must not enter
`Engine::new`: it needs the ports, and `Engine::new` has no ports. It is
installed by the worker thread body, after `host.build_engine()` returns and
before the script loads:

```rust
// den-stdlib-worker/src/scope.rs
#[derive(JsLifetime)]
pub struct WorkerScope {
  pub name: String,
  pub outbound: mpsc::UnboundedSender<Outbound>,               // worker → parent
  pub closing: CancellationToken,                               // close() cancels this
  pub inbound: RefCell<Option<mpsc::UnboundedReceiver<Inbound>>>, // parked here while no pump is ref'd
  pub pump: Cell<Option<CancellationToken>>,                    // child of `closing`; Some while the pump runs
}

pub async fn install(parts: &EngineParts, boot: &WorkerBoot, inbound: mpsc::UnboundedReceiver<Inbound>) {
  parts.context.with(|ctx| {
    // 1. make the global an EventTarget: JS-land prelude, same technique as
    //    DEFINE_ERRORS in den-stdlib-wasm/src/error.rs:22-43
    //      Object.setPrototypeOf(globalThis, DedicatedWorkerGlobalScope.prototype)
    //      Object.defineProperty(globalThis, "self", { get: () => globalThis })
    //      onmessage/onmessageerror accessors backed by addEventListener;
    //      onerror is the *special* OnErrorEventHandler (HTML §8.1.8.1): called as
    //      handler(message, filename, lineno, colno, error) — five positional args,
    //      not the ErrorEvent — and a `true` return value cancels the event
    //      globalThis.name = <name>
    // 2. native pieces: postMessage(value, transfer) → structuredClone-to-bytes → outbound.send
    //    close() → closing.cancel(); importScripts(...urls) → §7.3
    // 3. the inbound pump is NOT spawned here — see the two rules below.
    ctx.store_userdata(WorkerScope { name, outbound, closing, inbound: RefCell::new(Some(inbound)), pump: Cell::new(None) })
      .expect("fresh context has no guards alive");
  }).await;
}
```

Two rules govern the inbound pump, and the first draft of this note got both
wrong:

1. **Enable after the initial run.** HTML "run a worker" enables the inside
   port's message queue *after* "run the classic/module script"; until then
   messages posted by the parent sit in the queue (doc 08 §2.7 step 4 vs the
   `Worker` constructor's outside port, which is enabled immediately). If the
   pump were spawned before `script::run`, `async_with`'s spawner polling
   (`future.rs:113-114`) could dispatch a `message` event *during* the module
   load, before the script has assigned `onmessage` — the message is silently
   lost. The `tokio::sync::mpsc` channel already is the disabled queue:
   nothing is lost while nobody `recv()`s. So: `script::run` first, pump
   second (§9 thread body). For a classic script "initial run" = `eval_with_options`
   returned; for a module worker = `JS_LoadModule` returned (synchronous
   evaluation done, top-level `await` may still be pending) — then the pump
   runs while the module's promise is awaited, which is exactly the spec's
   order and what lets a module's top level `await` a message.
2. **Ref/unref, Node semantics.** The maintainer's lifetime rule is Node's:
   the *worker* ends when its script is done and nothing keeps its loop alive.
   In Node the inbound port keeps the loop alive only while it has a
   `message` listener (`MessagePort` ref-on-listener). Because `idle()` counts
   every spawned future (§3), an unconditionally spawned pump would keep every
   worker — and therefore the parent process — alive forever, contradicting
   test I-18. So the pump is spawned by the first `addEventListener("message"|"messageerror")`
   on the global (the `onmessage` setter goes through the same path) and
   cancelled — child token in `WorkerScope.pump` — when the last such listener
   is removed; the receiver goes back into `WorkerScope.inbound` so a later
   `addEventListener` can resume it, and messages that arrived meanwhile are
   still in the channel. Timers (`den-stdlib-timer` also uses `ctx.spawn`,
   `lib.rs:28,57`) and in-flight `fetch`es keep the worker alive the same way
   they do today in the main context.

```rust
// called from the prelude's addEventListener/removeEventListener for the global
pub fn ref_pump<'js>(ctx: &Ctx<'js>) {
  let scope = ctx.userdata::<WorkerScope>().expect("worker context");
  let Some(mut inbound) = scope.inbound.borrow_mut().take() else { return };   // already running
  let pump = scope.closing.child_token();
  scope.pump.set(Some(pump.clone()));
  let ctx = ctx.clone();
  ctx.clone().spawn(async move {
    while let Some(Some(message)) = pump.run_until_cancelled(inbound.recv()).await {
      dispatch_message_event(&ctx, &message);   // deserialise; failure → "messageerror" (HTML §9.4.4 step 5)
    }
    if !pump.is_cancelled() { return }           // parent dropped the sender: worker is done
    ctx.userdata::<WorkerScope>().map(|scope| *scope.inbound.borrow_mut() = Some(inbound));
  });
}
pub fn unref_pump<'js>(ctx: &Ctx<'js>) {
  ctx.userdata::<WorkerScope>().and_then(|scope| scope.pump.take()).map(|pump| pump.cancel());
}
```

"This context is a worker" is therefore not a flag threaded through
`Engine::new`; it is the presence of `WorkerScope` in the context userdata.
`close()` and `postMessage` on the global look it up; `new Worker` does not
care (nesting allowed). The only ordering rule: `install` runs after
`Engine::new` has returned (so the stdlib's own `store_userdata` calls are all
done) and never while a `UserDataGuard` is alive (`userdata.rs:119`).

### 2.2 Why the classes are JS-land, not `#[rquickjs::class]`

`den-stdlib-wasm/src/error.rs:5-8` already records the constraint: *"rquickjs
classes cannot extend a JS builtin"*, and the reverse also holds — a
`#[rquickjs::class]` cannot be the base of a JS-land `class X extends Y` with
correct `super()` semantics without hand-written constructor plumbing. `Worker`,
`MessagePort`, `BroadcastChannel` and `DedicatedWorkerGlobalScope` all
*extend EventTarget* by spec. Define `EventTarget` / `Event` / `MessageEvent` /
`ErrorEvent` / `MessageChannel` / `MessagePort` / `BroadcastChannel` / `Worker`
in one JS prelude evaluated once in the `evaluate` hook (the `DEFINE_*`
pattern), and give `Worker` and the global scope a *private native handle*
(`#[rquickjs::class] WorkerHandle { inbound_tx, stop, closing, … }` with
`#[qjs(skip_trace)]` fields, exactly like `CancellationTokenWrapper` in
`den-stdlib-core/src/cancellation.rs:9-15`) for the three things JS cannot do:
spawn the thread, write bytes to a channel, cancel a token. The Rust surface
shrinks to: `spawn`, `post`, `terminate`, `serialize`, `deserialize`.

---

## 3. Process lifetime — `idle()` already does the right thing

Confirmed from source, no worker count needed:

1. The parent's per-worker **inbound pump** is `ctx.spawn`-ed
   (`ctx.rs:418-423`) in the `Worker` constructor. It awaits
   `outbound_rx.recv()` on a `tokio::sync::mpsc` receiver.
2. While that future is pending, `Schedular::is_empty()` is false
   (`schedular.rs:60-62`, the task sits in the all-list), so `poll` returns
   `Pending` (`:152-158`) and `idle()` returns `Poll::Pending` (`async.rs:350`).
   `App::run_until_end` (`src/app.rs:111-114`) therefore keeps waiting. **A live
   worker keeps the process alive.**
3. A message from the worker thread wakes the task through the atomic queue
   (`waker.rs:23-48`); the registered root waker (`schedular.rs:145`) wakes the
   tokio task running `idle()`, which re-polls the pump *under the runtime
   lock*, so the pump may call straight into JS (`dispatchEvent`).
4. When the worker ends (`close()`, script done with no pump of its own, or
   `terminate()`), the `Outbound` sender drops — it lives in the worker
   context's userdata, cleared at `raw.rs:128` before `JS_FreeRuntime` — so
   `recv()` yields `None`, the pump completes, the task is popped
   (`schedular.rs:185-190`), `is_empty()` flips, and `idle()` resolves. **The
   process ends when the last worker is gone.** `terminate()` makes this
   immediate by dropping the parent's receiver as well as cancelling the
   worker, so no message already in flight is dispatched afterwards (spec:
   terminate discards pending tasks).

The same mechanism, one level down, is what keeps the **worker thread** alive
after its script's top level finishes — *conditionally*: the worker's inbound
pump (§2.1 rule 2) is spawned into the worker runtime only while the global has
a `message`/`messageerror` listener, so the worker's own `idle()` stays
pending until `close()` cancels `closing`, the last listener is removed and no
timer/fetch is pending, or the parent drops the inbound sender. A worker whose
script posts once and registers nothing therefore exits on its own (I-18); a
worker with `onmessage` set lives until `close()`/`terminate()` (I-17), as in
Node.

Two constraints fall out of `idle()` holding the runtime lock while pending
(`async.rs:314-318`):

- Worker → parent communication must be **channel + spawned pump only**. Never
  call `parent_context.with(...)` from the worker thread: it would block on the
  lock `idle()` holds until something else wakes the schedular — a deadlock
  if the thing it is waiting for is that very call.
- In REPL mode `run_until_end` awaits the stop token *before* `idle()`
  (`app.rs:108-110`); between REPL lines the pumps are polled by the
  `drive()` task spawned at `app.rs:106`, which takes the lock per poll
  (`spawner.rs:85-133`), and during a line by `async_with`'s own spawner poll
  (`future.rs:113-114`). Nothing changes for the REPL.

---

## 4. Shutdown propagation and joining

**Tokens.** The worker engine's `stop_token` is
`parent_engine.stop_token.child_token()`, passed through
`WorkerHost::build_engine`. tokio-util cancels children when the parent is
cancelled, never the reverse (`tokio-util/src/sync/cancellation_token.rs:167-210`).
So Ctrl-C (`app.rs:118-126`) or `Engine::stop` cancels every worker's token,
the worker's interrupt handler (`engine.rs:227-232`, inherited verbatim by the
child engine) returns `true`, the interpreter throws the uncatchable
`interrupted` (`quickjs.c:8215-8219`), `PromiseFuture` bails
(`promise.rs:193-200`), the worker's `run_until_cancelled(idle())` returns,
and the thread unwinds. `terminate()` is the same path with the *worker's own*
token: `handle.stop.cancel()` — a child, so it does not touch the parent.

**Join.** Today `main` returns right after `run_until_end` (`src/main.rs:80-81`)
and the process exits — with a detached thread that may be mid-`eprintln!` or
mid-`JS_FreeRuntime`. The `JoinHandle`s live in a registry in the spawning
context's userdata:

```rust
#[derive(JsLifetime, Default)]
pub struct WorkerRegistry(pub Mutex<Vec<std::thread::JoinHandle<()>>>);

pub async fn join_all(context: &AsyncContext) {
  let handles = context.with(|ctx| ctx.userdata::<WorkerRegistry>()
    .map(|registry| std::mem::take(&mut *registry.0.lock().unwrap()))
    .unwrap_or_default()).await;
  for handle in handles {
    // A worker stuck in block_in_place (a file or URL load) cannot be interrupted
    // (tokio/src/task/blocking.rs:27); it finishes the load, sees the token, and exits.
    let _ = tokio::task::spawn_blocking(move || handle.join()).await;
  }
}
```

and `App::run_until_end` gains one line after the `run_until_cancelled(idle())`:

```rust
// src/app.rs  (AFTER)
pub async fn run_until_end(&mut self) {
  tokio::spawn(self.engine.runtime.drive());
  if self.wait_for_cancel_signal {
    self.engine.stop_token.child_token().cancelled().await;
  }
  self.engine.stop_token.run_until_cancelled(self.engine.runtime.idle()).await;
  self.engine.shutdown().await;   // cancels stop_token (no-op if already), joins worker threads
}
```

`Engine::shutdown` = `self.stop_token.cancel(); den_stdlib_worker::join_all(&self.context).await`.
The worker thread body calls the same `join_all` on *its* context before
returning, so nested workers are joined bottom-up. Ponytail: no join timeout;
add one (`tokio::time::timeout` around the `spawn_blocking`) only if a
hung loader ever shows up in practice.

**Why join, not detach.** Detaching is what quickjs-libc does
(`JS_THREAD_CREATE_DETACHED`, `quickjs-libc.c:4202`) and it is exactly why
its workers cannot be nested (`:4161-4164`, "to avoid problems with resource
liberation"). Joining costs one `Vec` and buys deterministic teardown, which
§10's lifetime tests depend on.

---

## 5. Error propagation

### 5.1 Uncaught exception in the worker

All of this lives in the worker crate's thread body; nothing reaches den-core
or `main.rs`.

1. The worker's script eval returns `Err(rquickjs::Error::Exception)`; the
   thread body does what `main.rs:52-66` does — `ctx.catch()` — but instead of
   printing, it builds an `ErrorEventInit`:
   - `message`: `Exception::message()` (`value/exception.rs:73`), or the
     coerced string for non-Error throwables (`main.rs:59`).
   - `filename` / `lineno` / `colno`: quickjs-ng keeps these only inside
     `stack` (`quickjs.c:7956`), one frame per line as
     `    at name (file:line:col)` (`:7878-7893`). Parse the first frame. (The
     alternative, an `Error.prepareStackTrace` hook returning CallSites with
     `getFileName()` etc., `quickjs.c:62109-62115`, is spec-faithful but a
     global side effect a user script can clobber; parsing is local.)
   - `error`: the value itself, *if* `JS_WriteObject2` can serialise it —
     `Error` instances cannot (`quickjs.c:38345-38347`), so `error` is
     `undefined` across the thread boundary unless a dedicated Error
     encoding is added. Say so in the doc comment; browsers send it.
2. Spec order (HTML §10.2.5 "runtime script errors"): the worker's own global
   `error` event fires first — `globalThis.dispatchEvent(new ErrorEvent(...))`
   in the worker; if a listener calls `preventDefault()` it ends there.
3. Otherwise `outbound.send(Outbound::Error(init))`. The parent pump
   constructs an `ErrorEvent` and dispatches it on the `Worker` object.
4. If the parent-side event is not `defaultPrevented`, the pump prints it —
   `eprintln!("{name}: {message}\n    at {filename}:{lineno}:{colno}")` — the
   same `eprintln!` the binary uses for the main script (`main.rs:56-63`,
   `app.rs:61-67`). No tracing, no stdout: `idle()`'s own `println!` to stdout
   (`async.rs:337`) is an rquickjs wart to avoid, not a precedent.

Errors thrown *inside* an `onmessage` handler are caught at the pump's
`dispatchEvent` call (`Function::call` returns `Err`) and take the same path
from step 2 — which is what makes "a throwing listener in the worker surfaces
as `error` on the parent" testable.

Three things the first draft left out:

- **Script load failure is a different event.** If the worker script cannot be
  resolved, read or parsed (`ENOENT`, HTTP 404, bad MIME, `SyntaxError`), HTML
  "run a worker" `onComplete` step 1 fires a **plain `Event` named `error`**
  (not an `ErrorEvent`, no `message`) at the `Worker` object and discards the
  environment (doc 08 §2.7). `Outbound::LoadFailed` → parent pump dispatches
  `new Event("error")`; nothing is printed (browsers do not either). The thread
  ends as if `close()` had been called.
- **`self.onerror` is not an ordinary handler slot.** `WorkerGlobalScope.onerror`
  is typed `OnErrorEventHandler`: for an `error` event it is invoked as
  `handler(message, filename, lineno, colno, error)` and a `true` return
  cancels the event (HTML §8.1.8.1 "special error event handler"); listeners
  added with `addEventListener("error", …)` receive the `ErrorEvent` as usual.
  The `Worker` object's `onerror` is an ordinary `EventHandler`.
- **`terminate()` noise on stdout.** When the interrupt lands inside a pending
  job (a microtask, a resolved-promise callback) the worker's `idle()` prints
  `error executing job: Error: interrupted` to **stdout** (`async.rs:335-341`;
  doc 09 verification log). There is no rquickjs API to silence it. Accept it
  for now; the alternative — never letting `idle()` run a job that can fail by
  wrapping every host→JS entry in a catch — is the upstream fix, not ours.

### 5.2 Unhandled rejections

Spec: an unhandled rejection fires `unhandledrejection` on the *worker global*;
it does not become `error` on the parent `Worker`. Implementation, worker
crate, installed in `scope::install`:

```rust
/// Context userdata: the promises rejected without a handler since the last
/// checkpoint, keyed by object identity so a late `handled=true` can retract.
#[derive(JsLifetime, Default)]
pub struct PendingRejections(pub RefCell<Vec<(Persistent<Promise<'static>>, Persistent<Value<'static>>)>>);

parts.runtime.set_host_promise_rejection_tracker(Some(Box::new(|ctx, promise, reason, is_handled| {
  // QuickJS calls with is_handled=false at rejection (quickjs.c:54371-54375) and
  // with true if a handler is attached afterwards (:55165-55169); the report has
  // to wait for the microtask checkpoint, as Node does.
  let Some(pending) = ctx.userdata::<PendingRejections>() else { return };
  let mut pending = pending.0.borrow_mut();
  match is_handled {
    false => pending.push((Persistent::save(&ctx, promise.into_promise().unwrap()), Persistent::save(&ctx, reason))),
    true => pending.retain(|(saved, _)| saved.as_raw_ptr() != promise.as_raw().u.ptr),
  }
}))).await;
```

plus a `ctx.spawn`-ed reporter that, after each `yield_now`, dispatches
`unhandledrejection` for every promise still pending in the set and
`eprintln!`s the ones nobody `preventDefault()`ed. The tracker must be
`Send + 'static` (`runtime.rs:44-45`) and `Persistent` is `!Send` (§0), so the
closure **captures nothing** and reaches the set through the `Ctx` it is
handed — context userdata is already behind the runtime lock, no `Mutex`.

Note the asymmetry this exposes: **the main context has no tracker at all**
(`engine.rs`, none set), so a worker would report unhandled rejections and the
main script would not. Install the same tracker in `Engine::new` in the same
change — it is the same code with "the global object" instead of "the worker
global", and tests #I-13/#I-14 in §10 will otherwise pass for the wrong reason.

---

## 6. The transpiler is shareable — but sharing is moot

`EasyOxcTranspiler` is a zero-sized struct; `Engine::new` makes a fresh
`Arc::new(EasyOxcTranspiler)` (`engine.rs:37`), and
`transpiler_is_shareable_across_threads` (`den-transpiler-oxc/src/lib.rs:377-400`)
asserts `Send + Sync + 'static` and transpiles from 8 threads through one
`Arc`. A worker may share the parent's `Arc` with no restriction — and gains
nothing by doing so, because the type carries no state (ARCHITECTURE §4:
*"oxc keeps no interner, comment store or thread-local globals"*). Let the
child `Engine::new` make its own; `EngineParts.transpiler` exists only so the
worker crate can transpile classic scripts (§7.3), not to share anything.

---

## 7. Base URL and script resolution

### 7.1 What `den main.js` does today

`run_file` (`engine.rs:325-332`) evaluates `` await import(`main.js`) `` as a
global script named `eval_script` (`ctx.rs:199`). `FileResolver::resolve`
(`file_resolver.rs:130-152`) gets `base = "eval_script"`, `name = "main.js"`:
`name` does not start with `.` → joined onto the search path `./` → `main.js`
relative to **cwd**. Inside `main.js`, `import "./x.js"` resolves with
`base = "main.js"` → `parent("main.js") = ""` → `x.js`. So a den module's
name *is* its cwd-relative path, and `import.meta`-style relative resolution
works because the base is the importer's name.

### 7.2 Module workers: reuse the chain, exactly

`new Worker(spec, { type: "module" })` captures the base in the parent with
`ctx.script_or_module_name(level)`. **The level is 0 for the JS frame that
called an rquickjs-defined function** (§0: class-`call` dispatch pushes no
frame; doc 09 probe T8). But the `Worker` constructor is a JS-land class in
the prelude (§2.2) that calls the native `spawn`, so level 0 is the *prelude's*
frame, level 1 the user's — and a `class MyWorker extends Worker` or
`Reflect.construct` adds frames on top (doc 08 §2.7 item 2 flags exactly this).
Robust rule: give the prelude a distinctive `EvalOptions.filename` (say
`"den:worker/prelude"`) and walk `level = 0, 1, 2, …` until the name is neither
that nor `None`; a name of `"eval_script"` (REPL input, `Engine::eval`,
`run_file`'s wrapper — all evaluate without a filename, `engine.rs:327-331`,
`:369-373`) or `None` falls back to `""`, which `FileResolver` joins onto its
search path `./` — the same base `den main.js` itself gets (§7.1).

`base` and `spec` travel in `WorkerBoot`. The worker thread then loads the
module **directly**, not through a string eval:

```rust
// inside context.with / async_with on the worker thread
let promise = unsafe {
  let base = CString::new(boot.base.as_str())?;
  let spec = CString::new(boot.spec.as_str())?;
  let raw = qjs::JS_LoadModule(ctx.as_ptr(), base.as_ptr(), spec.as_ptr());   // bindings:1729
  // `Value::from_js_value`/`Ctx::handle_exception` are pub(crate); the public pair is
  // `qjs::JS_IsException` + `Value::from_raw` (value.rs:438), as memory.rs:275 does.
  if qjs::JS_IsException(raw) { return Err(Error::Exception) }
  Value::from_raw(ctx.clone(), raw).into_promise().expect("JS_LoadModule returns a promise")
};
// ... enable the inbound pump gate here (§2.1 rule 1) ...
promise.into_future::<()>().await   // top-level await, if any
```

`JS_LoadModule(ctx, basename, filename)` feeds `(base, spec)` to the worker
engine's *own* resolver tuple (rooted at `./`, same cwd, same process), loads
through its loaders (so TypeScript/JSX transpile and `http(s)://` come for
free), instantiates, and runs the module body synchronously before returning
the evaluation promise (§0). That is why it beats `eval("await import(...)")`
here: `import()` merely enqueues `js_dynamic_import_job`, so "the initial run
is done" would not be observable from Rust, and the `filename`-on-the-wrapper
trick would be needed only to smuggle `base` in. rquickjs's `Module::import`
wraps the same call but computes its own base from stack level 1 — useless
from Rust with no frame on the stack — so call the binding directly, as
`den-stdlib-wasm/src/memory.rs:9` already calls `qjs::`. `JS_LoadModuleInternal` never throws for a bad
specifier: resolution, loading, parse and evaluation failures all **reject the
promise** (`JS_GetException` → `resolving_funcs[1]`), so every failure comes
back as `Err(Error::Exception)` from `into_future` (doc 09 fact 11) with the
reason in `ctx.catch()`. To tell the load-failure (§5.1, plain `error` `Event`)
from a runtime throw (`ErrorEvent`): rquickjs throws resolver/loader failures
as exceptions whose message starts with `Error resolving module '` /
`Error loading module '` (`result.rs:440,454`), and a parse failure is a
`SyntaxError` instance; match those three, everything else is an `ErrorEvent`.
Ponytail ceiling: a user module that itself throws a `SyntaxError` at top level
is misclassified as a load failure; acceptable.

### 7.3 Classic workers (the default) cannot use the loader

A classic script is not a module: `Module::declare` compiles with
`JS_EVAL_TYPE_MODULE` (`module.rs:266-267`), which would reject
`importScripts`-style top-level code and make `this !== self`. The worker
crate resolves and loads classic scripts itself:

```rust
/// Same join `FileResolver::resolve` performs for a `./`-relative name
/// (file_resolver.rs:143-149), so a classic worker and a module worker given the
/// same specifier name the same file.
pub fn resolve_relative(base: &str, spec: &str) -> RelativePathBuf {
  RelativePath::new(base).parent().map(|dir| dir.join_normalized(spec)).unwrap_or_else(|| spec.into())
}
```

then `tokio::fs::read` → `transpiler.transpile(src, infer_transpile_syntax_by_extension(ext), IsModule::Bool(false), false)`
→ `ctx.eval_with_options(src, options)` with `options = EvalOptions::default()`
mutated to `global = true`, `strict = false`, `promise = false`,
`filename = Some(resolved)` (`EvalOptions` is `#[non_exhaustive]`, §0 — no
struct literal). `promise: false` matters: a classic worker has no top-level
`await`, and with `JS_EVAL_FLAG_ASYNC` the body would run as an async function
and its synchronous throw would become a rejection instead of `Err(Exception)`.
Extension-less specifiers: `FileResolver` tries `{}.js`/`{}.mjs` (+ `.jsx`,
`.ts`, `.tsx` under the transpile features, `engine.rs:93-100`), so
`new Worker("./w", { type: "module" })` resolves while the classic
`resolve_relative` join would not; make `resolve_relative` try the same
pattern list or document that classic specifiers need an extension — pick the
former, it is one `find_map`.
`importScripts(...urls)` is the same three steps, synchronously, relative to
the *worker script's* resolved path (HTML §10.2.7 "import scripts into worker
global scope"), via `block_in_place` as the loaders already do
(`mmap_script.rs:92`). Ponytail ceiling: classic scripts are file-only; an
`http(s)://` classic worker throws `TypeError("classic workers must be files; use { type: \"module\" }")`,
upgrade path is `HttpLoader`'s fetch + MIME gate (ARCHITECTURE §3 item 3).

### 7.4 The absolute-path bug, and whether workers make it worse

`FileResolver::resolve` treats every non-`.`-prefixed name as search-path
relative (`file_resolver.rs:137-141`), so `/abs/w.js` becomes `./abs/w.js`
and fails — ARCHITECTURE §6. Workers inherit it identically on the module path
(same resolver) and, because `resolve_relative` uses `RelativePath` too,
identically on the classic path. **Not worse, and consistent** — which is the
point of sharing the join: if the classic path used `std::path::Path` it would
accept `/abs/w.js` while `{ type: "module" }` rejected it.

It does bite the test suite (§10.4): fixtures cannot live in `/tmp`. Fixing
the bug is a one-line `if Path::new(name).is_absolute() { return Ok(name.into()) }`
resolver in front of `FileResolver` and is worth doing in the same series, but
§10.4's fixture strategy does not depend on it.

---

## 8. Registration and feature flag

Mirror `den:wasm` everywhere it appears:

| Step | Where | What |
|---|---|---|
| resolver | `engine.rs:84-87` | `#[cfg(feature = "stdlib-worker")] resolver = resolver.with_module("den:worker");` |
| loader | `engine.rs:167-170` | `loader.with_module("den:worker", den_stdlib_worker::js_worker)` |
| evaluate | `engine.rs:286-292` | `Module::evaluate_def::<den_stdlib_worker::js_worker, _>(ctx.clone(), "den:worker")?;` then `ctx.store_userdata(HostSlot(...))` (§1.3) |
| den-core feature | `den-core/Cargo.toml:85-93` | `stdlib-worker = ["dep:den-stdlib-worker", "den-stdlib-worker?/transpile"]` — the `?/transpile` rides on den-core's own `transpile` feature the way `den-transpiler-oxc?/typescript` does at `:64-67`; add `"stdlib-worker"` to the `stdlib` umbrella at `:74-84` |
| den-core dep | `den-core/Cargo.toml:42-53` | `den-stdlib-worker = { version = "*", path = "../den-stdlib-worker", optional = true }` |
| root feature | `Cargo.toml:114-122` | `stdlib-worker = ["den-core/stdlib-worker"]` |
| workspace member | `Cargo.toml:3-19` | `"den-stdlib-worker"` |
| crate graph doc | `ARCHITECTURE.md` §1, §2 list of "the seven that are evaluated" → eight | |

The `evaluate` hook both exports (`e.export("Worker", …)`, `den-stdlib-core/src/lib.rs:67-69`
style) and installs globals (`ctx.globals().set("Worker", …)`, `den-stdlib-wasm/src/lib.rs:255`
style); the spec puts all of them on the global, and `import { Worker } from "den:worker"`
costs nothing extra. `structuredClone` is a plain function export + global.

---

## 9. The thread body, end to end

```rust
// den-stdlib-worker/src/thread.rs
pub struct WorkerBoot {
  pub spec: String,
  pub base: String,                       // ctx.script_or_module_name(1) in the parent
  pub kind: ScriptKind,                   // Classic | Module   (classic is the default)
  pub name: String,
  pub stop_token: CancellationToken,      // parent.stop_token.child_token()
  pub closing: CancellationToken,         // close()/terminate() cancel this
  pub inbound: mpsc::UnboundedReceiver<Inbound>,
  pub outbound: mpsc::UnboundedSender<Outbound>,
}

pub fn spawn(host: Arc<dyn WorkerHost>, boot: WorkerBoot) -> std::io::Result<std::thread::JoinHandle<()>> {
  std::thread::Builder::new().name(format!("den-worker:{}", boot.name)).spawn(move || {
    // ponytail: multi_thread with one worker because both den loaders call
    // block_in_place (mmap_script.rs:92), which panics on current_thread
    // (tokio worker.rs:434). So "one OS thread per worker" is one thread plus
    // one tokio worker thread; fold to current_thread the day the loaders
    // stop blocking.
    let tokio = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()
      .expect("a worker thread's tokio runtime");
    tokio.block_on(async move {
      let parts = host.build_engine(boot.stop_token.clone()).await;
      scope::install(&parts, &boot).await;                       // §2.1 — stores WorkerScope, no pump yet
      // §7.2 / §7.3. `run` does the synchronous part (classic eval / JS_LoadModule),
      // then opens the pump gate (§2.1 rule 1), then awaits the module promise.
      match script::run(&parts, &boot).await {
        Ok(()) => {}
        Err(ScriptError::Load(reason)) => { outbound.send(Outbound::LoadFailed); /* §5.1: plain error Event */ }
        Err(ScriptError::Uncaught(err)) => report::uncaught(&parts, &boot, err).await,   // §5.1
      }
      tokio::spawn(parts.runtime.drive());
      boot.stop_token.run_until_cancelled(parts.runtime.idle()).await;   // pending while a ref'd pump / timer lives (§3)
      registry::join_all(&parts.context).await;                  // nested workers, §4
      // `parts` drops here: userdata (and the outbound sender) is cleared before
      // JS_FreeRuntime (raw.rs:123-132), which is what ends the parent's pump.
    });
    tokio.shutdown_background();   // runtime.rs:489 — never wait on a stray blocking task
  })
}
```

The `Worker` constructor in the parent: build the two channels, `spawn`, push
the `JoinHandle` into `WorkerRegistry`, `ctx.spawn` the parent pump, hand the
native `WorkerHandle { inbound_tx, stop_token, closing }` to the JS-land
`Worker` instance. `postMessage` = serialise (`JS_WriteObject2`, then detach
the transferred `ArrayBuffer`s with `ArrayBuffer::detach`, `array_buffer.rs:259`
— detaching *after* a successful write is the spec order and leaves the
sender's buffers intact on a `DataCloneError`) → `inbound_tx.send`.
`terminate` = `stop_token.cancel(); closing.cancel(); drop(inbound_tx)` and
drop the parent pump's receiver.

Buffers the worker crate *creates* from received bytes (the deserialised side
of a transfer, `MessageEvent.data` for a raw `ArrayBuffer`) must be
`ArrayBuffer::new_copy`, never `ArrayBuffer::new(ctx, vec)`: a later `detach`
of a `new`-built buffer double-frees through rquickjs's free hook (doc 09 fact
12, probe-aborted). `JS_ReadObject2` allocates its own buffers, so this only
bites hand-built ones.

`Inbound`/`Outbound` are `enum { Message(Vec<u8>), Error(ErrorInit), LoadFailed, … }` —
bytes, not JS values: nothing with a `'js` lifetime crosses a thread.

### 9.5 Surface details the integration depends on but no other note pins

- **`postMessage` has two shapes everywhere** (`Worker`, the global,
  `MessagePort`): `postMessage(message, transfer: object[])` and
  `postMessage(message, { transfer })` (`StructuredSerializeOptions`, doc 08
  §2.6). `BroadcastChannel.postMessage(message)` takes no transfer list.
  Passing the same buffer twice in `transfer`, or a non-`ArrayBuffer`, is a
  `DataCloneError` before anything is serialised.
- **`MessageEvent` defaults** on every dispatch: `origin = ""`, `lastEventId = ""`,
  `source = null`, `ports = []` (frozen). `messageerror` carries `data = null`.
- **`messageerror` fires on both sides**: on the worker global when the parent's
  bytes cannot be read, and on the `Worker` object when the worker's bytes
  cannot be read (doc 08 §2.7 "Events on the Worker object"). With one
  quickjs-ng version on both ends this only happens via the test injection
  path (I-15), but the parent pump must handle `JS_ReadObject2` returning an
  exception rather than unwrap it.
- **`Worker` event handler slots**: `onmessage`, `onmessageerror`, `onerror` —
  ordinary `EventHandler`s on the `Worker` object (only the *global's*
  `onerror` is special, §5.1).
- **BroadcastChannel is cross-thread or it is nothing.** The spec fans out to
  every channel with the same name in the process, excluding the sender (doc 08
  §2.9); an in-context-only implementation would pass U-16 and be useless
  between a worker and its parent. Design, all inside the worker crate:
  `static CHANNELS: Mutex<HashMap<String, Vec<(u64, mpsc::UnboundedSender<Arc<[u8]>>)>>>`
  (process-wide, the storage-key partition is the process); `new BroadcastChannel(name)`
  registers a sender with a fresh id and keeps its receiver in the native
  handle; `postMessage` serialises once (`JS_WriteObject2`, no transfer) and
  sends the shared bytes to every other entry of that name in registration
  order; `close()` unregisters. Delivery on the receiving side is a
  `ctx.spawn`-ed pump per channel, governed by the same ref/unref rule as the
  worker global (§2.1 rule 2): spawned by the first `message`/`messageerror`
  listener, cancelled by the last removal or `close()`, so an open channel
  with a listener keeps its runtime — and the process — alive until `close()`,
  exactly Node's `BroadcastChannel` (`ref()` by default). `Arc<[u8]>` is
  deserialised once per destination, each gets its own copy.
- **`structuredClone(value, { transfer })`** is the same serialise → deserialise
  → detach sequence inside one context. Pre-walk rejects `Symbol` (quickjs-ng
  would happily serialise it, §0) and the transfer-list rules above apply.
- **`DOMException`** is the native quickjs-ng one (§0). Throw with
  `new DOMException(message, "DataCloneError")` from the prelude, or from Rust by
  calling that constructor from `ctx.globals()`; no `DEFINE_ERRORS`-style class
  building (doc 10 §6).
- **What a worker script gets from den**: everything `Engine::new` installs
  (`console`, timers, `fetch`, `den:*` modules, `WebAssembly`) — and, because
  `Engine::new` is the composition root, the same `HttpLoader`/`FileResolver`
  rooted at the same cwd. `location`, `navigator`, `onlanguagechange`,
  `onoffline`/`ononline` are not installed (doc 08 §2.8 lists them N/A).

---

## 10. Test strategy

Layering follows ARCHITECTURE §8: the crate proves *semantics*
against a bare `AsyncContext`; `den-core/tests/` proves *wiring* through the
real `Engine`. Everything cross-thread is awaited through a promise with a
tokio timeout as the failure bound — no sleeps.

### 10.1 Unit tests — `den-stdlib-worker` (no den-core)

Harness: the `den-stdlib-wasm` one (`den-stdlib-wasm/src/lib.rs:354-385`)
minus the wasm bits — fresh `AsyncRuntime` + `AsyncContext::full`,
`evaluate_def::<js_worker>`, eval the snippet as a global script with
top-level `await`, return a `FromJs` value. Tests return a failure list the
way `stdlib.rs:50-58` does, so one assertion names every broken property.

| # | Test | Expected |
|---|---|---|
| U-1 | `structured_clone_round_trips_every_supported_type` | number/string/boolean/null/undefined/BigInt/Date/RegExp/Map/Set/Array/plain object/typed arrays/ArrayBuffer come back `deepEqual` and not identical (`quickjs.c:38280-38350` is the list) |
| U-2 | `structured_clone_preserves_cycles_and_shared_references` | `a.self === a` after clone; two properties pointing at one object still point at one object (`JS_WRITE_OBJ_REFERENCE`) |
| U-3 | `structured_clone_transfers_an_array_buffer_and_detaches_the_source` | `structuredClone(buf, { transfer: [buf] })` → `buf.byteLength === 0` (detached), clone has the bytes |
| U-4 | `structured_clone_throws_data_clone_error_for_functions_symbols_and_ports` | `DOMException` named `DataCloneError` for `() => {}`, `Symbol()` (the pre-walk, §0 — quickjs-ng itself would serialise it), `Symbol.for("x")`, `new Error("e")`, and `new MessageChannel().port1` (`MessagePort` has no constructor) |
| U-4b | `structured_clone_accepts_both_transfer_signatures_and_rejects_duplicates` | `structuredClone(v, { transfer: [buf] })` works; `postMessage(v, [buf])` and `postMessage(v, { transfer: [buf] })` both detach; `[buf, buf]` → `DataCloneError` with `buf` still intact |
| U-4c | `global_onerror_is_the_five_argument_handler` | in a `BareHost` worker: `self.onerror = (m, f, l, c, e) => { record(arguments.length === 5); return true }` — the parent receives **no** `error` event (returning `true` cancels) |
| U-4d | `broadcast_channel_reaches_a_worker_on_another_thread` | `BareHost` worker opens `new BroadcastChannel("x")` with `onmessage`; parent posts on its own `"x"` channel → worker relays via `postMessage`; then `close()` on both and `join_all` completes |
| U-5 | `structured_clone_rejects_transfer_of_a_detached_buffer` | `DataCloneError` |
| U-6 | `event_target_dispatches_listeners_in_registration_order` | `["a","b","c"]` |
| U-7 | `event_target_once_listener_fires_exactly_once` | count `1` after two dispatches |
| U-8 | `event_target_remove_during_dispatch_takes_effect_for_later_listeners_only` | listener B removed by A is not called in the same dispatch (DOM §2.9 step 5 snapshot) |
| U-9 | `event_target_stop_immediate_propagation_skips_the_rest` | only the first listener runs |
| U-10 | `event_handler_slot_onmessage_is_a_single_replaceable_listener` | setting `onmessage` twice fires once; setting `null` removes; order relative to `addEventListener` follows HTML §8.1.8.1 (slot registered when first set) |
| U-11 | `message_channel_delivers_in_order_after_start` | `port2.start()` then three messages arrive as `[1,2,3]` |
| U-12 | `message_port_onmessage_setter_starts_the_port_implicitly` | no `start()` call needed when `onmessage` is assigned |
| U-13 | `message_port_queues_until_started` | `addEventListener` without `start()` → nothing delivered; after `start()` → all delivered |
| U-14 | `message_port_close_stops_delivery` | a message posted after `close()` never arrives |
| U-15 | `message_channel_messages_are_clones_not_references` | mutation after `postMessage` is not seen by the receiver |
| U-16 | `broadcast_channel_fans_out_to_every_other_channel_with_the_same_name` | three channels `"x"`, one posts → the other two receive, the sender does not |
| U-17 | `broadcast_channel_ignores_other_names_and_closed_channels` | |
| U-18 | `error_event_and_message_event_expose_their_init_fields` | `message/filename/lineno/colno/error`, `data/origin/ports` |
| U-19 | `a_bare_host_can_run_a_classic_worker_on_another_thread` | `BareHost` (§10.2) + a classic fixture that echoes → round trip resolves |
| U-20 | `a_bare_host_worker_survives_terminate_while_spinning` | `while(true){}` worker; `terminate()`; `join_all` completes within 5 s |

### 10.2 The fake host (the testing seam from §1.2)

```rust
struct BareHost;
impl WorkerHost for BareHost {
  fn build_engine(&self, stop_token: CancellationToken) -> Pin<Box<dyn Future<Output = EngineParts> + Send>> {
    Box::pin(async move {
      let runtime = AsyncRuntime::new().unwrap();
      runtime.set_interrupt_handler(Some(Box::new(move || stop_token.is_cancelled()))).await;
      let context = AsyncContext::full(&runtime).await.unwrap();
      context.with(|ctx| {
        Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker").unwrap();
        ctx.store_userdata(HostSlot(Arc::new(BareHost))).unwrap();   // nesting works here too
      }).await;
      EngineParts { runtime, context, #[cfg(feature = "transpile")] transpiler: Arc::new(EasyOxcTranspiler) }
    })
  }
}
```

No loaders, so `BareHost` runs *classic* workers only (the crate reads those
files itself, §7.3); module workers are a den-core integration concern. That
split is honest: the loader chain is den-core's, and the unit layer should
not rebuild it.

### 10.3 Integration tests — `den-core/tests/workers.rs`

`#![cfg(feature = "stdlib-worker")]`, every test
`#[tokio::test(flavor = "multi_thread")]` because the loaders use `block_in_place`.

```rust
const DEADLINE: Duration = Duration::from_secs(10);

/// Evaluate `src` in a fresh engine; the snippet resolves (or rejects) on the
/// message it is waiting for, and the timeout is the failure bound.
async fn eval_within<T>(src: &str) -> eyre::Result<T>
where T: for<'js> FromJs<'js> + Send + Sync + 'static
{
  let engine = Engine::new().await;
  let value = tokio::time::timeout(DEADLINE, engine.eval::<T>(src)).await??;
  engine.shutdown().await;   // joins worker threads; a hung join is a test failure, not a leak
  Ok(value)
}

/// The JS side of "await one message": resolves with `event.data`, rejects on `error`.
const FIRST_MESSAGE: &str = r#"
  const firstMessage = (target) => new Promise((resolve, reject) => {
    target.addEventListener("message", (event) => resolve(event.data), { once: true });
    target.addEventListener("error", (event) => { event.preventDefault(); reject(new Error(event.message)); }, { once: true });
  });
"#;
```

| # | Test | Fixture | Expected |
|---|---|---|---|
| I-1 | `classic_worker_echoes_a_message` | `self.onmessage = (e) => postMessage(e.data)` | `"ping"` back |
| I-2 | `classic_is_the_default_type_and_sees_self_as_this` | posts `[this === self, typeof importScripts]` | `[true, "function"]` |
| I-3 | `module_worker_runs_a_static_import` | `import { double } from "./lib.js"; postMessage(double(21))` with `{ type: "module" }` | `42` |
| I-4 | `module_worker_rejects_import_scripts` | `{ type: "module" }` worker calling `importScripts` | `ErrorEvent` with a `TypeError` message (HTML §10.2.7 step 1) |
| I-5 | `import_scripts_loads_relative_to_the_worker_script` | worker in `sub/w.js` doing `importScripts("./helper.js")` | helper's global visible |
| I-6 | `typescript_worker_is_transpiled_on_both_paths` (`cfg(feature = "typescript")`) | `w.ts` classic and module | both echo |
| I-7 | `worker_specifier_resolves_against_the_spawning_script_not_cwd` | main module in `dir/main.js` spawning `./w.js` | `dir/w.js` runs (the §7.2 base) |
| I-8 | `every_clone_type_survives_the_thread_boundary` | worker echoes | same checklist as U-1, now across runtimes |
| I-9 | `transferred_array_buffer_is_detached_here_and_intact_there` | `postMessage(buf, [buf])` | parent `buf.byteLength === 0`; worker posts back `new Uint8Array(e.data)[0]` |
| I-10 | `terminate_stops_a_spinning_worker` | `while(true){}` | `w.terminate()` returns; `engine.shutdown()` completes within `DEADLINE` |
| I-11 | `close_from_inside_the_worker_ends_it` | `close()` after posting `"bye"` | `"bye"` arrives; `runtime.idle()` then resolves within `DEADLINE` |
| I-12 | `uncaught_error_becomes_an_error_event_with_location` | `throw new TypeError("boom")` on line 3 | `message`, `filename` ends with the fixture name, `lineno === 3` |
| I-13 | `error_in_a_message_handler_reaches_the_parent` | `onmessage = () => { throw new Error("in handler") }` | `ErrorEvent` after the first `postMessage` |
| I-14 | `unhandled_rejection_fires_unhandledrejection_on_the_worker_global` | `Promise.reject(new Error("x")); self.onunhandledrejection = (e) => postMessage(e.reason.message)` | `"x"`; nothing on the parent's `error` |
| I-15 | `messageerror_fires_when_the_payload_cannot_be_deserialised` | parent posts bytes the worker cannot read (inject via the native handle from Rust) | `messageerror` on the worker global |
| I-16 | `nested_worker_round_trips_through_two_hops` | `w.js` spawns `inner.js`, relays | `"inner"` reaches the main context |
| I-17 | `a_live_worker_keeps_idle_pending_and_terminate_releases_it` | worker with `onmessage` set, nothing else | `timeout(200ms, runtime.idle())` is `Elapsed`; **drop that future first** (it holds the lock, §3); eval `w.terminate()`; `timeout(DEADLINE, runtime.idle())` is `Ok` |
| I-18 | `a_worker_that_finishes_without_listeners_does_not_keep_idle_pending` | script with no `onmessage`, posts once | `idle()` resolves within `DEADLINE` |
| I-19 | `workers_run_in_parallel_with_the_main_context_and_each_other` | worker A `while(true){}`; worker B echoes | B's echo arrives while A spins (a shared thread could never deliver it); then `A.terminate()` — no wall clock involved |
| I-20 | `stop_token_terminates_every_worker` | two spinning workers | `engine.stop()`; `engine.shutdown()` completes within `DEADLINE` |
| I-21 | `terminated_worker_delivers_nothing_afterwards` | worker posts in a `setInterval(…, 0)` loop | after `terminate()`, a 100 ms `setTimeout` sees no further `message` |
| I-22 | `worker_exports_are_importable_from_den_worker` | `import { Worker, MessageChannel } from "den:worker"` | `typeof` both `"function"`, `===` the globals |
| I-23 | `engines_that_spawned_and_terminated_workers_survive_repeated_teardown` | `lifetime.rs` style churn (`den-core/tests/lifetime.rs:18-36`), 25 rounds | no abort, no leak growth |
| I-24 | `message_posted_before_the_script_runs_is_delivered_to_the_handler_it_installs` | parent does `const w = new Worker(...); w.postMessage("early")` synchronously; worker script (module type, with a static import so the load takes a tick) sets `onmessage` at the end of its body and echoes | `"early"` arrives (§2.1 rule 1 — a pump enabled before the initial run loses it) |
| I-25 | `module_top_level_await_can_wait_for_a_message` | `{ type: "module" }` worker whose body is `const first = await new Promise(r => self.onmessage = e => r(e.data)); postMessage(first)` | round trip resolves (the pump runs while the module promise is pending) |
| I-26 | `missing_worker_script_fires_a_plain_error_event` | `new Worker("./does-not-exist.js")` | an `error` event whose `constructor === Event` (not `ErrorEvent`), `message` absent; `engine.shutdown()` completes within `DEADLINE` |
| I-27 | `removing_the_last_message_listener_lets_the_worker_exit` | worker sets `onmessage`, then on the first message sets `onmessage = null` | `idle()` resolves within `DEADLINE` after that message (§2.1 rule 2) |
| I-28 | `broadcast_channel_fans_out_from_main_to_two_workers` | two workers each with `new BroadcastChannel("x").onmessage = e => postMessage(e.data)`; main posts once | both echo; main's own channel does **not** receive; after `close()` everywhere `idle()` resolves |
| I-29 | `worker_specifier_without_extension_resolves_on_both_paths` | `new Worker("./w")` classic and `{ type: "module" }`, file `w.js` | both echo (§7.3 pattern list) |

### 10.4 Fixtures on disk

`Worker` takes a URL, so integration tests need files. Constraints: `tempfile`
is not a direct dependency (only wasmtime's, `cargo tree -i tempfile`), and
absolute paths do not resolve (§7.4), so `std::env::temp_dir()` is out until
the resolver bug is fixed. `cargo nextest run` runs with cwd = `den-core/`
and `/target` is gitignored (`.gitignore:1`). So:

```rust
/// Write `files` under target/ and return the cwd-relative directory den can
/// resolve (`../target/…` starts with `.`, so FileResolver joins it onto the
/// base's parent — file_resolver.rs:143-149). One directory per test name so
/// parallel tests never share a file.
fn fixture(test: &str, files: &[(&str, &str)]) -> String {
  let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/den-worker-fixtures").join(test);
  std::fs::create_dir_all(&dir).unwrap();
  for (name, body) in files {
    std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
    std::fs::write(dir.join(name), body).unwrap();
  }
  format!("../target/den-worker-fixtures/{test}")
}
```

Nothing is checked in; the fixtures are regenerated on every run from string
constants next to the test that uses them, the way `webassembly.rs` assembles
WAT at test time rather than committing `.wasm`. Add `fixture` to the same
file; it is not worth a `tests/common/`. If `CARGO_TARGET_DIR` points
elsewhere the directory is still created relative to the manifest, which is
all the resolver cares about.

The unit layer (§10.1, U-19/U-20) needs the same helper with
`den-stdlib-worker`'s manifest dir; two copies of an eight-line function beat
a shared dev-crate.

### 10.5 Determinism rules, restated

- Every cross-thread wait is `await`-ing a promise settled by an event, under
  `tokio::time::timeout(DEADLINE, …)`. A test that passes by sleeping is a test
  that fails on a loaded CI runner.
- Parallelism is proven structurally (I-19: a spinning worker cannot starve a
  second one unless they share a thread), never by comparing wall-clock
  durations.
- Lifetime is proven by `idle()` resolving or not within a bound (I-17/I-18),
  and by `shutdown()` joining within a bound (I-10/I-20). Both are binary.
- One engine per test (`webassembly.rs:57-73` rationale: shared context
  userdata); `shutdown()` at the end so a leaked thread fails the test that
  leaked it rather than the one after.

### 10.6 CI

`.github/workflows/lint.yml:44-55` already runs the workspace tests on both
wasm backends with `--features stdlib,typescript,react,<backend>`; once
`stdlib-worker` joins the `stdlib` umbrella (§8) the new suite runs there
with no workflow change. Update the counts in ARCHITECTURE §8 when the
numbers settle.

---

## 11. What this note deliberately leaves to other notes

- The structured clone *algorithm* itself (which types, which errors, the
  `DataCloneError` surface) — here only the transport choice is fixed:
  `JS_WriteObject2`/`JS_ReadObject2` bytes across the channel, with the
  supported-type list at `quickjs.c:38280-38350` as the hard ceiling.
- `EventTarget` internals (listener list snapshotting, `passive`, `signal`).
- Cross-thread **transfer of `MessagePort`s**. The scope fixes transferable
  `ArrayBuffer`s only; a port posted to a worker is a `DataCloneError` in this
  design. Lifting it means a port-id routing table in the host; leave it.
- `SharedArrayBuffer` across workers: `JS_WRITE_OBJ_SAB` exists
  (`quickjs.h:1216`) and ARCHITECTURE §6 explains why den refuses shared
  memory today. Not in scope.

---

## Verification log

Second pass, 2026-08-22, against the same pinned sources. Every claim below
was re-read at the cited line; "confirmed" means the text matched, "corrected"
means the note above was changed.

| # | Claim | Result | Evidence |
|---|---|---|---|
| 1 | `block_in_place` panics on `current_thread`, allowed inside a multi-thread `Runtime::block_on` on the non-worker thread, and hands the core off inside a worker thread (so `worker_threads(1)` is enough) | **confirmed** | `tokio/src/runtime/scheduler/multi_thread/worker.rs:408-445` (the `(Entered{allow_block_in_place}, false)` arm), `:450-492` (core hand-off to `spawn_blocking`), `multi_thread/mod.rs:91` and `handle.rs:373` enter with `allow_block_in_place = true`, `current_thread/mod.rs:206` with `false`; `exit_runtime(f)` at `:503-510` is what lets the nested `Handle::block_on` enter again |
| 2 | `Builder::worker_threads(0)` asserts; `Runtime::shutdown_background` exists | **confirmed** | `builder.rs:517-521`, `runtime.rs:489` |
| 3 | `idle()` is `Ready` only on `SchedularPoll::Empty` and holds the runtime `MutexGuard` across `Pending` | **confirmed** | `runtime/async.rs:313-360` (`lock` moved into the `ManualPoll` closure), `schedular.rs:60-62`, `:140-158` |
| 4 | `async_with` polls the spawner and runs pending jobs between polls of the root future | **confirmed** | `context/async/future.rs:109-144` |
| 5 | `Ctx::spawn` needs only `F: Future<Output = ()> + 'js` | **confirmed** | `context/ctx.rs:418-423` |
| 6 | `InterruptHandler` / `RejectionTracker` are `Send + 'static` under `parallel`; setters are `async fn` on `AsyncRuntime` | **confirmed** | `runtime.rs:44-52`, `runtime/async.rs:163,185` |
| 7 | Interrupt → uncatchable `InternalError: interrupted`; `PromiseFuture` bails on an uncatchable pending exception | **confirmed** | `quickjs.c:8215-8219` (`JS_ThrowInterrupted` + `JS_SetUncatchableError`), `:8221-8238` (counter), `value/promise.rs:193-200` |
| 8 | Runtime drop clears userdata before `JS_FreeRuntime`; `store_userdata` fails while a guard is alive | **confirmed** | `runtime/raw.rs:123-131`, `runtime/userdata.rs:114-121` |
| 9 | `JS_WriteObject2` supports Map/Set and rejects `Error` | **confirmed** — `JS_CLASS_MAP`/`JS_CLASS_SET` arms at `quickjs.c:38334-38339`, no `JS_CLASS_ERROR` arm, default → `unsupported object class` (`:38344-38347`) | |
| 10 | "everything else is `unsupported object class`" | **corrected** — `Symbol` *is* serialised (`BC_TAG_SYMBOL`, `:38362-38371`); the spec's `DataCloneError` for symbols must be a pre-walk. Signature corrected to the 0.15.1 `JSSABTab *` form (`quickjs.h:1221-1222`, bindings `:1671,1691,1708`) | |
| 11 | `script_or_module_name(1)` is the caller from a native constructor | **corrected** — level **0** for rquickjs functions: they are class objects dispatched through the class `call` hook (`runtime/opaque.rs:123`, `quickjs.c:17617-17625`) with no `JSStackFrame`; `js_call_c_function` (`:17359-17379`) pushes one only for `JS_NewCFunction` natives, which is why quickjs-libc and `Module::import` use 1. Matches doc 09 probe T8 and doc 08 §2.7's "level is not stable" warning. §7.2 now walks levels past the prelude's own filename | |
| 12 | `EvalOptions { …, ..Default::default() }` | **corrected** — `#[non_exhaustive]` (`context/ctx.rs:28`); den mutates a `default()` (`engine.rs:327-331`) | |
| 13 | Module worker = `eval("await import(spec)")` with `filename = base` | **corrected** to raw `qjs::JS_LoadModule(ctx, base, spec)` (`quickjs.h:1247`, bound at `bindings:1729`): it resolves against an explicit base, runs the synchronous part of evaluation before returning, and rejects (never throws) on resolve/load/parse failure (`JS_LoadModuleInternal`: `JS_GetException` → `resolving_funcs[1]`). `Value::from_js_value` and `Ctx::handle_exception` are `pub(crate)` (`value.rs:128`, `result.rs:724`); the public pair is `JS_IsException` + `Value::from_raw` (`value.rs:438`) | |
| 14 | Inbound pump spawned in `scope::install`, before the script runs | **corrected** — HTML "run a worker" enables the inside port *after* the initial script run; with `async_with` polling the spawner during the load a message could be dispatched before `onmessage` exists. Pump is now enabled after the synchronous run (§2.1 rule 1, I-24/I-25) | |
| 15 | Worker thread stays alive while its pump lives; I-18 expects a listener-less worker to exit | **corrected** — contradiction resolved with Node's ref-on-listener rule (§2.1 rule 2, §3, I-27); `den-stdlib-timer` keeps a context alive through `ctx.spawn` the same way (`den-stdlib-timer/src/lib.rs:28,57`) | |
| 16 | Rejection tracker stores `Persistent` in a `Mutex` captured by the closure | **corrected** — `Persistent` is `!Send` (`persistent.rs:88-100`, doc 09 fact 2), so a capturing closure is not `Send`; the set now lives in context userdata reached through the handed-in `Ctx` (§5.2) | |
| 17 | `WorkerHost::build_engine` returns a `Send` future | **corrected** — bound dropped; awaited only via `Runtime::block_on` on its own thread (§1.3) | |
| 18 | `DataCloneError` needs a `DOMException` class | **corrected** — native in quickjs-ng, installed by `JS_NewContext` → `JS_AddIntrinsicAToB` → `JS_AddIntrinsicDOMException` (`quickjs.c:63338-63343`, `:62329-62356`), reached by `AsyncContext::full` (`context/async.rs:161-163`); so `BareHost` has it too | |
| 19 | `ArrayBuffer::detach` is safe on transferred buffers | **qualified** — safe for JS-created buffers; UB for `ArrayBuffer::new`-built ones (doc 09 fact 12); §9 now mandates `new_copy` for crate-built buffers | |
| 20 | doc 09 "does not exist" | **corrected** — `docs/research/09-rquickjs-threads-and-event-loop.md`, `08-…`, `10-…` all exist; cross-referenced | |
| 21 | den file:line anchors (`engine.rs:25-31` struct, `:35` `new`, `:225-232` token + interrupt, `:308-339` `run_file`, `:350-380` `eval`, `:382-388` `stop`; `app.rs:99-115`, `:118-126`; `main.rs:52-66`; `mmap_script.rs:92`, `http.rs:116`; `den-core/Cargo.toml` features `:59-95`; root `Cargo.toml` members `:3-19`, features `:114-130`; `lint.yml:44-55`; `den-stdlib-wasm/src/lib.rs:206-257`, `:354-385`; `error.rs:5-8`; `memory.rs:9`; `den-stdlib-core/src/lib.rs:65-75`, `cancellation.rs:9-15`; `den-transpiler-oxc/src/lib.rs:375-400`; `tests/lifetime.rs:18-36`, `webassembly.rs:57-73`, `stdlib.rs:50-58`) | **confirmed** (struct starts at `:25`, not `:24`) | |
| 22 | `FileResolver::resolve` join rule; no `DOMException`/`EventTarget`/`structuredClone` anywhere in den today | **confirmed** | `loader/file_resolver.rs:130-152`; `grep -rn` over `den-*`/`src` empty |
| 23 | `tokio-util` in the lock is 0.7.19; `child_token`/`cancel`/`run_until_cancelled` exist | **confirmed** | `Cargo.lock`, `tokio-util-0.7.19/src/sync/cancellation_token.rs:204,220,300` |

Added, not previously covered: script **load failure** → plain `Event` (§5.1,
I-26); `WorkerGlobalScope.onerror` as the five-argument special handler (§2.1,
§5.1, U-4c); `terminate()` stdout noise from `idle()` (§5.1);
`postMessage` overloads, `MessageEvent` defaults, two-sided `messageerror`,
`Worker` handler slots (§9.5); **cross-thread `BroadcastChannel`** design and
liveness rule (§9.5, U-4d, I-28); extension-less classic specifiers (§7.3,
I-29); `promise: false` for classic evaluation (§7.3).

Not verified (no local copy): the HTML spec step ordering is quoted from doc
08's verified reading and from memory of "run a worker" (inside port enabled
after the script runs) and §8.1.8.1 (`OnErrorEventHandler`); both are stable
spec text but were not re-fetched here.
