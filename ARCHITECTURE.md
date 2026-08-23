# den — architecture

A description of the tree as it stands, not of where it is going. Every claim
here is checkable against a file in this repository; where a mechanism is
subtle the owning file is named.

Background research (written during the big dependency upgrade, versioned
snapshots rather than living docs) lives in [`docs/research/`](docs/research/).

## 1. Crate graph

```
den (src/)                      binary: CLI, REPL, ctrl-c, tracing subscriber
 └── den-core                   the embeddable runtime: Engine, loaders, resolvers
      ├── den-transpiler-oxc    TS/JSX → JS  (optional, `transpile`)
      ├── den-stdlib-console    globalThis.console → tracing
      ├── den-stdlib-core       atob/btoa/gc, CancellationToken
      ├── den-stdlib-crypto     crypto.getRandomValues / randomUUID
      ├── den-stdlib-fs         den:fs
      ├── den-stdlib-networking den:networking (TCP sockets)
      ├── den-stdlib-sqlite     den:sqlite (rusqlite, bundled)
      ├── den-stdlib-text       TextEncoder / TextDecoder
      ├── den-stdlib-timer      setTimeout / setInterval
      ├── den-stdlib-whatwg-fetch  fetch() + Response (reqwest)
      ├── den-stdlib-wasm       the WebAssembly JS API (optional, one backend)
      └── den-stdlib-worker     Web Workers: Worker, MessageChannel/MessagePort,
                                BroadcastChannel, EventTarget, AbortController,
                                performance, structuredClone
```

`den-core` owns everything about *how* JavaScript gets in: the `Engine`
(runtime + context + stop token), module resolution, module loading and the
transpile hook. Each `den-stdlib-*` crate owns one JS-visible API and knows
nothing about the loader chain. The binary owns only process concerns.

## 2. How one Rust module becomes both `den:x` and a global

`den-core/src/engine.rs` wires the same `#[rquickjs::module]` type into three
places, and each place buys a different thing:

1. **`BuiltinResolver::with_module("den:x")`** (`Engine::new`) — makes the
   specifier `den:x` resolvable, so `import … from "den:x"` is not a
   resolution error.
2. **`ModuleLoader::with_module("den:x", den_stdlib_x::js_x)`** (`Engine::new`)
   — supplies the module's definition when that specifier is actually
   imported.
3. **`Module::evaluate_def::<den_stdlib_x::js_x, _>(ctx, "den:x")`**
   (`Engine::new`, in the `context.with` block) — evaluates the module *once
   at context creation*, which is what runs its `#[qjs(evaluate)]` hook
   eagerly.

The globals come out of step 3, not out of some separate registration: a
module's `evaluate` hook receives the `Ctx` and writes to `ctx.globals()`
directly. `den-stdlib-console` sets `console`; `den-stdlib-core` exports
`atob`/`btoa`/`gc` *and* sets the same three as globals; `den-stdlib-wasm`
builds the whole `WebAssembly` namespace object and installs it. So
"importable module" and
"ambient global" are the same code with two entry points, and the cfg blocks in
all three lists must stay in lockstep — a module registered in one list and not
the others is the failure mode to watch for.

`den:fs`, `den:networking` and `den:sqlite` are import-only: they appear in the
resolver and loader lists but are not `evaluate_def`'d, so they contribute no
globals. The seven that are — `den:console`, `den:core`, `den:text`,
`den:timer`, `den:whatwg-fetch`, `den:crypto`, `den:wasm` — are exactly the
ones whose APIs a script expects to find without importing anything.

## 3. Resolver and loader chain

rquickjs takes one resolver tuple and one loader tuple; each is tried in order.

**Resolvers** (`den-core/src/engine.rs`, `den-core/src/resolver/http.rs`):

1. `BuiltinResolver` — the `den:*` specifiers.
2. `HttpResolver` — joins the specifier against the importing module's URL,
   then applies an optional allowlist and an optional denylist (both
   `matchit::Router`s), then accepts only `http` / `https`. Allow is checked
   before deny. Both are `pub(crate)` and `Engine::new` uses
   `HttpResolver::default()`, so as wired today neither list is populated and
   every `http`/`https` specifier resolves; the mechanism exists, the policy
   does not.
3. `FileResolver` — `./` plus one pattern per enabled extension: `.js`/`.mjs`
   always, `.jsx`/`.mjsx` under `react`, `.ts` under `typescript`, `.tsx` under
   both.

**Loaders** (`den-core/src/loader/`):

1. `BuiltinLoader`.
2. `ModuleLoader` — the `den:*` native modules.
3. `HttpLoader` (`loader/http.rs`) — `reqwest::get`, then MIME sniffing. The
   sniff is a *gate*, not a hint: no `Content-Type` is an error, and only
   `text/*` or `application/*` with subtype `javascript` (or `typescript`, when
   that feature is on) is accepted. The sniffed extension is what the
   transpiler is then told the source is.
4. `MmapScriptLoader` (`loader/mod.rs`) — checks the extension against its
   registered list, then memory-maps the file with `fmmap`. The mapping
   constructor is `unsafe` because an external writer truncating the file while
   the mapping is live is UB; the safety comment on that call states the
   exposure.

Both file loaders are synchronous `Loader` impls wrapping async work, so both
end in `tokio::task::block_in_place(|| Handle::current().block_on(task))` —
which is why `den-core` pulls in tokio's `rt-multi-thread`.

Transpilation hooks into exactly those two loaders, and nowhere else: each
holds an `Arc<EasyOxcTranspiler>` handed to it by `Engine::new`, and calls
`transpile(src, infer_transpile_syntax_by_extension(extension), IsModule::Bool(true), false)`
before `Module::declare`. Without the `transpile` feature the same code paths
hand the raw bytes to `Module::declare`. The third transpile site is
`Engine::eval`, which uses `IsModule::Unknown` because a REPL line may be
either a script or a module.

## 4. The transpiler (`den-transpiler-oxc`)

oxc 0.146 (`oxc_parser`, `oxc_semantic`, `oxc_transformer`, `oxc_codegen`,
`oxc_sourcemap` 8.1.2). One public type, `EasyOxcTranspiler`, which is a
zero-sized struct — oxc keeps no interner, comment store or thread-local
globals, so there is nothing to carry between calls, and the type is trivially
`Send + Sync`.

`transpile()` is a four-stage pipeline:

- **parse** — `Parser::new(&allocator, source, is_module.apply(syntax)).parse()`.
  `IsModule::Bool(true|false)` forces module/script; `IsModule::Unknown` sets
  oxc's *unambiguous* mode, which is what lets a REPL line containing top-level
  `await` be upgraded to ESM. A `panicked` parse is checked explicitly: it
  yields an empty AST that would otherwise codegen into a silently empty
  module.
- **semantic** — `SemanticBuilder::new_compiler().with_enum_eval(true)`. The
  `with_enum_eval` is not optional: the TS enum lowering reads pre-computed
  member values out of `Scoping` and emits wrong reverse mappings without them.
  `into_scoping()` releases the shared borrow of the program so the transformer
  can take it `&mut`.
- **transform** — only compiled in under `typescript` or `react`. Types are
  stripped, class fields stay native, nothing is downlevelled, and JSX uses the
  **classic** runtime (`React.createElement`) because den's resolver has no
  `react` module and the automatic runtime's `import … from "react/jsx-runtime"`
  would fail to load.
- **codegen** — `source_map_path` is both the sourcemap on/off switch and the
  source of `sources[0]`.

**Arena discipline.** A fresh `Allocator` is created per call and dropped at the
end of it. Program, scoping and diagnostics all borrow from it, so nothing
borrowed may escape into the return value. Two consequences are load-bearing:
the sourcemap is detached with `map.into_owned()` before the arena drops, and
diagnostics are rendered to a `String` eagerly (`EasyOxcTranspilerError::render`)
because an `OxcDiagnostic` carries spans only and is worthless once the source
text is gone.

Every buffer is transpiled as `<anonymous>`: den transpiles in-memory sources
whose real path is not known at this layer.

## 5. `den-stdlib-wasm`

Implements the WebAssembly JS API (`WebAssembly.{validate, compile, instantiate,
compileStreaming, instantiateStreaming, Module, Instance, Memory, Table, Global,
Tag, Exception, CompileError, LinkError, RuntimeError}`, plus a den-specific
`wat2wasm`). `compileStreaming`/`instantiateStreaming` are written in JS
(`DEFINE_STREAMING` in `lib.rs`) and duck-type the `Response`, so the crate
does not depend on den's fetch.

### 5.1 The `backend` shim

`src/backend/mod.rs` declares a contract; `backend/wasmtime.rs` (wasmtime 48 +
wasmtime-wasi 48) and `backend/wasmi.rs` (wasmi 1.1) each implement it with the
*same item names* — `pub type Store = ::wasmtime::Store<StoreData>` versus
`::wasmi::Store<StoreData>`, and so on for two dozen types plus a set of shim
functions. Everything above the shim (`module.rs`, `instance.rs`, `memory.rs`,
`table.rs`, `global.rs`, `tag.rs`, `exception.rs`, `store.rs`, `utils.rs`) names
`crate::backend::*` and is written once.

Why cfg'd type aliases and not a trait or an enum: exactly one implementation
exists per build (the two `compile_error!`s at the top of `backend/mod.rs` enforce
that), so a trait would add generic plumbing and an enum would add a dispatch
arm — both for a choice that is already made at compile time. Aliases also let
the shared code use the engines' own inherent methods directly, which a trait
would have to re-declare.

Where the backends genuinely disagree, the shim owns the difference:

- `ValKind` / `ValView` — backend-neutral discriminants. wasmtime nests
  reference types in `ValType::Ref(RefType)` and derives neither `Copy` nor
  `PartialEq`; wasmi has a flat `Copy + PartialEq` enum with no `anyref`. So
  shared code asks `val_type_is(ty, ValKind::I64)` rather than matching.
- Capability constants. Each backend file declares `NAME`, `SUPPORTS_TAGS`,
  `SUPPORTS_ANYREF`, `SUPPORTS_V128` and `SUPPORTS_WASI`; `backend/mod.rs`
  declares `SUPPORTS_SHARED_MEMORY` and `WASI_NAMESPACE` once for both, because
  those two are den's limits rather than an engine's. Being `const bool`, a
  "not supported by the {NAME} backend of this build" `TypeError` is a plain
  branch the optimiser folds away on the backend that does have the feature.

  Every one of them is consumed twice over, which is what keeps them honest.
  `new_engine` derives the engine's `Config` from the constant
  (`.wasm_exceptions(SUPPORTS_TAGS)`, `.wasm_gc(SUPPORTS_ANYREF)`,
  `.wasm_simd(SUPPORTS_V128)`, `.wasm_threads(SUPPORTS_SHARED_MEMORY)`), so
  what a module is allowed to *validate* follows the constant; and the JS layer
  branches on the same constant — `SUPPORTS_TAGS` in `tag.rs` and
  `exception.rs`, `SUPPORTS_SHARED_MEMORY` in `memory.rs`, `SUPPORTS_WASI` in
  `store.rs`. `SUPPORTS_ANYREF` and `SUPPORTS_V128` have no JS-layer branch
  because those *values* never cross the boundary on either backend: `utils.rs`
  refuses `v128` in both directions and `anyref` for anything but null,
  regardless of which engine accepted the module.

  `backend/mod.rs`'s `parity` test module runs one JS program against whichever
  engine was compiled in and derives its expectations from the constants, so a
  constant that stops matching its backend fails there rather than in a script.
- `WasiCtx` — `wasmtime_wasi::p1::WasiP1Ctx` on wasmtime,
  `core::convert::Infallible` on wasmi, so the `Option<WasiCtx>` slot in the
  store payload stays backend-neutral at zero cost and can never be filled on
  wasmi. `link_wasi` is the only thing that fills it; wasmi's is the negative
  half of the contract, and is the one function that changes the day a
  `wasmi_wasi` dependency appears.

### 5.2 `OwnedCtx` and the `'static` store payload

wasmtime 48 bounds `T: 'static` on `Linker<T>` / `Instance` / `Func`, so the
store payload cannot borrow `'js`. `OwnedCtx` (`backend/mod.rs`) parks a
`Ctx<'static>` obtained via `Ctx::from_raw`, which performs `JS_DupContext` and
therefore owns a reference. Access is only ever through
`OwnedCtx::with(|ctx| …)`, which mints a fresh callback-scoped `'js` — a
`fn ctx(&self) -> Ctx<'_>` would hand out a lifetime the caller could outlive,
and `Ctx` is invariant in `'js` so it cannot simply be reborrowed.

`unsafe impl Sync for OwnedCtx` is the crate's one foundational `unsafe`. Its
invariant, stated in full on that impl: every path that can reach
an `OwnedCtx` runs under the rquickjs runtime lock (the value lives in the
`Store` payload, the `Store` lives in the userdata of the very context the
handle points at, and a wasm host callback is only entered from a JS call that
already holds the lock for the whole closure), and the pointee cannot dangle
because the duplicated reference lives as long as the handle and the runtime
drops its userdata before `JS_FreeRuntime`.

This is what makes the JS↔wasm bridge possible: `HostFunction::run`
(`instance.rs`) reaches JS from inside an engine callback via
`caller.data().with_ctx(…)`.

### 5.3 Store and engine as context userdata

The engine (`engine.rs`) and the store (`store.rs`) are separate pieces of
userdata on purpose: compiling and validating need an engine but no store, so
`WebAssembly.compile` is not refused merely because wasm is currently running.
One store per JS context is what makes every `Memory`, `Table`, `Global` and
`Instance` of that context interchangeable as imports.

### 5.4 `Memory.buffer` and why it is not an ordinary `ArrayBuffer`

`memory.buffer` *aliases* the wasm linear memory: `MemoryBuffers::alias`
(`memory.rs`) calls `JS_NewArrayBuffer` on the memory's base pointer with **no**
free function, so JS reads and writes hit the wasm pages directly and QuickJS
never claims ownership of them. (`ArrayBuffer::from_source` is unusable here —
it registers a free function that QuickJS runs twice on detach, in
`JS_DetachArrayBuffer` and again in the finalizer, which double-frees rquickjs'
boxed closure.)

An alias is only sound while the pages it names are still there, so a
context-wide registry (`MemoryBuffers`, one `RefCell<Vec<LiveBuffer>>` in the
context userdata) owns every live buffer:

- `buffer()` sweeps first and then reuses the entry whose base matches, which is
  what makes `memory.buffer === memory.buffer` and what makes two wrappers over
  the same linear memory hand out the same object.
- `refresh()` is the spec's "refresh the memory buffer" applied to every live
  buffer at once: any whose base or length has changed is detached, so the next
  `.buffer` read builds a fresh view. It runs on entry to a host callback
  (`refresh_in`, `instance.rs`) and after every export call (`utils.rs`), which
  is what catches a `memory.grow` executed *inside* wasm — wasmi backs a linear
  memory with a `Vec`, so an internal grow reallocates and every previously
  built `Uint8Array` would otherwise point at freed memory.
- `detach_at()` is the unconditional version `Memory.prototype.grow` needs,
  because the spec replaces `[[BufferObject]]` even for a zero delta, which
  moves and resizes nothing and so is invisible to `refresh`.

The other half is `seal_against_transfer`. The spec tags the buffer with
`[[ArrayBufferDetachKey]] = "WebAssembly.Memory"` so that only the engine may
detach it; quickjs-ng has no key concept at all — `JS_DetachArrayBuffer` takes
no key, and `js_array_buffer_transfer` checks only `shared`/`immutable`/
`detached`. So the four detaching methods — `transfer`,
`transferToFixedLength`, `transferToImmutable`, `resize` — are shadowed on each
buffer by *own* properties that throw a `TypeError`, defined non-writable,
non-configurable and non-enumerable so script can neither overwrite nor
`delete` them to reach the originals on `ArrayBuffer.prototype`.

That is a memory-safety measure, not a conformance detail: `transfer` reaches
`js_realloc(ctx, abuf->data, new_len)`, and `abuf->data` is the wasm linear
memory base, which QuickJS never allocated. Without the shadow,
`new WebAssembly.Memory({initial:1}).buffer.transfer(1)` hands a foreign
pointer to QuickJS' allocator and corrupts the heap.

### 5.5 WASI is an explicit opt-in, and nothing else

den links no host API implicitly. A module that imports
`wasi_snapshot_preview1` and is given nothing for it fails like any other
unsatisfied import, with a `TypeError` out of `Instance::read_imports`.

Asking for WASI means passing `wasiImports()`, which is an export of the
`den:wasm` *module* and deliberately not a member of `WebAssembly`:

```js
import { wasiImports } from "den:wasm";
await WebAssembly.instantiate(bytes, { wasi_snapshot_preview1: wasiImports() });
```

What it returns is an opaque marker class (`WasiImports`, `store.rs`), not a
bag of functions, and that is forced by the problem rather than chosen: preview1
is implemented by the *engine*, and every call reads and writes the *calling*
instance's linear memory, which no JS function can stand in for. So
`read_imports` recognises the marker where it resolves a namespace object and
calls `WasiImports::link` instead of reading names out of it. Three properties
of that path are load-bearing:

- `WasiImports::namespace` throws a `TypeError` naming the backend when
  `SUPPORTS_WASI` is false, so `wasiImports()` on a wasmi build never yields a
  marker at all.
- `WasiImports::link` refuses any namespace but `wasi_snapshot_preview1` with a
  `LinkError` naming both, so `{ env: wasiImports() }` cannot quietly swallow a
  module's real `env` imports.
- It is idempotent (`allow_shadowing` around `add_to_linker_sync`, restored
  afterwards), because the hook is reached once per WASI import and
  `add_to_linker_sync` defines the whole namespace each time.

`backend::link_wasi` is the only place a `WasiCtx` is ever built, lazily via
`StoreData::wasi_or_init`, and building one inherits the host's stdio and
environment — which is exactly why it may only run once a caller has spelled
out `wasiImports()`. Holding the marker grants nothing on its own; it builds
nothing.

## 6. `den-stdlib-worker`

Web Workers, `MessageChannel`/`MessagePort`, `BroadcastChannel`, the
`EventTarget` family, `AbortController`/`AbortSignal`, `performance`,
`structuredClone` and `reportError`. The design notes are
[`docs/research/08`-`11`](docs/research/); this is the shape they settled into.

### 6.1 Why half of it is JavaScript

Every interface here `extends EventTarget`, and an `#[rquickjs::class]` can
neither extend a JS class nor be extended by one. So the split is not a
stylistic one:

- **Rust owns** transport (channels, threads), (de)serialisation, and the two
  or three things JS cannot do — reading a thrown value's location out of its
  stack, reaching another thread, cancelling a runtime.
- **JS owns** the API surface: `src/prelude/*.js`, seven files evaluated in
  dependency order (`events`, `abort`, `performance`, `clone`, `port`,
  `worker`, `broadcast`). The `performance` clock is a native `Instant`
  captured when natives install; the prelude is the object a script sees.

Each prelude file is one `(function (natives, api) { …; return { …api, X } })`.
They are chained — each receives the previous one's return value — and the last
`api` becomes both the module's exports and a set of globals, driven by the
`API` list in `lib.rs`. `natives` is a shared, mutable bag: a later prelude may
publish a hook on it for an *earlier* one to call back into. That is how the
clone pre-pass gets a way to build a `MessagePort` (`natives.wrapPort`, set by
`port.js` and read by `clone.js`), and how the preludes reach `dispatchTrusted`
without it ever appearing on `globalThis` — a script that could call it could
forge a trusted event.

The same bag is this realm's exception sink (`den-stdlib-core/src/report.rs`),
which is what puts a throwing `setTimeout` body in a worker onto the worker's
error chain instead of straight onto stderr: every reporter in the process —
`den-stdlib-timer`, a port pump, a listener that throws — resolves
`natives.reportException` at report time, and `worker.js` replaces that one
entry. `den-core` reaches the same bag for `dispatchTrusted` when it fires
`unhandledrejection`.

### 6.2 The `WorkerHost` seam

`den-stdlib-worker` knows nothing about loaders, the transpiler or the stdlib;
`den-core` knows nothing about threads. Between them is one trait with one
method (`host.rs`):

```rust
fn build_engine(&self, stop: CancellationToken, base: BaseUrl)
    -> Result<WorkerEngine, WorkerHostError>;
```

Lifetime: **singleton** — one `Arc<dyn WorkerHost>` per process, cloned into the
userdata of every context that may run `new Worker`, worker contexts included,
which is what makes nesting free. It is called on the worker's own OS thread,
inside that thread's tokio runtime, before any script runs. Two more userdata
slots complete the picture: `BaseUrl` (what a relative worker URL resolves
against, following the entry point rather than the working directory) and
`RealmStop` (this realm's cancellation token, of which every worker it spawns
takes a *child* — so `Engine::stop()` interrupts a whole tree, and `shutdown()`
then reaps it bottom-up).

### 6.3 One OS thread, one tokio worker thread

`new Worker` spawns a `std::thread` named `den-worker:<name>`, and that thread
builds a **`new_multi_thread().worker_threads(1)`** tokio runtime of its own. A
current-thread scheduler would be the obvious choice; den's loaders call
`block_in_place`, which panics on it (`docs/research/09` §6.1) — the
`ponytail:` comment on `WorkerThread::serve` names that as the reason and the
upgrade path.

The runtime's own worker and blocking threads inherit the `den-worker:` name,
so joining only the outer `std::thread` would let `Engine::shutdown()` return
with threads still alive. The thread body therefore ends with a bounded
`tokio.shutdown_timeout(...)`, which is what makes "joined" mean "every thread
this worker had is gone". `den-core/tests/workers.rs` asserts exactly that by
counting `/proc/self/task` entries by name.

A panic on a worker thread is caught (`panic::catch_unwind`) and sent to the
parent as an `ErrorEvent`: every other thread is holding a live QuickJS
runtime, so letting it unwind out would take the process down.

### 6.4 Structured clone: quickjs-ng's serialiser plus a JS pre/post pass

The bytes are quickjs-ng's own `JS_WriteObject2`/`JS_ReadObject`. Around them
sit two JS functions in `prelude/clone.js`, registered with Rust through
`natives.registerClone`: `prepare` runs on the sender before the write,
`restore` on the receiver after the read. Four kinds of work happen there:

1. **Things the serialiser gets wrong.** `RegExp` and `Map`/`Set` are taken
   apart and rebuilt from their parts, never handed to the writer:
   `JS_ReadRegExp` forgets to `BC_add_object_ref` the object it builds while the
   writer adds every object, so one `RegExp` shifts every later back-reference
   by one; `js_map_write` emits zombie records `js_map_read` does not expect.
   Both desynchronise the stream — `docs/research/10` §4.4 has the line numbers.
2. **Things the spec requires and the serialiser refuses**, chiefly invoking
   getters and dropping symbol keys. The walk rebuilds plain objects and arrays
   with `CreateDataProperty` semantics (`Object.defineProperty`, never `[[Set]]`)
   so that an own `__proto__` *data* property survives and no inherited setter
   intercepts a value.
3. **Things the spec rejects that quickjs would accept, or reject badly.**
   `Promise`, `WeakMap`/`WeakSet`/`WeakRef`, `FinalizationRegistry`,
   `SharedArrayBuffer`, a `Proxy` (recognised by class id, before any trap can
   run), a `MessagePort` that is not in the transfer list, and an
   `ArrayBufferView` left out of bounds by a shrunk resizable buffer — for which
   quickjs writes a stale offset and only its *reader* complains, on the far
   side of a thread, long after the sender returned.
4. **Transfer.** `MessagePort` is `[Transferable]` but not `[Serializable]`, so
   the walk replaces a port with its index in the transfer list and `restore`
   rebuilds it. Transfer is all-or-nothing: `message.rs` validates the whole
   list — duplicates, detached buffers, immutable buffers, buffers carrying an
   own `transfer` (den's spelling of `[[ArrayBufferDetachKey]]`, which is how
   `WebAssembly.Memory#buffer` refuses), started ports — *before* it detaches
   anything, and re-validates after the walk, because a getter is free to close
   a port mid-walk.

Anything the walk hands through unchanged that the reader then refuses becomes a
`DataCloneError` rather than the reader's own `RangeError` (same realm) or a
far-side `messageerror` (across a worker).

### 6.5 The process-lifetime rule

`AsyncRuntime::idle()` resolves only when no `ctx.spawn`-ed future is left
(`docs/research/09` §2.2), so "what keeps den alive" is exactly "what is
spawned". Four rules, and they are the whole of it:

1. **A queue the script opened stays open until the script closes it.**
   `port.start()`, assigning `onmessage` on a `MessagePort` (§9.4.4 says that
   enables the queue "as if `start()` had been called"), and
   `new BroadcastChannel` all say *keep listening*; `close()` is how that is
   taken back.
2. **A queue the platform opened is reffed by its listeners.** A `Worker`'s two
   ends were opened on the script's behalf and nobody asked for them, so they
   keep the loop alive only while at least one `message` or `messageerror`
   listener exists on that target. Without this rule
   `new Worker("noop.js")` hangs den for ever, because the worker's own end is
   listening to a script that stopped caring. Unreffing stops *delivery*, never
   receipt: envelopes stay queued in the channel and the next pump resumes at
   the first one.
3. **A live worker keeps its parent alive** — through the fault pump
   `new Worker` spawns in the parent realm, not through the parent's port. So a
   parent that installed no listener still waits for a worker that is doing
   work, and stops waiting the moment the worker's context is dropped.
4. **A worker realm ends** when its own `idle()` resolves (no timers, no started
   ports, no in-flight fetch), or at `close()`, or at `terminate()`, or when the
   parent hangs up and its pump sees the channel close.

The practical consequence, and the one to remember: `new Worker("./w.js")` where
`w.js` only prints exits 0 like Node, while a worker that registers `onmessage`
keeps the process alive until `close()` or `terminate()`.

## 7. Known limitations

- **No re-entrancy while an export is running.** The store is one
  `Rc<RefCell<backend::Store>>`, and `Store::with_mut` (`store.rs`) answers a
  failed `try_borrow_mut` with a `WebAssembly.RuntimeError` — *"a WebAssembly
  export is still running and has called back into JS: this build cannot
  re-enter its wasm store, so calling another export — or creating a Memory,
  Table, Global or Tag — is unsupported until that call returns"*. That is the
  full extent of it: a wasm → JS → wasm call throws rather than panicking, and
  so does constructing a new store-backed object from inside a host callback.
  `store.rs`'s `a_host_callback_cannot_reach_another_export_of_the_same_store`
  pins the ceiling end to end. Lifting it needs the borrow scoped per call
  frame rather than per outermost call.
- **`v128` and `anyref` values never cross the JS boundary, on either
  backend.** Modules *containing* `v128` do validate everywhere now — wasmi's
  `simd` cargo feature is enabled and `SUPPORTS_V128` is `true` on both, because
  `v128` is what LLVM emits for ordinary Rust and C — but `utils.rs` refuses a
  `v128` in both directions with a `TypeError`, and `anyref` accepts only null,
  since it is not in the spec's `ValueType` enum and den has no
  `i31`/`struct`/`array` conversions. `funcref` and `externref` are fully
  supported: a `funcref` round-trips as the same Exported Function object,
  through the identity cache in `utils.rs`.
- **wasmi is still the smaller backend**, in two places rather than four: no
  tags, so `WebAssembly.Tag` and `WebAssembly.Exception` construction throw
  `TypeError`; and no WASI, since `wasmi_wasi` is not a dependency, so
  `wasiImports()` throws instead of yielding a marker.
- **Shared memory is refused on both backends, by choice.**
  `SUPPORTS_SHARED_MEMORY` is a single `false` in `backend/mod.rs` rather than a
  per-backend constant: the JS-API spec's §5.6 requires the `[[BufferObject]]` to be a
  `SharedArrayBuffer`, and den has no way to build one that aliases linear
  memory — QuickJS silently refuses to detach a shared buffer
  (`JS_DetachArrayBuffer`), which would turn §5.4's growth protocol into a
  use-after-free. So `new WebAssembly.Memory({ shared: true })` is a
  `TypeError`, and `new_engine` derives `wasm_threads` from the same constant so
  that no module can smuggle a shared memory past the JS API either.
  `memory.rs`'s `shared_memory_needs_a_maximum_and_is_not_allocatable_either_way`
  pins both halves.
- **WASI grants the host's stdio and environment when it is asked for.** There
  is no sandboxing knob: `wasiImports()` is all-or-nothing, and a caller passing
  it hands the module the real `WasiCtx` (§5.5). Nothing is linked without it.
- Several `den:fs` entry points (`metadata`, `readDir`, `readLink`,
  `setPermissions`, `symlinkMetadata`) are declared but unimplemented.
- **The binary swallows failures.** `src/main.rs` prints a load or run error and
  still returns `Ok(())`, so `den missing.js` exits 0. (Absolute entry points do
  resolve now: `den-core/src/resolver/file.rs`'s `AbsolutePathResolver` covers
  absolute paths, `file:` URLs and specifiers relative to either.)
- **Web Workers: the v1 divergences.** Each is pinned by a test.
  - *Structured clone.* Array holes arrive as `undefined` and non-index array
    properties are dropped. `RegExp` and `Map`/`Set` are rebuilt from their
    parts rather than handed to `JS_WriteObject2`, working around two
    quickjs-ng reference-table bugs (§6.4).
  - *No shared memory between realms.* quickjs-ng does register
    `SharedArrayBuffer` and `Atomics` — including `Atomics.wait` — but a
    `SharedArrayBuffer` cannot leave its realm: the clone walk refuses it with a
    `DataCloneError`, so nothing a worker can reach is ever shared and
    cross-thread atomics are effectively unavailable. Each worker gets a whole
    QuickJS runtime of its own and messages are copied.
  - *Classic workers are `file:`-only.* An `http(s)` classic worker is a
    `TypeError` pointing at `{ type: "module" }`; module workers go through
    den's full resolver/loader/transpiler chain and may be remote.
  - *A module worker's message queue opens only after its top-level `await`
    settles.* HTML §10.2.4 step 2.13 is implemented literally — the queue is
    enabled once the script "has run", and for a module with top-level `await`
    that means once the evaluation promise resolves. Nothing is lost (envelopes
    queue in the channel), but timers scheduled before the `await` fire while
    `message` events still wait, which a browser would interleave differently.
  - *`ErrorEvent.error` is `undefined` across a thread*, because an `Error` does
    not serialise; message/filename/lineno/colno are carried. A worker script
    that fails to *load* reports an `ErrorEvent` carrying the reason where HTML
    fires a bare `Event` — strictly more useful, and one code path fewer.
  - *A started `MessagePort` refuses to be transferred* with a
    `DataCloneError`. The spec ships a started port together with its
    undelivered messages; here that queue lives in the receiving realm's runtime
    and cannot be packed up for another one (`NativePort::is_started` states the
    reason).
  - *`MessagePort`'s `close` event needs a started port.* HTML queues the event
    until the queue is enabled; den fires it from the pump, and a port that was
    never started has no pump.
  - *den's main global is not an `EventTarget`.* Only a worker scope is, so in
    the main realm there is no `addEventListener`/`dispatchEvent`, an
    `unhandledrejection` cannot be heard (the rejection prints as before) and
    `reportError()` prints instead of firing a cancelable `error` event. Inside
    a worker both are fully spec-shaped, `onerror` and `onunhandledrejection`
    included.

  The design notes are `docs/research/08`-`11`.

## 8. Feature flags

Root `Cargo.toml` default: `stdlib, typescript, react, wasm-wasmtime, mimalloc`.
Nearly every root feature is a pass-through to `den-core`.

| Feature | Effect |
|---|---|
| `stdlib` | all of `stdlib-console/core/crypto/fs/networking/sqlite/text/timer/whatwg-fetch/worker` |
| `stdlib-*` | one standard-library crate each |
| `transpile` | pulls in `den-transpiler-oxc`; loaders start transpiling |
| `typescript` | implies `transpile`; `.ts`/`.tsx` and TS lowering |
| `react` | implies `transpile`; `.jsx`/`.mjsx`/`.tsx` and classic-runtime JSX |
| `wasm-wasmtime` | `den-stdlib-wasm` with the wasmtime 48 backend |
| `wasm-wasmi` | `den-stdlib-wasm` with the wasmi 1.1 backend |
| `wasm` | alias for `wasm-wasmtime` |
| `mimalloc` | mimalloc as the global allocator (binary only) |
| `tracing` | `color-eyre` span traces / `track_caller` |
| `tokio-console` | `console-subscriber`; also needs `--cfg tokio_unstable` |

`wasm-wasmtime` and `wasm-wasmi` are mutually exclusive and cargo will not
enforce that for you: because features are additive, asking for `wasm-wasmi`
without `--no-default-features` leaves `wasm-wasmtime` on and the build fails
on `den-stdlib-wasm`'s `compile_error!`. That is why `wasm` is a plain alias
rather than a `dep:`-only feature.

## 9. Build and test

```bash
cargo build                                  # debug
cargo build --release
cargo build --profile min-size-release       # size-favoured

# wasmtime backend (default)
cargo test --workspace --all-targets

# wasmi backend
cargo test --workspace --all-targets --no-default-features \
  --features stdlib,typescript,react,wasm-wasmi
```

Both invocations must be green: 299 tests on wasmtime, 297 on wasmi (two are
`#[cfg(feature = "wasmtime")]`, both about WASI). Two crates carry the bulk.
`den-stdlib-worker` has 129 unit tests, proving the worker semantics against
bare `AsyncContext`s; `den-stdlib-wasm` has 93 driving the JS API through a real
QuickJS context and branching on the capability constants, so the same test
asserts the right thing on either backend. The rest is `den-transpiler-oxc`
(13: pipeline and arena behaviour), `den-core`'s integration tests in
`den-core/tests/` (47 across `webassembly.rs`, `workers.rs`, `stdlib.rs`,
`transpile.rs` and `lifetime.rs`), and 17 unit tests spread over `den-core`,
`den-stdlib-networking`, `den-stdlib-text` and `den-stdlib-whatwg-fetch`.

`den-core/tests/workers.rs` is the layer that proves a *user* gets the worker
semantics: it writes its fixtures under `std::env::temp_dir()` at test time and
drives them through `Engine::run_file`, so the loaders, the transpiler, the
`BaseUrl` and `Engine::shutdown`'s thread reaping are all in the path. Every
cross-thread wait is a promise settled by an event under a
`tokio::time::timeout`; nothing synchronises by sleeping.

CI (`.github/workflows/lint.yml`) runs clippy, `fmt --check`, `doc` and the
test suite across both backends as a matrix — a green wasmtime run says nothing
about wasmi, since they share the JS-API layer but not its capabilities.
