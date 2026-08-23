# wasmtime 27.0.0 → 48.0.0 / wasmtime-wasi 27 → 48 migration

Status: research complete, **migration verified to compile**; audited a second time against
the crate sources — corrections and new findings are logged in §12.
Scope: `den-stdlib-wasm` (the only crate that touches wasmtime).

All claims below were read out of the local crate sources:

- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-27.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-48.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-wasi-27.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-wasi-48.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-internal-core-48.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmparser-0.254.0/`

`RELEASES.md` is **not vendored** in either crate (`ls wasmtime-48.0.0/` → only
`build.rs Cargo.lock Cargo.toml LICENSE proptest-regressions README.md src tests`).
`https://raw.githubusercontent.com/bytecodealliance/wasmtime/release-48.0.0/RELEASES.md`
only carries the 48.0.0 section and links to per-branch files, so *which* of 28…47
introduced each break could not be pinned down. It does not matter for the port:
everything below is verified against the 48.0.0 source directly.

---

## 0. TL;DR — verification method and result

`Cargo.toml` **already declares 48.0.0** (`den-stdlib-wasm/Cargo.toml:25-26`,
`Cargo.lock:4980-4982`); only the code was never ported. `cargo check -p den-stdlib-wasm`
on a scratch copy of the repo (after unblocking an unrelated `den-stdlib-core`
error) produced **17 errors**, of which these are wasmtime's:

```
den-stdlib-wasm/src/store.rs:9:30   E0477 the type `(WasiP1Ctx, rquickjs::Ctx<'js>)` does not fulfill the required lifetime
den-stdlib-wasm/src/store.rs:13:23  E0477 (same)
den-stdlib-wasm/src/store.rs:9:17   lifetime may not live long enough: `'js` must outlive `'static`
den-stdlib-wasm/src/store.rs:30:9   lifetime may not live long enough: `'js` must outlive `'static`
den-stdlib-wasm/src/store.rs:5:21   E0432 unresolved import `wasmtime_wasi::preview1`
den-stdlib-wasm/src/instance.rs:220 E0433 cannot find `preview1` in `wasmtime_wasi`
den-stdlib-wasm/src/instance.rs:157 lifetime may not live long enough: `'js` must outlive `'static`
den-stdlib-wasm/src/instance.rs:219 E0521 `import_object` escapes ... `'js` must outlive `'static`
den-stdlib-wasm/src/instance.rs:241 E0004 non-exhaustive patterns: `ExternType::Tag(_)` not covered
den-stdlib-wasm/src/module.rs:87    E0004 non-exhaustive patterns: `ExternType::Tag(_)` not covered
den-stdlib-wasm/src/global.rs:45    lifetime may not live long enough: `'js` must outlive `'static`
den-stdlib-wasm/src/memory.rs:62,73,100 lifetime may not live long enough: `'js` must outlive `'static`
den-stdlib-wasm/src/table.rs:88     E0521 `ctx` escapes ... `'js` must outlive `'static`
```

Applying the fixes in §2 and §3 below drove that to **zero wasmtime errors**
(remaining failures are rquickjs 0.12 / edition-2024 / `wabt`→`wat`, tracked
elsewhere). The working scratch tree is at
`/tmp/claude-1000/-home-steve-git-github-com-stevefan1999-personal-den/0aae9ff5-defc-4c2f-8b83-11b508e5f823/scratchpad/den/den-stdlib-wasm/`.

There are effectively **only two hard wasmtime breaks**: `T: 'static` on the store
payload, and `ExternType::Tag`. Everything else (`Memory`, `Table`, `Global`,
`Func`, `Module`, `Linker` method shapes) is byte-for-byte identical to 27.

---

## 1. `wasmtime::Error` is no longer `anyhow::Error`

wasmtime 27, `src/lib.rs:394`:

```rust
pub use anyhow::{Error, Result};
```

wasmtime 48, `src/lib.rs:512-523`:

```rust
pub use wasmtime_environ::ToWasmtimeResult;
#[doc(inline)]
pub use wasmtime_environ::error;
pub use self::error::{Error, OutOfMemory, Result, bail, ensure, format_err};
```

`wasmtime::Error` is now a bespoke one-word type
(`wasmtime-internal-core-48.0.0/src/error/error.rs:425`, with a
`const _ERROR_IS_ONE_WORD_LARGE` assertion at line 431). Conversions:

- `impl<E: core::error::Error + Send + Sync + 'static> From<E> for Error` (error.rs:487).
  → `?` on a `rquickjs::Error` inside a host closure still works
  (`rquickjs-core-0.12.2/src/result.rs:361` has `impl StdError for Error {}`).
- `From<wasmtime::Error> for anyhow::Error` exists; the reverse does **not** —
  use `wasmtime::Error::from_anyhow(e)` (documented at error.rs:398-421).

**Impact on den: none today.** Every call site only does
`.map_err(|x| Exception::throw_internal(ctx, &format!("...: {}", x)))`, and `Display`
is implemented (error.rs:471). `anyhow = "1.0.104"` in
`den-stdlib-wasm/Cargo.toml:14` is **unused** (`grep -rn anyhow den-stdlib-wasm/src/`
→ nothing) — delete it.

New OOM-aware siblings you may want: `Store::try_new`, `Func::try_new`,
`OutOfMemory` (`wasmtime-internal-core-48.0.0/src/error/oom.rs:99`).

---

## 2. THE breaking change: `Store<T>` requires `T: 'static`

wasmtime 27, `src/runtime/store.rs:176` / `src/runtime/store/context.rs:56`:

```rust
pub struct Store<T> { ... }
pub trait AsContext { type Data; ... }
```

wasmtime 48, `src/runtime/store.rs:196`, `src/runtime/store/context.rs:11,19,36,103`:

```rust
pub struct Store<T: 'static> { ... }
pub struct StoreContext<'a, T: 'static>(pub(crate) &'a StoreInner<T>);
pub struct StoreContextMut<'a, T: 'static>(pub(crate) &'a mut StoreInner<T>);

pub trait AsContext {
    type Data: 'static;                                   // <- was `type Data;`
    fn as_context(&self) -> StoreContext<'_, Self::Data>;
}
pub trait AsContextMut: AsContext {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, Self::Data>;
}
```

`Linker` grew the same bound on essentially every method
(`src/runtime/linker.rs:262,303,376,386,415,440,547,637,773,913,1095,1176,1198,1235,1271,1304,1325,1360`,
plus `impl<T: 'static> Default for Linker<T>` at 1398). `Memory::data`/`data_mut`,
`Instance::exports`/`module`, `ExternRef::data` all moved `T: 'a` → `T: 'static`
(e.g. 27 `memory.rs:358` `pub fn data<'a, T: 'a>` vs 48 `memory.rs:408`
`pub fn data<'a, T: 'static>`).

### Note: `Send` was never the problem

`rquickjs-core-0.12.2/src/context/ctx.rs:103` has `unsafe impl Send for Ctx<'_> {}`,
so `(WasiP1Ctx, Ctx<'js>)` *is* `Send`. That is why den compiled on 27. The only
new obstruction is `'static`. `Store<T>` itself is `Send + Sync` when `T` is —
wasmtime asserts `_assert_send_and_sync::<Store<()>>()` at
`wasmtime-48.0.0/src/runtime.rs:188`.

### BEFORE — `den-stdlib-wasm/src/store.rs`

```rust
use wasmtime_wasi::{preview1::WasiP1Ctx, WasiCtxBuilder};

pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);

#[derive(Trace, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store<'js> {
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData<'js>>>>,
}

unsafe impl<'js> JsLifetime<'js> for Store<'js> {
    type Changed<'to> = Store<'to>;
}

#[rquickjs::methods]
impl<'js> Store<'js> {
    #[qjs(constructor)]
    pub fn new(engine: &crate::engine::Engine, ctx: Ctx<'js>) -> Self {
        let wasi_ctx = WasiCtxBuilder::new().inherit_stdio().inherit_env().build_p1();
        let inner = wasmtime::Store::new(&engine, (wasi_ctx, ctx));
        Self { inner: Arc::new(RefCell::new(inner)) }
    }
}
```

### AFTER — verified to compile

`Ctx::from_raw` (`rquickjs-core-0.12.2/src/context/ctx.rs:442`) bumps the JSContext
refcount (`JS_DupContext`) and returns a **lifetime-unbounded** `Ctx<'js>`;
`Drop` calls `JS_FreeContext` (ctx.rs:97-101). So parking a `Ctx<'static>` in the
store payload is refcount-correct and needs no hand-written `Send` impl.

```rust
use std::{cell::RefCell, sync::Arc};

use derive_more::derive::{Deref, DerefMut, From};
use rquickjs::{class::Trace, Ctx, JsLifetime};
use wasmtime_wasi::{p1::WasiP1Ctx, WasiCtxBuilder};

/// `wasmtime::Store<T>` requires `T: 'static` in wasmtime 48, so the payload can
/// no longer borrow `'js`. `Ctx::from_raw` hands back an unbounded lifetime and
/// bumps the JSContext refcount (released on drop), so we park a `Ctx<'static>`
/// and re-derive a `Ctx<'js>` on use.
pub struct StoreData {
    pub wasi: WasiP1Ctx,
    ctx:      Ctx<'static>,
}

impl StoreData {
    /// SAFETY: only call while the QuickJS runtime lock is held, i.e. from
    /// inside a host call originating from this context.
    pub fn ctx<'js>(&self) -> Ctx<'js> {
        unsafe { Ctx::from_raw(self.ctx.as_raw()) }
    }
}

#[derive(Trace, JsLifetime, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store {                       // no more `'js` parameter
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData>>>,
}

#[rquickjs::methods]
impl Store {
    #[qjs(constructor)]
    pub fn new(engine: &crate::engine::Engine, ctx: Ctx<'_>) -> Self {
        let wasi = WasiCtxBuilder::new().inherit_stdio().inherit_env().build_p1();
        let ctx = unsafe { Ctx::from_raw(ctx.as_raw()) };
        let inner = wasmtime::Store::new(&engine, StoreData { wasi, ctx });
        Self { inner: Arc::new(RefCell::new(inner)) }
    }
}
```

Dropping the `'js` parameter from `Store` also deletes the hand-written
`unsafe impl JsLifetime` (derive works again) and fixes the eight downstream
`'js must outlive 'static` errors in `global.rs`, `memory.rs`, `table.rs`,
`instance.rs` with no further edits: they all just say
`ctx.userdata::<crate::store::Store>()` / `&mut wasmtime::Store<StoreData>`,
which now names a `'static` type.

`den-stdlib-wasm/src/table.rs:53` `store: &mut wasmtime::Store<StoreData>` needs no
textual change — `StoreData` simply stopped being generic.

Nothing outside the crate names `Store<'js>`: den-core only touches
`den_stdlib_wasm::js_wasm` (`den-core/src/engine.rs:170,289`), so dropping the
lifetime parameter does not ripple into any other crate.

### What parking a `Ctx<'static>` actually costs

Two consequences the code above does not make obvious — neither is a blocker, both
are things to know before you sign off on it:

1. **The JSContext outlives its `Context` handle.** `Ctx::store_userdata` puts the
   `Store` in the *runtime's* opaque, not the context's (`ctx.rs:480-485` →
   `Opaque::insert_userdata`, `opaque.rs:180-186`, field `userdata` at
   `opaque.rs:57`). The `Ctx<'static>` inside `StoreData` therefore holds a
   `JS_DupContext` refcount that is only released when the *runtime* goes away:
   `RawRuntime::drop` runs `opaque.clear()` (which drops the userdata map) *before*
   `JS_FreeRuntime` (`raw.rs:123-131`, `opaque.rs:284-292`). So: no use-after-free,
   no process-lifetime leak, but the context cannot be reclaimed early, and in a
   multi-context runtime every host call gets the context that first evaluated the
   `den:wasm` module — not necessarily the calling one.
2. **`StoreData::ctx()` is a safe fn handing out an unbounded `'js`.** It can mint a
   `Ctx<'static>` for a caller that outlives the real context. Make it
   `pub(crate) unsafe fn`, or keep the `SAFETY:` contract enforceable by never
   exposing `StoreData` outside this crate.

### Rejected alternative

Keeping `StoreData = WasiP1Ctx` and capturing a `Send`-wrapped
`NonNull<qjs::JSContext>` in each `func_new` closure also works (den already has
the `DangerouslyImplementSync` newtype at `instance.rs:54-57`), but it touches
more sites. A raw-`NonNull` variant of `StoreData` also compiles but then needs a
hand-written `unsafe impl Send for StoreData` because `add_to_linker_sync`
demands `T: Send` (§7); the `Ctx<'static>` form gets `Send` honestly.

---

## 3. `ExternType::Tag` — new variant, two non-exhaustive matches

wasmtime 27 `src/runtime/types.rs:1151`:

```rust
pub enum ExternType { Func(FuncType), Global(GlobalType), Table(TableType), Memory(MemoryType) }
```

wasmtime 48 `src/runtime/types.rs:1445`:

```rust
pub enum ExternType {
    Func(FuncType), Global(GlobalType), Table(TableType), Memory(MemoryType),
    Tag(TagType),                       // NEW
}
```

`Extern` likewise gained `Tag(Tag)` (`src/runtime/externals.rs:37`), and
`Extern::ty` maps it (externals.rs:124). `Instance::get_tag` is new
(`src/runtime/instance.rs:586`).

### BEFORE — `den-stdlib-wasm/src/module.rs:86-93` (E0004)

```rust
fn extern_type_to_str(x: ExternType) -> &'static str {
    match x {
        wasmtime::ExternType::Func(_) => "function",
        wasmtime::ExternType::Global(_) => "global",
        wasmtime::ExternType::Table(_) => "table",
        wasmtime::ExternType::Memory(_) => "memory",
    }
}
```

### AFTER

```rust
fn extern_type_to_str(x: ExternType) -> &'static str {
    match x {
        wasmtime::ExternType::Func(_) => "function",
        wasmtime::ExternType::Global(_) => "global",
        wasmtime::ExternType::Table(_) => "table",
        wasmtime::ExternType::Memory(_) => "memory",
        wasmtime::ExternType::Tag(_) => "tag",   // JS-API ImportExportKind, exception-handling
    }
}
```

Second site: `den-stdlib-wasm/src/instance.rs:241` (`match ext.ty(&mut *store)` in
`Instance::exports`). Minimum viable arm:

```rust
wasmtime::ExternType::Tag(_) => {
    return Err(Exception::throw_internal(&ctx, "tag export not implemented"));
}
```

Third site (compiles today but is now **reachable**):
`den-stdlib-wasm/src/instance.rs:166` `_ => unreachable!()` in `resolve_imports`.
A module importing a tag will hit it and abort the process. Replace with a `Tag`
arm or a thrown `LinkError`. Note this arm is only reached when
`module_import.ty()` is not `Func`, so `Tag` lands squarely on it.

---

## 4. Config: what is on by default, and what you must turn on

`Engine::new(&Config)` is unchanged (48 `src/engine.rs:107`, 27 `src/engine.rs:90`).
What changed is the **default feature set**.

wasmtime 27 `src/config.rs:1792-1832`: `WASM2` + `MULTI_MEMORY` + `RELAXED_SIMD` +
`TAIL_CALL` + `EXTENDED_CONST`, plus `GC_TYPES`/`THREADS` gated on cargo features.

wasmtime 48 `src/config.rs:2520-2578`: starts from `WasmFeatures::WASM3`, then
`features.set(GC_TYPES, cfg!(feature="gc"))`, `set(EXCEPTIONS, cfg!(feature="gc"))`,
`set(THREADS, cfg!(feature="threads"))`.

`wasmparser-0.254.0/src/features.rs:382-417`:

```rust
WASM1 = MVP(FLOATS|GC_TYPES) | MUTABLE_GLOBAL
WASM2 = WASM1 | BULK_MEMORY | REFERENCE_TYPES | SIGN_EXTENSION | SATURATING_FLOAT_TO_INT | MULTI_VALUE | SIMD
WASM3 = WASM2 | GC | TAIL_CALL | EXTENDED_CONST | FUNCTION_REFERENCES | MULTI_MEMORY
                | RELAXED_SIMD | THREADS | EXCEPTIONS | MEMORY64
```

`wasmtime`'s cargo `default` includes `gc`, `gc-copying`, `gc-drc`, `gc-null`,
`threads` (`wasmtime-48.0.0/Cargo.toml` default block), so **den already gets
everything the JS-API needs from a bare `Config::new()`** — including `GC`,
`FUNCTION_REFERENCES`, `EXCEPTIONS` and `MEMORY64`, which 27 did *not* enable.

### Full JS-API surface — explicit knob list

`den-stdlib-wasm/src/engine.rs:23-27` currently constructs `Config::new()` and sets
nothing (with a `let mut config` that triggers an `unused_mut` warning). Spelling the
knobs out documents the intended JS-API surface and makes a trimmed cargo-feature
set fail loudly instead of silently — but "loudly" means a panic at startup, so read
the block after the list before adopting it. Verified to compile:

```rust
let mut config = wasmtime::Config::new();
config
    .wasm_bulk_memory(true)          // config.rs:1180  (default on)
    .wasm_multi_value(true)          // config.rs:1194  (default on)
    .wasm_reference_types(true)      // config.rs:1041  (default on)
    .wasm_simd(true)                 // config.rs:1109  v128 (default on)
    .wasm_relaxed_simd(true)         // config.rs:1136  (default on)
    .wasm_tail_call(true)            // config.rs:940   (default on)
    .wasm_extended_const(true)       // config.rs:1234  (default on)
    .wasm_multi_memory(true)         // config.rs:1208  (default on)
    .wasm_memory64(true)             // config.rs:1223  (default on in 48, off in 27)
    .wasm_function_references(true)  // config.rs:1060  (default on in 48, off in 27)
    .wasm_gc(true)                   // config.rs:1088  anyref/i31/struct/array; NOT cfg-gated
    .wasm_exceptions(true)           // config.rs:1404  #[cfg(feature="gc")]; default on in 48
    .wasm_threads(true)              // config.rs:1004  #[cfg(feature="threads")]; shared memory + atomics
    .wasm_custom_page_sizes(true)    // config.rs:979   OFF by default
    .wasm_wide_arithmetic(true);     // config.rs:1071  OFF by default
```

Three gotchas in that list, all read out of `config.rs`:

- `wasm_threads` (1004) and `wasm_exceptions` (1404) / `wasm_function_references`
  (1060) are `#[cfg(feature = ...)]`: trim the cargo feature and the *call itself*
  stops existing (compile error). `wasm_gc` (1088) is **not** cfg-gated — trimming
  the `gc` cargo feature leaves the call compiling and moves the failure to
  `Engine::new`.
- `wasm_memory64`'s rustdoc still says "`false` by default", but
  `WasmFeatures::WASM3` contains `MEMORY64`, so it is on. Trust `WASM3`, not the
  rustdoc.
- Enabling a feature explicitly is not free — see the next block.

### Being explicit turns a silent downgrade into a startup panic

`Config::validate` (config.rs:2606) rejects *explicitly enabled* features that the
selected compiler/target/cargo-features cannot support, rather than dropping them:

```rust
let unsupported = features & self.compiler_panicking_wasm_features();     // 2613
if !unsupported.is_empty() {
    bail!("the wasm_{} feature is not supported on this compiler configuration", ...)  // 2619
}
if !cfg!(feature = "gc") && features.gc_types() {
    bail!("support for GC was disabled at compile time")                  // 2638-2639
}
if !cfg!(feature = "gc") && features.contains(WasmFeatures::EXCEPTIONS) {
    bail!("exceptions support requires garbage collection (GC) to be enabled in the build")  // 2642-2643
}
```

`compiler_panicking_wasm_features` (2412) marks `THREADS`/`STACK_SWITCHING`
unsupported on Pulley, and `GC`/`FUNCTION_REFERENCES`/`RELAXED_SIMD`/… unsupported
under Winch (2469-2472). Since `den-stdlib-wasm/src/engine.rs:26` is
`Engine::new(&config).unwrap()`, any such mismatch becomes a **panic during
`WebAssembly` module evaluation**, i.e. at interpreter startup, not a degraded
engine.

Runtime-verified: with wasmtime's default cargo features on
x86_64-unknown-linux-gnu + Cranelift, the exact list above yields `Ok` from
`Engine::new` (smoke test in §12). If you adopt it, either stop `unwrap()`-ing
`Engine::new` or keep only the two knobs that are genuinely off by default:

```rust
config.wasm_custom_page_sizes(true).wasm_wide_arithmetic(true);
```

New in 48 and relevant:

- `Config::wasm_features(WasmFeatures, bool)` — public bulk setter, config.rs:923.
  The type is re-exported as `wasmtime::WasmFeatures` (`config.rs:10 pub use
  wasmparser::WasmFeatures`), so you do not need a direct wasmparser dependency.
- `Config::guest_debug(bool)` — config.rs:504; makes `Module::debug_bytecode()`
  return the original wasm bytes (module.rs:705). An alternative to §5.
- `Config::wasm_shared_everything_threads` (1019), `wasm_stack_switching` (1252) —
  not needed for the JS-API.
- `Config::wasm_backtrace` is **deprecated** in favour of
  `wasm_backtrace_max_frames` (config.rs:520-521). den does not call it.
- `Config::wasm_legacy_exceptions` is `#[doc(hidden)] #[deprecated]` (config.rs:1409-1414).

### `Config::async_support` is a deprecated no-op — async is per-store now

`engine.rs:24` carries `// config.async_support(true);`. Uncommenting it in 48 does
**nothing** except emit a deprecation warning:

```rust
// wasmtime-48.0.0/src/config.rs:427-431
#[doc(hidden)]
#[deprecated(note = "no longer has any effect")]
#[cfg(feature = "async")]
pub fn async_support(&mut self, _enable: bool) -> &mut Self { self }
```

There is no engine-wide async mode in 48:

- `Func::call_async` (`func.rs:1113`, now `impl AsContextMut<Data: Send>`) is usable
  on *any* store whenever the `async` cargo feature is on — and it always is here:
  wasmtime's `default` list contains `async`, and `wasmtime-wasi`'s `p1` feature
  pulls `p2` which pulls `wasmtime/async`.
- `Func::call` (`func.rs:961`) begins with `store.0.validate_sync_call()?`
  (`store.rs:2247-2253`), which only fails once *that store* has been put into
  async-required mode — `Store::fuel_async_yield_interval` (`store.rs:1923`),
  `Store::set_debug_handler` (`store.rs:1208`), or an epoch async-yield deadline.
  The message is `store configuration requires that "*_async" functions are used
  instead`.

So the correct action is to **delete** the commented-out line, not to uncomment it.
If den ever wants async wasm calls, change the call site (`instance.rs:262`) to
`call_async` and leave `Config` alone; sync and async calls may coexist on one store
as long as no fuel/epoch/debug yield is configured.

---

## 5. `Module::customSections` — how to actually implement it

There is **no** `Module::custom_sections` in either version
(`grep "pub fn " wasmtime-48.0.0/src/runtime/module.rs` → `new, from_file,
from_binary, from_trusted_file, deserialize*, validate, serialize, name,
debug_bytecode, imports, exports, get_export, get_export_index, engine,
resources_required, image_range, initialize_copy_on_write_image, address_map,
text, functions, debug_index_in_store, same`). `serialize()` produces a
*compiled artifact*, not the original wasm — useless here.

Two viable routes:

1. **Keep the original bytes in den's `Module` wrapper** (recommended: no config
   coupling, works with any `Config`).
2. `Config::guest_debug(true)` + `Module::debug_bytecode() -> Option<&[u8]>`
   (module.rs:705). Costs debug-info retention on every module.

Parsing: enable the `reexport-wasmparser` cargo feature
(`wasmtime-48.0.0/Cargo.toml:170`, `src/lib.rs:541`) and use
`wasmtime::wasmparser`. That pins to wasmtime's exact wasmparser (0.254.0,
`Cargo.toml:369-372`) instead of adding a fourth wasmparser to the graph — the
lockfile already has 0.239.0, 0.254.0 and 0.257.1.

`den-stdlib-wasm/Cargo.toml`:

```toml
wasmtime = { version = "48.0.0", optional = true, features = ["reexport-wasmparser"] }
```

### BEFORE — `den-stdlib-wasm/src/module.rs:80-83`

```rust
#[qjs(static)]
pub fn custom_sections<'js>(_module: &Module, ctx: Ctx<'js>) -> Result<Vec<Object<'js>>> {
    Err(Exception::throw_internal(&ctx, "not implemented"))
}
```

### AFTER — verified to compile

```rust
#[derive(Trace, JsLifetime, Deref, DerefMut, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    #[deref]
    #[deref_mut]
    pub(crate) inner: wasmtime::Module,
    // wasmtime never retains the original bytes; `customSections` needs them.
    #[qjs(skip_trace)]
    pub(crate) bytes: Arc<[u8]>,
}

// ... in new_inner, after `Module::from_binary`:
Ok(Module { inner, bytes: Arc::from(buf) })

#[qjs(static)]
pub fn custom_sections<'js>(
    module: &Module,
    section_name: String,
    ctx: Ctx<'js>,
) -> Result<Vec<ArrayBuffer<'js>>> {
    use wasmtime::wasmparser::{Parser, Payload};

    Parser::new(0)
        .parse_all(&module.bytes)
        .filter_map(|payload| match payload {
            Ok(Payload::CustomSection(reader)) if reader.name() == section_name => {
                Some(Ok(reader.data()))
            }
            Ok(_) => None,
            Err(e) => Some(Err(Exception::throw_internal(
                &ctx, &format!("wasm parse error: {e}"),
            ))),
        })
        .map(|data| ArrayBuffer::new(ctx.clone(), data?))
        .collect()
}
```

API references: `Parser::parse_all(self, &[u8]) -> impl Iterator<Item = Result<Payload<'_>>>`
(`wasmparser-0.254.0/src/parser.rs:1083`), `Payload::CustomSection(CustomSectionReader<'a>)`
(parser.rs:334), `CustomSectionReader::name() -> &'a str` (readers/core/custom.rs:20),
`::data() -> &'a [u8]` (custom.rs:31).

Two more things to fix while you are here:

- The JS-API takes **two** arguments (`Module.customSections(module, sectionName)`),
  and returns `sequence<ArrayBuffer>`. The current stub takes one and returns
  `Vec<Object>`.
- `den-stdlib-wasm/src/lib.rs:130` wires `js_custom_sections` under
  `WebAssembly.Module.customSections`; the arity change flows through automatically.

Adding `bytes` as a second field breaks derive_more's `Deref`/`DerefMut`/`From`/`Into`
on `Module` — annotate `#[deref] #[deref_mut]` on `inner` and drop `From`/`Into`
(both verified).

---

## 6. Exception handling: `Tag`, `TagType`, `ExnRef` (den's empty `Tag` class)

`den-stdlib-wasm/src/tag.rs` is a 10-line empty shell. wasmtime 48 has the full
proposal, all re-exported at the crate root
(`src/runtime/externals.rs:10 pub use tag::Tag;`, `src/runtime/gc/enabled.rs:16
pub use exnref::*;`, `src/runtime.rs:95,97,111`):

| API | Location |
|---|---|
| `Tag::new(store: impl AsContextMut, ty: &TagType) -> Result<Tag>` | `src/runtime/externals/tag.rs:37` |
| `Tag::ty(&self, store: impl AsContext) -> TagType` | tag.rs:45 |
| `Tag::eq(a: &Tag, b: &Tag, store: impl AsContext) -> bool` | tag.rs:87 |
| `TagType::new(ty: FuncType) -> TagType` | `src/runtime/types.rs:3034` |
| `TagType::ty(&self) -> &FuncType` | types.rs:3039 |
| `TagType::default_value(&self, store) -> Result<Tag>` | types.rs:3053 |
| `FuncType::new(&Engine, params: impl IntoIterator<Item=ValType>, results: ...)` | types.rs:2387 |
| `ExnType::new(&Engine, fields: impl IntoIterator<Item=ValType>) -> Result<ExnType>` | types.rs:2809 |
| `ExnType::from_tag_type(&TagType) -> Result<ExnType>` | types.rs:2825 |
| `ExnRef::new(store, &ExnRefPre, &Tag, fields: &[Val]) -> Result<Rooted<ExnRef>>` | `src/runtime/gc/enabled/exnref.rs:209` |
| `ExnRefPre::new(store, ty: ExnType) -> Self` | exnref.rs:81 |
| `ExnRef::field(&self, store, index) -> Result<Val>` | exnref.rs:600 |
| `ExnRef::tag(&self, store) -> Result<Tag>` | exnref.rs:622 |
| `Instance::get_tag(&self, store, name) -> Option<Tag>` | `src/runtime/instance.rs:586` |
| `Val::ExnRef(Option<Rooted<ExnRef>>)` / `Ref::Exn(...)` | `src/runtime/values.rs:58`, `:769` |
| `HeapType::Exn` / `ConcreteExn(ExnType)` / `NoExn` | types.rs:803, 810, 838 |
| `RefType::EXNREF` / `NULLEXNREF`, `ValType::EXNREF` | types.rs:507, 513, 170, 173 |

### `WebAssembly.Tag` — verified to compile

```rust
use wasmtime::{AsContext, AsContextMut, FuncType, TagType, ValType};

#[derive(Trace, JsLifetime, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Tag {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Tag,
}

#[rquickjs::methods]
impl Tag {
    #[qjs(constructor)]
    pub fn new(desc: TagDescriptor, ctx: Ctx<'_>) -> Result<Self> {
        let engine = ctx.userdata::<crate::engine::Engine>().unwrap();
        let params = desc.parameters.iter().map(|name| match name.as_str() {
            "i32" => Ok(ValType::I32),
            "i64" => Ok(ValType::I64),
            "f32" => Ok(ValType::F32),
            "f64" => Ok(ValType::F64),
            "v128" => Ok(ValType::V128),
            "externref" => Ok(ValType::EXTERNREF),
            "anyfunc" | "funcref" => Ok(ValType::FUNCREF),
            x => Err(Exception::throw_type(&ctx, &format!("invalid value type {x}"))),
        }).collect::<Result<Vec<_>>>()?;

        // A JS Tag has parameters only; results are always empty.
        let ty = TagType::new(FuncType::new(&engine.inner, params, []));
        let store = ctx.userdata::<crate::store::Store>().unwrap();
        let inner = wasmtime::Tag::new(store.inner.borrow_mut().as_context_mut(), &ty)
            .map_err(|x| Exception::throw_internal(&ctx, &format!("wasm tag new error: {x}")))?;
        Ok(Self { inner })
    }

    #[qjs(rename = "type")]
    pub fn type_<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let store = ctx.userdata::<crate::store::Store>().unwrap();
        let ty = self.inner.ty(store.inner.borrow().as_context());
        let parameters = ty.ty().params().map(|p| p.to_string()).collect::<Vec<_>>();
        indexmap::indexmap! { "parameters" => parameters }.into_js(&ctx)
    }
}
```

`WebAssembly.Exception` (`den-stdlib-wasm/src/error.rs:4-11`, also an empty shell)
maps to `ExnRefPre` + `ExnRef::new`, with `.is(tag)` → `Tag::eq(&self.tag, tag, store)`
and `.getArg(tag, i)` → `ExnRef::field(store, i)`. `Rooted<ExnRef>` needs a
`RootScope` (`src/runtime/gc/enabled/rooting.rs:1379,1429`) or the store's LIFO
root set — plan for that before wiring it to a JS class that outlives a call.

---

## 7. wasmtime-wasi: `preview1` → `p1`

wasmtime-wasi 27 `src/lib.rs:198,200`: `pub mod preview0; pub mod preview1;`
wasmtime-wasi 48 `src/lib.rs:37-47`:

```rust
#[cfg(feature = "p1")] pub mod p0;
#[cfg(feature = "p1")] pub mod p1;
pub mod p2;                       // FIXME comment: not yet gated on `p2`
#[cfg(feature = "p3")] pub mod p3;
```

— `grep -rn "preview1\|preview0" wasmtime-wasi-48.0.0/src/lib.rs` returns **nothing**;
there is no deprecated alias.

**`p1` is now a cargo feature, and it is load-bearing.** `wasmtime-wasi-48.0.0/Cargo.toml`:

```toml
default = ["p1", "p2"]
p1 = ["dep:wiggle", "p2"]
p2 = ["wasmtime/component-model", "wasmtime/async"]
```

den declares `wasmtime-wasi = { version = "48.0.0", optional = true }` with default
features, so `wasmtime_wasi::p1` exists. Two implications: adding
`default-features = false` (a tempting "slim the build" change) silently deletes the
whole `p1` module, and keeping it means `wasmtime/async` + `wasmtime/component-model`
are force-enabled graph-wide — which is why `Func::call_async` is always available
(see §4).

| 27 | 48 |
|---|---|
| `wasmtime_wasi::preview1::WasiP1Ctx` | `wasmtime_wasi::p1::WasiP1Ctx` (`src/p1.rs:142`) |
| `wasmtime_wasi::preview1::add_to_linker_sync` | `wasmtime_wasi::p1::add_to_linker_sync` (`src/p1.rs:847`) |
| `wasmtime_wasi::preview1::add_to_linker_async` | `wasmtime_wasi::p1::add_to_linker_async` (`src/p1.rs:781`) |
| `wasmtime_wasi::preview0::*` | `wasmtime_wasi::p0::*` |
| `wasmtime_wasi::{WasiCtx, WasiCtxBuilder}` | unchanged, still crate root (`src/lib.rs:54`) |
| `wasmtime_wasi::{WasiImpl, WasiView}` (`27 lib.rs:210`) | `WasiView` moved to `self::view`, `WasiImpl` gone; `WasiCtxView` added (`48 lib.rs:58`) |

Signature change:

```rust
// 27, src/preview1.rs:804
pub fn add_to_linker_sync<T: Send>(
    linker: &mut wasmtime::Linker<T>,
    f: impl Fn(&mut T) -> &mut WasiP1Ctx + Copy + Send + Sync + 'static,
) -> anyhow::Result<()>

// 48, src/p1.rs:847
pub fn add_to_linker_sync<T: Send + 'static>(          // + 'static, and wasmtime::Result
    linker: &mut wasmtime::Linker<T>,
    f: impl Fn(&mut T) -> &mut WasiP1Ctx + Copy + Send + Sync + 'static,
) -> wasmtime::Result<()>
```

`WasiCtxBuilder` is unchanged for den's usage — `new()` (`src/ctx.rs:65`),
`inherit_stdio()` (ctx.rs:135), `inherit_env()` (ctx.rs:219), `build_p1()`
(ctx.rs:480) all still exist. New knobs worth knowing: `inherit_args`,
`initial_cwd`, `allow_tcp`/`allow_udp`/`inherit_network` (ctx.rs:240,249,390-428).
**Behaviour change in 48.0.0:** wasmtime-wasi now denies TCP/UDP socket creation
by default. den's WASI ctx never enabled sockets, so no impact.

`preopened_dir` changed shape (`DirPerms`/`FilePerms` → `FsPerms`, `OpenMode`,
`src/lib.rs:56`); den does not call it.

### BEFORE — `den-stdlib-wasm/src/instance.rs:220`

```rust
wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |(wasi_ctx, _)| wasi_ctx).unwrap();
```

### AFTER

```rust
wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |data| &mut data.wasi).unwrap();
```

(the tuple projection dies with the tuple; `data: &mut StoreData` from §2).

Aside: this `.unwrap()` on every `new WebAssembly.Instance()` also **re-registers
WASI into a fresh Linker each time** — `Linker::new` is at `instance.rs:197`. Not a
migration break, but a per-instantiation cost and a panic path.

---

## 8. APIs verified **unchanged** between 27 and 48

Everything below has the identical signature in both; no edits required beyond the
`T: 'static` ripple.

**Linker** (`src/runtime/linker.rs`)

```rust
pub fn new(engine: &Engine) -> Linker<T>                                     // 48:167, 27:—
pub fn func_new(
    &mut self, module: &str, name: &str, ty: FuncType,
    func: impl Fn(Caller<'_, T>, &[Val], &mut [Val]) -> Result<()> + Send + Sync + 'static,
) -> Result<&mut Self> where T: 'static                                      // 48:408, 27:389
pub fn define(&mut self, store: impl AsContext<Data = T>, module: &str, name: &str,
              item: impl Into<Extern>) -> Result<&mut Self> where T: 'static // 48:369, 27:350
pub fn instantiate(&self, store: impl AsContextMut<Data = T>, module: &Module)
    -> Result<Instance> where T: 'static                                     // 48:1090, 27:1107
```

The `func_new` closure shape is **byte-identical**; only `Result` now means
`wasmtime::Result`. den's closure at `instance.rs:71-111` compiles unmodified
once `T` is `'static`. New in 48: `define_unknown_imports_as_default_values`
(linker.rs:298).

**Module** — `validate(&Engine, &[u8]) -> Result<()>` (48:584 / 27:515),
`from_binary(&Engine, &[u8]) -> Result<Module>` (48:320 / 27:317),
`imports()` (48:758 / 27:672), `exports()` (48:824 / 27:744),
`engine()` (48:912 / 27:828). `ImportType::module()/name()` return `&'module str`
(types.rs:3568,3574); `ExportType::name()` likewise (types.rs:3631).

**Instance** — `exports<'a, T: 'static>(store: impl Into<StoreContextMut<'a,T>>)
-> impl ExactSizeIterator<Item = Export<'a>>` (48:390 / 27:394),
`get_func` (48:487), `get_global` (48:574), `get_table` (48:533),
`get_memory` (48:545). `Export::name` (externals.rs:216) and `Export::into_extern`
(externals.rs:230) — both used at `instance.rs:238` — are unchanged. New:
`get_tag` (48:586) and `get_shared_memory` (48:557); see §10 for why the latter
matters.

**Memory** — `new(store, MemoryType) -> Result<Memory>` (48:269 / 27:237),
`ty(store)` (48:333), `data<'a, T: 'static>` (48:408), `data_mut` (48:425),
`data_ptr(store) -> *mut u8` (48:479), `data_size(store) -> usize` (48:506),
`size(store) -> u64` (48:531), `grow(store, delta: u64) -> Result<u64>` (48:637).
`MemoryTypeBuilder` unchanged: `Default` (48:3205), `new` (48:3230),
`min(u64)` (48:3280), `max(Option<u64>)` (48:3289), `shared(bool)` (48:3318),
`memory64(bool)`, `page_size_log2(u8)` (48:3342), `build() -> Result<MemoryType>`
(48:3353). `SharedMemory::page_size` changed `u32` → `u64` (48:909 vs 27:840).

**Table** — `new(store, TableType, init: Ref) -> Result<Table>` (48:98 / 27:76),
`ty` (48:166), `get(store, index: u64) -> Option<Ref>` (48:202),
`set(store, index: u64, val: Ref) -> Result<()>` (48:255),
`size(store) -> u64` (48:289), `grow(store, delta: u64, init: Ref) -> Result<u64>` (48:324),
`copy`, `fill`. **`TableType::new(element: RefType, min: u32, max: Option<u32>)`
still takes `u32`** (48:3076) — `new64(element, u64, Option<u64>)` is the memory64
form (48:3103). `minimum()`/`maximum()` return `u64`/`Option<u64>` in both
(48:3135,3143), which is why `den`'s `as u32` casts at `instance.rs:326-327` still
compile.

**Global** — `new(store, GlobalType, Val) -> Result<Global>` (48:99 / 27:74),
`ty` (48:135), `get(store) -> Val` (48:148), `set(store, Val) -> Result<()>` (48:233).
`GlobalType::new(ValType, Mutability)` (48:2973), `content()`, `mutability()`.
`Mutability::{Const, Var}` unchanged.

**Func** — `new<T: 'static>(store, FuncType, closure) -> Self` (48:374; now panics
on OOM, `try_new` added at 48:387), `call(store, &[Val], &mut [Val]) -> Result<()>`
(48:961), `wrap` (48:809), `typed<Params, Results>` (48:1414), `ty(store)` (48:872).
`call_async` re-shaped to `impl AsContextMut<Data: Send>` (48:1113) from
`impl AsContextMut<Data = T> where T: Send` (27:1149). `Caller<'a, T: 'static>`
(48:2003), `Caller::data(&self) -> &T` (48:2125), `data_mut` (48:2132).

**Val / ValType / RefType / HeapType** — `ValType::{I32,I64,F32,F64,V128,Ref}`,
the 14 `RefType`/`ValType` constants (types.rs:435-513, 134-173), `matches()`
(types.rs:295, 545, 1153), `Val::null_func_ref/null_extern_ref/null_any_ref`
(values.rs:105,114,123 — now `const fn`), `Val::null_ref(&HeapType)` (values.rs:96),
`Val::default_for_ty(&ValType) -> Option<Val>` (values.rs:131),
`Val::ty(store) -> Result<ValType>` (values.rs:159), `Val::matches_ty` (values.rs:198).
`Val` is `#[derive(Debug, Clone, Copy)]` in both (values.rs:22); `Ref` is
`#[derive(Debug, Clone)]` — **not `Copy`** — in both (48:702, 27:592).

New `Val`/`Ref`/`HeapType` variants in 48 (all reachable only via GC / EH /
stack-switching; den's matches all have catch-alls so they still compile):

- `Val::ExnRef(Option<Rooted<ExnRef>>)` (values.rs:58), `Val::ContRef(Option<ContRef>)` (values.rs:64)
- `Ref::Exn(Option<Rooted<ExnRef>>)` (values.rs:769)
- `HeapType::{Exn, ConcreteExn(ExnType), Cont, ConcreteCont(ContType), NoCont, NoExn}`
  (types.rs:803,810,815,821,827,838)

Other useful additions: `ExternType::default_value(store) -> Result<Extern>`
(types.rs:1525), `ValType::default_value() -> Option<Val>` (types.rs:375),
`TableType::default_value` (types.rs:3157), `V128::as_u128`/`From<u128>`
(v128.rs:33,39), `I31::{new_u32,new_i32,wrapping_u32,wrapping_i32,get_u32,get_i32}`
(gc/enabled/i31.rs:126-218), `AnyRef::{from_i31,as_i31,unwrap_i31}`
(gc/enabled/anyref.rs:176,524,545), `ExternRef::new<T: 'static + Any + Send + Sync>`
(gc/enabled/externref.rs:216).

---

## 9. Val ↔ JS conversion per the JS-API spec — and three pre-existing bugs

The JS-API `ToJSValue` / `ToWebAssemblyValue` rules:
`i32` ↔ Number (`ToInt32`), `i64` ↔ **BigInt** (`ToBigInt64`), `f32`/`f64` ↔ Number,
`v128` → throws `TypeError`, `externref` ↔ any JS value, `funcref` ↔ Function or null,
`anyref`/`i31ref` ↔ Object or unboxed int.

wasmtime encodes floats as **raw bits**: `Val::F32(u32)`, `Val::F64(u64)`
(values.rs:33-43, "the raw bits of the float are stored here"), with
`Val::f32()` → `f32::from_bits(*e)` and `Val::f64()` → `f64::from_bits(*e)`
(values.rs:369-370), and `From<f32> for Val` → `Val::F32(val.to_bits())` (values.rs:554).

### Bug 1 — floats handed to JS as raw bit patterns

`den-stdlib-wasm/src/utils.rs:13-14`:

```rust
wasmtime::Val::F32(x) => Ok(x.into_js(ctx)?),   // x: u32 raw bits  -> JS sees 1065353216 for 1.0f32
wasmtime::Val::F64(x) => Ok(x.into_js(ctx)?),   // x: u64 raw bits
```

Fix:

```rust
wasmtime::Val::F32(x) => f32::from_bits(x).into_js(ctx),
wasmtime::Val::F64(x) => f64::from_bits(x).into_js(ctx),
```

### Bug 2 — an f32 result slot is filled with an `F64`

`den-stdlib-wasm/src/instance.rs:99-105`:

```rust
if matches!(result, Val::F32(_)) && item.f64().is_some() {
    *result = item.f64().unwrap().into();   // f64 -> Val::F64, but the slot must be Val::F32
}
```

`Func::store_untyped_results` calls `ret.ensure_matches_ty(store, &ty)` with
`.context("function attempted to return an incompatible value")`
(`src/runtime/func.rs:2565-2567`), so this is a guaranteed runtime error. Fix:
`*result = Val::F32((item.f64().unwrap() as f32).to_bits());`

That patch is necessary but **not sufficient**: `item.f64()` is `Some` only for
`Val::F64`, and `WasmValueConverter::from_js` maps a JS *integer* to `Val::I32`
(`utils.rs:28`), never to a float. So `(func (result f32))` or `(result f64)` whose
JS import returns `1` still stores a `Val::I32` into a float slot and hits the same
`ensure_matches_ty` error. The only correct shape is to convert against the expected
type: zip `results` with `func_type.results()` and build the `Val` from the
`ValType`, exactly as §9 bug 3 requires for the null cases. Do both in one pass.

### Bug 3 — `null`/`undefined` coerced to `anyref`

`utils.rs:24-26` maps JS `null`/`undefined`/uninitialized to `Val::null_any_ref()`
regardless of the expected param type. Per spec these should become
`Val::null_func_ref()` for `funcref` params and `Val::null_extern_ref()` for
`externref`. `FromJs` has no type context — this conversion needs the target
`ValType` threaded in (`Instance::exports` already has `func_type.params()` at
`instance.rs:251-252`).

### `get_default_value_for_val_type` is dead weight

`den-stdlib-wasm/src/utils.rs:36-54` reimplements `Val::default_for_ty`, which has
existed since **27** (`27 values.rs:116`, `48 values.rs:131`). Verified replacement:

```rust
pub(crate) fn get_default_value_for_val_type(x: &wasmtime::ValType)
    -> std::result::Result<wasmtime::Val, ()>
{
    wasmtime::Val::default_for_ty(x).ok_or(())
}
```

…or delete the helper and inline `Val::default_for_ty` at
`instance.rs:256` and `instance.rs:303`.

---

## 10. Other pre-existing bugs this port should sweep up

### `Ref::Any(None)` on a FUNCREF table — confirmed a bug

`den-stdlib-wasm/src/table.rs:63-68`:

```rust
"anyfunc" => (TableType::new(RefType::FUNCREF, desc.initial, desc.maximum),
              Ref::Any(None)),                    // wrong hierarchy
```

`Table::new` runs `init.ensure_matches_ty(store, ty.element())` with
`.context("type mismatch: value does not match table element type")`
(`src/runtime/externals/table.rs:130-131`), and `Ref::_matches_ty` only accepts
`(Ref::Any(_), HeapType::Any | ...)` against an `any`-hierarchy element
(values.rs:1067-1074). A `funcref` table gets `bail!("type mismatch: expected {ty},
found {actual_ty}")` from values.rs:1128. **Correct value: `Ref::Func(None)`** —
wasmtime's own doctest does exactly that (`table.rs:81`:
`Table::new(&mut store, ty, Ref::Func(None))?`).

### `heap_type().to_string()` never matches den's own element names

`den-stdlib-wasm/src/instance.rs:324` builds a `TableDescriptor` with
`.element(ty.element().heap_type().to_string())`, then feeds it back into
`Table::new_inner`, which only accepts `"externref"` or `"anyfunc"`
(`table.rs:56-74`). But `HeapType`'s `Display` (types.rs:843-864) writes
`"func"`, `"extern"`, `"any"`, `"nofunc"`, … — never `"anyfunc"`/`"externref"`.
Every re-created export table throws
`"Either externref or anyfunc is accepted for element type, found func"`.

Verified mapping helper (put it on `Table`, per the repo's no-free-functions rule):

```rust
use wasmtime::HeapType;

impl Table {
    /// JS-API `TableKind` name for a wasmtime table element type.
    pub(crate) fn element_name(ty: &wasmtime::TableType) -> &'static str {
        match ty.element().heap_type() {
            HeapType::Func | HeapType::ConcreteFunc(_) | HeapType::NoFunc => "anyfunc",
            HeapType::Extern | HeapType::NoExtern => "externref",
            HeapType::Any | HeapType::None => "anyref",
            HeapType::Eq => "eqref",
            HeapType::I31 => "i31ref",
            HeapType::Struct | HeapType::ConcreteStruct(_) => "structref",
            HeapType::Array | HeapType::ConcreteArray(_) => "arrayref",
            HeapType::Exn | HeapType::ConcreteExn(_) | HeapType::NoExn => "exnref",
            HeapType::Cont | HeapType::ConcreteCont(_) | HeapType::NoCont => "contref",
        }
    }
}
```

Note this `match` is now exhaustive over 48's larger `HeapType`; a wildcard would
hide the next addition.

### `new WebAssembly.Memory({shared: true})` cannot work through `Memory::new`

`den-stdlib-wasm/src/memory.rs:53` sets `.shared(opts.shared.unwrap_or(false))`,
then calls `wasmtime::Memory::new`. 48's `Memory::_new` starts with
`if ty.is_shared() { bail!("shared memories must be created through `SharedMemory`") }`
(`src/runtime/memory.rs:303-305`). Route shared memories through
`SharedMemory::new(engine, ty)` (memory.rs:875, `#[cfg(feature = "threads")]`),
which additionally requires a maximum
(`MemoryTypeBuilder::validate`: `if self.ty.shared && self.ty.limits.max.is_none()
{ bail!("shared memories must have a maximum size") }`, types.rs:3253-3255).
`Extern::SharedMemory(SharedMemory)` is a distinct variant (externals.rs:34).

### A *shared memory export* also breaks `Instance::exports` — new failure in 48

Same root cause, different site, and this one is newly reachable because `THREADS`
is in `WasmFeatures::WASM3` (§4). `den-stdlib-wasm/src/instance.rs:334-346` handles
exported memories as:

```rust
wasmtime::ExternType::Memory(ty) => {
    let memory = if let Some(memory) = self.get_memory(&mut *store, &name) { Ok(memory) }
                 else { wasmtime::Memory::new(&mut *store, ty) }        // <- bails for shared
```

but for a shared memory wasmtime reports the *type* as `Memory` while the *extern*
is a different variant:

```rust
// externals.rs:116-126
Extern::Memory(ft)       => ExternType::Memory(ft.ty(store)),
Extern::SharedMemory(ft) => ExternType::Memory(ft.ty()),
```

and `Instance::get_memory` is `self.get_export(store, name)?.into_memory()`
(instance.rs:545-547), where `into_memory` returns `None` for
`Extern::SharedMemory` (externals.rs:78-83). So the `else` branch runs and
`Memory::new` bails with *"shared memories must be created through `SharedMemory`"*.
Any `(memory 1 1 shared)` export throws. Fix: try
`Instance::get_shared_memory(store, name)` (instance.rs:557) first, or branch on
`ty.is_shared()` before the fallback.

### The single `RefCell<Store>` is re-entrant-hostile (pre-existing)

Not a wasmtime break, but the port makes it easy to hit: every JS-visible entry
point does `ctx.userdata::<Store>().unwrap().borrow_mut()`, and
`instance.rs:262` holds that borrow across the whole `func.call(...)`. If the wasm
call re-enters JS through an import (`instance.rs:67-111`) and that JS constructs
`new WebAssembly.Memory/Table/Global` or calls another export, the second
`borrow_mut()` panics with *already mutably borrowed*. The host-import closure
itself is safe because it goes through `Caller`, not the userdata store. Worth a
`try_borrow_mut()` + thrown `RuntimeError` at minimum.

### `Memory.prototype.buffer` still throws

`den-stdlib-wasm/src/memory.rs:72-97` computes `data_mut(...)` then throws `"TODO"`.
The wasmtime side is ready: `data_ptr(store) -> *mut u8` (memory.rs:479) +
`data_size(store) -> usize` (memory.rs:506) feed the commented-out
`qjs::JS_NewArrayBuffer` block. Out of scope for the version bump but blocked on
nothing in wasmtime 48.

---

## 11. Ordered work list

1. `den-stdlib-wasm/Cargo.toml` — add `features = ["reexport-wasmparser"]` to
   `wasmtime`; drop the unused `anyhow` dep.
2. `src/store.rs` — replace `StoreData<'js>` with the `'static` `StoreData`
   struct (§2); delete the `'js` parameter from `Store` and the hand-written
   `JsLifetime` impl; `preview1` → `p1`.
3. `src/instance.rs` — drop `<'js>` from `StoreData`/`Linker<StoreData>` in
   `resolve_imports`/`make_linker`; `caller.data()` tuple destructure →
   `caller.data().ctx()`; `preview1::add_to_linker_sync(..., |(a,_)| a)` →
   `p1::add_to_linker_sync(..., |d| &mut d.wasi)`; add the `ExternType::Tag` arm;
   replace `_ => unreachable!()` at line 166; fix the F32 result store (§9 bug 2);
   use `Table::element_name` at line 324.
4. `src/module.rs` — add the `ExternType::Tag(_) => "tag"` arm; add `bytes: Arc<[u8]>`
   + `#[deref]`; implement `custom_sections(module, name)` (§5).
5. `src/table.rs` — `Ref::Any(None)` → `Ref::Func(None)`; add `element_name`.
6. `src/utils.rs` — fix float bit conversions; delegate to `Val::default_for_ty`.
7. `src/engine.rs` — delete the dead `// config.async_support(true);` line (§4:
   deprecated no-op in 48); optionally the explicit `Config` knobs, but then stop
   `unwrap()`-ing `Engine::new` (§4).
8. `src/tag.rs` / `src/error.rs` — implement `Tag` (§6) and, optionally, `Exception`.
9. `src/instance.rs:334` — route shared-memory exports through
   `Instance::get_shared_memory` (§10); today they throw.

Non-wasmtime blockers found in the same build (out of scope here, but the crate
will not compile without them): `wabt::wat2wasm` → the `wat` crate already in
`Cargo.toml:24`; `getset` is not a declared dependency (`module.rs:5`);
`rquickjs` 0.12 rejects bare `JsClass` struct fields (`lib.rs:33` `ResultObject`
must hold `Class<'js, Module>` / `Class<'js, Instance>`); edition-2024 rejects
`Either::Left(ref x)` at `lib.rs:67-68`.

Toolchain: wasmtime 48 and wasmtime-wasi 48 both declare `rust-version = "1.95.0"`;
the workspace is `rust-version = "1.97"` on `channel = "stable"`
(`rust-toolchain.toml`), so there is no MSRV work.


---

## 12. Verification log

Second pass (completeness/accuracy audit) over this document. Everything below was
re-read from the local crate sources listed at the top; nothing was taken from the
original text on trust.

### Runtime smoke test (new evidence, not just a compile)

The scratch tree at
`/tmp/claude-1000/-home-steve-git-github-com-stevefan1999-personal-den/0aae9ff5-defc-4c2f-8b83-11b508e5f823/scratchpad/den/`
still builds clean (`cargo check -p den-stdlib-wasm` → 2 warnings, 0 errors). A
temporary `den-stdlib-wasm/tests/engine_smoke.rs` was added there and **passed** on
x86_64-unknown-linux-gnu / Cranelift, proving four claims that compilation alone
does not:

- `Engine::new` with the full §4 knob list returns `Ok` (does not trip
  `Config::validate`).
- `Table::new(.., FUNCREF ty, Ref::Func(None))` succeeds and
  `Table::new(.., FUNCREF ty, Ref::Any(None))` **errors** → §10 bug 1 is real.
- `Memory::new` with a `shared(true)` `MemoryType` **errors** → §10 bug 3 is real.
- `Tag::new(&mut store, &TagType::new(FuncType::new(&engine, [I32], [])))` succeeds
  → the §6 `WebAssembly.Tag` sketch is usable, not just compilable.

### Claims checked and CONFIRMED

| Claim | Where verified |
|---|---|
| `pub struct Store<T: 'static>`, `type Data: 'static` | store.rs:196, store/context.rs:11,19,36 |
| `ExternType::Tag(TagType)` is a new 5th variant | types.rs:1445-1455 |
| `Extern::ty` maps `SharedMemory → ExternType::Memory` | externals.rs:116-126 |
| `p1`/`p0` replace `preview1`/`preview0`, no alias | wasmtime-wasi lib.rs:37-47 |
| `WasiP1Ctx` p1.rs:142, `add_to_linker_sync` p1.rs:847 `<T: Send + 'static> → wasmtime::Result` | p1.rs:781,847 |
| `WasiCtxBuilder::{new,inherit_stdio,inherit_env,build_p1}` | ctx.rs:65,135,219,480 |
| TCP/UDP default flipped `true` (27 ctx.rs:672-678) → `false` (48 ctx.rs:410-431) | both |
| `Ctx::from_raw` does `JS_DupContext`; `Drop` does `JS_FreeContext`; `unsafe impl Send for Ctx` | rquickjs ctx.rs:442-443, 97-101, 103 |
| `reexport-wasmparser` is a real (empty, off-by-default) feature; `pub use wasmparser` | Cargo.toml:170, lib.rs:540-541 |
| `Parser::parse_all` / `Payload::CustomSection` / `name()` / `data()` | wasmparser parser.rs:1083,334; readers/core/custom.rs:20,31 |
| wasmtime pins wasmparser `0.254.0` (`default-features = false`, `simd`); `mod parser` is ungated so `Parser` is reachable | wasmtime Cargo.toml:369-372, wasmparser lib.rs:1332 |
| `WASM3` contents and `features.set(GC_TYPES/EXCEPTIONS/THREADS, cfg!(...))` | wasmparser features.rs:398-417, wasmtime config.rs:2526-2553 |
| Cargo `default` really does contain `gc`, `gc-*`, `threads`, `async`, `component-model` | wasmtime Cargo.toml default block |
| `Module::{from_binary 320, validate 584, imports 758, exports 824, engine 912, debug_bytecode 705}`; no `custom_sections` | module.rs |
| `Module::imports/exports` return `impl ExactSizeIterator` (den relies on `.len()`) | module.rs:758-760, 824-826 |
| `Linker::{new 167, define 369, func_new 408, instantiate 1090}` all `where T: 'static`; `func_new` closure shape unchanged | linker.rs |
| `Instance::{exports 390, get_func 487, get_table 533, get_memory 545, get_shared_memory 557, get_global 574, get_tag 586}` | instance.rs |
| `Export::{name 216, into_extern 230}` unchanged (den uses both at instance.rs:238) | externals.rs |
| `Memory::{new 269, ty 333, data 408, data_mut 425, data_ptr 479, data_size 506, size 531, grow 637}` | memory.rs |
| `MemoryTypeBuilder::{min 3280, max 3289, memory64 3303, shared 3318, page_size_log2 3342, build 3353}` | types.rs |
| `TableType::new(RefType, u32, Option<u32>)` still `u32` | types.rs:3076 |
| `HeapType` has 19 variants, is **not** `#[non_exhaustive]`, and §10's `element_name` match covers all 19 | types.rs:720-840 |
| `Val::F32(u32)` / `F64(u64)` raw bits; `default_for_ty` | values.rs:37,43,131 |
| `ensure_matches_ty` + `"function attempted to return an incompatible value"` | func.rs:2565-2567 |
| `Func::call` 961 / `call_async` 1113 / `validate_sync_call` 968 | func.rs |
| Tag/TagType/ExnRef API surface | externals/tag.rs:36,45,87; types.rs:3032-3053; gc/enabled/exnref.rs |
| `Cargo.lock` already at wasmtime 48.0.0; `anyhow` genuinely unused in `den-stdlib-wasm/src` | Cargo.lock:4980-4982, grep |
| Nothing outside `den-stdlib-wasm` names `Store<'js>` | den-core/src/engine.rs:170,289 |

### Claims found WRONG and corrected in place

1. **§4, `Config::async_support`.** The doc said "do not call it unless you switch
   every `Func::call` to `call_async`; 48 turned the old panic into a hard error".
   In 48 the method is `#[doc(hidden)] #[deprecated(note = "no longer has any
   effect")]` and its body is `self` (config.rs:427-431) — calling it does nothing
   at all. Async-ness moved onto the *store*
   (`Store::set_async_required(Asyncness)`, store.rs:2267; set by
   `fuel_async_yield_interval` 1923 / `set_debug_handler` 1208), and
   `validate_sync_call` (store.rs:2247-2253) only fires for those stores.
   §4 was rewritten; §11 step 7 now says *delete* the commented-out line.

2. **§4 knob annotations.** `wasm_gc` is **not** `#[cfg(feature = "gc")]`
   (config.rs:1088) — it compiles with the feature off and fails at `Engine::new`
   instead; `wasm_function_references` (1060) **is** gc-gated, which the doc omitted.
   Also noted that `wasm_memory64`'s own rustdoc ("false by default") contradicts
   `WASM3`.

3. **Line-number drift** (all corrected): `Tag::new` 36 (was 37), `Tag::ty` 45 (46),
   `Tag::eq` 87 (88), `TableType::new64` 3103 (3101), `TableType::minimum/maximum`
   3135/3143 (3132/3141), `MemoryTypeBuilder::page_size_log2` 3342 (3343),
   `Val::default_for_ty` 131 (130).

4. **§0's error list is a merge of two runs.** A fresh `cargo check` of HEAD's
   `den-stdlib-wasm` (dropped into the warm scratch workspace) produces **30**
   errors, not 17, and the `'js must outlive 'static` diagnostics do *not* appear
   while `wasmtime_wasi::preview1` is still unresolved — the broken import poisons
   `StoreData`, so those sites report cascade errors (`E0599 no method named
   borrow_mut for UserDataGuard<Store<'_>>`, `E0614`, `str` sizedness) instead.
   Fix `preview1` → `p1` first and the list in §0 is what you get. Left as-is
   because the *set of edits* it implies is correct; do not expect the exact text.

### Gaps found and FILLED

- **§2** — what the parked `Ctx<'static>` actually costs: userdata lives on the
  *runtime* opaque (opaque.rs:57,180; ctx.rs:480-485), so the JSContext refcount is
  only released by `RawRuntime::drop`'s `opaque.clear()` before `JS_FreeRuntime`
  (raw.rs:123-131) — no UAF, no permanent leak, but the context cannot be reclaimed
  early and a multi-context runtime gets the wrong `Ctx`. Plus: `StoreData::ctx()`
  is a *safe* fn minting an unbounded `'js`; it should be `unsafe`/crate-private.
- **§4** — `Config::validate` (2606-2643) turns explicitly-enabled-but-unsupported
  features into `Engine::new` errors, and `engine.rs:26` unwraps → startup panic.
  Added the failure list, the Winch/Pulley cases (2412-2472), and the minimal
  two-knob alternative.
- **§7** — `wasmtime_wasi::p1` is `#[cfg(feature = "p1")]` with
  `default = ["p1","p2"]`, `p1 = ["dep:wiggle","p2"]`,
  `p2 = ["wasmtime/component-model","wasmtime/async"]`. `default-features = false`
  silently deletes the module; keeping it is what makes `wasmtime/async` always on.
- **§10** — new bug: **shared-memory exports throw** in `Instance::exports`
  (instance.rs:334-346), because `Extern::SharedMemory` types as
  `ExternType::Memory` (externals.rs:122) while `get_memory` → `into_memory()`
  returns `None` for it (instance.rs:545-547, externals.rs:78-83), so the fallback
  `Memory::new` hits the shared-memory bail. Use `Instance::get_shared_memory`
  (instance.rs:557). Newly reachable in 48 because `THREADS` is in `WASM3`.
- **§10** — the single `RefCell<Store>` panics on re-entrancy (JS import that
  constructs a `Memory`/`Table`/`Global` or calls another export while
  `instance.rs:262` holds `borrow_mut()`).
- **§9 bug 2** — the proposed one-line fix is necessary but insufficient: a JS
  *integer* returned for an `f32`/`f64` result becomes `Val::I32` (utils.rs:28) and
  still fails `ensure_matches_ty`. Conversion must be driven by
  `func_type.results()`, same pass as bug 3.
- **§11** — added the shared-memory step, the `async_support` deletion, and an MSRV
  line (both wasmtime crates declare `rust-version = "1.95.0"`; toolchain is stable
  1.97).

### Not verified

- The 27→48 *per-release* attribution (which of 28…47 introduced each break) —
  unchanged from the original doc: `RELEASES.md` is not vendored.
- `Table::{ty 166, get 202, set 255, size 289, grow 324}` and the `ExnRef`/`ExnType`
  line numbers in §6's table were spot-checked only for existence, not line-by-line.
- §5's `customSections` and §6's `Tag` code compile (scratch tree) but were not
  exercised from JavaScript; no JS-API conformance test was run.
