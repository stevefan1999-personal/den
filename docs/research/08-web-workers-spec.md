# 08 — Web Workers API: normative surface & den conformance checklist

Status: research note. Written 2026-08-22 against the spec snapshots and crate sources listed in §0.
Audience: whoever implements dedicated workers, channel messaging, `BroadcastChannel` and
`structuredClone()` for den on rquickjs 0.12.2 / quickjs-ng (rquickjs-sys 0.12.2) / tokio 1.53.

Decisions already made by the maintainer, and **not** re-opened here: dedicated `Worker` +
`DedicatedWorkerGlobalScope` only (no `SharedWorker`, no `ServiceWorker`); one OS thread per worker,
each with its own tokio runtime and its own QuickJS runtime/heap; `terminate()` must stop a running
script; default script type `"classic"`, `{ type: "module" }` fully supported; a live worker keeps
the process alive and the process ends when every worker has closed or been terminated.

## 0. Sources actually read

Normative (fetched 2026-08-22, converted to text in the session scratchpad and read in full for the
sections cited; `§` numbers below are the spec's own):

| Spec | URL | Snapshot |
|---|---|---|
| HTML — Web workers | <https://html.spec.whatwg.org/multipage/workers.html> | Living Standard, 21 August 2026 |
| HTML — Cross-document / channel messaging / broadcast | <https://html.spec.whatwg.org/multipage/web-messaging.html> | same |
| HTML — Safe passing of structured data | <https://html.spec.whatwg.org/multipage/structured-data.html> | same |
| HTML — The `MessageEvent` interface | <https://html.spec.whatwg.org/multipage/comms.html> | same |
| HTML — Scripting: runtime errors, unhandled rejections, event handlers | <https://html.spec.whatwg.org/multipage/webappapis.html> (§8.1.4.5–8.1.4.7, §8.1.8.1) | same |
| DOM — Events | <https://dom.spec.whatwg.org/> (§2.2, §2.5, §2.7, §2.9, §2.10) | Living Standard, 20 August 2026 |

Local crate sources (the source of truth for every library-API claim):

- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/src/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-macro-0.12.2/src/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-sys-0.12.2/quickjs/` (`quickjs.c`, `quickjs.h`, `quickjs-libc.c`) and `rquickjs-sys-0.12.2/src/bindings/*.rs`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.53.1/src/`

den sources read: `den-core/src/engine.rs`, `src/app.rs`, `src/main.rs`,
`den-core/src/loader/{http,mmap_script}.rs`, `den-stdlib-timer/src/lib.rs`,
`den-stdlib-core/src/{lib,cancellation}.rs`, `den-stdlib-whatwg-fetch/src/lib.rs`,
`den-stdlib-wasm/src/error.rs`, `den-core/tests/stdlib.rs`, `ARCHITECTURE.md`, and
`docs/research/05-webassembly-js-api-spec.md` (format model for this note).

Every claim below carries either a spec section or a `file:line` that was read.

---

## 1. Ground truth from the local sources

This section is the part an implementer cannot get from the specs: what the engine and the
bindings already provide, and where they diverge from what the specs demand.

### 1.1 QuickJS already has a structured-clone-shaped serializer — but it is not structured clone

quickjs-ng ships `JS_WriteObject2` / `JS_ReadObject2` (`quickjs.h:1220-1235`,
`quickjs.c:38431-38488` and `:39729-39768`). With flags `JS_WRITE_OBJ_SAB | JS_WRITE_OBJ_REFERENCE`
(`quickjs.h:1216-1217`) it is exactly what quickjs-libc's own `os.Worker` uses for `postMessage`
(`quickjs-libc.c:4241-4243`) and `JS_ReadObject(... JS_READ_OBJ_SAB | JS_READ_OBJ_REFERENCE)` on the
receiving side (`quickjs-libc.c:2698-2699`). The blob is runtime-independent: atoms are written as
strings (`JS_WriteObjectAtoms`, `quickjs.c:38385-38425`; `first_atom = 1` when bytecode is not
allowed, `:38447-38450`), so a blob produced in one `JSRuntime` can be read in another.

What it supports, by tag (`quickjs.c:37631-37653`, dispatch at `:38218-38380`):

| JS value | Writer behaviour | Spec (§2.7.3) says | Verdict |
|---|---|---|---|
| `undefined`, `null`, booleans, int32, float64, strings (incl. ropes) | `:38228-38268` | primitive record | OK |
| BigInt (`JS_TAG_SHORT_BIG_INT` / `JS_TAG_BIG_INT`) | `:38355-38358` | primitive record | OK |
| Symbol | **serialized** (`BC_TAG_SYMBOL`, `:38359-38372`), re-created on read `:39627-39638` | **throw `DataCloneError`** | WRONG — must pre-reject |
| Boolean/Number/String/BigInt wrapper objects | `BC_TAG_OBJECT_VALUE`, `:38326-38333`; read via `JS_ToObject` `:39492-39510` | typed wrapper records | OK |
| `Date` | `BC_TAG_DATE` + `object_data` `:38323-38325`; read `:39465-39490` | `[[DateValue]]` | OK |
| `RegExp` | pattern + compiled bytecode `:38320-38322`, `:38207-38215` | source + flags | OK (engine-private bytecode, same binary both sides) |
| `ArrayBuffer` (incl. resizable: `max_byte_length` written) | `:38175-38188`; **detached → `TypeError`** via `JS_ThrowTypeErrorDetachedArrayBuffer` `:38179` | detached → `DataCloneError`; copy bytes | WRONG error type |
| `SharedArrayBuffer` | only with `allow_sab`; writes the raw data pointer into `sab_tab` `:38190-38205` | share the same data block | OK mechanically, but see §1.2 |
| TypedArrays (`Uint8ClampedArray`…`Float64Array`) | `BC_TAG_TYPED_ARRAY` with class index, count, offset, then the buffer `:38160-38171`; read `:39306-39352` | `ArrayBufferView` record | OK — **buffer identity preserved** because the buffer goes through the reference table |
| `DataView` | **not** in the `is_typed_array` class range (`JS_CLASS_UINT8C_ARRAY..JS_CLASS_FLOAT64_ARRAY` is `quickjs.c:151-162`, `JS_CLASS_DATAVIEW` is `:163`, `JS_TYPED_ARRAY_COUNT` `:201`, `is_typed_array` `:58395`) → falls to `default:` → `TypeError "unsupported object class"` `:38346-38349` | `ArrayBufferView` with `[[Constructor]] = "DataView"` | MISSING |
| `Map` / `Set` | `:38334-38340`, entries in insertion order `:52803-52822` | `[[MapData]]` / `[[SetData]]` | OK |
| Array | `BC_TAG_ARRAY`, writes `length` then `JS_GetPropertyUint32(i)` for every `i < length` `:38080-38110` | **holes are skipped** (only `EnumerableOwnProperties`, §2.7.3 deep step 4); getters on indices are invoked | WRONG: holes come back as `undefined` own properties (`JS_ReadArray` defines every index, `:39260-39290`) |
| Plain object (`JS_CLASS_OBJECT`) | own **enumerable** props only `:38123-38157`; **symbol keys are emitted** (the shape walk filters on `JS_PROP_ENUMERABLE` only; `JS_WriteObjectAtoms` records `atom_type` `:38405-38409` and `JS_ReadObjectAtoms` re-creates a *fresh* symbol in the target runtime); **accessor props → `TypeError "only value properties are supported"`** `:38140-38143` | own enumerable **String**-keyed props, `[[Get]]` invoked (getters run) | WRONG on accessors; WRONG on symbol keys (must be dropped, they are copied as new symbols) — the pre-pass must strip them |
| `Error` objects (`JS_CLASS_ERROR`, `quickjs.c:132`) | `default:` → `TypeError "unsupported object class"` | `{name, message, stack}` record with prototype chosen by `name` | MISSING |
| Functions, `Proxy` (`:180`), `Promise` (`:181`), `WeakMap` (`:167`), `WeakSet`, `WeakRef` (`:191`), every rquickjs class instance | `default:` → `TypeError "unsupported object class"` | `DataCloneError` | WRONG error type |
| Cycles / shared references | `BC_TAG_OBJECT_REFERENCE` when `allow_reference` `:38283-38292`; read `:39606-39620` | memory map preserves identity | OK |
| Prototype of class instances | only `JS_CLASS_OBJECT` is written; the read side creates a plain object | prototype dropped | OK |

So the engine serializer gives ~80% of the type table for free, but **every one of its divergences
is on the spec's observable surface**: wrong exception class (`TypeError` vs `"DataCloneError"
DOMException`), `Symbol` accepted instead of rejected, `Error`/`DataView` rejected instead of
cloned, accessor properties rejected instead of read, holes filled in. §3.4 gives the build
strategy (pre-validation walk now, own serializer later).

### 1.2 `SharedArrayBuffer` cross-runtime sharing needs allocator hooks rquickjs does not install

`JS_ReadObjectRec` refuses `BC_TAG_SHARED_ARRAY_BUFFER` unless `allow_sab` **and**
`ctx->rt->sab_funcs.sab_dup` is set (`quickjs.c:39589-39592`). rquickjs never calls
`JS_SetSharedArrayBufferFunctions` (grep over `rquickjs-core-0.12.2/src` → 0 hits; the binding
exists at `rquickjs-sys-0.12.2/src/bindings/aarch64-apple-darwin.rs:1472`, struct at `:1441`,
same symbol set in every target file). Without the hooks a SAB is allocated with plain
`js_mallocz` (`quickjs.c:57775-57779`) and owned by one runtime, so:

- `structuredClone(new SharedArrayBuffer(8))` today throws (read side), and
- sharing a SAB between two runtimes requires a refcounted allocator exactly like quickjs-libc's
  `js_sab_alloc` / `js_sab_free` / `js_sab_dup` (`quickjs-libc.c:3945-3972`, header with atomic
  `ref_count` `:3931-3934`), installed on **every** runtime that may touch the block, plus the
  `JSSABTab` returned by `JS_WriteObject2` (`quickjs.c:38468-38474`) being `sab_dup`'d by the
  sender (`quickjs-libc.c:4272-4274`) and `sab_free`'d when the message is dropped (`:3997-4007`).

This is the only piece of the clone story that needs an `unsafe extern "C"` allocator trio on the
Rust side. It is P3 (§4); the maintainer's required scope says "transferable ArrayBuffers", not
SABs.

### 1.3 `DOMException` already exists in den's context

`AsyncContext::full` calls `JS_NewContext` (`rquickjs-core-0.12.2/src/context/async.rs:161-163`),
which calls `JS_AddIntrinsicAToB` (`quickjs.c:2544`), which registers the `DOMException` class and
global when not yet registered (`quickjs.c:63339-63342`, `JS_AddIntrinsicDOMException` at
`:62329`). The name table includes `"DataCloneError"` (code 25), `"InvalidStateError"` (11),
`"SyntaxError"` (12), `"NotSupportedError"` (9), `"AbortError"` (20) (`quickjs.c:62150-62176`),
`code` is derived from the name (`:62258-62283`), `@@toStringTag` is `"DOMException"` (`:62290`),
and the constructor requires `new` (`:62236-62242`).

Throwing one from Rust: `qjs::JS_ThrowDOMException(ctx, name, fmt, ...)` is variadic
(`quickjs.h:842`; binding `bindings/x86_64-unknown-linux-gnu.rs:886-891`). rquickjs has no safe
wrapper (the six helpers at `rquickjs-core-0.12.2/src/value/exception.rs:105-191` are
`Error`/`SyntaxError`/`TypeError`/`ReferenceError`/`RangeError`/`InternalError` only). Two routes:

```rust
// Route A — the C helper. `%s` keeps the formatter out of the message.
pub fn throw_dom_exception(ctx: &Ctx<'_>, name: &CStr, message: &str) -> rquickjs::Error {
  let message = CString::new(message).unwrap_or_default();
  unsafe { qjs::JS_ThrowDOMException(ctx.as_raw().as_ptr(), name.as_ptr(), c"%s".as_ptr(), message.as_ptr()) };
  rquickjs::Error::Exception            // the exception is now pending in the context
}

// Route B — pure rquickjs: `new DOMException(message, name)` via the global constructor.
let ctor: Constructor = ctx.globals().get("DOMException")?;
Err(ctx.throw(ctor.construct::<_, Value>((message, "DataCloneError"))?))
```

Route B is the one that survives `globalThis.DOMException` being replaced only if the constructor is
captured at module-evaluate time and parked in context userdata — the same trick
`den-stdlib-wasm/src/error.rs:80-104` uses for its three error constructors. Prefer B with the
cached constructor; it needs no `unsafe`.

### 1.4 Errors in quickjs-ng carry `stack` but no `lineNumber`/`columnNumber`

`lineNumber`/`columnNumber` exist only on **function** objects (`quickjs.c:41842-41844`); error
objects get a `stack` string from `build_backtrace` and optionally `cause`
(`quickjs.c:41942-41949`). rquickjs exposes `Exception::message()` and `Exception::stack()`
(`rquickjs-core-0.12.2/src/value/exception.rs:73,83`). So `ErrorEvent.lineno`/`colno`/`filename`
must be parsed out of the first `    at …(file:line:col)` frame of `stack`, or left at `0` / `""`.
The spec allows this: they are "implementation-defined values derived from exception"
(HTML §8.1.4.6 "extract error information", step 3).

### 1.5 rquickjs facts the design depends on

| Need | rquickjs 0.12.2 fact | Where |
|---|---|---|
| A runtime per thread, handles movable | `AsyncRuntime: Send + Sync` under `parallel` (den enables it) | `runtime/async.rs:85-97`, `InnerRuntime: Send` `:51` |
| Stop a running script | `InterruptHandler = Box<dyn FnMut() -> bool + Send + 'static>` under `parallel`; returning `true` raises an uncatchable exception | `runtime.rs:52`, `runtime/base.rs:89-93`; den already wires it to a `CancellationToken` at `den-core/src/engine.rs:227-232` |
| Unhandled rejections | `RejectionTracker = Box<dyn for<'a> Fn(Ctx<'a>, Value<'a>, Value<'a>, bool) + Send + 'static>` (promise, reason, is_handled) | `runtime.rs:44`, `runtime/base.rs:77-83` |
| Run a Rust future on the JS thread with `Ctx` access | `Ctx::spawn<F: Future<Output=()> + 'js>` — no `Send` bound, the future lives in the runtime's spawner | `context/ctx.rs:418-423`; used by den timers at `den-stdlib-timer/src/lib.rs:28-37` |
| Keep the loop alive until futures finish | `AsyncRuntime::idle()` polls jobs + spawned futures until `SchedularPoll::Empty`; `drive()` is the detached variant | `runtime/async.rs:313-360`, `:365`; `runtime/spawner.rs:59-78` |
| Where uncaught job errors go today | `idle()` **prints** `"error executing job: …"` to stdout and continues — not interceptable | `runtime/async.rs:329-345` |
| `idle()` holds the runtime mutex for its whole life | `let mut lock = self.inner.lock().await;` is taken once and kept while the spawner is pending. Any `context.with()` from *another* tokio task on the same runtime blocks until `idle()` returns, so every cross-thread input must enter through a channel awaited by a `ctx.spawn`ed future; only lock-free signals (`CancellationToken::cancel`, `Engine::stop` at `den-core/src/engine.rs:382`) may be used from outside | `runtime/async.rs:314-360` |
| Run the microtask checkpoint from inside a spawned future | `Ctx::execute_pending_job() -> bool` (one job; `true` while jobs remain; an exception is left pending → `ctx.catch()`). `idle()` only drains jobs *between* spawner polls, so a task that must observe "after the microtask checkpoint" (§2.11 rejections) drains this itself | `context/ctx.rs:404-409`, `:257-262` |
| Realm-scoped state | `Ctx::store_userdata` / `userdata` / `remove_userdata` require `U: JsLifetime<'js>` (derive `rquickjs::JsLifetime`, as `den-stdlib-core/src/cancellation.rs:6-15` does); userdata is refcount-held, not GC-traced | `context/ctx.rs:480-508` |
| Hold a JS value across `with` scopes | `Persistent::save` / `restore` (same runtime only, `Error::UnrelatedRuntime`) | `persistent.rs:88,102` |
| Build a JS function from a Rust closure | `Function::new(ctx, f)` with `f: Fn(..) -> R + 'js` | `value/function.rs:47-58`, `function/into_func.rs:15-33` |
| Classes | `JsClass::prototype` returns a **fresh** `Object` by default; `Class::instance_proto(value, proto)` lets you choose the prototype; `Class::define(&globals)` installs the constructor; the macro has no `extends` (class attrs are `frozen`/`rename`/`rename_all` only) | `class.rs:100-105,242,273`; `rquickjs-macro-0.12.2/src/class.rs:19-23`; method attrs `get/set/enumerable/configurable/static/constructor/rename` at `methods/method.rs:18-27`; field attrs `get/set/enumerable/configurable/skip_trace/rename` at `fields.rs:15-21` |
| Accessor properties from Rust | `Object::prop(key, Accessor::new(get).set(set))` | `value/object/property.rs:29,170,190` |
| ArrayBuffer | `ArrayBuffer::new` (takes a `Vec`), `new_copy`, `as_bytes() -> Option<&[u8]>` (None when detached), `detach(&mut self)`, `as_raw()` | `value/array_buffer.rs:91,123,242,259,304` |
| TypedArray ↔ buffer | `TypedArray::arraybuffer()`, `from_arraybuffer()` | `value/typed_array.rs:219,232` |
| Calling script's name (for URL resolution) | `Ctx::script_or_module_name(stack_level)` | `context/ctx.rs:452-463` (wraps `JS_GetScriptOrModuleName`, `quickjs.h:1245`) |
| Module ops | `Module::declare`, `evaluate`, `evaluate_def`, `import` | `value/module.rs:260,308,323,426` |
| Eval as classic script | `EvalOptions { global: true, strict, promise, filename }` | `context/ctx.rs:29-41`; den uses it at `den-core/src/engine.rs:326-332` |

### 1.6 The tokio constraint: the per-worker runtime must be `multi_thread`

den's two file loaders end in `tokio::task::block_in_place(move || Handle::current().block_on(task))`
(`den-core/src/loader/http.rs:116`, `den-core/src/loader/mmap_script.rs:92`; the manifest comment at
`den-core/Cargo.toml` "block_in_place + Handle::current in the loaders need these" explains the
`rt-multi-thread` feature). `block_in_place` **panics** on a current-thread runtime with
`"can call blocking only when running on the multi-threaded runtime"`
(`tokio-1.53.1/src/runtime/scheduler/multi_thread/worker.rs:434`). Therefore the "small tokio runtime
per worker" is `Builder::new_multi_thread().worker_threads(1).enable_all().build()`
(`runtime/builder.rs:276,376,1072`) — **not** `new_current_thread()` (`:261`). One OS thread for
the QuickJS side plus one tokio worker thread per `Worker`; that is the floor.

Why it works even though the QuickJS thread is the `rt.block_on` caller and *not* a pool worker:
`MultiThread::block_on` enters the runtime with `allow_block_in_place = true`
(`scheduler/multi_thread/mod.rs:91`; the current-thread scheduler passes `false`,
`scheduler/current_thread/mod.rs:206`), `block_in_place` accepts that case explicitly
(`worker.rs:413-424`, "only okay if we are in a thread pool runtime's block_on method") and runs the
closure under `exit_runtime` (`worker.rs:505`) so the nested `Handle::current().block_on(..)` does
not hit "cannot start a runtime from within a runtime". This is exactly the configuration den's main
thread already runs in (`#[tokio::main]` = multi-thread `block_on`, `src/main.rs:24`), so the loader
path is proven, not theoretical. Consequence for tests: anything that constructs an `Engine` must be
`#[tokio::test(flavor = "multi_thread")]` (`den-core/tests/stdlib.rs:12`).

### 1.7 quickjs-libc's `os.Worker` is the local prior art, and it is the design to mirror

`quickjs-libc.c` implements a dedicated worker with the same constraints (one runtime per thread):

- Message = owned byte blob + SAB table (`JSWorkerMessage`, `:150-157`); pipe = mutex + list +
  waker fd (`JSWorkerMessagePipe`, `:167-173`); one handler per pipe (`:175-179`).
- Thread body: new runtime, new context, `JS_SetCanBlock(rt, true)` (so `Atomics.wait` works in the
  worker, `:4096`), load the module, run the loop, free everything (`worker_func`, `:4060-4115`).
- Constructor: refuses nested workers (`:4162-4163` — den should **not** copy this: the HTML spec
  allows nested dedicated workers, §10.2.3 "relevant owner to add"), resolves the script relative
  to the calling module via `JS_GetScriptOrModuleName(ctx, 1)` (`:4167`), creates two pipes, spawns
  a detached thread (`:4198`).
- `postMessage`: serialize, **re-copy into `malloc` memory because the receiving runtime has a
  different allocator** (`:4255-4257`), bump SAB refcounts (`:4272-4274`), lock, signal the waker,
  append (`:4276-4281`).
- `onmessage` is a single getter/setter slot, `null` removes, non-function → `TypeError`
  (`:4298-4345`); delivery builds a bare `{ data }` object and calls the handler
  (`handle_posted_message`, `:2675-2715`).
- Unhandled rejections: tracked in a list (`js_std_promise_rejection_tracker`, `:4782-4806`) and
  checked "once the application is about to sleep" — and then it **prints and `exit(1)`s**
  (`:4812-4826`). den must not copy the exit; see §2.11.

Everything it gets right (owned blobs, per-pipe FIFO, a waker the receiving loop can await,
refcounted SABs) carries over; everything it skips (EventTarget, queue enabling, transfer lists,
ErrorEvent propagation, `close()`, `terminate()`) is the spec surface this note enumerates.

### 1.8 den today

- `Engine::new` builds runtime, resolver/loader chain, interrupt handler and context, and
  `evaluate_def`s every stdlib module so that its `#[qjs(evaluate)]` hook installs globals
  (`den-core/src/engine.rs:35-306`; the three-place registration rule is `ARCHITECTURE.md` §2).
  A worker thread can call `Engine::new()` as-is to get the full stdlib and loader chain.
- Process lifetime is `App::run_until_end`: `tokio::spawn(runtime.drive())`, then
  `stop_token.run_until_cancelled(runtime.idle())` (`src/app.rs:99-115`). `idle()` returns only when
  no spawned future is pending (`runtime/async.rs:350-356`). So **a pending receiver future spawned
  with `ctx.spawn` is exactly what "a live Worker keeps the process alive" needs** — no new
  lifetime machinery.
- Uncaught errors from `run_file`/`eval` are printed in `src/main.rs:52-66` and `src/app.rs:55-71`
  via `ctx.catch()`; there is no `EventTarget`, no `addEventListener`, no `structuredClone`, no
  `DOMException` usage anywhere in den (grep over the workspace → 0 hits outside this doc).
- `den-stdlib-wasm/src/error.rs:22-43` is the pattern for JS-land classes built once at evaluate
  time and cached in userdata (`:80-104`); `den-stdlib-whatwg-fetch/src/lib.rs:8-26,85-88,181-187`
  is the pattern for an rquickjs class with getters installed as a global.

---

## 2. Conformance checklist

Legend: `MISSING` (nothing exists in den) · `PARTIAL` (engine provides part of it) · `N/A` (cannot
apply to a CLI runtime; the decision is recorded so it is not mistaken for an omission).
Everything in §2 is `MISSING` in den today unless a row says otherwise.

### 2.0 Globals that must exist

In the **main realm** (installed by a new stdlib module's `evaluate` hook, per `ARCHITECTURE.md` §2):

`EventTarget`, `Event`, `CustomEvent` (trivial, comes with `Event`), `MessageEvent`, `ErrorEvent`,
`PromiseRejectionEvent`, `MessageChannel`, `MessagePort`, `BroadcastChannel`, `Worker`,
`structuredClone`, `reportError` (one-liner on top of "report an exception"), and `DOMException`
(already present, §1.3).

In a **worker realm**, all of the above plus the `DedicatedWorkerGlobalScope` surface on
`globalThis`: `self`, `name`, `postMessage`, `close`, `importScripts`, `onmessage`,
`onmessageerror`, `onerror`, `onunhandledrejection`, `onrejectionhandled`, `onlanguagechange`,
`onoffline`, `ononline` (last three exist but never fire), `location` (`WorkerLocation`, P3),
`navigator` (`WorkerNavigator`, P3). `globalThis` in a worker must itself be an `EventTarget`
(`WorkerGlobalScope : EventTarget`, HTML §10.2.1.1 IDL) — `self.addEventListener("message", …)`
must work.

WebIDL property attributes: interface members are `{writable: true, enumerable: false,
configurable: true}` on the prototype; attributes are accessor pairs on the prototype. If the classes
are written in JS-land (§3.1) `class` syntax gives non-enumerable methods for free; getters need
`Object.defineProperty` or `get x()` in the class body. Each prototype needs
`@@toStringTag` (`"EventTarget"`, `"Worker"`, …) so `Object.prototype.toString.call(w)` is
`"[object Worker]"`.

### 2.1 `EventTarget` (DOM §2.7)

```webidl
[Exposed=*] interface EventTarget {
  constructor();
  undefined addEventListener(DOMString type, EventListener? callback,
                             optional (AddEventListenerOptions or boolean) options = {});
  undefined removeEventListener(DOMString type, EventListener? callback,
                                optional (EventListenerOptions or boolean) options = {});
  boolean dispatchEvent(Event event);
};
callback interface EventListener { undefined handleEvent(Event event); };
dictionary EventListenerOptions { boolean capture = false; };
dictionary AddEventListenerOptions : EventListenerOptions {
  boolean passive; boolean once = false; AbortSignal signal;
};
```

Required behaviour (each item is a DOM step):

- **Event listener record** = `{type, callback, capture, passive, once, signal, removed}`
  (DOM §2.7 "An event listener can be used to observe…"). `removed` matters: a listener removed
  *during* dispatch must not run even though the list was cloned before dispatch (DOM §2.9 "inner
  invoke" step 2 iterates "whose removed is false").
- **flatten options**: a boolean is `capture`; a dictionary supplies `capture`, `once`, and
  optionally `passive`/`signal` (DOM §2.7 "flatten" / "flatten more").
- **add an event listener**: if `signal` is aborted → return; if `callback` is null → return;
  if the list already has a listener with the same `(type, callback, capture)` → do **not** append
  (dedupe); if `signal` is non-null, add abort steps that remove the listener (DOM §2.7 "add an
  event listener" steps 2–6). `callback` may be a function **or** an object with a `handleEvent`
  method ("call a user object's operation", inner-invoke step 11).
- **removeEventListener** matches on `(type, callback, capture)` and sets `removed = true`.
- **dispatchEvent(event)**: if `event`'s dispatch flag is set, or its initialized flag is not set →
  throw `"InvalidStateError"` `DOMException`; set `isTrusted = false`; dispatch; return `false` if
  the canceled flag is set, else `true` (DOM §2.7 "dispatchEvent" steps 1–3, §2.9 step 13).
- **dispatch** (DOM §2.9) for a target that is not in a tree (all den targets: `get the parent`
  returns null, so the path has one item) reduces to: set dispatch flag; `event.target = target`;
  `eventPhase = AT_TARGET`; **clone** the listener list; inner invoke; then `eventPhase = NONE`,
  `currentTarget = null`, clear dispatch/stop-propagation/stop-immediate-propagation flags
  (§2.9 steps 1, 6.3, 6.13–14, 7–10). Because the phase is always `AT_TARGET`, **both capture and
  non-capture listeners run, in registration order** (inner-invoke steps 3–4 only skip when the phase
  is `capturing`/`bubbling`). `capture` still participates in identity/dedupe.
- **inner invoke** (DOM §2.9): for each listener with `removed == false` and `type ==
  event.type`: if `once`, remove it **before** calling; if `passive`, set the in-passive-listener
  flag for the call; call `callback(event)` (or `callback.handleEvent(event)`) with `this =
  currentTarget`; **an exception thrown by a listener is reported (§2.11) and does not stop the
  remaining listeners**; unset the passive flag; if the stop-immediate-propagation flag is set →
  break (steps 2.1–2.14).
- `AbortSignal` is out of scope (den has no `AbortController`); accept and ignore `signal` until
  one exists, documenting it.
- `"ServiceWorkerGlobalScope"` warnings (add/remove step 1): N/A.

Ordering guarantee that the spec's tests check: listeners fire in registration order; the
`onX` handler slot occupies the position of its **first** non-null assignment (§2.3).

### 2.2 `Event` (DOM §2.2, §2.5)

```webidl
[Exposed=*] interface Event {
  constructor(DOMString type, optional EventInit eventInitDict = {});
  readonly attribute DOMString type;
  readonly attribute EventTarget? target;
  readonly attribute EventTarget? srcElement;      // legacy alias of target
  readonly attribute EventTarget? currentTarget;
  sequence<EventTarget> composedPath();
  const unsigned short NONE = 0; const unsigned short CAPTURING_PHASE = 1;
  const unsigned short AT_TARGET = 2; const unsigned short BUBBLING_PHASE = 3;
  readonly attribute unsigned short eventPhase;
  undefined stopPropagation();
  attribute boolean cancelBubble;                   // legacy alias of stopPropagation()
  undefined stopImmediatePropagation();
  readonly attribute boolean bubbles;
  readonly attribute boolean cancelable;
  attribute boolean returnValue;                    // legacy
  undefined preventDefault();
  readonly attribute boolean defaultPrevented;
  readonly attribute boolean composed;
  [LegacyUnforgeable] readonly attribute boolean isTrusted;
  readonly attribute DOMHighResTimeStamp timeStamp;
  undefined initEvent(DOMString type, optional boolean bubbles = false, optional boolean cancelable = false);
};
dictionary EventInit { boolean bubbles = false; boolean cancelable = false; boolean composed = false; };
[Exposed=*] interface CustomEvent : Event {
  constructor(DOMString type, optional CustomEventInit eventInitDict = {});
  readonly attribute any detail;
  undefined initCustomEvent(DOMString type, optional boolean bubbles = false,
                            optional boolean cancelable = false, optional any detail = null);
};
dictionary CustomEventInit : EventInit { any detail = null; };
```

Required behaviour:

- Seven internal flags, all initially unset: stop propagation, stop immediate propagation,
  canceled, in passive listener, composed, initialized, dispatch (DOM §2.2).
- Constructor = "inner event creation steps": set the initialized flag, `timeStamp` = now relative
  to the time origin, copy every `eventInitDict` member that names an attribute, then `type`
  (DOM §2.5 steps 1–3 + "inner event creation steps" 1–5). `isTrusted` is `false` for
  constructor-created events and `true` for events the runtime creates via "create an event"
  (DOM §2.5 "create an event" step 4) — so `message`/`error` events den fires have `isTrusted ===
  true`, and `dispatchEvent` forces it to `false`.
- `stopPropagation()` sets the stop-propagation flag; `stopImmediatePropagation()` sets both;
  `cancelBubble` get/set mirror the stop-propagation flag (setting `false` is a no-op) (DOM §2.2).
- `preventDefault()` = "set the canceled flag": only if `cancelable` is true **and** the
  in-passive-listener flag is unset (DOM §2.2 "To set the canceled flag"). `defaultPrevented`
  reads it; `returnValue` getter is `!canceled`, setter with `false` sets it.
- `composedPath()` returns `[currentTarget]` during dispatch and `[]` otherwise (DOM §2.2 steps
  1–6 with a single-item path).
- `initEvent` is a no-op if the dispatch flag is set; otherwise re-initializes (DOM §2.2 "initEvent"
  + "initialize an event" steps 1–7).
- `[LegacyUnforgeable] isTrusted` means a non-configurable **own** accessor on the instance, not a
  prototype getter.

### 2.3 Event handler IDL attributes (`onmessage`, `onerror`, …) (HTML §8.1.8.1)

This is the part that is most often implemented wrongly as a plain data property. The normative
model:

- An **event handler** is `{value: null | callback, listener: null | event listener}` kept in the
  target's **event handler map**, one entry per supported handler name (§8.1.8.1 "An event handler is
  a struct…", "event handler map").
- **Getter**: return the current value (null or the callback).
- **Setter**: if the value is `null` → *deactivate* (set value to null, remove the listener from the
  listener list, set listener to null). Otherwise set value, then *activate*: if `listener` is
  already non-null do nothing; else create one listener whose callback runs "the event handler
  processing algorithm" and **add it to the event listener list at that moment** (§8.1.8.1
  "setter" / "activate an event handler" / "deactivate an event handler").
  `[LegacyTreatNonObjectAsNull]`: assigning a non-object (e.g. a string) stores `null`; assigning a
  non-callable object stores the object and the processing algorithm simply does nothing
  (§8.1.8.1 `EventHandlerNonNull` IDL).
- **Consequence**: the handler keeps its position in the listener list from the first non-null
  assignment; reassigning swaps the value without moving; assigning `null` then a function moves
  it to the end (the two example sequences "ONE TWO THREE FOUR" / "ONE … FIVE" in §8.1.8.1).
- **Processing algorithm**: call `value(event)` with `this = currentTarget`; if it throws, the
  exception propagates to dispatch and is reported. Return-value handling: for `onerror` **on a
  global** (`WindowOrWorkerGlobalScope`, i.e. `self.onerror` inside a worker) with an `ErrorEvent`
  of type `"error"`, the callback is invoked with **five arguments** `(message, filename, lineno,
  colno, error)` and a return value of `true` cancels the event; for **every other** handler
  (including `worker.onerror` on a `Worker` object, which is not a global) the callback gets
  `(event)` and a return value of `false` cancels (§8.1.8.1 "event handler processing algorithm"
  steps 4–6).
- Which names each object must support: `Worker` — `onerror` (`AbstractWorker`, HTML §10.2.6.1),
  `onmessage`, `onmessageerror` (`MessageEventTarget`, §9.4.3); `MessagePort` — `onmessage`,
  `onmessageerror`, `onclose` (§9.4.4); `BroadcastChannel` — `onmessage`, `onmessageerror` (§9.5);
  `DedicatedWorkerGlobalScope` — `onmessage`, `onmessageerror` plus `WorkerGlobalScope`'s
  `onerror` (typed `OnErrorEventHandler`), `onlanguagechange`, `onoffline`, `ononline`,
  `onrejectionhandled`, `onunhandledrejection` (§10.2.1.1).
- **Special rule**: the first time a `MessagePort`'s `onmessage` is set, its port message queue is
  enabled "as if `start()` had been called" (§9.4.4 last paragraph). Only `MessagePort` has this
  rule; the `Worker` object's implicit port is always enabled (§10.1.3.2 note: "there is no
  equivalent to the MessagePort interface's start() method on the Worker interface").

### 2.4 `MessageEvent` (HTML §9.1)

```webidl
[Exposed=(Window,Worker,AudioWorklet)] interface MessageEvent : Event {
  constructor(DOMString type, optional MessageEventInit eventInitDict = {});
  readonly attribute any data;
  readonly attribute USVString origin;
  readonly attribute DOMString lastEventId;
  readonly attribute MessageEventSource? source;
  readonly attribute FrozenArray<MessagePort> ports;
  undefined initMessageEvent(DOMString type, optional boolean bubbles = false, optional boolean cancelable = false,
    optional any data = null, optional USVString origin = "", optional DOMString lastEventId = "",
    optional MessageEventSource? source = null, optional sequence<MessagePort> ports = []);
};
dictionary MessageEventInit : EventInit {
  any data = null; USVString origin = ""; DOMString lastEventId = "";
  MessageEventSource? source = null; sequence<MessagePort> ports = [];
};
typedef (WindowProxy or MessagePort or ServiceWorker) MessageEventSource;
```

- `data` returns what it was initialized to (§9.1 "The data attribute…"); default `null`.
- `origin`: the getter returns `""` when the internal origin is null (§9.1 "origin getter steps"
  2). den never has an origin for worker/port messages, so `origin === ""` always, and
  `BroadcastChannel` messages get `sourceOrigin` = `""` as well. **N/A beyond the empty string.**
- `lastEventId` is for server-sent events: always `""`. **N/A.**
- `source`: `WindowProxy`/`ServiceWorker` are N/A; `MessagePort` as a source only appears in the
  shared-worker `connect` event (§9.1 "source attribute", §10.2.6.4), so `null` always.
- `ports`: a **frozen** array (`Object.freeze`) of the `MessagePort`s transferred with the message,
  in transfer-list order (§9.4.4 post-message task step 2.6 "newPorts … maintaining their relative
  order"). Empty frozen array by default. `messageerror` events have `data = null`, `ports = []`.
- `initMessageEvent` re-initializes like `initEvent` (no-op if dispatching).

### 2.5 `ErrorEvent` and `PromiseRejectionEvent` (HTML §8.1.4.6–8.1.4.7)

```webidl
[Exposed=*] interface ErrorEvent : Event {
  constructor(DOMString type, optional ErrorEventInit eventInitDict = {});
  readonly attribute DOMString message; readonly attribute USVString filename;
  readonly attribute unsigned long lineno; readonly attribute unsigned long colno;
  readonly attribute any error;
};
dictionary ErrorEventInit : EventInit {
  DOMString message = ""; USVString filename = ""; unsigned long lineno = 0; unsigned long colno = 0; any error;
};
[Exposed=*] interface PromiseRejectionEvent : Event {
  constructor(DOMString type, PromiseRejectionEventInit eventInitDict);
  readonly attribute object promise; readonly attribute any reason;
};
dictionary PromiseRejectionEventInit : EventInit { required object promise; any reason; };
```

- `error` "must initially be initialized to undefined" (§8.1.4.6) — note `undefined`, not `null`,
  when the dictionary omits it; the propagated-to-parent `ErrorEvent` has `error = null` because
  the spec sets `errorInfo[error]` to null before crossing the boundary (§8.1.4.6 "report an
  exception" step 7.1).
- `PromiseRejectionEvent`'s constructor requires the dictionary (`promise` is `required`) →
  `TypeError` when missing.
- `unsigned long` for `lineno`/`colno`: WebIDL conversion (ToNumber, modulo 2^32).

### 2.6 `MessageChannel` / `MessagePort` (HTML §9.4.2–9.4.5)

```webidl
[Exposed=(Window,Worker)] interface MessageChannel {
  constructor();
  readonly attribute MessagePort port1; readonly attribute MessagePort port2;
};
[Exposed=(Window,Worker,AudioWorklet), Transferable] interface MessagePort : EventTarget {
  undefined postMessage(any message, sequence<object> transfer);
  undefined postMessage(any message, optional StructuredSerializeOptions options = {});
  undefined start();
  undefined close();
  attribute EventHandler onclose;
};
MessagePort includes MessageEventTarget;            // onmessage, onmessageerror
dictionary StructuredSerializeOptions { sequence<object> transfer = []; };
```

State per port (§9.4.4): *entangled-with* (symmetric), **port message queue** (a task source,
initially empty and **disabled**; once enabled it can never be disabled again), *has been shipped*
flag, `[[Detached]]`, and a *message event target* (defaults to the port itself; the worker's inside
port's target is the `DedicatedWorkerGlobalScope` and the outside port's target is the `Worker`
object — §10.2.4 step 2.3.1, §10.2.6.3 constructor step 5).

Required behaviour:

- `new MessageChannel()`: two new ports, entangle them (§9.4.2 steps 1–3).
- **entangle(A, B)**: if either is already entangled, disentangle that pair first; then associate
  (§9.4.4 "entangle" steps 1–2). **disentangle(initiator)**: find the other port, dissociate, **fire
  `close` at the other port** (steps 1–4). `close` fires on the *other* side in all three cases:
  explicit `close()`, realm destroyed (worker exit/terminate), port garbage-collected (§9.4.4 note).
- **postMessage(message, transfer | {transfer})** = "message port post message steps" with
  `targetPort` = entangled port or null (§9.4.4 steps):
  1. if `transfer` contains **this** port → throw `"DataCloneError"`;
  2. if `transfer` contains the **target** port → `doomed = true` (serialize anyway, then drop the
     message; the channel is lost — optionally warn);
  3. `StructuredSerializeWithTransfer(message, transfer)`, rethrowing (so a `DataCloneError` from
     the message itself is synchronous, and nothing has been transferred when it throws — §2.7.7
     step 2 validates the transfer list *before* serializing and detaches only in step 4);
  4. if `targetPort` is null (not entangled: closed, never entangled, or peer gone) or `doomed` →
     **return silently** (the message is dropped, no error);
  5. otherwise add a task to the target's port message queue that: finds `finalTargetPort` (the
     queue may have moved with a transfer), deserializes with transfer in the target realm, and
     on failure fires `messageerror` (a `MessageEvent` with no data) at the message event target,
     else fires `message` with `data` and frozen `ports`.
- **Queue enabling rules** (the "a message posted before `onmessage` is set is queued, not lost"
  guarantee): tasks accumulate in the port message queue while it is disabled; `start()` enables
  it; the first assignment to `onmessage` enables it; `addEventListener("message", …)` does **not**
  (§9.4.1.1 "The key difference is that when using addEventListener(), the start() method must also
  be invoked"). Once enabled, queued tasks are dispatched in FIFO order and later tasks are
  dispatched as they arrive. Dispatch is a *task*, never synchronous inside `postMessage` — even
  when both ports live in the same realm (`channel.port1.postMessage` from `port2.onmessage` must
  not recurse).
- **Ordering**: one queue per port, FIFO. Messages from port A to port B are observed in the order
  `postMessage` was called on A. There is no cross-port ordering guarantee (and den should not
  promise one), but within the worker↔parent implicit channel the single pipe gives FIFO.
- `start()`: enable if not already (§9.4.4). Idempotent.
- `close()`: set `[[Detached]] = true`; if entangled, disentangle (which fires `close` on the peer)
  (§9.4.4 "close() method steps"). After `close()`, `postMessage` on this port silently drops
  (target is null) and this port can no longer be transferred (`[[Detached]]` → `DataCloneError`,
  §2.7.7 step 4.2).
- **Transfer steps** (§9.4.4 "MessagePort objects are transferable objects"): set *has been shipped*
  on the port and on its peer; hand over the port message queue and the remote port in the data
  holder. **Transfer-receiving steps**: new port in the target realm, *has been shipped* = true,
  move the queued `message` tasks onto the new port's queue **leaving it disabled**, entangle with
  the remote port (which implicitly disentangles the remote from the now-dead original). So a port
  can be shipped with unread messages and they arrive after the receiver calls `start()`/sets
  `onmessage`.
- **GC / lifetime** (§9.4.5): a port with a `message`/`messageerror` listener that is entangled must
  be kept alive by its peer; a port with non-empty enabled queue or pending tasks must not be
  collected. For den: a port's Rust-side shared state is an `Arc` held by both ends, and the JS
  object must be kept reachable (e.g. parked in a per-realm registry) while it has listeners and
  is entangled; `close()` removes it from the registry.
- `Worker.postMessage` / `self.postMessage` are defined as "act as if … invoked postMessage on the
  outside/inside port" (§10.2.6.3, §10.2.1.2) — same code path, same errors.

N/A: `WindowProxy` entanglement, "associate tasks with the Document", unshipped port message queue
vs. shipped distinction (den has no per-Document event loop; one queue per port is the observable
behaviour either way).

### 2.7 `Worker` (HTML §10.2.6.1–10.2.6.3)

```webidl
[Exposed=(Window,DedicatedWorker,SharedWorker)] interface Worker : EventTarget {
  constructor((TrustedScriptURL or USVString) scriptURL, optional WorkerOptions options = {});
  undefined terminate();
  undefined postMessage(any message, sequence<object> transfer);
  undefined postMessage(any message, optional StructuredSerializeOptions options = {});
};
dictionary WorkerOptions {
  DOMString name = ""; WorkerType type = "classic";
  RequestCredentials credentials = "same-origin"; // only used if type is "module"
};
enum WorkerType { "classic", "module" };
interface mixin AbstractWorker { attribute EventHandler onerror; };
Worker includes AbstractWorker;
Worker includes MessageEventTarget;                  // onmessage, onmessageerror
```

**Constructor** (§10.2.6.3 "new Worker(scriptURL, options)" steps):

1. `scriptURL` → string (`TrustedScriptURL`: N/A, treat as string).
2. Parse `scriptURL` relative to **outside settings' API base URL** ("encoding-parsing a URL …
   relative to outsideSettings"). That base is a property of the *realm*, not of the calling
   script: in a browser it is the document URL for every script in the page, and for a worker it
   is the worker's `url`. den mirrors that with a per-realm `base_url` in context userdata —
   the entry file for the main realm (set by `Engine::run_file`), the resolved worker URL for a
   worker realm, the process CWD for REPL/`eval` input. **Do not** use
   `Ctx::script_or_module_name(1)` for this (quickjs-libc does, `quickjs-libc.c:4167`, but its
   constructor is a C function called directly from user code): `JS_GetScriptOrModuleName` walks
   raw stack frames and returns the `filename` of the bytecode function `n` frames up
   (`quickjs.c:30890-30912`); with the JS-shell design of §3.1 frame 1 is the shell's own `Worker`
   constructor (filename = the shell's eval name), and a user subclass or `Reflect.construct` adds
   frames, so the level is not stable. `file:`/plain paths, `http:`/`https:` are accepted — the
   same specifier space den's resolver chain already handles (`ARCHITECTURE.md` §3). A parse
   failure → **`"SyntaxError"` `DOMException`** thrown synchronously.
3. `options` is a dictionary: `name` (`ToString`, default `""`), `type` (`"classic"` | `"module"`,
   anything else → `TypeError` per WebIDL enum conversion), `credentials` (**N/A**, accept and
   ignore).
4. Create the outside port, set its message event target to the `Worker` object; its queue is
   implicitly enabled (§10.1.3.2).
5. Return immediately; "run a worker" happens **in parallel** (§10.2.6.3 step 10) — the
   constructor never blocks on the thread start or the script fetch.
6. `new` is required (`TypeError` when called as a function) — WebIDL constructor semantics.
7. Nested workers are allowed: a worker may construct a `Worker`; the relevant owner is then that
   `WorkerGlobalScope` (§10.2.3 "relevant owner to add"). Errors chain upward (§2.11).

**postMessage**: exactly §2.6 on the outside port. After `terminate()` the outside port is
disentangled → silent drop.

**terminate()** = "terminate a worker" (§10.2.4 "When a user agent is to terminate a worker"):

1. set the worker global's **closing flag**;
2. discard every task in the worker's task queues without running them (queued messages, timers,
   pending fetch callbacks);
3. **abort the script currently running** — "Killing scripts" (§8.1.4.5): the execution context
   stack is emptied *without running `finally` blocks*; in den this is the interrupt handler
   returning `true` (an uncatchable exception) — den already has this per-engine at
   `den-core/src/engine.rs:227-232`; `Atomics.wait` cannot be interrupted by it (known ceiling).
   Latency and scope, from `quickjs.c`: the handler is polled only when `ctx->interrupt_counter`
   (reset to `JS_INTERRUPT_COUNTER_INIT = 10000`, `:479`) reaches zero, decremented per loop
   back-edge and per call (`:8221-8238`, `:16348`, `:17590`, `:18666-18755`), so a tight loop
   stops within ≤10000 iterations and **each** later job/callback also gets up to 10000 steps
   before it is cut (the counter is reset on every poll). The thrown value is an
   `InternalError("interrupted")` flagged uncatchable (`:8215-8219`); the bytecode loop then skips
   every `JS_TAG_CATCH_OFFSET` handler — `catch`, `finally` *and* iterator-close — when the
   exception is uncatchable (`:20328-20349`), which is exactly HTML's "Killing scripts". den's
   handler reads a `CancellationToken`, so once cancelled it answers `true` forever — correct for a
   terminated worker (no JS must run again), and the reason the worker's token must be distinct
   from the parent's;
4. **empty the port message queue of the outside port** — messages the worker already posted but
   the parent has not yet dispatched are dropped (§10.2.4 terminate step 4).

`terminate()` is idempotent and returns `undefined`. After it, `onmessage` on the `Worker` never
fires again, `onerror` does not fire for the abort, and the thread is reclaimed.

**Events on the `Worker` object**: `message` (`MessageEvent`), `messageerror` (`MessageEvent`,
deserialization failure), `error` (`ErrorEvent`, §2.11). A **fetch/parse failure of the worker
script** is *not* an exception from the constructor: "run a worker" `onComplete` step 1 queues a
task to **fire a plain `error` event** (`Event`, not `ErrorEvent`, no message) at the `Worker`
(§10.2.4 "onComplete" step 1.1) and discards the environment.

N/A: `TrustedScriptURL`, `credentials`, CSP initialization (§10.2.4 fetch step 2.4), embedder
policy / cross-origin isolation (steps 2.5–2.9), `data:` URL opaque origins, "closing orphan
workers" monitoring (den workers have exactly one owner and live until `close()`/`terminate()`, per
the maintainer's lifetime decision), "suspending workers" (never suspendable).

### 2.8 `WorkerGlobalScope` / `DedicatedWorkerGlobalScope` (HTML §10.2.1, §10.2.4, §10.3.1)

```webidl
[Exposed=Worker] interface WorkerGlobalScope : EventTarget {
  readonly attribute WorkerGlobalScope self;
  readonly attribute WorkerLocation location;
  readonly attribute WorkerNavigator navigator;
  undefined importScripts((TrustedScriptURL or USVString)... urls);
  attribute OnErrorEventHandler onerror;
  attribute EventHandler onlanguagechange; attribute EventHandler onoffline; attribute EventHandler ononline;
  attribute EventHandler onrejectionhandled; attribute EventHandler onunhandledrejection;
};
[Global=(Worker,DedicatedWorker),Exposed=DedicatedWorker]
interface DedicatedWorkerGlobalScope : WorkerGlobalScope {
  [Replaceable] readonly attribute DOMString name;
  undefined postMessage(any message, sequence<object> transfer);
  undefined postMessage(any message, optional StructuredSerializeOptions options = {});
  undefined close();
};
DedicatedWorkerGlobalScope includes MessageEventTarget;   // onmessage, onmessageerror
```

State: owner set, `type`, `url`, `name`, **closing flag** (initially false, §10.2.2), inside port
(never exposed, never collected before the global — §10.2.1.2), module map.

**"run a worker"** (§10.2.4), reduced to what a CLI runtime does, in order:

1. Create the agent (thread), realm (`JSRuntime` + `JSContext`), global object; set `name` from
   `options["name"]`; record the owner.
2. Fetch the script: `"classic"` → fetch a classic worker script; `"module"` → fetch a module
   worker script graph. On failure → `error` at the `Worker` (§2.7) and stop. In den the
   `"module"` path is `Module::import(&ctx, url)` (`value/module.rs:426`, returns the evaluation
   `Promise`) which goes through the existing resolver/loader chain. The `"classic"` path has
   **no loader support**: both loaders only produce `Module`s via `Module::declare`
   (`den-core/src/loader/mmap_script.rs:84`, `http.rs`), so the worker must read the bytes itself
   (`std::fs::read` for `file:`/paths, `reqwest` through `block_in_place` for `http(s):`, as
   `HttpLoader` does at `http.rs:116`) and `eval_with_options` them with
   `EvalOptions { global: true, strict: false, promise: false, filename: Some(url) }`.
3. Create the inside port, message event target = the global; **entangle** outside ↔ inside.
4. Run the script: classic → "run the classic script" (global code, sloppy by default, **a
   top-level uncaught exception is reported** per §2.11); module → "run the module script" (strict,
   `import` allowed, top-level `await` allowed, an evaluation error is reported the same way).
   The run may be "prematurely aborted by the terminate a worker algorithm" (§10.2.4 step 2.11
   note).
5. **Only after the script returns**: enable the outside port's queue (§10.2.4 step 2.12) and
   **enable the inside port's queue** (step 2.13). This is the ordering guarantee that makes
   `onmessage = …` at top level catch messages the parent posted before the script finished; the
   parent's messages are queued, not lost, and are delivered **after** top-level code has run.
   **Top-level `await` does not extend this window**: "run a module script" (HTML §8.1.4.3)
   calls `record.Evaluate()`, attaches a rejection handler that reports the exception, and
   *returns the evaluation promise without awaiting it*, so steps 2.12–2.13 run as soon as the
   module has evaluated up to its first `await`. A module worker that sets `onmessage` after an
   `await` can miss messages — that is browser behaviour and den must match it (in the skeleton
   of §3.2: enable the queues right after `Module::import(..)` returns the promise, and let the
   pump and the promise run concurrently; the rejection of that promise is reported per §2.11).
6. Event loop until destroyed — i.e. until the closing flag is true **and** the current task ends.
7. Teardown: clear active timers, **disentangle all of the worker's ports** (which fires `close` at
   each peer, including the `Worker` side's outside port — observable only through transferred
   `MessagePort`s since the outside port is hidden), empty the owner set (steps 2.16–2.18).

**self**: returns the global itself. **name**: `[Replaceable]` → a getter on the global whose
assignment *replaces* it with a data property (WebIDL `[Replaceable]` setter semantics).

**postMessage** = §2.6 on the inside port (§10.2.1.2).

**close()** = "close a worker" (§10.2.1.2): (1) discard any tasks already queued for this worker,
(2) set the closing flag. It does **not** abort the current script — code after `close()` keeps
running to the end of the current task, and `postMessage(result); close();` delivers `result`
because the *parent's* queue is not closing (§10.2.2: the closing flag discards tasks "that would
be added to *them*" — the worker's own queues). Timers stop firing, nothing new is queued, the
thread exits when the current task returns.

**importScripts(...urls)** (§10.3.1 "import scripts into worker global scope"):

1. if the worker's `type` is `"module"` → throw `TypeError`;
2. if `urls` is empty → return;
3. parse every URL first (relative to the worker's `url`); any failure → `"SyntaxError"`
   `DOMException` **before any script runs**;
4. for each URL in order: fetch synchronously ("fetch a classic worker-imported script"), then run
   it as a **classic script in the global scope** with "rethrow errors" — the first script that
   fails to load, fails to parse, or throws aborts the loop and the exception propagates to the
   caller of `importScripts` (step 5.2 note: "letting the exception … continue to be processed by
   the calling script"). A network error surfaces as a `"NetworkError"` `DOMException`.
   In den: file → read; `http(s)` → `reqwest` via `block_in_place` exactly as `HttpLoader` does
   (`den-core/src/loader/http.rs:116`); transpile with `IsModule::Bool(false)` if TS is on; eval
   with `EvalOptions { global: true, .. }`.

**Event handlers** on the global (§10.2.1.1 table): `onerror` with the **five-argument** special
call (§2.3), `onlanguagechange`/`onoffline`/`ononline` (exist, never fire — N/A),
`onrejectionhandled`/`onunhandledrejection` (§2.11), plus `onmessage`/`onmessageerror`.

**location** (`WorkerLocation`, §10.3.3): `href` = the worker's `url` serialized, `origin`,
`protocol`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, stringifier = `href`. For a
`file:` URL most are `""`. P3; cheap to add with the `url` crate den-core already depends on.
**navigator** (`WorkerNavigator`, §10.3.2): `hardwareConcurrency` =
`std::thread::available_parallelism()` (§10.2.7: a number ≥ 1), `userAgent`, `language`,
`onLine = true`. P3.

N/A: owner-set / "permissible" / "protected" / "actively needed" / "suspendable" lifetime
algebra (§10.2.3) — den's rule is simpler and stricter: a worker lives until `close()` or
`terminate()` and the parent process waits for it; CSP, embedder policy, cross-origin isolation,
policy container, module map sharing, `credentials`, service-worker client queue (§10.2.4 step
2.15), "between-loads"/"extended lifetime" timeouts (shared workers only).

### 2.9 `BroadcastChannel` (HTML §9.5)

```webidl
[Exposed=(Window,Worker)] interface BroadcastChannel : EventTarget {
  constructor(DOMString name);
  readonly attribute DOMString name;
  undefined postMessage(any message);
  undefined close();
  attribute EventHandler onmessage; attribute EventHandler onmessageerror;
};
```

- Constructor: `name` → string (`ToString`, so `new BroadcastChannel(1).name === "1"`), closed
  flag false (§9.5 constructor steps 1–2). Requires `new`.
- **Eligible for messaging**: a `WorkerGlobalScope` whose closing flag is false (and is not
  suspendable — always true in den) or a Window with a fully active Document — for den's main
  realm: always eligible while the process runs (§9.5 "eligible for messaging").
- **postMessage(message)** (§9.5 steps):
  1. if not eligible → return silently (a closing worker's channel drops sends);
  2. if closed flag → throw **`"InvalidStateError"` `DOMException`**;
  3. `StructuredSerialize(message)` — **no transfer list**; rethrow `DataCloneError`; a
     `SharedArrayBuffer` is still allowed (it is `StructuredSerialize`, not `…ForStorage`);
  4. destinations = every `BroadcastChannel` in the **process** (storage-key partition: N/A — den's
     partition is the process) with the same `name`, eligible, **excluding the sender** (a channel
     never receives its own message; a *different* `BroadcastChannel` object with the same name in
     the same realm does);
  5. sort: channels of the same agent in **creation order, oldest first**; cross-agent order is
     implementation-defined;
  6. for each destination queue a task on **its** realm: if its closed flag is now true → abort;
     `StructuredDeserialize` per destination (each gets its own copy); failure → `messageerror`
     with `origin = ""`; else `message` with `data` and `origin = ""`.
- **close()**: set the closed flag (§9.5). Not idempotency-sensitive; after it `postMessage`
  throws and the object receives nothing.
- **Lifetime**: while not closed and with a `message`/`messageerror` listener, the global holds a
  strong reference (§9.5 "there must be a strong reference") — a process-wide registry
  (`Mutex<HashMap<name, Vec<Endpoint>>>`) plus a per-realm strong table gives exactly that; `close()`
  removes the entry.
- Ordering: per (sender, destination) pair messages arrive in send order because the destination
  task queue is FIFO; across senders no guarantee.

N/A: storage key / origin partitioning (`sourceStorageKey`), `sourceOrigin` (always `""`),
Document fully-active checks.

### 2.10 Structured clone: `structuredClone()` and the (de)serialize algorithms (HTML §2.7)

```webidl
// on WindowOrWorkerGlobalScope:
any structuredClone(any value, optional StructuredSerializeOptions options = {});
```

`structuredClone(value, options)` = `StructuredSerializeWithTransfer(value, options["transfer"])`
then `StructuredDeserializeWithTransfer(serialized, this's relevant realm)`, return
`[[Deserialized]]` (§2.7.10). A second argument that is not an object → `TypeError`
(WebIDL dictionary conversion); `transfer` that is not iterable → `TypeError`.

#### 2.10.1 The full type table `StructuredSerializeInternal` must handle (§2.7.3, in spec order)

The order matters because the checks are `if … otherwise if …`; the first match wins.

| # | Value | Serialize | Deserialize (§2.7.6) |
|---|---|---|---|
| 0 | `memory[value]` exists | return the existing record (identity / cycles) | `memory[serialized]` exists → return existing object |
| 1 | `undefined`, `null`, Boolean, Number, BigInt, String primitives | `{Type: "primitive", Value}` | same value |
| 2 | **Symbol** | **throw `DataCloneError`** | — |
| 3 | `[[BooleanData]]` / `[[NumberData]]` / `[[BigIntData]]` / `[[StringData]]` wrapper objects | `{Type: "Boolean"…, data}` | new wrapper object in target realm |
| 4 | `[[DateValue]]` | `{Type: "Date", DateValue}` (NaN allowed) | new `Date` |
| 5 | `[[RegExpMatcher]]` | `{Type: "RegExp", OriginalSource, OriginalFlags}` — **`lastIndex` is dropped** (it resets to 0) | new `RegExp(source, flags)` |
| 6 | `[[ArrayBufferData]]`, shared | if `forStorage` → `DataCloneError`; else `{Type: "SharedArrayBuffer" | "GrowableSharedArrayBuffer", same data block, AgentCluster}` — cross-origin-isolation check is **N/A** (treat as true) | new SAB over the **same** block; agent-cluster mismatch → `DataCloneError` (den: same process = same cluster) |
| 7 | `[[ArrayBufferData]]`, not shared | `IsDetachedBuffer` → **`DataCloneError`**; copy bytes (`CreateByteDataBlock` may throw `RangeError` on OOM); record `maxByteLength` when resizable | new `ArrayBuffer` (resizable if it was); allocation failure → `DataCloneError` |
| 8 | `[[ViewedArrayBuffer]]` (every TypedArray **and `DataView`**) | out-of-bounds view → `DataCloneError`; serialize the **buffer through the same memory** (so two views on one buffer share one clone); record `Constructor` (`"DataView"` or the `[[TypedArrayName]]`), `ByteLength`, `ByteOffset`, `ArrayLength` | new view of the right constructor over the deserialized buffer |
| 9 | `[[MapData]]` | `{Type: "Map"}`, deep: entries snapshot **before** recursing (a getter mutating the map mid-walk does not change the set of entries) | new `Map`, entries re-inserted in order |
| 10 | `[[SetData]]` | `{Type: "Set"}`, deep, same snapshot rule | new `Set` |
| 11 | `[[ErrorData]]` and not a platform object | `name = Get(value, "name")`; if not one of `Error`, `EvalError`, `RangeError`, `ReferenceError`, `SyntaxError`, `TypeError`, `URIError` → `"Error"`; `message` = `ToString` of the **own data property** `message` if present, else `undefined`; `stack` = implementation-defined string; "should attach a serialized representation of any interesting accompanying data" — **`cause` is not normative** but browsers clone it; den should clone `cause` recursively through `memory` (it is an own data property) and may drop `errors` of `AggregateError` | object with the prototype chosen by `name`, own non-enumerable writable configurable `message` **only if not undefined**, `stack` restored; `DOMException` is a platform object and goes through row 12 (it is `[Serializable]`: `name`, `message`) |
| 12 | Array exotic object | `{Type: "Array", Length: value.length}`, deep | `ArrayCreate(length)` then properties — so **holes stay holes** and `length` is preserved even when the last slots are holes |
| 13 | Platform object that is `[Serializable]` (`DOMException`; den's own classes are not unless they opt in) | `[[Detached]]` true → `DataCloneError`; `{Type: interfaceName}`, deep via its serialization steps | if the interface is not exposed in the target realm → `DataCloneError` |
| 14 | Any other platform object (every rquickjs `#[rquickjs::class]` instance, `MessagePort` not in the transfer list, `Worker`, `EventTarget`…) | **`DataCloneError`** | — |
| 15 | `IsCallable(value)` (functions, classes, callable proxies, bound functions) | **`DataCloneError`** | — |
| 16 | any internal slot other than `[[Prototype]]`, `[[Extensible]]`, `[[PrivateElements]]` — `Promise`, `WeakMap`, `WeakSet`, `WeakRef`, `FinalizationRegistry`, generators, `Proxy` handlers' targets… | **`DataCloneError`** | — |
| 17 | exotic object that is not `%Object.prototype%` — **`Proxy`**, module namespace objects, `arguments` is *not* exotic in this sense (it is ordinary after ES2015? — it has `[[ParameterMap]]`: row 16 throws) | **`DataCloneError`** | — |
| 18 | everything else: ordinary objects, incl. class instances, `Object.create(null)`, frozen objects, `%Object.prototype%` itself | `{Type: "Object"}`, deep: for each key of `EnumerableOwnProperties(value, key)` (**String keys only — symbol-keyed properties are dropped**; order = integer keys ascending, then strings in creation order) that is still an own property (`HasOwnProperty` re-check, because a getter may have deleted a later key), `inputValue = value.[[Get]](key)` (**getters run**, exceptions propagate as-is, not wrapped in `DataCloneError`), serialize it | plain `Object` with `Object.prototype`, properties defined with `CreateDataProperty` (all writable/enumerable/configurable; **accessors become data properties**; frozen-ness is dropped; prototype/class is dropped; `private` fields are dropped) |

After the record is built, `memory[value] = serialized`; the deep walk happens **after** the
memory insertion, which is what makes `o.self = o` terminate and `[a, a]` deserialize as two
references to one object.

Primitive identity is not preserved (two equal strings are two records) — unobservable.

#### 2.10.2 Transfer list rules (`StructuredSerializeWithTransfer`, §2.7.7)

Validation happens **before** serialization so that nothing is detached if anything throws:

1. for each `transferable`: if it has neither `[[ArrayBufferData]]` nor `[[Detached]]` (i.e. it is
   not an `ArrayBuffer` and not a `[Transferable]` platform object — in den: `MessagePort` only) →
   `DataCloneError`; if it is a **SharedArrayBuffer** → `DataCloneError`; if it is **already in
   `memory`** (a **duplicate** in the list) → `DataCloneError`; then `memory[transferable] =
   placeholder` (so the serializer, on meeting it inside `message`, records a reference to the
   transferred object instead of copying it — a transferred buffer referenced from `message`
   arrives as the *same* transferred object; a transferred buffer **not** referenced from `message`
   is still transferred and still detached);
2. `StructuredSerializeInternal(value, false, memory)`;
3. for each `transferable` **again**: `ArrayBuffer` that is **now detached** → `DataCloneError`
   (a getter in the message could have detached it); platform object with `[[Detached]]` true →
   `DataCloneError` (a closed `MessagePort`, or one transferred earlier); `ArrayBuffer` → hand
   over the data block (+ `maxByteLength` if resizable) and **`DetachArrayBuffer`** it (WebAssembly
   `Memory.buffer` has an `[[ArrayBufferDetachKey]]` and throws `TypeError` here — den's wasm
   `Memory.buffer` is a `JS_NewArrayBuffer` alias, `ARCHITECTURE.md` §5.4; detaching it must be
   refused); platform object → its transfer steps, then `[[Detached]] = true`.

After `postMessage` returns, `buffer.byteLength === 0` and every view on it is detached.

`StructuredDeserializeWithTransfer` (§2.7.8): rebuild each transferred `ArrayBuffer` over the
**same** data block (no copy), create each `MessagePort` and run its transfer-receiving steps,
seed `memory` with them, then deserialize the main record. `[[TransferredValues]]`, in list order,
is what becomes `MessageEvent.ports` (filtered to ports).

In den, "same data block" across two QuickJS runtimes is only literally achievable by stealing the
buffer's allocation; since `JS_DetachArrayBuffer` (`quickjs.h:1055`) frees the data, the honest
first implementation **copies the bytes into a `Vec<u8>` and then detaches** (observably identical
except for O(n) cost; ceiling noted in §3.4).

#### 2.10.3 The `DataCloneError` set, spelled out

Synchronous `"DataCloneError"` `DOMException` (`code === 25`, `name === "DataCloneError"`,
`instanceof DOMException`, `instanceof Error`) for: `Symbol` anywhere in the graph; any function;
`Proxy`; `Promise`; `WeakMap`/`WeakSet`/`WeakRef`/`FinalizationRegistry`; generator/async-generator
objects; module namespaces; any platform object that is not `[Serializable]` (`MessagePort` not
listed in `transfer`, `Worker`, `EventTarget`, `Event`, `BroadcastChannel`, every den stdlib class
such as `Response`); a detached `ArrayBuffer` (in the message or in the transfer list); an
out-of-bounds view; a `SharedArrayBuffer` in the transfer list; a duplicate transfer-list entry; a
non-transferable object in the transfer list; a closed/already-transferred `MessagePort` in the
transfer list; transferring a port to itself (`postMessage` step 2). **Not** `DataCloneError`: an
exception thrown by a getter during the walk (propagates unchanged); `RangeError` on buffer
allocation failure during serialization; `TypeError` from a detach-key-protected buffer.

### 2.11 Error semantics (HTML §8.1.4.6–8.1.4.7, §10.2.5, DOM §2.9)

**"Report an exception" for a global** (§8.1.4.6 steps):

1. `errorInfo` = `{error: exception, message, filename, lineno, colno}` (last four
   implementation-defined; den derives them from `Exception::message()`/`stack()`, §1.4).
2. muted errors ("Script error.") — cross-origin only: **N/A**.
3. If the global is not already in **error reporting mode** (re-entrancy guard): set it, fire
   `error` at the global using `ErrorEvent` with `cancelable = true` and the `errorInfo`
   attributes, clear the mode; `notHandled = !canceled`. Inside the guard a *second* error (thrown
   by the `error` handler itself) is **not** re-dispatched (§10.2.5 "if the error did not occur
   while handling a previous script error") — it goes straight to step 4's fallback.
4. If `notHandled`: set `errorInfo[error] = null`; if the global is a
   `DedicatedWorkerGlobalScope`, **queue a task on the parent** to fire `error` at the associated
   `Worker` object using `ErrorEvent(cancelable: true)` with `message/filename/lineno/colno` and
   `error = null`; if *that* is not canceled, **report an exception for the parent's global with
   `omitError = true`** — recursion up the chain of nested workers until the main realm. Otherwise
   (main realm) "the user agent may report exception to a developer console" → den prints to
   stderr exactly as `src/main.rs:52-66` does today, and **keeps running** (a worker's uncaught
   error never kills the parent).
5. If the implicit port has been disentangled (parent terminated the worker) act as if the
   `Worker` had no handler: the report still climbs to the parent's global (§8.1.4.6 paragraph
   after the steps).

What counts as "uncaught runtime script error" in a worker (§10.2.5): an exception escaping the
top-level classic script, the module evaluation (a rejected module promise), an event listener
callback (DOM inner-invoke step 11.1 "report exception for listener's callback's … global"), an
`onX` handler, a timer callback, an `importScripts` exception that reaches the top level, and a
rejected promise that becomes an uncaught rejection is handled by the *other* channel below — it is
**not** an `error` event.

Cancelling: in the worker, `self.onerror = (msg, file, line, col, err) => true` or
`self.addEventListener("error", e => e.preventDefault())`; in the parent, `worker.onerror = e =>
false` **or** `e.preventDefault()` — note the asymmetry of §2.3 (`true` cancels for the global's
`onerror`, `false` cancels everywhere else).

**Unhandled promise rejections** (§8.1.4.7): the host tracker (`HostPromiseRejectionTracker`; in
rquickjs `set_host_promise_rejection_tracker`, called with `is_handled = false` on a rejection with
no handler and `is_handled = true` when a handler is attached later — the same protocol quickjs-libc
consumes at `quickjs-libc.c:4782-4806`) appends to the global's **about-to-be-notified rejected
promises list** / removes from it. "Notify about rejected promises" runs as a task **after the
current task's microtask checkpoint** — for den: after each pump iteration that ran JS **and after
that iteration has drained the job queue itself** with `Ctx::execute_pending_job` (§3.3; a
`.catch()` attached from a microtask must count as handled, and `idle()` only runs jobs after the
spawned future has yielded, so notifying straight after `dispatch` would report false positives): for
each promise still unhandled fire `unhandledrejection` (`PromiseRejectionEvent`, `cancelable`,
`promise`, `reason`) at the global; if not canceled, report to the console (stderr —
**never `exit(1)` like quickjs-libc `:4825`**); if still unhandled, remember it in the
*outstanding rejected promises weak set* so that a later `is_handled = true` fires
`rejectionhandled` (§8.1.4.7 steps 4.1.1–4.1.4 and the `rejectionhandled` half of
`HostPromiseRejectionTracker`). Unhandled rejections do **not** propagate to the parent `Worker`
object — only `error` does.

**Deserialization failure** → `messageerror` (`MessageEvent`, `data = null`, `ports = []`) at the
port's message event target (§9.4.4 post-message task step 2.4); for `BroadcastChannel` the same
with `origin = ""` (§9.5 step 10.4). Never an exception, never `error`.

**Listener exceptions during dispatch** are reported for the **listener's** global (DOM §2.9
inner-invoke step 11.1) and dispatch continues with the next listener. In den the `Function::call`
`Err` is caught in the dispatch loop, turned into "report an exception" for the current realm, and
the loop proceeds.

### 2.12 Lifetime and process exit (maintainer decision + §10.2.4)

- The parent's `runtime.idle()` must not complete while any `Worker` the parent created is alive
  (`src/app.rs:111-114`). Implementation: the realm's receive pump is a `ctx.spawn`ed future;
  `idle()` waits on it for free (`runtime/async.rs:350-356`). Its exit condition is **explicit**
  — "mailbox empty **and** `live_workers == 0`" — not "sender dropped": the realm keeps a clone of
  its own mailbox sender for same-realm `MessageChannel`/`BroadcastChannel` delivery (§3.3), so
  `recv()` would never return `None` and the process would never exit. Every worker thread sends
  `Envelope::WorkerExited` as its last act (and the parent decrements on `terminate()`), which is
  what lets the pump re-check the condition.
- A worker exits when: the closing flag is set (`close()`, `terminate()`, or parent exit) **and**
  its current task has returned. `terminate()` additionally aborts the running task.
- A worker whose script has finished, has no listeners, no timers and nothing pending still stays
  alive until closed/terminated — that is the Node/maintainer semantics (HTML's "protected"
  algebra would let a UA close it; den does not).
- Parent exit (Ctrl-C → `stop_token.cancel()`, `src/app.rs:117-126`; or main realm finishing with
  no workers): every worker is terminated (closing flag, interrupt), threads joined with a bounded
  wait, then the process exits. Nested workers are terminated transitively because their owner's
  teardown disentangles all ports and terminates its children.
- A `MessagePort` keeps nothing alive by itself; only `Worker` objects do (plus an explicit
  `BroadcastChannel` with listeners keeps *its own object* alive, not the process).

### 2.13 N/A summary (considered, deliberately not implemented)

| Spec feature | Why N/A in den | What den does instead |
|---|---|---|
| `origin` / same-origin checks / storage-key partition (§9.3, §9.5) | no origins in a CLI | `MessageEvent.origin === ""`; one partition per process |
| CSP, embedder policy, cross-origin isolation (§10.2.4) | no web security model | skip; `SharedArrayBuffer` serialization always allowed (§2.7.3 step 6.1.1 treated as true) |
| `TrustedScriptURL`, Trusted Types | no DOM | strings only |
| `credentials` (`WorkerOptions`) | no cookies/CORS | accepted, ignored |
| `SharedWorker`, `ServiceWorker`, `connect` event, `MessageEventSource` ≠ null | out of scope by decision | `source === null` |
| `WindowProxy` targets (`window.postMessage`) | no windows | — |
| Document-associated tasks, bfcache, "fully active", suspendable workers | no documents | workers never suspend |
| "permissible / protected / actively needed" GC of orphan workers (§10.2.3) | maintainer lifetime rule | live until `close()`/`terminate()` |
| muted errors / `"Script error."` (§8.1.4.6 step 3) | cross-origin only | full message always |
| `onlanguagechange`, `onoffline`, `ononline` | no OS hooks | attributes exist, never fire |
| `navigator.onLine`, `language(s)`, `userAgent` | — | P3 constants |
| `AbortSignal` in `addEventListener` options | den has no `AbortController` yet | accepted, ignored (documented) |
| Developer console reports | no console protocol | stderr |
| `WorkerLocation` from a real URL | only `file:`/`http(s):` | P3; `href` from the resolved specifier |

---

## 3. Mapping onto den / rquickjs (skeletons)

### 3.1 Where the classes live: JS shells, Rust cores

rquickjs classes cannot `extends` each other (§1.5: `JsClass::prototype` is a fresh object, the
macro has no `extends`), and `Worker : EventTarget`, `MessagePort : EventTarget`,
`BroadcastChannel : EventTarget`, `MessageEvent : Event`, `ErrorEvent : Event` are all
inheritance. The cheapest correct shape is the one `den-stdlib-wasm/src/error.rs:22-43` already
uses: **define the classes in a JS source string evaluated once at module-evaluate time**, with the
thread/serialization primitives exposed as a handful of Rust functions on a hidden internal object.

```rust
// den-stdlib-events/src/lib.rs (new crate; the DOM half, no threads)
const DEFINE_EVENTS: &str = r#"
(internal) => {
  const REPORT = internal.reportException;        // Rust: "report an exception" for this realm
  class Event {
    #flags = 0; #type; #target = null; #currentTarget = null; #phase = 0; #timeStamp = internal.now();
    constructor(type, init = {}) { /* inner event creation steps, §2.2 */ }
    /* getters, stopPropagation, stopImmediatePropagation, preventDefault, initEvent, composedPath … */
  }
  Object.defineProperty(Event.prototype, Symbol.toStringTag, { value: "Event", configurable: true });
  class EventTarget {
    #listeners = [];                               // {type, callback, capture, passive, once, removed}
    #handlers = new Map();                         // name -> {value, listener}  (§8.1.8.1 event handler map)
    addEventListener(type, callback, options) { /* flatten more + add an event listener */ }
    removeEventListener(type, callback, options) { /* … */ }
    dispatchEvent(event) { /* InvalidStateError checks; isTrusted=false; return internal.dispatch(this, event) */ }
  }
  /* MessageEvent, ErrorEvent, PromiseRejectionEvent, CustomEvent extend Event */
  internal.defineEventHandler = (proto, name) => Object.defineProperty(proto, name, { get(){…}, set(v){…}, configurable: true });
  return { Event, EventTarget, CustomEvent, MessageEvent, ErrorEvent, PromiseRejectionEvent };
}
"#;
```

The dispatch loop itself (`inner invoke`) can stay in JS; the only Rust hook it needs is
`reportException(value)` so that listener exceptions reach §2.11's reporting (and, in a worker, the
parent). `internal.now()` is just `performance.now()` — quickjs-ng installs `performance` in every
context (`JS_AddPerformance`, `quickjs.c:2551`).

**Do not keep listener state in `#private` fields.** The worker global must *be* an `EventTarget`
(§2.0) but `globalThis` is never constructed by `new EventTarget()`; it gets the prototype chain by
`Object.setPrototypeOf(globalThis, DedicatedWorkerGlobalScope.prototype)` (→ `WorkerGlobalScope.prototype`
→ `EventTarget.prototype`), and a private field added by a constructor the object never ran
through is a `TypeError` on first access. Keep the listener list and the event handler map in a
module-level `WeakMap<EventTarget, State>` that `addEventListener`/`defineEventHandler` create
lazily; the brand check for `dispatchEvent` is "has an entry or inherits from
`EventTarget.prototype`". The same `WeakMap` is the per-realm "strong table" that keeps a
`MessagePort`/`BroadcastChannel` alive while it has listeners (§2.6, §2.9) when paired with a
`Set` of live ports that `close()` removes from. Keep the constructors in context userdata (like `WebAssemblyErrors`,
`den-stdlib-wasm/src/error.rs:80-104`) so Rust can `construct` a `MessageEvent` without touching
`globalThis`.

`Worker`, `MessagePort`, `MessageChannel`, `BroadcastChannel` are then `class X extends
EventTarget` in a second source string (`den-stdlib-worker`), whose methods call Rust:

```rust
// Rust functions handed to the JS shell through `internal` (all #[rquickjs::function] or Function::new)
internal.spawnWorker(url: String, kind: WorkerKind, name: String, outsideEndpoint) -> WorkerHandle
internal.terminate(handle)
internal.portPost(endpoint, message: Value, transfer: Vec<Value>) -> Result<()>   // serialize + enqueue
internal.portStart(endpoint) / internal.portClose(endpoint)
internal.channel() -> (endpointA, endpointB)
internal.broadcast(name, message)
internal.structuredClone(value, transfer) -> Value
```

A JS-visible `MessagePort` wraps an rquickjs class `PortEndpoint` (`Arc<PortShared>`) stored in a
private field; the `[[Detached]]` slot is a boolean on the shell. This keeps every `EventTarget`
semantic in one JS file and every thread/byte-level concern in Rust.

### 3.2 Threads and runtimes

```rust
// den-stdlib-worker/src/thread.rs
pub struct WorkerHandle {
  stop:     CancellationToken,              // terminate(): interrupt + closing flag
  inbox:    mpsc::UnboundedSender<Envelope>,  // parent -> worker
  thread:   Option<std::thread::JoinHandle<()>>,
}

pub fn spawn(worker: WorkerId, script: ResolvedScript, name: String, kind: WorkerKind,
             to_parent: mpsc::UnboundedSender<Envelope>) -> WorkerHandle {
  let (inbox_tx, inbox_rx) = mpsc::unbounded_channel();
  let stop = CancellationToken::new();
  let thread = std::thread::Builder::new().name(format!("den-worker:{name}")).spawn({
    let stop = stop.clone();
    move || {
      // §1.6: must be multi_thread because den's loaders call block_in_place.
      let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
      rt.block_on(async move {
        let engine = den_core::engine::Engine::new().await;        // full stdlib + loaders, own JSRuntime
        // Engine::new already installs an interrupt handler on engine.stop_token (engine.rs:227-232);
        // chain ours into it so terminate() aborts the running script.
        let stop_child = stop.child_token();
        tokio::spawn({ let engine = engine.clone(); async move { stop_child.cancelled().await; engine.stop(); } });
        engine.context.with(|ctx| install_worker_globals(&ctx, &name, kind, inbox_rx, to_parent.clone())).await;
        run_top_level(&engine, &script, kind).await;              // classic: eval global; module: import() (promise not awaited, §2.8 step 5)
        engine.context.with(|ctx| enable_inside_port_queue(&ctx)).await;   // §10.2.4 step 2.13
        // Like App::run_until_end (src/app.rs:111-114): idle() alone never returns while a
        // setInterval future is alive, and terminate()/close() must end the thread regardless.
        stop.run_until_cancelled(engine.runtime.idle()).await;
      });
      let _ = to_parent.send(Envelope::WorkerExited { worker });  // lets the parent pump re-check its exit rule (§2.12)
    }
  }).expect("spawn worker thread");
  WorkerHandle { stop, inbox: inbox_tx, thread: Some(thread) }
}
```

Points that are easy to get wrong:

- **Timers are not bound to the engine's stop token.** `setTimeout`/`setInterval` are `ctx.spawn`ed
  futures cancelled only by their own `clearX` token (`den-stdlib-timer/src/lib.rs:28-37,57-68`), so
  `idle()` never completes while an interval exists. `run_until_cancelled(idle())` drops the idle
  future (releasing the runtime lock) and then the `Engine`, which drops every pending spawned
  future — that is the "clear the map of active timers" teardown step (§2.8 step 7).
- **`close()` must not cancel `stop` synchronously** — `stop` also drives the interrupt handler, and
  code after `close()` in the same task must keep running (§2.8). `close()` only sets
  `RealmState.closing` (userdata); the pump checks it after the current dispatch returns (and
  `run_top_level` checks it after the script returns) and *then* cancels `stop`.

- `Engine::new` is `async` and must run **inside** the worker's tokio runtime (it awaits
  `runtime.set_max_stack_size`, `set_loader`, `AsyncContext::full`: `den-core/src/engine.rs:40,222,234`).
- `terminate()` = `stop.cancel()` → the interrupt handler returns `true` on the next interrupt
  check, the running script unwinds with an uncatchable exception, and `run_top_level`/the pump see
  `Err`; also drop `inbox` so the pump's `recv()` yields `None`.
- `close()` from inside = set the closing flag in the worker's userdata and drop the inbox receiver
  after the current task; the pump exits, `idle()` returns, the thread ends.
- `JS_SetCanBlock(rt, true)` (`quickjs.h:1143`) is required for `Atomics.wait` inside a worker;
  rquickjs has no wrapper, so it is one `unsafe` call on `runtime.inner` — defer to the SAB phase.
- Do **not** copy quickjs-libc's "cannot create a worker inside a worker"
  (`quickjs-libc.c:4162-4163`); nested workers only need the parent-side registry to be per realm
  (stored in context userdata) rather than a process global.

### 3.3 The two pumps (port message queues as futures)

Every realm owns one mailbox `UnboundedReceiver<Envelope>`; an `Envelope` names the target port id
inside that realm. A `ctx.spawn`ed pump (the pattern of `den-stdlib-timer/src/lib.rs:28-37`)
drains it:

```rust
pub enum Envelope {
  Message { port: PortId, payload: SerializedMessage },
  PortClosed { port: PortId },                       // peer disentangled -> fire "close"
  WorkerError { worker: WorkerId, info: ErrorInfo },  // §2.11 step 4, error = null
  WorkerLoadFailed { worker: WorkerId },              // plain "error" Event at the Worker
}

ctx.spawn(async move {
  loop {
    // Exit rule (§2.12): nothing queued and no live worker. `recv()` alone would never end because
    // the realm holds a clone of its own sender for same-realm delivery.
    let envelope = match mailbox.try_recv() {
      Ok(envelope) => envelope,
      Err(TryRecvError::Empty) if registry.live_workers() == 0 => break,
      Err(TryRecvError::Empty) => match mailbox.recv().await { Some(envelope) => envelope, None => break },
      Err(TryRecvError::Disconnected) => break,
    };
    match envelope {
      Envelope::Message { port, payload } => {
        let port_state = registry.port(port);
        port_state.queue.push_back(payload);            // the port message queue
        if port_state.enabled { port_state.flush(&ctx) } // else wait for start()/onmessage (§2.6)
      }
      Envelope::WorkerExited { worker } => registry.worker_exited(worker),
      /* … */
    }
    // "Perform a microtask checkpoint" for the task just run: idle() only drains jobs after this
    // future yields, so drain here, report job exceptions (idle() would only println! them), then
    // notify about rejected promises (§2.11) — in that order.
    while ctx.execute_pending_job() {
      if ctx.has_exception() { report_exception(&ctx, ctx.catch(), false) }
    }
    notify_rejected_promises(&ctx);
    if realm.closing.get() { realm.stop.cancel() }      // close(): end the thread after the current task (§3.2)
  }
  registry.mark_pump_stopped();
});
```

The pump is **spawned lazily** — by `spawnWorker`, by `portPost`/`broadcast` whenever the target
lives in this realm, and by the worker thread before `run_top_level` — guarded by a
`pump_running` flag in `RealmState`; a realm that never creates a worker or posts a message spawns
nothing and exits as today. Spawning from inside JS is fine: `ctx.spawn` only pushes onto the
schedular (`runtime/spawner.rs:28-34`) and `idle()` picks the new future up on its next poll.

`flush` pops in FIFO order and, per message, runs `StructuredDeserializeWithTransfer` in this realm,
constructs a `MessageEvent` (or `messageerror` on failure) via the cached constructor, and
dispatches it at the port's message event target. `start()` sets `enabled` and calls `flush`; the
first `onmessage` assignment does the same; `terminate()` on the parent clears the outside port's
`queue` (§2.7 terminate step 4).

Because the pump is a spawned future, `runtime.idle()` (`src/app.rs:111-114`) waits on it, which
gives the process-lifetime rule for free; when the sender side is dropped (`terminate()`, thread
exit) the loop ends and `idle()` can complete.

Same-realm channels (`new MessageChannel()` used without transfer) go through the same mailbox —
the realm sends to itself — which is what makes delivery a task rather than a synchronous call.

### 3.4 Serialization: pre-validate now, own serializer later

Phase 1 (P0/P1): `JS_WriteObject2(ctx, &mut size, value, JS_WRITE_OBJ_REFERENCE, &mut sab_tab)`
(`quickjs.h:1221`, binding `bindings/*.rs` `JS_WriteObject2`) **after** a Rust walk that enforces
the spec's reject set and prepares transfers:

```rust
pub struct SerializedMessage {
  blob:      Vec<u8>,                 // JS_WriteObject2 output, re-copied into a Vec (the quickjs buffer is js_malloc'd)
  buffers:   Vec<TransferredBuffer>,  // { bytes: Vec<u8>, max_byte_length: Option<usize>, slot: u32 }
  ports:     Vec<PortDataHolder>,     // { remote: Option<PortAddress>, queued: VecDeque<SerializedMessage> }
}

fn serialize_with_transfer(ctx: &Ctx<'_>, value: Value<'_>, transfer: &[Value<'_>]) -> Result<SerializedMessage> {
  let mut memory = IdentitySet::new();
  for item in transfer { validate_transferable(ctx, item, &mut memory)? }  // §2.7.7 step 2: non-transferable, SAB, duplicate
  walk_reject_set(ctx, &value)?;    // §2.10.3: Symbol, callable, Proxy, Promise, Weak*, platform objects, detached, OOB views, accessor props
  // Transferred ArrayBuffers are swapped for a placeholder {__denTransfer: slot} object in a *shadow* graph so the
  // quickjs writer never sees them; identity inside `memory` keeps "transferred buffer referenced twice" correct.
  let blob = write_object(ctx, shadow_graph)?;
  let buffers = transfer.iter().filter_map(ArrayBuffer::from_value).map(|mut buffer| {
    let bytes = buffer.as_bytes().ok_or_else(|| data_clone_error(ctx, "detached"))?.to_vec();
    buffer.detach();                                                   // rquickjs-core array_buffer.rs:259
    Ok(TransferredBuffer { bytes, .. })
  }).collect::<Result<_>>()?;
  Ok(SerializedMessage { blob, buffers, ports: transfer_ports(ctx, transfer)? })
}
```

Known limits of Phase 1 (each is a row in §1.1 and a test in §4): `Error` objects, `DataView`,
accessor properties on plain objects and array holes are **rejected with `DataCloneError`** rather
than cloned (the writer cannot express them); symbol-keyed own properties must be stripped by the
shadow-graph pass (the writer would copy them as fresh symbols, §1.1); `SharedArrayBuffer` is
rejected until §1.2's hooks exist.

FFI details for the byte-level core (both directions are `unsafe` calls on `ctx.as_raw()`):

- `JS_WriteObject2(ctx, &mut size, value, flags, &mut sab_tab) -> *mut u8` returns a `js_malloc`'d
  buffer (`quickjs.c:38431-38475`; `NULL` with a pending exception on failure). Copy it into the
  `Vec<u8>` and release it with `qjs::js_free(ctx, buf)`; when `sab_tab` was passed, free
  `sab_tab.tab` the same way (it is only non-empty once §1.2 lands).
- `JS_ReadObject2(ctx, buf, buf_len, flags, &mut sab_tab) -> JSValue` (`quickjs.c:39729-39730`)
  with `JS_READ_OBJ_REFERENCE` (never `JS_READ_OBJ_BYTECODE`) in the **target** realm; a
  `JS_EXCEPTION` result is the `messageerror` path (§2.11).
- Transferred bytes arrive zero-copy: `ArrayBuffer::new(ctx, bytes_vec)` hands the `Vec`'s
  allocation to QuickJS with a free function (`value/array_buffer.rs:91-118`), so a transfer costs
  exactly one copy (sender side, §2.10.2), not two. Everything else — wrapper objects, `Date`, `RegExp`, typed arrays with shared buffers,
`Map`/`Set`, cycles, prototype dropping — is correct out of the box.

Phase 2 (P2): replace `JS_WriteObject2` with a Rust serializer producing the §2.10.1 records
directly (`enum Record { Primitive(..), Object(Vec<(String, RecordId)>), Array { length, props },
Error { name, message, stack, cause }, ArrayBuffer(..), View { ctor, .. }, Map(..), Set(..), .. }`
in an arena with a `memory: HashMap<ObjectPtr, RecordId>`), and a deserializer that builds values
through rquickjs (`Object::new`, `Array`, `ArrayBuffer::new`, `TypedArray::from_arraybuffer`
(`typed_array.rs:232`), `JS_NewDate` (`quickjs.h:949`), `RegExp` via the global constructor, `Error`
prototypes via the globals). The reject-set walk and transfer handling are unchanged; only the
byte-level core moves. Reading the graph: `JS_GetOwnPropertyNames` with
`JS_GPN_STRING_MASK | JS_GPN_ENUM_ONLY` (`quickjs.h:979-987`) gives exactly "enumerable own
String-keyed" in spec order; `JS_IsError` (`:820`), `JS_IsDate` (`:950`), `JS_IsRegExp` (`:925`),
`JS_IsMap` (`:926`), `JS_IsProxy` (`:943`), `JS_IsPromise` (`:1111`), `JS_IsDataView` (`:931`),
`JS_GetTypedArrayType` (`:1089`), `JS_GetClassID` (`:690`) are the classifiers.

### 3.5 Error reporting plumbing

```rust
pub struct ErrorInfo { message: String, filename: String, lineno: u32, colno: u32 }

impl ErrorInfo {
  /// "extract error information" (§8.1.4.6): message from Exception::message(), position parsed
  /// from the first "    at …(file:line:col)" frame of Exception::stack() (§1.4), else 0/"".
  fn extract(ctx: &Ctx<'_>, thrown: &Value<'_>) -> Self { .. }
}

/// "report an exception" for the current realm; `in_error_reporting_mode` is the re-entrancy guard.
pub fn report_exception(ctx: &Ctx<'_>, thrown: Value<'_>, omit_error: bool) {
  let realm = ctx.userdata::<RealmState>().unwrap();
  let info = ErrorInfo::extract(ctx, &thrown);
  let mut not_handled = true;
  if !realm.in_error_reporting_mode.replace(true) {
    not_handled = !fire_error_event(ctx, &realm.global_target, &info, if omit_error { None } else { Some(&thrown) });
    realm.in_error_reporting_mode.set(false);
  }
  if not_handled {
    match &realm.kind {
      RealmKind::Worker { to_parent, worker_id } => { let _ = to_parent.send(Envelope::WorkerError { worker: *worker_id, info }); }
      RealmKind::Main => eprintln!("{}", info.message),   // what src/main.rs:52-66 does today
    }
  }
}
```

The parent-side pump turns `Envelope::WorkerError` into an `ErrorEvent` at the `Worker` object and,
if not canceled, calls `report_exception(ctx, null, true)` for its own realm — which recurses up
through nested workers until stderr.

Hook points that must call `report_exception`: the `Err` of `run_top_level`; every `Function::call`
in the dispatch loop and in the timer crate (`den-stdlib-timer/src/lib.rs:34,65` currently
`let _ = func.call(..)` — **swallows** the error; route it through `report_exception` once the
events crate exists); the `unhandledrejection` path (`set_host_promise_rejection_tracker`,
`runtime/base.rs:77`).

---

## 4. Prioritised build order

Each step is independently testable with `Engine::eval` in `den-core/tests/` (the style of
`den-core/tests/stdlib.rs:12-26`) plus unit tests inside the crate.

**P0 — `den-stdlib-events` (no threads): `EventTarget`, `Event`, `CustomEvent`, event handler
attributes, `MessageEvent`, `ErrorEvent`, `PromiseRejectionEvent`, `reportError`.**
Tests: listener order; `(type, callback, capture)` dedupe; `once` removed before call; removal
during dispatch honoured; `stopImmediatePropagation`; `preventDefault` only when cancelable and not
passive; `dispatchEvent` re-entrancy → `InvalidStateError`; `isTrusted` false from `dispatchEvent`;
`onX` slot position rules ("ONE TWO THREE FOUR" example, §8.1.8.1); `handleEvent` objects;
`onerror` five-argument special call on a global-like target; listener exception does not stop the
others and is reported; `Object.prototype.toString` tags.

**P0 — `structuredClone` with transferable `ArrayBuffer`s (Phase 1 serializer, §3.4) and a cached
`DOMException` constructor (§1.3).**
Tests: every row of §2.10.1 that Phase 1 supports; the full §2.10.3 reject set with
`e instanceof DOMException && e.name === "DataCloneError" && e.code === 25`; cycles and shared
references preserved; symbol keys dropped; prototype dropped; `Map`/`Set` order; typed arrays
sharing one buffer → one cloned buffer; transfer detaches (`byteLength === 0`), duplicates and
detached inputs throw **before** anything is detached; transferred-but-unreferenced buffer still
detached; wasm `Memory.buffer` refused. Phase-1 limits (`Error`, `DataView`, accessors, holes, SAB)
asserted as `DataCloneError` with `// ponytail:` markers pointing at Phase 2.

**P1 — `MessageChannel` / `MessagePort` within one realm.**
Tests: queue disabled until `start()`/first `onmessage` (messages posted before are delivered
after, in order); `addEventListener` alone does not start; delivery is a task (not re-entrant);
`close()` fires `close` on the peer and silences the closer; posting a port to itself →
`DataCloneError`; transferring the target port dooms the message silently; transferring a port
moves its queued messages and leaves the new port disabled; `MessageEvent.ports` frozen and
ordered; `messageerror` on a poisoned payload.

**P1 — `Worker` + `DedicatedWorkerGlobalScope` (classic and module), `terminate()`, `close()`,
`postMessage` both ways, `importScripts`, process lifetime.**
Tests: `new Worker` returns before the script runs; messages posted before the worker's top-level
finished are delivered after it; FIFO both directions; `self === globalThis`, `name`;
`close()` after `postMessage` still delivers; `terminate()` stops `while(true){}` within the
interrupt period and drops undispatched messages; `postMessage` after `terminate()` is a silent
no-op; bad URL → `SyntaxError` DOMException synchronously; missing file → plain `error` `Event` at
the `Worker`; module worker with top-level `await`; `importScripts` in a module worker →
`TypeError`; `importScripts` runs in order and rethrows; nested workers; `Engine::run_file` of a
script that spawns a worker does not exit until the worker closes; Ctrl-C terminates workers;
`terminate()` ends a worker that owns a live `setInterval` (thread joins, §3.2); `close()` called
from inside a `setInterval` callback ends the worker; a module worker that sets `onmessage` only
after a top-level `await` misses a message posted during the await (§2.8 step 5); a process whose
script only uses a same-realm `MessageChannel` still exits after delivering queued messages
(§2.12 exit rule); a realm that never touches workers/channels spawns no pump. All engine tests are
`#[tokio::test(flavor = "multi_thread")]` (§1.6).

**P1 — Error semantics (§2.11).**
Tests: uncaught throw in worker top-level → `ErrorEvent` at `self` (cancelable, five-arg
`onerror`) → if not canceled, `ErrorEvent` at the parent's `Worker` with `error === null` and the
same `message`/`filename`/`lineno` → if not canceled, stderr line and the parent keeps running;
`worker.onerror = () => false` cancels; `e.preventDefault()` cancels; error thrown inside the
worker's `error` handler is not re-dispatched; `unhandledrejection` fires with `promise`/`reason`,
`rejectionhandled` fires on late `.catch`; unhandled rejection does **not** produce a parent
`error` event; listener exceptions inside `onmessage` are reported the same way.

**P2 — `BroadcastChannel`.**
Tests: sender excluded; same-name different objects in one realm receive; creation-order delivery
within a realm; each destination gets an independent clone (mutating one does not affect another);
`close()` then `postMessage` → `InvalidStateError`; a closing worker's channel drops sends; cross-
thread delivery between two workers; `messageerror` path.

**P2 — Phase 2 serializer (§3.4): `Error` (+ `cause`), `DataView`, accessor properties invoked,
array holes, `lastIndex` reset, `AggregateError`, resizable buffers preserved, `DOMException`
as `[Serializable]`.**

**P3 — `SharedArrayBuffer` sharing (§1.2: refcounted `JSSharedArrayBufferFunctions` on every
runtime, `JS_SetCanBlock`, `Atomics.wait` in workers), `WorkerLocation`, `WorkerNavigator`,
`AbortSignal` support in `addEventListener` once `AbortController` exists.**

---

## 5. Open questions for the maintainer

1. **URL base for `new Worker("x.js")`.** The spec resolves against the realm's API base URL
   (§2.7 step 2), which den does not track today. Proposal: a `base_url` in context userdata set
   by `Engine::run_file` (the entry file), by the worker thread (the worker's resolved URL) and
   defaulting to CWD for REPL/`eval`, matching `FileResolver::with_path("./")`
   (`den-core/src/engine.rs:94`). Confirm that "relative to the entry file, not the importing
   module" is acceptable for `new Worker()` called from a nested module.
2. **Classic scripts and the transpiler.** Both loaders force `IsModule::Bool(true)`
   (`den-core/src/loader/mmap_script.rs:78`). A `"classic"` worker script (the default) must be
   evaluated as a global script; TS-in-classic-worker needs a `transpile(.., IsModule::Bool(false), ..)`
   path that does not exist yet. Is a classic TS worker a goal, or is classic JS-only acceptable?
3. **Where does the main realm's "report an exception" print?** Today `src/main.rs:52-66` prints
   the *caught* exception after `run_file` returns. With `reportError`/worker errors arriving at
   any time, a single `report_exception` sink (stderr via `tracing::error!`?) should replace both
   sites. Confirm stderr vs. `tracing`.
4. **Thread join on exit.** Bounded wait (e.g. 1 s after `stop_token.cancel()`) then detach, or
   unbounded join? `Atomics.wait` and a blocking `reqwest` inside `importScripts` are the cases
   that cannot be interrupted by the QuickJS interrupt handler.
5. **Transfer = copy (§2.10.2) is O(n).** Acceptable ceiling, or is zero-copy buffer stealing
   (custom `JS_NewArrayBuffer` with a free-func that hands the allocation to the receiver) wanted
   from the start? Both runtimes share the Rust global allocator (`rust-alloc`), so stealing is
   feasible but needs a `free_func` protocol on both sides.
6. **`den-stdlib-timer` swallows callback errors** (`let _ = func.call(..)`,
   `den-stdlib-timer/src/lib.rs:34,65`). Routing them through `report_exception` changes visible
   behaviour for the main realm (errors become printed). Intended?

---

## 6. Verification log

All library/engine claims were checked by reading the files named; nothing in §1 or §3 is from
memory. Spec text was read from the 21 August 2026 HTML snapshot and the 20 August 2026 DOM
snapshot after stripping markup; section numbers are the specs' own.

| Claim | Verified at |
|---|---|
| `JS_WriteObject2` flags, SAB table, reference table, atom strings | `quickjs.c:38431-38488`, `:38283-38292`, `:38385-38425`; `quickjs.h:1214-1221` ✓ |
| Writer type dispatch (Array/Object/ArrayBuffer/SAB/RegExp/Date/wrappers/Map/Set/typed array/default TypeError) | `quickjs.c:38301-38350` ✓ |
| Symbol is serialized, not rejected | `quickjs.c:38359-38372`, reader `:39627-39638` ✓ |
| Accessor property → `TypeError "only value properties are supported"`; enumerable-only | `quickjs.c:38137-38143` ✓ |
| Array holes filled (`JS_GetPropertyUint32` over `length`) | `quickjs.c:38080-38110`; reader defines every index `:39278-39286` ✓ |
| Detached buffer → `TypeError` | `quickjs.c:38175-38180` ✓ |
| `DataView` outside the typed-array class range | `quickjs.c:151-163`, `:201`, `is_typed_array` `:58395` ✓ |
| `Error`/`Proxy`/`Promise`/`WeakMap`/`WeakRef` class ids hit `default:` | `quickjs.c:132,167,180,181,191`, `:38346-38349` ✓ |
| SAB read requires `sab_dup`; rquickjs never sets `sab_funcs`; fallback `js_mallocz` | `quickjs.c:39589-39592`, `:57775-57779`; grep over `rquickjs-core-0.12.2/src` → 0 ✓ |
| `DOMException` registered by `JS_NewContext` → `JS_AddIntrinsicAToB` → `JS_AddIntrinsicDOMException`; names/codes; `JS_ThrowDOMException` | `quickjs.c:2533-2556`, `:63339-63342`, `:62150-62176`, `:62296-62326`; `rquickjs-core …/context/async.rs:161-163` ✓ |
| Errors carry `stack`/`cause`, no `lineNumber` | `quickjs.c:41842-41844` (function proto only), `:41942-41949` ✓ |
| quickjs-libc worker: message/pipe structs, thread body, ctor, postMessage, onmessage, rejection tracker + `exit(1)` | `quickjs-libc.c:150-179`, `:4060-4115`, `:4151-4224`, `:4226-4296`, `:4298-4349`, `:4782-4826` ✓ |
| `AsyncRuntime: Send + Sync` (parallel); `InterruptHandler: Send`; `RejectionTracker` signature | `runtime/async.rs:85-97`; `runtime.rs:44,52`; `runtime/base.rs:77-93` ✓ |
| `Ctx::spawn` bound `'js` only; `idle()` semantics and job-error printing; `drive()` | `context/ctx.rs:418-423`; `runtime/async.rs:313-360,365` ✓ |
| `Function::new` + `Fn + 'js`; `Class::instance_proto`/`define`; macro has no `extends`; attrs | `value/function.rs:47-58`; `function/into_func.rs:15-33`; `class.rs:100-105,242,273`; `rquickjs-macro …/class.rs:19-23`, `methods/method.rs:18-27`, `fields.rs:15-21` ✓ |
| `ArrayBuffer::{new,new_copy,as_bytes,detach}`; `TypedArray::{arraybuffer,from_arraybuffer}` | `value/array_buffer.rs:91,123,242,259`; `value/typed_array.rs:219,232` ✓ |
| `Exception::{message,stack}`, throw helpers, `Error::Exception` | `value/exception.rs:73,83,105-191`; `result.rs:63,209` ✓ |
| `Persistent::{save,restore}` same-runtime rule | `persistent.rs:88-110` ✓ |
| `EvalOptions` fields; `script_or_module_name` | `context/ctx.rs:29-41,452-463` ✓ |
| Bindings exist for `JS_WriteObject2`, `JS_ReadObject2`, `JSSABTab`, `JSSharedArrayBufferFunctions`, `JS_SetSharedArrayBufferFunctions`, `JS_ThrowDOMException` (variadic), `JS_SetCanBlock`, `JS_DetachArrayBuffer`, classifiers | `rquickjs-sys-0.12.2/src/bindings/aarch64-apple-darwin.rs:1692,1709,1672,1441,1472,1555,1376,…`; `x86_64-unknown-linux-gnu.rs:886-891` ✓ |
| `block_in_place` panics off the multi-thread runtime; den loaders use it | `tokio-1.53.1/src/runtime/scheduler/multi_thread/worker.rs:434`; `den-core/src/loader/http.rs:116`, `mmap_script.rs:92` ✓ |
| tokio builder / mpsc / Notify line refs | `runtime/builder.rs:261,276,376,1072`; `sync/mpsc/unbounded.rs:95`; `sync/notify.rs:657,740` ✓ |
| den: `Engine::new` structure, interrupt handler on `stop_token`, `run_file` TLA import trick; `App::run_until_end` drive+idle; Ctrl-C; error printing; timer `ctx.spawn` + swallowed errors; wasm JS-land error classes + userdata; fetch class pattern | `den-core/src/engine.rs:35-306,308-338`; `src/app.rs:99-126`; `src/main.rs:43-73`; `den-stdlib-timer/src/lib.rs:28-37,57-68`; `den-stdlib-wasm/src/error.rs:22-43,80-104`; `den-stdlib-whatwg-fetch/src/lib.rs:8-26,85-88,181-187` ✓ |
| No existing `EventTarget`/`structuredClone`/`DOMException` use in den | grep over the workspace (excluding `target/` and this doc) → 0 hits ✓ |

Not verified / caveats:

- The `bindings/*.rs` line numbers are from the `aarch64-apple-darwin` file except where the
  `x86_64-unknown-linux-gnu` file is named; the symbol set is identical across targets but line
  numbers drift by a few lines.
- ~~The writer's treatment of **symbol-keyed** own properties was not traced.~~ Resolved in the
  second pass (§1.1, below): they are emitted and re-created as fresh symbols.
- `JS_GetOwnPropertyNames` ordering (integer keys first) is the ES `OrdinaryOwnPropertyKeys`
  guarantee; not separately traced in `quickjs.c`.
- The "Phase 1 rejects `Error`/`DataView`/accessors/holes" limitation is a design choice here, not
  a measured behaviour; the engine facts behind it are verified above.

## Verification log — second pass (completeness review, 2026-08-22)

Independent re-read of the claims that would hurt most if wrong, against the same local sources.
Corrections were applied in place; this table records what was checked and what changed.

| Claim | Result | Evidence |
|---|---|---|
| `idle()` waits on spawned futures and returns on `SchedularPoll::Empty` | Confirmed — **and it holds the runtime mutex for its whole life** (added to §1.5); jobs are only drained between spawner polls (→ §2.11/§3.3 microtask-checkpoint fix) | `rquickjs-core-0.12.2/src/runtime/async.rs:314-360` |
| `Ctx::spawn` requires only `'js` | Confirmed | `context/ctx.rs:418-423` |
| `Ctx::execute_pending_job`, `catch`, `has_exception`, `throw` | Confirmed; added to §1.5 and used by the pump | `context/ctx.rs:257-277,404-409` |
| Userdata API bound is `JsLifetime<'js>` | Confirmed; added to §1.5 | `context/ctx.rs:480-508`; derive used at `den-stdlib-core/src/cancellation.rs:6-15` |
| `AsyncRuntime: Send + Sync` (parallel); `InterruptHandler`/`RejectionTracker` aliases; `is_handled` is the `bool` | Confirmed | `runtime/async.rs:85-97`; `runtime.rs:41-52`; `runtime/raw.rs:357-369` |
| Interrupt returning `true` raises an uncatchable exception that skips `catch`/`finally` | Confirmed; latency (`JS_INTERRUPT_COUNTER_INIT 10000`, counter reset per poll) and the skipped-handler behaviour added to §2.7 | `quickjs.c:479,8215-8238,20328-20349`; `runtime/base.rs:86-93` |
| `JS_WriteObject2` handles Map/Set; `Error`/`DataView`/`Proxy`/`Promise` hit `default:` `TypeError`; detached buffer `TypeError`; Symbol serialized; accessor `TypeError`; holes filled | Confirmed | `quickjs.c:38080-38157,38160-38215,38218-38380` |
| Symbol-keyed own properties (previously "unverified") | **Emitted** — shape walk filters on `JS_PROP_ENUMERABLE` only; atom type is written and a fresh symbol re-created on read. §1.1 row and §3.4 corrected | `quickjs.c:38123-38143,38385-38409,39649-39700,39627-39638` |
| SAB read requires `sab_funcs.sab_dup` | Confirmed | `quickjs.c:39589-39592` |
| `JS_NewContext` → `JS_AddIntrinsicAToB` → `JS_AddIntrinsicDOMException`; `JS_ThrowDOMException` variadic; `performance` installed | Confirmed (`JS_AddPerformance` at `:2551` is why `performance.now()` exists, §3.1) | `quickjs.c:2533-2556,62300,62329,63338-63342`; `quickjs.h:542,842`; `bindings/x86_64-unknown-linux-gnu.rs:886` |
| `JS_WriteObject2` / `JS_ReadObject2` signatures and ownership of the output buffer | Confirmed; FFI notes added to §3.4 | `quickjs.c:38431-38490,39729-39730`; bindings `:1691,1708,1671` |
| `JS_SetCanBlock` binding; quickjs-libc call site | Binding confirmed (`:1554`); libc line corrected `:4092` → `:4096` | `quickjs.h:1143`; `quickjs-libc.c:4096` |
| quickjs-libc refuses nested workers; rejection tracker `exit(1)` | Confirmed | `quickjs-libc.c:4160-4164,4822-4827` |
| `block_in_place` panics on current-thread; works from a multi-thread `block_on` caller | Confirmed, with the mechanism (`allow_block_in_place`, `exit_runtime`) added to §1.6 | `tokio-1.53.1/src/runtime/scheduler/multi_thread/worker.rs:413-434,505`; `multi_thread/mod.rs:91`; `current_thread/mod.rs:206`; `runtime/handle.rs:373`; `src/main.rs:24` |
| `Ctx::script_or_module_name(1)` as the base URL for `new Worker` | **Wrong for the JS-shell design** — `JS_GetScriptOrModuleName` returns the bytecode function's `filename` `n` raw frames up, so frame 1 is the shell constructor; the spec's base is realm-wide anyway. §2.7 step 2 and §5 Q1 rewritten | `quickjs.c:30890-30912`; `context/ctx.rs:452-463` |
| Module worker with top-level `await` delays port-queue enabling | **Wrong** — "run a module script" returns the evaluation promise without awaiting it (HTML §8.1.4.3). §2.8 step 5 corrected, test added to §4 | spec text |
| Process lifetime: pump ends "when the sender is dropped" | **Wrong as written** — the realm keeps its own sender for same-realm delivery, so `recv()` never yields `None`. Explicit exit rule (mailbox empty ∧ no live workers), lazy spawning and `Envelope::WorkerExited` added to §2.12/§3.3 | `tokio::sync::mpsc` semantics; §3.3 design |
| Worker thread `engine.runtime.idle().await` returns after `terminate()` / `close()` | **Wrong** — den timers are `ctx.spawn`ed futures cancelled only by their own tokens, so `idle()` never returns while a `setInterval` exists. §3.2 skeleton now uses `stop.run_until_cancelled(idle())` as `App` does, with `close()` deferred to end-of-task | `den-stdlib-timer/src/lib.rs:28-37,57-68`; `src/app.rs:99-115`; `den-core/src/engine.rs:382` |
| `notify_rejected_promises` straight after dispatch | **Wrong timing** — runs before the microtask checkpoint; fixed in §2.11/§3.3 by draining `execute_pending_job` first | `runtime/async.rs:320-356`; `context/ctx.rs:404` |
| `#private` listener fields on `EventTarget` work for the worker global | **Wrong** — `globalThis` is never constructed through the class; §3.1 now mandates a `WeakMap` side table | ES semantics of private fields |
| Classic worker script can be loaded through den's loaders | **Missing** — loaders only produce `Module`s; classic path (read bytes + `eval_with_options`) added to §2.8 step 2 | `den-core/src/loader/mmap_script.rs:84-92`; `loader/http.rs:116` |
| `Engine::new` is parameterless `async`, `Engine: Clone`, `stop()` is lock-free | Confirmed | `den-core/src/engine.rs:24-35,225-232,382-387` |
| `ArrayBuffer::new(ctx, Vec)` is zero-copy into QuickJS | Confirmed; noted in §3.4 | `value/array_buffer.rs:91-118` |
| `Module::import` returns the evaluation `Promise` | Confirmed | `value/module.rs:426` |

Still unverified (no local copy, not fetched in this pass): the exact wording of the HTML snapshot
for §8.1.4.3 "run a module script" steps 6–9 and §10.2.4 steps 2.10–2.13 was checked against the
reviewer's knowledge of the living standard, not re-downloaded; the conclusion (ports are enabled
as soon as evaluation *starts*) matches shipping browsers.
