# 05 — WebAssembly JS API: full required surface & den conformance checklist

Status: research note. Written 2026-08-22 against the specs and crate sources listed below.
Audience: whoever implements `den-stdlib-wasm` against wasmtime 48 / rquickjs 0.12.

## 0. Sources actually read

Normative:

| Spec | URL | Snapshot |
|---|---|---|
| WebAssembly JavaScript Interface (the merged ED — includes Tag/Exception/GC/multi-memory/memory64) | <https://webassembly.github.io/spec/js-api/> | Editor's Draft, 12 August 2026 |
| WebAssembly Web API (streaming) | <https://webassembly.github.io/spec/web-api/> | same series |
| Exception handling JS API (proposal repo overlay) | <https://webassembly.github.io/exception-handling/js-api/> | diverges from the merged ED, see §2.8 |
| Threads JS API overlay (`shared`, `SharedArrayBuffer`) | <https://webassembly.github.io/threads/js-api/> | not merged into the ED |
| JS type reflection overlay (`type()`, descriptor `type` member) | <https://webassembly.github.io/js-types/js-api/> | not merged into the ED |

Local crate sources:

- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-48.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmtime-wasi-48.0.0/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rquickjs-core-0.12.2/`
- `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wat-1.257.1/`

den source under review: `/home/steve/git/github.com/stevefan1999-personal/den/den-stdlib-wasm/src/` (all 11 files, 1088 lines).

**Important framing:** the manifests in the working tree have *already* been bumped
(`git diff HEAD -- den-stdlib-wasm/Cargo.toml`): wasmtime `27.0.0 → 48.0.0`,
wasmtime-wasi `27 → 48`, wasmi `0.40 → 1.1.0`, rquickjs `0.8.1 → 0.12.2`,
`wabt 0.10` and `getset 0.1.3` **removed**, `wat 1.257.1` added. None of the Rust source
has been updated. The crate therefore does not compile at all (§1). Every "current status"
below describes the *intent* of the existing code plus the compile blocker.

---

## 1. Build state: `den-stdlib-wasm` does not compile (30 errors)

Reproduced with `cargo check -p den-stdlib-wasm`, re-measured 2026-08-22 (§6): exactly
30 errors + 1 warning, and `den-stdlib-core` now builds clean, so no workaround is needed to reach
the wasm crate.

The 30 do **not** include the `Store<T: 'static>` violation of §1.1 — the unresolved
`wasmtime_wasi::preview1` import poisons `StoreData` and masks it. Fixing only §1.2 and §1.3 drops
the count to 17 and surfaces §1.1 as the dominant remaining blocker. Budget for that: the error
count is not monotonically decreasing evidence of progress.

### 1.1 Blocker A — `wasmtime::Store<T>` now requires `T: 'static`

`wasmtime-48.0.0/src/runtime/store.rs:196`:

```rust
pub struct Store<T: 'static> {
```

den stores the JS context *inside* the wasm store — `den-stdlib-wasm/src/store.rs:7`:

```rust
pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);
```

`Ctx<'js>` is not `'static`, so `wasmtime::Store<StoreData<'js>>` is now ill-formed. This
cascades into every `AsContext<Data = StoreData<'js>>` bound (`instance.rs:28,32,191,196`) and
into `Instance::exports` (`wasmtime-48.0.0/src/runtime/instance.rs:390`,
`pub fn exports<'a, T: 'static>`).

Observed diagnostics (measured, see §6): the primary errors are
**`error[E0477]: the type `(WasiP1Ctx, rquickjs::Ctx<'js>)` does not fulfill the required
lifetime`** at `store.rs:9` and `store.rs:13`, plus nine cascading
`lifetime may not live long enough` / `E0521 borrowed data escapes` errors at
`store.rs:9,30`, `global.rs:45`, `instance.rs:157,219`, `memory.rs:62,73,100`, `table.rs:88`.

Note: the `str is not Sized` errors are **not** caused by this blocker — they are downstream of
Blocker C's `borrow_mut` resolution failure (§1.3) and disappear the moment `store.inner` is named.

Escape hatch (verified available): carry the raw context pointer instead of `Ctx`.
`rquickjs-core-0.12.2/src/context/ctx.rs:512` `pub fn as_raw(&self) -> NonNull<qjs::JSContext>`
and `:442` `pub unsafe fn from_raw(ctx: NonNull<qjs::JSContext>) -> Self`.
`NonNull<JSContext>` is `'static`.

```rust
// BEFORE (den-stdlib-wasm/src/store.rs:7)
pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);

// AFTER
pub struct StoreData { pub wasi: WasiP1Ctx, pub ctx: NonNull<qjs::JSContext> }
// and inside a host callback:
//   let ctx = unsafe { Ctx::from_raw(caller.data().ctx) };
```

**Trap in this fix — `Send`.** `NonNull<T>` is `'static` but it is **not `Send`**, and the current
code path needs `Send`:

- `wasmtime_wasi::p1::add_to_linker_sync<T: Send + 'static>`
  (`wasmtime-wasi-48.0.0/src/p1.rs:847`) — called at `instance.rs:220`. The *existing*
  `(WasiP1Ctx, Ctx<'js>)` satisfies `Send` (rquickjs has an unconditional
  `unsafe impl Send for Ctx<'_>` at `rquickjs-core-0.12.2/src/context/ctx.rs:103`) and fails only
  `'static`; the `NonNull` rewrite flips that around and fails `Send` instead.
- `Linker::func_new`'s closure is `impl Fn(..) + Send + Sync + 'static`
  (`wasmtime-48.0.0/src/runtime/linker.rs:408-414`), which is why `instance.rs:54-60` already
  carries the `DangerouslyImplementSync` newtype.

So the `StoreData` rewrite must either wrap the pointer in a newtype with `unsafe impl Send`
(sound only because den drives QuickJS single-threaded — document that invariant), or drop the
WASI injection entirely, which §3 item 28 recommends on independent grounds. Do not skip this:
following §1.1 verbatim trades one compile error for a new `E0277` at `instance.rs:220`.

### 1.2 Blocker B — missing / renamed dependencies

| Site | Symbol | Problem | Fix |
|---|---|---|---|
| `src/module.rs:5,10,14` | `getset::Getters` | `getset` removed from Cargo.toml | three sites, not two: the `use` (`:5`), the `Getters` entry in the `#[derive(...)]` list (`:10`), and the `#[getset(get)]` field attribute (`:14`). Drop all three; `Module` already derives `Deref` to `wasmtime::Module` |
| `src/lib.rs:103` | `wabt::wat2wasm` | `wabt` removed, never in the lockfile | `wat::parse_str(&str) -> Result<Vec<u8>>` (`wat-1.257.1/src/lib.rs:193`) |
| `src/store.rs:5`, `src/instance.rs:220` | `wasmtime_wasi::preview1` | module renamed | `wasmtime_wasi::p1` (`wasmtime-wasi-48.0.0/src/lib.rs:41`) |

### 1.3 Blocker C — rquickjs 0.12 API changes

| Site | Error | Cause / fix |
|---|---|---|
| `lib.rs:33,36,38` | *"using a `JsClass` type directly as a class field is not supported"* | `ResultObject`'s `module`/`instance` fields must be `Class<'js, Module>` / `Class<'js, Instance>`. (Moot — §2.1 says this should be a plain object, not a class.) |
| `global.rs:68` (`borrow_mut`), `global.rs:70` (`borrow`); `instance.rs:219,222,234`; `memory.rs:64,74,102`; `table.rs:88` | *"no method named `borrow_mut` found for `UserDataGuard<'_, Store<'_>>`"* (and `borrow` at `global.rs:70`) | `UserDataGuard: Deref<Target = U>` (`rquickjs-core-0.12.2/src/runtime/userdata.rs:180`) but method resolution now picks `core::borrow::{Borrow, BorrowMut}` (needs `&mut self`) before reaching `RefCell`. Fix: name the field — `store.inner.borrow_mut()`. |
| `lib.rs:67,68` | *"cannot explicitly borrow within an implicitly-borrowing pattern"* | edition 2024 match-ergonomics: `Either::Left(ref x)` inside a `&`-matched scrutinee. Drop the `ref`. |

Cascade from the `borrow_mut` row (five `E0277 str is not Sized` at `instance.rs:233,235×2,294,349`,
one `E0599 no method named clone found for type str` at `instance.rs:285`, and one
`E0308 expected IndexMap<String, Value>, found IndexMap<str, Value>` at `instance.rs:352`): the
blanket `impl<T> BorrowMut<T> for T` leaves `Borrowed` unconstrained, inference unifies it with
`str` from the export-name keys, and the whole `exports` body then type-checks against
`IndexMap<str, _>`. All seven vanish once `store.inner` is named — do not chase them individually.

### 1.4 Blocker D — wasmtime 48 enum widening

`wasmtime-48.0.0/src/runtime/types.rs:1445` — `ExternType` now has **5** variants (added `Tag(TagType)`);
`src/runtime/externals.rs:22` — `Extern` now has **6** (added `SharedMemory(SharedMemory)` and `Tag(Tag)`).
Neither is `#[non_exhaustive]`, so all matches must be updated:

- `module.rs:87` — `error[E0004]: non-exhaustive patterns: ExternType::Tag(_) not covered`.
- `instance.rs:116-167` — the `match module_import.ty()` ends in `_ => unreachable!()` at line 166, i.e. a **panic** for tag imports.
- `instance.rs:241-348` — the export `match ext.ty(..)` covers only 4 arms; needs `Tag`.

### 1.5 Blocker E — `Val::F32`/`F64` are raw bit patterns

`wasmtime-48.0.0/src/runtime/values.rs:37,43` — `F32(u32)`, `F64(u64)` hold `to_bits()` output;
accessors at `:369-370` do `f32::from_bits(*e)`. den never converts (§2.12).
`instance.rs:102,104` additionally do `*item` on a `wasmtime::Val`, which no longer derefs
(`error[E0614]`), because `WasmValueConverter` derives `Deref` to `Val` — the old code relied on
`Val: Copy` + deref chain that no longer type-checks.

### 1.6 Blocker F — the `wasmi` feature is dead code

`den-stdlib-wasm/Cargo.toml` declares `wasmi = ["dep:wasmi"]`, but **no** source file has a
`#[cfg(feature = ...)]` and every module hard-references `wasmtime::`
(`grep -rn "cfg(feature" den-stdlib-wasm/src/` → only two comments, `lib.rs:86,100`).
`--no-default-features --features wasmi` cannot build. Either delete the feature or gate the code.

Deleting it is not local to this crate: `den-core/Cargo.toml:86`
(`wasm-wasmi = ["wasm", "den-stdlib-wasm?/wasmi"]`) and the workspace root
`Cargo.toml` (`wasm-wasmi = ["den-core/wasm-wasmi"]`) both forward to it, so all three manifests
have to change together.

### 1.7 Which rquickjs API throws which JS error

The doc below names an exact JS error type on nearly every line. rquickjs 0.12 offers exactly six
throw helpers (`rquickjs-core-0.12.2/src/value/exception.rs`), and **none** of them produce the
three WebAssembly errors:

| Required error | rquickjs call |
|---|---|
| `TypeError` | `Exception::throw_type(&ctx, msg)` — `:131` |
| `RangeError` | `Exception::throw_range(&ctx, msg)` — `:171` |
| `SyntaxError` | `Exception::throw_syntax(&ctx, msg)` — `:111` |
| `ReferenceError` | `Exception::throw_reference(&ctx, msg)` — `:151` |
| `InternalError` | `Exception::throw_internal(&ctx, msg)` — `:191` (den's current catch-all; should end up used nowhere) |
| plain `Error` | `Exception::throw_message(&ctx, msg)` — `:105` |
| `CompileError` / `LinkError` / `RuntimeError` | **no helper** — build them per §2.9, keep the three constructors, then `Constructor::construct::<_, Value>((msg,))` (`src/value/function.rs:275`) and `ctx.throw(value)` (`src/context/ctx.rs:271`) |

All six return `rquickjs::Error`, i.e. they are used as `return Err(Exception::throw_range(..))`,
not as values.

---

## 2. Conformance checklist

Legend for **den status**: `MISSING` (not exposed at all) · `STUB` (present, throws TODO/not-implemented) ·
`WRONG` (present, non-conformant behaviour) · `OK`.

### 2.0 Namespace object shape

The whole namespace is installed as a plain `IndexMap` literal at `lib.rs:120-134`:

```rust
ctx.globals().set("WebAssembly", indexmap! {
    "instantiate" => …, "validate" => …, "compile" => …, "wat2wasm" => …,
    "Module" => indexmap! { "imports" => …, "exports" => …, "customSections" => … },
})?;
```

Consequences, all confirmed by reading that literal:

- `WebAssembly.Module` is a **plain object**, not a constructor → `new WebAssembly.Module(bytes)`
  throws `TypeError: not a function`. **WRONG** (`lib.rs:127-132`).
- `WebAssembly.Instance`, `.Memory`, `.Table`, `.Global`, `.Tag`, `.Exception`,
  `.CompileError`, `.LinkError`, `.RuntimeError`, `.JSTag`, `.compileStreaming`,
  `.instantiateStreaming` are **all absent from the global namespace**. **MISSING**.
  (The classes exist as ES-module exports of `den:wasm` via the `pub use` at `lib.rs:22-30` —
  the `#[rquickjs::module]` macro turns public `use` items into module exports,
  `rquickjs-macro-0.12.2/src/module/mod.rs:225-234` — but that is not the spec surface.)
- `WebAssembly.wat2wasm` (`lib.rs:101-112`) is a **den extension**, not in any spec. Harmless, but
  it must not be advertised as conformance.

**Required:** `[Exposed=*] namespace WebAssembly` carrying, as own properties: `validate`,
`compile`, `instantiate`, `JSTag` (getter), the seven interface constructors (`Module`, `Instance`,
`Memory`, `Table`, `Global`, `Tag`, `Exception`), and the three error constructors. Web-embedding
adds `compileStreaming` / `instantiateStreaming`.

Property attributes matter and den gets them wrong for free by using an `IndexMap` literal: WebIDL
namespace members are `{writable: true, enumerable: false, configurable: true}`, whereas
`IndexMap: IntoJs` produces **enumerable** data properties. So today
`Object.keys(WebAssembly)` returns the member list; conformantly it must return `[]`. The namespace
object also needs `@@toStringTag = "WebAssembly"` (so
`Object.prototype.toString.call(WebAssembly) === "[object WebAssembly]"`), and each interface
prototype needs `@@toStringTag = "WebAssembly.Module"` etc. — `#[rquickjs::class(rename = ...)]`
does not set that.

---

### 2.1 `namespace WebAssembly`

```webidl
dictionary WebAssemblyInstantiatedSource { required Module module; required Instance instance; };
dictionary WebAssemblyCompileOptions {
  USVString? importedStringConstants;
  sequence<USVString> builtins;
};
[Exposed=*] namespace WebAssembly {
  boolean validate([AllowResizable] AllowSharedBufferSource bytes,
                   optional WebAssemblyCompileOptions options = {});
  Promise<Module> compile([AllowResizable] AllowSharedBufferSource bytes,
                          optional WebAssemblyCompileOptions options = {});
  Promise<WebAssemblyInstantiatedSource> instantiate(
      [AllowResizable] AllowSharedBufferSource bytes, optional object importObject,
      optional WebAssemblyCompileOptions options = {});
  Promise<Instance> instantiate(Module moduleObject, optional object importObject);
  readonly attribute Tag JSTag;
};
```

#### `validate(bytes, options)` → `boolean`

Required: copy the bytes first ("Let stableBytes be a copy of the bytes held by the buffer"),
decode + validate, return `false` on any error. **It never throws for a bad module.** It *does*
throw `TypeError` if `bytes` is not a `BufferSource`.

den: `lib.rs:75-84` / `61-73`. Status **WRONG (partial)**:

- Accepts only `Either<TypedArray<u8>, ArrayBuffer>` (`lib.rs:77`). Spec accepts **any**
  `ArrayBufferView` — `Uint16Array`, `Int8Array`, `Float64Array`, `DataView` — plus
  `SharedArrayBuffer`, plus resizable buffers. A `DataView` today yields an rquickjs conversion
  error, not the correct behaviour.
- `lib.rs:70` `.unwrap()` on `as_bytes()` → **panic** (aborts the runtime) for a detached buffer.
- No copy is made, so a `SharedArrayBuffer` mutated concurrently is a TOCTOU hazard.
- `options` (`builtins` / `importedStringConstants`) not supported.
- Return value itself is correct (`wasmtime::Module::validate(..).is_ok()`, `lib.rs:72`).

#### `compile(bytes, options)` → `Promise<Module>`

Required: async; rejects with **`CompileError`** if decode/validate fails; rejects with
`TypeError` if `bytes` is not a BufferSource. Never throws synchronously (WebIDL wraps sync
`TypeError`s into a rejection).

den: `lib.rs:87-98`. Status **WRONG**:

- `async fn` → rquickjs returns a Promise. Good.
- It validates then calls `Module::new_inner`, which on failure throws
  `Exception::throw_internal(..)` (`module.rs:31-33`) — an **InternalError**, not `CompileError`.
- The `CompileError` it does throw at `lib.rs:96` is a bare `#[rquickjs::class]` with no
  `message`, no `.name`, no `.stack`, and no `Error` in its prototype chain (§2.9).
- Double-parses the module (validate then compile).

#### `instantiate(bytes, importObject, options)` → `Promise<WebAssemblyInstantiatedSource>`

Required ordering (this ordering is observable):
1. copy bytes → 2. async compile → `CompileError` on failure →
3. *read the imports* (this runs `Get(importObject, moduleName)` and `Get(o, name)` — arbitrary
   JS getters, in module-import order) → `TypeError`/`LinkError` on failure →
4. instantiate core → `LinkError`, or `RuntimeError`/propagated JS error from the start function →
5. resolve with a **plain object** `{ module, instance }` (a WebIDL dictionary, i.e. an ordinary
   object with `Object.prototype`, two own enumerable data properties).

#### `instantiate(moduleObject, importObject)` → `Promise<Instance>`

The `Module` overload resolves with the **bare `Instance`**, not `{module, instance}`.
Overload selection: if arg0 `implements Module` → second overload; else BufferSource overload.

den: `lib.rs:47-59`. Status **WRONG**:

- Always returns `ResultObject { module, instance }` — the `Module` overload is non-conformant
  (`lib.rs:53-58`).
- `ResultObject` is an `#[rquickjs::class]` (`lib.rs:32-45`) with getters, not a plain dictionary
  object; it also exposes a useless `constructor` (`lib.rs:43-44`).
- `import_object: Opt<IndexMap<String, IndexMap<String, Value>>>` (`lib.rs:50`) converts the whole
  import object eagerly and by *enumerable own keys* of the JS object, not by
  `Get(importObject, moduleName)` per declared import. Getter side-effects and their ordering are
  wrong, and a namespace whose value is a Proxy/primitive produces an rquickjs conversion error
  rather than `TypeError`.
- No `options`.
- Because `Instance::new` is called synchronously inside the async fn, a start-function trap
  surfaces as `InternalError` (`instance.rs:223-225`), not `RuntimeError`.

#### `JSTag` getter → `Tag`

Required: lazily `tag_alloc(store, « externref » → « »)`, cached per agent; the same `Tag` object
every time. Used so that a JS exception thrown out of a host function can be caught by wasm
`catch` on `JSTag` and re-thrown to JS unchanged.

den: **MISSING**.

---

### 2.2 `WebAssembly.Module`

```webidl
enum ImportExportKind { "function", "table", "memory", "global", "tag" };
dictionary ModuleExportDescriptor { required USVString name; required ImportExportKind kind; };
dictionary ModuleImportDescriptor { required USVString module; required USVString name;
                                    required ImportExportKind kind; };
[LegacyNamespace=WebAssembly, Exposed=*]
interface Module {
  constructor([AllowResizable] AllowSharedBufferSource bytes,
              optional WebAssemblyCompileOptions options = {});
  static sequence<ModuleExportDescriptor> exports(Module moduleObject);
  static sequence<ModuleImportDescriptor> imports(Module moduleObject);
  static sequence<ArrayBuffer> customSections(Module moduleObject, DOMString sectionName);
};
```

Internal slots: `[[Module]]`, `[[Bytes]]` (a **copy** of the source bytes — needed by
`customSections`), `[[BuiltinSets]]`, `[[ImportedStringModule]]`.

**`ImportExportKind` now has five values** — `"tag"` was added by exception handling.
den's `extern_type_to_str` (`module.rs:86-93`) knows four and no longer compiles (§1.4).

**The `type` member.** The base ED's descriptors are `{name, kind}` / `{module, name, kind}` only.
The **js-types** overlay (shipped in V8/SpiderMonkey/JSC) redefines them as:

```webidl
enum ExternKind { "function", "table", "memory", "global" };
dictionary AnyExternType {
  sequence<ValueType> parameters; sequence<ValueType> results;   // kind = "function"
  unsigned long minimum; unsigned long maximum;                  // kind = "table" | "memory"
  TableKind element;                                             // kind = "table"
  ValueType value; boolean mutable;                              // kind = "global"
};
dictionary ExternType { required ExternKind kind; AnyExternType type; };
dictionary ModuleExportDescriptor : ExternType { required USVString name; };
dictionary ModuleImportDescriptor : ExternType { required USVString module; required USVString name; };
```

So a real-world-conformant `imports()` entry is
`{module, name, kind, type: {parameters:[…], results:[…]}}` for a function,
`{…, type: {minimum, maximum?, element}}` for a table,
`{…, type: {minimum, maximum?}}` for a memory,
`{…, type: {value, mutable}}` for a global. `maximum` is **omitted** (not `undefined`) when absent.

| Member | Required behaviour | den status |
|---|---|---|
| `constructor(bytes, options)` | copy bytes; compile; **`CompileError`** on decode/validate failure; `TypeError` on non-BufferSource; store `[[Bytes]]` | **WRONG** — not exposed as a constructor at all (`lib.rs:127`); the underlying `Module::new` (`module.rs:45-51`) throws `InternalError` (`module.rs:31-33`) instead of `CompileError`, and never keeps `[[Bytes]]` |
| `static imports(m)` | ordered list, `{module,name,kind[,type]}`; skips builtin/imported-string modules; `TypeError` if `m` is not a `Module` | **WRONG** — `module.rs:53-65` returns `{module,name,kind}` with no `type`; no `"tag"` kind (won't compile); no receiver-type check |
| `static exports(m)` | ordered list, `{name,kind[,type]}` | **WRONG** — `module.rs:67-78`, same issues |
| `static customSections(m, name)` | walk `[[Bytes]]` per the module grammar; return a `sequence<ArrayBuffer>` of **copies** of each `customsec` payload whose name (UTF-8 decoded) equals `name`; empty array if none; `TypeError` if `m` is not a `Module` | **STUB** — `module.rs:80-83` throws `InternalError("not implemented")` |

Implementation note for `customSections`: wasmtime 48 has **no** `Module::custom_sections` API
(grepped `wasmtime-48.0.0/src/runtime/module.rs`). Keep a `Bytes` copy on den's `Module` and walk it
with `wasmparser::Parser` (already in the tree at 0.254.0 / 0.257.1 via wasmtime and `wat`; would
need to be a direct dependency).

Also spec'd but web-only: `Module` is `[Serializable]` (structured clone) — out of scope for den.

---

### 2.3 `WebAssembly.Instance`

```webidl
[LegacyNamespace=WebAssembly, Exposed=*]
interface Instance {
  constructor(Module module, optional object importObject);
  readonly attribute object exports;
};
```

Required `exports` semantics — *"create an exports object"*:

1. `exportsObject = OrdinaryObjectCreate(null)` — **null prototype**.
2. For each `(name, externtype)` of `module_exports` **in module order**,
   `CreateDataProperty(exportsObject, name, value)` (writable/enumerable/configurable = true at
   this point).
3. `SetIntegrityLevel(exportsObject, "frozen")` — **`Object.isFrozen(i.exports) === true`**.
4. Stored **once** in `[[Exports]]` at instantiation time. The getter returns that same object, so
   `i.exports === i.exports` and `i.exports.f === i.exports.f` must both be `true`.

Constructor error mapping: `TypeError` if `module` is not a `Module`; `TypeError` if the module has
imports and `importObject` is `undefined`, or a namespace is not an Object; `LinkError` for kind
mismatches; `LinkError` for most link failures; `RuntimeError` (or the propagated JS error) if the
start function traps/throws.

den: `instance.rs:209-353`. Status **WRONG**, several ways:

- `exports` is a **getter that rebuilds everything on every access** (`instance.rs:229-353`). It
  returns `IndexMap<String, Value>` (`:230`, `:352`), which rquickjs converts to a fresh ordinary
  object with `Object.prototype`. So: not frozen, wrong prototype, and
  `i.exports !== i.exports`, `i.exports.f !== i.exports.f`. Every access re-wraps every export,
  re-allocating a new JS `Function` per exported wasm function — identity and the
  "Exported Function cache" requirement (§2.13) are both violated.
- For a missing export it *creates a fresh Global/Table/Memory* (`instance.rs:303-307`,
  `:322-330`, `:338`) instead of asserting — dead paths that silently fabricate wrong values.
- No `Tag` export handling (won't compile, §1.4).
- Constructor (`instance.rs:211-227`) throws `InternalError` (`:223-225`) where `LinkError` /
  `RuntimeError` are required.
- Missing import object → `Exception::throw_internal("import object is not an object")`
  (`instance.rs:200-201`); spec wants `TypeError`.
- `instance.rs:39`: `if let Some(o) = import_object.get(module)` with **no else branch** — a
  missing import namespace is silently skipped and later fails inside
  `linker.instantiate` as an opaque anyhow error. Spec: `TypeError` at read-the-imports time.
- **WASI is force-injected**: `instance.rs:220`
  `wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, …).unwrap()`. Any module importing
  `wasi_snapshot_preview1` gets the host's stdio/env (see `store.rs:24-27`
  `.inherit_stdio().inherit_env()`) whether or not the user supplied it. This is both a
  conformance break (only `importObject` may supply imports) and a sandbox escape by default.
  The `.unwrap()` is also a panic path.
- Using `wasmtime::Linker` at all is a mismatch: the spec resolves imports **positionally**, in
  `module_imports` order, into a list. A `Linker` resolves by `(module, name)` string lookup and
  will happily satisfy an import den never intended to define. `Instance::new(store, module, &[Extern])`
  is the faithful primitive.

---

### 2.4 `WebAssembly.Memory`

Merged ED (memory64 landed; **`index` was renamed to `address`**, and `shared` is *not* in the ED —
it lives in the threads overlay):

```webidl
dictionary MemoryDescriptor { required AddressValue initial; AddressValue maximum; AddressType address; };
enum AddressType { "i32", "i64" };
typedef any AddressValue;
[LegacyNamespace=WebAssembly, Exposed=*]
interface Memory {
  constructor(MemoryDescriptor descriptor);
  AddressValue grow(AddressValue delta);
  ArrayBuffer toFixedLengthBuffer();
  ArrayBuffer toResizableBuffer();
  readonly attribute ArrayBuffer buffer;
};
```

Threads overlay adds `boolean shared = false;` and widens `buffer` to
`(ArrayBuffer or SharedArrayBuffer)`.

Internal slots: `[[Memory]]` (address), `[[BufferObject]]`.

#### Constructor
- `addrtype = descriptor["address"] ?? "i32"`.
- `initial = AddressValueToU64(descriptor["initial"], addrtype)`; same for `maximum` if present.
  → for `"i32"` this is `[EnforceRange] unsigned long` semantics (`TypeError` on non-finite,
  `TypeError` on out-of-range); for `"i64"` it is `ToBigInt` then range-check `0 ≤ n ≤ 2^64-1`,
  `TypeError` otherwise.
- Missing `initial` → `TypeError` (required dictionary member).
- Invalid memtype (e.g. `maximum < initial`) → **`RangeError`**.
- `mem_alloc` failure → **`RangeError`**.
- Threads overlay: `shared: true` **without** `maximum` → **`TypeError`**.

#### `buffer` getter
Returns `[[BufferObject]]` — an `ArrayBuffer` whose `[[ArrayBufferData]]` *is* the wasm linear
memory (no copy), with `[[ArrayBufferDetachKey]] = "WebAssembly.Memory"` so that user code cannot
`transfer()`/detach it. For `shared: true` it is a **frozen `SharedArrayBuffer`**
(`SetIntegrityLevel(buffer, "frozen")`).

#### `grow(delta)` → previous size **in pages**
1. `delta64 = AddressValueToU64(delta, addrtype)`.
2. `ret = mem_size(store, memaddr)` — the size **before** growing.
3. `mem_grow`; on failure → **`RangeError`**.
4. **Refresh the Memory buffer** (below).
5. return `U64ToAddressValue(ret, addrtype)` — a `Number` for i32 memories, a **`BigInt`** for i64.

#### Refresh-on-grow (the detach rule) — also fires after the wasm `memory.grow` instruction
- If `[[BufferObject]]` is fixed-length **and not shared**:
  `DetachArrayBuffer(buffer, "WebAssembly.Memory")`, build a **new** fixed-length buffer, store it.
  → the old `mem.buffer` and every `TypedArray` over it become detached (`byteLength === 0`,
  element access throws).
- If it is resizable: keep the same object, just update `[[ArrayBufferData]]` and
  `[[ArrayBufferByteLength]]`. **No detach.**
- Shared: never detach; a new `SharedArrayBuffer` object is produced for the fixed-length case.

#### `toFixedLengthBuffer()` / `toResizableBuffer()` — **yes, these are in the current ED**
- `toFixedLengthBuffer()`: if already fixed-length, return it; else create a fixed-length buffer,
  detach the old one, store and return the new one.
- `toResizableBuffer()`: if `mem_type` has **no max** → **`TypeError`**. If already resizable,
  return it. Else `maxsize = max × 65536`, create the resizable buffer, detach the old one, store
  and return.
- `HostResizeArrayBuffer` is redefined so that `buffer.resize(n)` on a resizable memory buffer
  grows the wasm memory: `lengthDelta < 0` or `lengthDelta % 65536 !== 0` → **`RangeError`**.

den: `memory.rs`. Status:

| Item | den | Status |
|---|---|---|
| constructor | `memory.rs:46-69`; reads `initial`/`maximum`/`shared` via `Object::get` (`:21-33`) | **WRONG** — `initial: u64` with no `[EnforceRange]`/AddressValue handling; missing `initial` yields an rquickjs error not `TypeError`; all failures become `InternalError` (`:55-60`, `:64-66`) instead of `RangeError`/`TypeError`; no `address`/`index`; `shared` is passed to `MemoryTypeBuilder::shared` (`:53`) then to `wasmtime::Memory::new` — shared memories need `wasmtime::SharedMemory::new` (`wasmtime-48.0.0/src/runtime/memory.rs:875`) |
| `buffer` | `memory.rs:71-97` | **STUB** — `Err(ctx.throw("TODO"))` at `:75`, and it throws a bare **string**, not even an Error |
| `grow` | `memory.rs:99-107` | **WRONG** — returns `Result<()>` → JS sees `undefined`; spec requires the previous page count. wasmtime already returns it: `Memory::grow(..) -> Result<u64>` (`wasmtime-48.0.0/src/runtime/memory.rs:637`). Errors become `InternalError`, not `RangeError`. No buffer refresh (there is no buffer). |
| `toFixedLengthBuffer` / `toResizableBuffer` | — | **MISSING** |
| detach-on-grow | — | **MISSING** |

wasmtime 48 primitives for this section (exact signatures — `MemoryTypeBuilder` methods take
`&mut self` and return `&mut Self`, so the `.build()` chain in `memory.rs:50-54` is already the
right shape):

| Need | wasmtime 48 API | Site |
|---|---|---|
| `address: "i32"` memtype | `MemoryType::new(minimum: u32, maximum: Option<u32>)` | `types.rs:3382` |
| `address: "i64"` memtype | `MemoryType::new64(minimum: u64, maximum: Option<u64>)`, or `MemoryTypeBuilder::memory64(bool)` | `types.rs:3407`, `:3303` |
| builder | `MemoryTypeBuilder::default()` (`impl Default` exists, `types.rs:3205`), `.min(u64)` `:3280`, `.max(Option<u64>)` `:3289`, `.shared(bool)` `:3318`, `.page_size_log2(u8)` `:3342`, `.build() -> Result<MemoryType>` `:3353` |  |
| read back min/max | `MemoryType::minimum(&self) -> u64` / `maximum(&self) -> Option<u64>` | `types.rs:3470,3481` |
| alloc | `Memory::new(store, ty) -> Result<Memory>` | `memory.rs:269` |
| grow (returns previous size in pages) | `Memory::grow(&self, store, delta: u64) -> Result<u64>` | `memory.rs:637` |
| shared alloc | `SharedMemory::new(engine: &Engine, ty) -> Result<Self>`, `SharedMemory::grow(&self, delta: u64) -> Result<u64>`, `SharedMemory::data_size(&self)` | `memory.rs:875,970,919` |

Note `MemoryType::minimum`/`maximum` return `u64` even for i32 memories, so `AddressType` must be
tracked separately (from `MemoryType`'s index type) to decide `Number` vs `BigInt` in
`U64ToAddressValue`.

Implementation note for `buffer`: rquickjs 0.12 can build an external-backed `ArrayBuffer`
(`rquickjs-core-0.12.2/src/value/array_buffer.rs:141 from_source`, `:153 from_source_shared`,
`:168 from_source_immutable`, `:259 detach`) — `ArrayBufferSource` (`:29`) is an `unsafe trait`
of `as_ptr`/`len` (`is_empty` is defaulted), so a zero-cost non-owning view type over
`Memory::data_ptr`/`data_size` (`wasmtime-48.0.0/src/runtime/memory.rs:479,506`) satisfies it.
Three constraints the signature imposes that are easy to miss:

- The bound is `S: ArrayBufferSource + ParallelSend + 'static`. den's workspace enables rquickjs'
  `parallel` feature (root `Cargo.toml`), and with it `ParallelSend: Send`
  (`rquickjs-core-0.12.2/src/markers.rs:24-34`). A view struct holding a `*mut u8` is not `Send`,
  so it needs an `unsafe impl Send` justified by den's single-threaded QuickJS invariant.
- `ArrayBuffer::from_external` (`:185`) — the primitive that actually takes a raw pointer plus a
  drop closure — is **private**. `from_source` with a custom `ArrayBufferSource` impl is the only
  public route. `from_source_immutable` is not an option: JS writes to it throw, and wasm linear
  memory must be writable from JS.
- `from_source`'s contract is "the source is moved into the buffer … the returned pointer must
  remain valid until `self` is dropped, including across moves of `self`". A view over
  `Memory::data_ptr` violates that the instant the memory grows and reallocates — which is exactly
  why the detach-and-replace dance on `grow` is mandatory, not optional.

---

### 2.5 `WebAssembly.Table`

```webidl
enum TableKind { "externref", "anyfunc" };
dictionary TableDescriptor { required TableKind element; required AddressValue initial;
                             AddressValue maximum; AddressType address; };
[LegacyNamespace=WebAssembly, Exposed=*]
interface Table {
  constructor(TableDescriptor descriptor, optional any value);
  AddressValue grow(AddressValue delta, optional any value);
  any get(AddressValue index);
  undefined set(AddressValue index, optional any value);
  readonly attribute AddressValue length;
};
```

Note the enum spelling: **`"anyfunc"`**, not `"funcref"` (the js-types overlay adds `"funcref"` as
an alias). `ToValueType("anyfunc") = funcref`.

| Member | Required behaviour incl. exact errors | den status |
|---|---|---|
| constructor | `elementtype = ToValueType(descriptor["element"])`; not a reftype → **`TypeError`**. `addrtype = descriptor["address"] ?? "i32"`. `initial`/`maximum` via `AddressValueToU64` → **`TypeError`** on range. Invalid tabletype → **`RangeError`**. `value` missing → `DefaultValue(elementtype)`; else `ToWebAssemblyValue(value, elementtype)`. `table_alloc` failure → **`RangeError`**. | **WRONG** — `table.rs:84-91`/`50-82`. No `value` parameter at all. `initial: u32` via `Object::get` (`:16-28`), no EnforceRange, missing `initial` → rquickjs error. Bad element string → `InternalError` (`:70-74`), spec wants `TypeError`. All alloc failures → `InternalError` (`:77-79`). **Bug:** `"anyfunc"` builds `TableType::new(RefType::FUNCREF, …)` but initialises with `Ref::Any(None)` (`table.rs:63-68`) — a type mismatch that makes `wasmtime::Table::new` fail for every funcref table. Should be `Ref::Func(None)`. |
| `get(index)` | `elementtype` matches `exnref` → **`TypeError`**. `index64 = AddressValueToU64(index, addrtype)`. `table_read` out of bounds → **`RangeError`**. else `ToJSValue(result)` (so a funcref becomes an Exported Function, `null` for a null ref). | **MISSING** |
| `set(index, value)` | same `exnref`/index rules; `value` missing → `DefaultValue(elementtype)`, error → **`TypeError`**; else `ToWebAssemblyValue(value, elementtype)`. Out of bounds → **`RangeError`**. Returns `undefined`. | **MISSING** |
| `grow(delta, value)` | returns the **previous** length (`Number` for i32 tables, `BigInt` for i64). `value` missing → `DefaultValue`, error → `TypeError`. `table_grow` failure (insufficient memory **or** invalid size) → **`RangeError`**. | **MISSING** |
| `length` getter | `U64ToAddressValue(table_size(...), addrtype)` | **MISSING** |

wasmtime 48 primitives (`wasmtime-48.0.0/src/runtime/externals/table.rs`) — note every index and
size is `u64`, and every element is a `Ref`, not a `Val`:

```rust
Table::new(store, ty: TableType, init: Ref) -> Result<Table>          // :98
Table::ty(&self, store) -> TableType                                   // :166
Table::get(&self, store, index: u64) -> Option<Ref>                    // :202  None == out of bounds -> RangeError
Table::set(&self, store, index: u64, val: Ref) -> Result<()>           // :255
Table::size(&self, store) -> u64                                       // :289
Table::grow(&self, store, delta: u64, init: Ref) -> Result<u64>        // :324  returns previous size
```

Table types: `TableType::new(element: RefType, min: u32, max: Option<u32>)` (`types.rs:3076`) for
`address: "i32"`, `TableType::new64(element: RefType, min: u64, max: Option<u64>)`
(`types.rs:3103`) for `"i64"`. `TableType::minimum()`/`maximum()` return `u64`/`Option<u64>`
(`types.rs:3135,3143`) regardless — hence the lossy `ty.minimum() as u32` at `instance.rs:325`.

`Ref` variants are `Func(Option<Func>)`, `Extern(Option<Rooted<ExternRef>>)`,
`Any(Option<Rooted<AnyRef>>)`, `Exn(Option<Rooted<ExnRef>>)`
(`wasmtime-48.0.0/src/runtime/values.rs:703+`) — the source of the `Ref::Any(None)` bug above.

---

### 2.6 `WebAssembly.Global`

```webidl
enum ValueType { "i32", "i64", "f32", "f64", "v128", "externref", "anyfunc" };
dictionary GlobalDescriptor { required ValueType value; boolean mutable = false; };
[LegacyNamespace=WebAssembly, Exposed=*]
interface Global {
  constructor(GlobalDescriptor descriptor, optional any v);
  any valueOf();
  attribute any value;
};
```

- Constructor: `valuetype = ToValueType(descriptor["value"])`; **`v128` or `exnref` → `TypeError`**.
  `v` missing → `DefaultValue(valuetype)` (note: `DefaultValue(externref) = ToWebAssemblyValue(undefined, externref)`,
  i.e. a host reference to `undefined`, *not* null). Else `ToWebAssemblyValue(v, valuetype)`.
  An unrecognised `value` string is a WebIDL enum failure → **`TypeError`**.
- `value` getter / `valueOf()`: both are `GetGlobalValue(this)`. `v128`/`exnref` content →
  **`TypeError`** on every read. Otherwise `ToJSValue` (so an `i64` global reads as a **`BigInt`**).
- `value` **setter**: `v128`/`exnref` → `TypeError`; **immutable global → `TypeError`**;
  else `ToWebAssemblyValue(newValue, valuetype)` then `global_write`.

den: `global.rs`. Status **WRONG / MISSING**:

- **No `value` getter, no `value` setter, no `valueOf()`.** The class has only a constructor
  (`global.rs:41-83`). A `WebAssembly.Global` in den is write-only and unreadable. **MISSING.**
- Descriptor validation (`global.rs:91-113`) accepts `"anyref"` (**not** a `ValueType`) and
  **rejects `"anyfunc"`** (which *is* required) — `global.rs:97`. Meanwhile the constructor's own
  match at `global.rs:55` handles `"anyfunc"`, so those two are mutually unreachable.
- Rejection is `Exception::throw_internal` (`global.rs:99-102`), spec wants `TypeError`.
- Constructor value coercion (`global.rs:47-65`) matches on `(descriptor.value, value.type_of())`
  pairs — it is **not** `ToWebAssemblyValue`. `new WebAssembly.Global({value:"i32"}, "3")` must
  give `3` (`ToInt32("3")`); den throws `InternalError`. `new WebAssembly.Global({value:"f64"}, 1)`
  must give `1`; den throws, because `1` is `Type::Int` not `Type::Float`.
- `v` is **required** in den's signature (`global.rs:44`, `value: Value<'js>`); spec makes it
  optional with a `DefaultValue`.
- `v128`/`externref`/`anyfunc` all throw the bare string `"TODO"` (`global.rs:53-55`).
  Only `v128` is spec'd to throw (`TypeError`); `externref`/`anyfunc` must work.
- `Global::from_type` (`global.rs:14-39`) ends in `_ => unreachable!()` at `:32` — a **panic** for
  any ref type that is neither funcref nor externref, and for non-null funcref inputs it throws
  `TypeError` (`:28-30`) instead of accepting an Exported Function.

wasmtime 48 primitives for the two missing accessors
(`wasmtime-48.0.0/src/runtime/externals/global.rs`):

```rust
Global::new(store, ty: GlobalType, val: Val) -> Result<Global>   // :99
Global::ty(&self, store) -> GlobalType                            // :135  -> .content(): &ValType, .mutability()
Global::get(&self, store) -> Val                                  // :148  note: takes AsContextMut, infallible
Global::set(&self, store, val: Val) -> Result<()>                 // :233  Err on immutable / type mismatch
```

`Global::get` needs `&mut` store access even though it only reads — relevant to the re-entrancy
hazard in §2.10. `Global::set` returning `Err` is *not* the immutable-global `TypeError`: the spec
requires that check before `global_write`, so test `ty.mutability()` yourself and throw
`TypeError` rather than mapping wasmtime's error.

---

### 2.7 `WebAssembly.Tag`

```webidl
dictionary TagType { required sequence<ValueType> parameters; };
[LegacyNamespace=WebAssembly, Exposed=(Window,Worker,Worklet)]
interface Tag { constructor(TagType type); };
```

- Constructor: map each `parameters[i]` through `ToValueType` (unrecognised → **`TypeError`**);
  `tag_alloc(store, wasmParameters → « »)`; initialise `[[Address]]`. Missing `parameters` →
  `TypeError`.
- Tag **identity is by address**, and the Tag object cache (§2.13) guarantees one JS object per
  address, so `is()`/`getArg()` identity checks work across import/export boundaries.
- **`Tag.prototype.type()`** is *not* in the js-api ED, but it is in the js-types overlay and is
  shipped by browsers (MDN documents it: returns `{ parameters: [...] }`, the array of value-type
  strings from the constructor). Treat as required-for-compat, optional-for-spec.

den: `tag.rs:1-10` — an empty `#[rquickjs::class]` whose constructor returns `()`.
**STUB**, and not exposed on the namespace. No `[[Address]]`, no `type()`, no tag import/export
support anywhere (`instance.rs:166` `unreachable!()`).

wasmtime 48 has the primitive: `Tag::new(store, ty: &TagType) -> Result<Tag>`
(`wasmtime-48.0.0/src/runtime/externals/tag.rs:36`) and
`Tag::ty(&self, store) -> TagType` (`:45`), plus `Extern::Tag` / `ExternType::Tag`. Note `new`
takes `&TagType`, not `TagType`.

---

### 2.8 `WebAssembly.Exception`

The merged ED and the proposal repo **disagree on `getArg`**:

```webidl
// merged js-api ED (and what browsers ship — confirmed against MDN)
interface Exception {
  constructor(Tag exceptionTag, sequence<any> payload, optional ExceptionOptions options = {});
  any getArg(Tag exceptionTag, [EnforceRange] unsigned long index);
  boolean is(Tag exceptionTag);
  readonly attribute (DOMString or undefined) stack;
};

// exception-handling proposal repo (stale): any getArg([EnforceRange] unsigned long index);
```

Implement the **two-argument** form. `dictionary ExceptionOptions { boolean traceStack = false; };`

Internal slots: `[[Address]]`, `[[Type]]` (tag address), `[[Payload]]`, `[[Stack]]`.

| Member | Required behaviour | den status |
|---|---|---|
| constructor | If `exceptionTag.[[Address]]` is the **JSTag** → **`TypeError`**. If `types.size !== payload.size` → **`TypeError`**. For each pair: `v128`/`exnref` → **`TypeError`**; else `ToWebAssemblyValue`. `exn_alloc`. If `options["traceStack"]` → set `[[Stack]]` to a stack string (or leave `undefined`). Non-`Tag` arg 0 → `TypeError`. | **STUB** — `error.rs:3-11` is an empty class named `Exception`, re-exported as `WasmException` (`lib.rs:23`), not on the namespace |
| `getArg(tag, index)` | `this.[[Type]] !== tag.[[Address]]` → **`TypeError`**. `index >= payload.size` → **`RangeError`**. `types[index]` matches `v128`/`exnref` → **`TypeError`**. Else `ToJSValue(payload[index])`. | **MISSING** |
| `is(tag)` | `true` iff `this.[[Type]] === tag.[[Address]]` (identity, not structural) | **MISSING** |
| `stack` getter | returns `[[Stack]]` — a string or `undefined` | **MISSING** |

Cross-cutting: when an Exported Function traps with an exception (§2.10) whose tag **is** the
JSTag, the *payload[0]* is rethrown to JS directly (so a JS error thrown into wasm and back out
arrives unchanged); otherwise a `WebAssembly.Exception` object is created from the address and
thrown. den does none of this.

---

### 2.9 Error objects: `CompileError` / `LinkError` / `RuntimeError`

Spec (§5.10): when the namespace object is created, for each of
`« "CompileError", "LinkError", "RuntimeError" »` define a constructor **implementing the NativeError
Object Structure**. That means, for each `E`:

- `E` is a constructor, callable with and without `new`, `E.length === 1`, `E.name === "E"`.
- `E.prototype.__proto__ === Error.prototype`; `E.__proto__ === Error`.
- `E.prototype.name === "E"`, `E.prototype.message === ""`, `E.prototype.constructor === E`.
- `new E(msg)` sets an own `message`; `e instanceof Error === true`; `e.stack` present
  (implementation-defined but expected).
- Constructible **from JS**: `new WebAssembly.CompileError("x")` must work.

Which operation throws which:

| Error | Thrown by |
|---|---|
| `CompileError` | `new Module(bytes)`; `compile()` rejection; `instantiate(bytes,…)` rejection at the compile step; `compileStreaming`/`instantiateStreaming` compile failure; any module exceeding an §8 implementation-defined limit |
| `LinkError` | read-the-imports kind mismatches (non-callable for a func import; a non-`Memory`/`Table`/`Tag` value for those kinds; `i64` global given a non-BigInt; `i32`/`f32`/`f64` global given a non-Number; `v128` global at all; a `ToWebAssemblyValue` `TypeError` while creating a global import is **converted** to `LinkError`); most `module_instantiate` linking failures |
| `RuntimeError` | wasm traps (unreachable, OOB, div-by-zero, indirect-call type mismatch…); a trap in the start function; exceeding a runtime limit from §8; JS-string-builtin misuse |
| `TypeError` | wrong JS types at the API boundary: non-BufferSource; missing/`undefined` `importObject` when the module has imports; a namespace that is not an Object; `v128`/`exnref` crossing the boundary; immutable `Global.value` setter; `Table.get/set` on an `exnref` table; `toResizableBuffer()` on a max-less memory; `getArg` tag mismatch; streaming MIME/status/CORS failures |
| `RangeError` | `Memory`/`Table` constructor with an invalid or unallocatable type; `Memory.grow`/`Table.grow` failure; `Table.get/set` out of bounds; `HostResizeArrayBuffer` bad delta; `getArg` index out of range; `AddressValueToU64` out of range for i64 is a `TypeError`, for i32 `[EnforceRange]` is a `TypeError` too — do not confuse these |

den: `error.rs`. Status **WRONG** across the board:

- `CompileError` (`error.rs:13-31`), `LinkError` (`:33-41`), `RuntimeError` (`:43-51`) are bare
  `#[rquickjs::class]` structs with no fields. No `Error` prototype, no `name`, no `message`, no
  `stack`, `instanceof Error === false`.
- `LinkError::new()` and `RuntimeError::new()` return `()` (`:40`, `:50`) — they cannot even be
  constructed from Rust in a useful way; `lib.rs:96` and `instance.rs:181` throw values built from
  those.
- They are **not on the `WebAssembly` namespace** (`lib.rs:120-134`).
- Almost every failure path in the crate uses `Exception::throw_internal` instead
  (`module.rs:32,82`; `instance.rs:83,176,201,224,264,289,310,341`; `memory.rs:56,65,104`;
  `table.rs:70,78`; `global.rs:35,57,79,99`) — JS sees `InternalError`.
- Several paths throw a bare **string** rather than an Error object:
  `instance.rs:258`, `instance.rs:305`, `memory.rs:75`, `global.rs:53,54,55`
  (`ctx.throw("TODO".into_js(&ctx)?)`), and `memory.rs:30`, `table.rs:25`, `global.rs:110`
  (`ctx.throw("not an object".into_js(ctx)?)`).

Laziest conformant fix: define the three constructors in JS once at module-evaluate time and stash
them, rather than modelling NativeError in Rust. Note that `class extends Error` does **not** work
here — class constructors throw when called without `new`, and the NativeError Object Structure
requires `WebAssembly.CompileError("x")` (no `new`) to work. Use an ordinary function:

```rust
// den-stdlib-wasm/src/error.rs (AFTER)
const DEFINE_ERRORS: &str = r#"
  (ns) => Object.fromEntries(
    ["CompileError", "LinkError", "RuntimeError"].map((n) => {
      // ordinary function: callable with and without `new`, and `.length === 1`
      const C = function (message) {
        const e = new Error(message);                 // gives a real .stack
        Object.setPrototypeOf(e, new.target ? new.target.prototype : C.prototype);
        if (message === undefined) delete e.message;  // no own `message` when absent
        return e;                                     // [[Construct]] honours the returned object
      };
      Object.setPrototypeOf(C, Error);                // C.__proto__ === Error
      C.prototype = Object.create(Error.prototype, {  // C.prototype.__proto__ === Error.prototype
        constructor: { value: C,  writable: true, configurable: true },
        name:        { value: n,  writable: true, configurable: true },
        message:     { value: "", writable: true, configurable: true },
      });
      Object.defineProperty(C, "name", { value: n, configurable: true });
      Object.defineProperty(ns, n, { value: C, writable: true, configurable: true });
      return [n, C];
    }),
  )
"#;
```

That satisfies every bullet above: `E.length === 1`, `E.name === "E"`, `E.__proto__ === Error`,
`E.prototype.__proto__ === Error.prototype`, `E.prototype.{name,message,constructor}`,
`e instanceof Error`, `e.stack`, and no spurious own `name`. QuickJS supplies `.stack` for free
because the instance really is an `Error`.

Rust side: eval the string, call it with the namespace object, keep the returned three
`Constructor<'js>` values (in `Ctx` userdata alongside the `Engine`), and throw with
`ctx.throw(ctor.construct::<_, Value>((msg,))?)` — see §1.7 for the exact rquickjs entry points.

---

### 2.10 Exported Function objects

Requirements ("create a new Exported Function", "call an Exported Function"):

- Built-in function object, `[[Prototype]] = %Function.prototype%`, **not a constructor**
  (no `[[Construct]]`; `new instance.exports.f()` must throw `TypeError`).
- `.length` = **number of parameters** of the wasm functype.
- `.name` = `ToString(index)` — the function's **index** in the module (or, for host functions,
  the index of the host function). So `instance.exports.foo.name === "0"`, not `"foo"`. (Browsers
  do implement this literally.)
- Has a `[[FunctionAddress]]` internal slot; `WebAssembly.Function`-shaped identity.
- **Cached**: the Exported Function cache maps `funcaddr → function object`, so the *same* JS
  function object is returned everywhere the same wasm function surfaces (two instances sharing an
  imported func, `table.get`, an export accessed twice, …).

Call algorithm:

1. If any parameter **or result** type matches `v128` or `exnref` → **`TypeError`**, thrown on
   *every* `[[Call]]`.
2. For each parameter type `t` at position `i`: `arg = argValues[i]` if present, **else
   `undefined`**; append `ToWebAssemblyValue(arg, t)`. → *missing arguments are `undefined` and are
   coerced per type*; `f()` on an `(i32)` function passes `0`. Extra arguments are ignored.
3. `func_invoke`; on error throw `RuntimeError` (or, for an exception, §2.8's JSTag rule).
4. `outArity == 0` → return **`undefined`**; `== 1` → `ToJSValue(ret[0])`; `> 1` →
   `CreateArrayFromList` of the coerced values (a real `Array`).

den: `instance.rs:246-287`. Status **WRONG**:

- `.length` = `func_type.params().len()` (`:245`, `:284`) — correct.
- `.name` set to the **export name** (`:285`), spec wants the index string. Minor, but observable.
- `set_constructor(false)` is never called; rquickjs `Function::new` produces a non-constructor by
  default (`rquickjs-core-0.12.2/src/value/function.rs:164-176` shows the toggle exists) — verify,
  and set it explicitly.
- No `v128`/`exnref` guard.
- **No argument-count adaptation** (`instance.rs:248-252` maps only the arguments actually
  passed) → calling with fewer args produces a wasmtime arity error surfaced as `InternalError`
  instead of coercing `undefined`.
- **Argument coercion is not type-directed** — see §2.12. `WasmValueConverter::from_js` has no
  `ValType` parameter, so `f(1.5)` into an `f32` parameter produces `Val::F64`, and `f("3")` into
  `i32` throws.
- 0 results returns **`Value::new_null`** (`instance.rs:270`) — spec requires `undefined`.
- Trap → `Exception::throw_internal("failed to lock store: …")` (`instance.rs:263-268`) —
  wrong message *and* wrong error type; should be `RuntimeError`.
- No caching: a new `rquickjs::Function` is allocated on every `exports` access (§2.3).
- Re-entrancy hazard: the call does `store.borrow_mut()` (`instance.rs:262`) on a shared
  `Arc<RefCell<…>>` (`store.rs:13`). wasm → JS import → wasm export re-entry **panics** with
  `already mutably borrowed`.

---

### 2.11 Host functions (JS callable imported into wasm)

"Create a host function" + "run a host function":

1. `jsArguments` = **all** wasm arguments, each through `ToJSValue`. (No padding/truncation on this
   side — the JS function's own `.length` handles that.) Called with `this = undefined`.
2. If any param/result type matches `v128` or `exnref` → **`TypeError`** before calling.
3. Result adaptation by `results.size`:
   - `0` → return `« »`; **the JS return value is ignored entirely**.
   - `1` → `« ToWebAssemblyValue(ret, results[0]) »`. Note: a returned *Array* is **not**
     destructured here — `ToWebAssemblyValue([5], i32)` is `ToInt32([5])` = `5` via
     `Array.prototype.toString`, and `[1,2]` → `NaN` → `0`.
   - `>1` → `GetMethod(ret, %Symbol.iterator%)`; `undefined` → **`TypeError`**;
     `IteratorToList`; `values.size !== resultsSize` → **`TypeError`**; then per-position
     `ToWebAssemblyValue`.
4. If the JS callable throws `v`: if `v implements Exception`, rethrow its wasm exception address;
   otherwise allocate an exception with the **JSTag** and payload `« ToWebAssemblyValue(v, externref) »`
   and `throw_ref` it. Net effect: the JS exception propagates through the wasm activation and
   reaches the outer JS caller **as the same object**.

den: `instance.rs:67-112`. Status **WRONG**:

- Args: `params.iter().map(WasmValueConverter::from)` (`:77-79`) — hits the F32/F64 raw-bits bug
  (§2.12), and `v128`/refs throw a bare-`"TODO"` `TypeError` (`utils.rs:15`).
- Result adaptation is array-shaped rather than arity-shaped (`:81-109`):
  - resultsSize `1` + JS returns `[x]` → den destructures (spec: coerce the array itself).
  - resultsSize `>1` + JS returns a **non-array iterable** → den silently falls into the
    `results.first_mut()` branch (`:107`), writing only result 0 and leaving the rest at their
    defaults. Spec: use the iterator, or `TypeError`.
  - resultsSize `>1` + JS returns a non-iterable → den silently writes result 0. Spec: `TypeError`.
  - Array length mismatch → `Exception::throw_internal` (`:83-92`); spec wants `TypeError`.
- F32 result path is broken: `instance.rs:99-105` writes `item.f64().unwrap().into()` into an
  `F32` slot. `impl From<f64> for Val` yields `Val::F64` (`wasmtime-48.0.0/src/runtime/values.rs:561-566`),
  so an `f32`-returning import always produces a wasmtime type error.
- A JS exception thrown inside the callback is turned into an anyhow error by `?`
  (`instance.rs:80`); the actual JS exception object is lost and the outer caller sees a generic
  wasmtime trap. No JSTag machinery.

---

### 2.12 `ToJSValue` / `ToWebAssemblyValue`

`ToJSValue(w)` — wasm → JS:

| wasm value | JS result |
|---|---|
| `i32.const u32` | `Number(signed_32(u32))` |
| `i64.const u64` | **`BigInt(signed_64(u64))`** — required, not optional |
| `f32.const` / `f64.const` | `Number` (±Infinity preserved; NaN → `NaN`) |
| `v128.const` | **assert: unreachable** — v128 is rejected earlier with `TypeError` |
| `ref.null t` | `null` |
| `ref.i31 u31` | `Number(signed_31(u31))` |
| `ref.func funcaddr` | Exported Function (cached) |
| `ref.struct` / `ref.array` | Exported GC Object (opaque exotic object, null proto, no props) |
| `ref.host hostaddr` | the original JS value from the host value cache |
| `ref.extern ref` | `ToJSValue(ref)` |
| `ref.exn` | assert: unreachable (rejected with `TypeError` earlier) |

`ToWebAssemblyValue(v, type)` — JS → wasm, **type-directed**:

| target type | algorithm | throws |
|---|---|---|
| `i64` | `ToBigInt64(v)` | `TypeError` if `v` is a Number/Symbol; `SyntaxError` on a bad string |
| `i32` | `ToInt32(v)` | `TypeError` if `v` is a BigInt or Symbol |
| `f32` | `ToNumber(v)` then round-to-nearest-even to f32 | `TypeError` for BigInt/Symbol |
| `f64` | `ToNumber(v)` | ditto |
| `v128` | assert: not reached (callers must have thrown `TypeError`) | — |
| `ref null heaptype` | `null` → `ref.null`; matches `ref null extern` → wrap `ToWebAssemblyValue(v, ref any)`; Exported Function + matches `ref null func` → `ref.func`; a Number equal to `ToInt32(v)` in `[-2^30, 2^30)` → **`ref.i31`**; Exported GC Object → `ref.struct`/`ref.array`; otherwise intern into the **host value cache** → `ref.host`. Finally `match_valtype(actualtype, type)` must hold, else **`TypeError`** | `TypeError` on type mismatch |

`AddressValueToU64(v, addrtype)` / `U64ToAddressValue(v, addrtype)`:
- `"i32"`: `ConvertToInt(v, 32, "unsigned")` with `[EnforceRange]` → `TypeError` for
  non-finite/out-of-range; back-conversion yields a `Number`.
- `"i64"`: `ToBigInt(v)`, then `0 ≤ n ≤ 2^64-1` else **`TypeError`**; back-conversion yields a
  **`BigInt`**.

den: `utils.rs`. Status **WRONG** — this is the single most-wrong file in the crate.

`IntoJs for WasmValueConverter` (`utils.rs:8-19`):

```rust
// BEFORE (utils.rs:11-15) — F32/F64 leak raw IEEE bit patterns as integers
wasmtime::Val::I32(x) => Ok(x.into_js(ctx)?),
wasmtime::Val::I64(x) => Ok(x.into_js(ctx)?),
wasmtime::Val::F32(x) => Ok(x.into_js(ctx)?),   // x: u32 bits  -> 1.0f32 becomes 1065353216
wasmtime::Val::F64(x) => Ok(x.into_js(ctx)?),   // x: u64 bits  -> 1.0f64 becomes 4607182418800017408
_ => Err(rquickjs::Exception::throw_type(ctx, "TODO")),

// AFTER — use the accessors, which do from_bits (values.rs:369-370)
Val::I32(x) => x.into_js(ctx),
Val::I64(x) => BigInt::from_i64(ctx.clone(), x)?.into_js(ctx),   // spec: BigInt, not Number
Val::F32(bits) => f32::from_bits(bits).into_js(ctx),
Val::F64(bits) => f64::from_bits(bits).into_js(ctx),
```

Other defects in the same file:

- `i64` currently becomes a **Number** via rquickjs' `i64: IntoJs` (lossy above 2^53). Spec
  mandates `BigInt`.
- `FromJs` (`utils.rs:21-34`) takes **no target `ValType`** — it cannot be `ToWebAssemblyValue`.
  Every caller (`instance.rs:96-97`, `:108`, `:251`) therefore guesses from the JS type.
- `undefined`/`null` → `Val::null_any_ref()` unconditionally (`utils.rs:24-26`). For an `i32`
  parameter the spec wants `ToInt32(undefined) = 0`.
- `Type::Float` → `Val::F64` always (`utils.rs:29`) — never `F32`.
- `Type::BigInt` → `Val::I64` always (`utils.rs:30`) — for an `i32` target the spec wants
  `TypeError`.
- Strings/objects → `TypeError "not implemented"` (`utils.rs:31`); spec wants `ToInt32`/`ToNumber`
  coercion (numeric targets) or host-reference interning (`externref`).
- No i31ref, no funcref-from-Exported-Function, no host value cache, no GC objects.
- `get_default_value_for_val_type` (`utils.rs:36-54`) duplicates
  `wasmtime::Val::default_for_ty(&ValType) -> Option<Val>`
  (`wasmtime-48.0.0/src/runtime/values.rs:131-146`). Delete it and call the wasmtime one; it
  additionally handles non-nullable refs correctly (returns `None`).

---

### 2.13 JS object caches and identity (§4.2)

Per agent, seven caches: Memory, Table, Exported Function, Exported GC Object, Global, Tag,
Exception (address → JS object), plus the **host value cache** (host address → JS value) used by
`externref` round-tripping.

Observable requirements:
- `instance.exports.mem === instance.exports.mem` and, across two instances sharing an imported
  memory, both `exports.mem` are the **same object**.
- `new WebAssembly.Global(...)` passed as an import and then re-exported comes back as the same
  `Global` object.
- An `externref` sent into wasm and returned is `===` the original JS value.
- `tag` identity is what makes `Exception.is(tag)` / `getArg(tag, i)` meaningful.

den: **MISSING** entirely. Every `Instance::exports` access constructs brand-new wrapper objects
(`instance.rs:229-353`), and there is no host value cache, so `externref` round-tripping cannot
work at all.

---

### 2.14 Web API: streaming

```webidl
[Exposed=(Window,Worker)]
partial namespace WebAssembly {
  Promise<Module> compileStreaming(Promise<Response> source,
                                   optional WebAssemblyCompileOptions options = {});
  Promise<WebAssemblyInstantiatedSource> instantiateStreaming(
      Promise<Response> source, optional object importObject,
      optional WebAssemblyCompileOptions options = {});
};
```

"Compile a potential WebAssembly response", in order, each failure **rejecting** (never throwing):

1. `source` may be a `Response` **or a promise for one**; react to it.
2. `Content-Type` header missing → **`TypeError`**.
3. Strip leading/trailing HTTP tab/space; if not a byte-case-insensitive match for exactly
   `application/wasm` → **`TypeError`**. Parameters are **not** allowed — even
   `application/wasm;` fails.
4. Response not CORS-same-origin → `TypeError`. (In den: no origins; either skip or reject
   `opaque`-equivalents.)
5. Response status not an ok status (200–299) → **`TypeError`**.
6. Consume the body as an ArrayBuffer; body errors reject with that reason.
7. Async-compile the stable bytes → **`CompileError`** on failure.

`instantiateStreaming` = that, then "instantiate a promise of a module" (so it resolves with
`{module, instance}`, and rejects with `TypeError`/`LinkError`/`RuntimeError` from the instantiate
half).

den: **MISSING** — neither name appears in `den-stdlib-wasm`. den does have a fetch stdlib
(`den-stdlib-whatwg-fetch`), so wiring is feasible; the cross-crate `Response` dependency is the
design question.

---

### 2.15 Which operations are async, and what they reject with

| Operation | Returns | Rejects with |
|---|---|---|
| `WebAssembly.compile` | `Promise<Module>` | `TypeError` (bad BufferSource / bad options), `CompileError` |
| `WebAssembly.instantiate(bytes, …)` | `Promise<{module, instance}>` | `TypeError`, `CompileError`, `LinkError`, `RuntimeError`, or a JS error propagated from the start function |
| `WebAssembly.instantiate(module, …)` | `Promise<Instance>` | `TypeError`, `LinkError`, `RuntimeError`, propagated JS error |
| `WebAssembly.compileStreaming` | `Promise<Module>` | `TypeError` (MIME/status/CORS/body), `CompileError` |
| `WebAssembly.instantiateStreaming` | `Promise<{module, instance}>` | as above plus `LinkError`/`RuntimeError` |
| everything else (`validate`, all constructors, all methods, all getters/setters) | **synchronous** | throws as tabulated in §2.9 |

Note the WebIDL rule: for a `Promise`-returning operation, *any* exception raised by argument
conversion or by the algorithm's synchronous prologue is converted into a **rejection**, never a
synchronous throw. den's `#[rquickjs::function] async fn` does this correctly for
`instantiate`/`compile`; the *error types* are wrong, not the async-ness.

---

### 2.16 Implementation-defined limits (§8) — must reject with `CompileError`

Selected values (full list in the spec): module ≤ 1 GiB; ≤ 1,000,000 each of types, functions,
imports, exports, globals, tags; ≤ 100,000 data segments; ≤ 100,000 tables; table size ≤ 10,000,000;
≤ 100 memories; 32-bit memory min/max ≤ 65,536 pages; 64-bit memory min/max ≤ 2^37-1 pages;
≤ 1,000 params and ≤ 1,000 results per function/block; function body ≤ 7,654,321 bytes; ≤ 50,000
locals; ≤ 10,000 struct fields; subtype depth ≤ 63.

At **runtime** a `RuntimeError` is required when exceeding: table size 10,000,000; 32-bit memory
65,536 pages; 64-bit memory 262,144 pages.

Stack overflow and OOM are explicitly implementation-defined (§7.1, §7.2).

den: **MISSING**. wasmtime's own limits differ; if strict conformance matters, cross-check with
`wasmtime::Config`/`wasmtime::PoolingAllocationConfig` or pre-scan with `wasmparser`.

---

### 2.17 Extension surface worth knowing about (not required by the base ED)

- **js-types** (`type()` reflection): `Memory.prototype.type()`, `Table.prototype.type()`,
  `Global.prototype.type()`, `Tag.prototype.type()`, `WebAssembly.Function` +
  `Function.prototype.type()`, and the `type` member on module import/export descriptors (§2.2).
  Shipped by all major engines. `wasmtime` gives all the inputs (`Memory::ty`, `Table::ty`,
  `Global::ty`, `Tag::ty`, `Func::ty`).
- **threads**: `MemoryDescriptor.shared`, `SharedArrayBuffer` buffers, `Atomics.wait` integration.
  wasmtime 48 exposes `SharedMemory` (`src/runtime/memory.rs:875-1052`, including
  `atomic_notify`/`atomic_wait32`/`atomic_wait64`) and `Extern::SharedMemory`.
- **JS string builtins** (`WebAssemblyCompileOptions.builtins`, `"wasm:js-string"`): 14 builtin
  functions specified in §6.1 of the ED. Large; skip unless something needs it.
- **GC**: Exported GC Objects (§5.8) — exotic objects with null prototype, `[[IsExtensible]]`
  false, all property operations no-ops or `TypeError`. Only reachable once `anyref`/`structref`
  values cross the boundary.

Engine feature flags: wasmtime 48 enables the `WASM3` feature set by default
(`wasmtime-48.0.0/src/config.rs:2532`), with `GC_TYPES`/`EXCEPTIONS` gated on the crate's `gc`
feature and `THREADS` on `threads` (`:2550-2552`) — both are default-on. den's
`engine.rs:22-28` builds a default `Config` and toggles nothing, which is fine; `async_support` is
commented out at `engine.rs:24` (leave it off — enabling it changes every call signature to
`*_async`).

---

## 3. Prioritised gap list

### P0 — the crate does not build (nothing else can be tested)

1. `store.rs:7` — `StoreData<'js> = (WasiP1Ctx, Ctx<'js>)` violates `Store<T: 'static>`.
   Redesign to carry `NonNull<qjs::JSContext>`. Touches `store.rs`, `instance.rs:28-206`.
   **Do this last of the P0s** (it is masked until 2 is fixed) and read the `Send` trap in §1.1:
   the redesigned `StoreData` must be `Send` or `instance.rs:220` will not compile.
2. `module.rs:5,14` — remove `getset`. `lib.rs:103` — `wabt::wat2wasm` → `wat::parse_str`.
   `store.rs:5`, `instance.rs:220` — `wasmtime_wasi::preview1` → `::p1`.
3. `module.rs:87`, `instance.rs:116-167`, `instance.rs:241-348` — add `ExternType::Tag` /
   `Extern::Tag` / `Extern::SharedMemory` arms; delete `instance.rs:166 unreachable!()`.
4. `lib.rs:33,36,38` — `ResultObject` fields must be `Class<'js, T>` (or drop the class, §2.1).
5. `lib.rs:67,68` — remove the `ref` binding modifiers (edition 2024).
6. `global.rs:68,70`, `instance.rs:219,222,234`, `memory.rs:64,74,102`, `table.rs:88` —
   `store.borrow_mut()` → `store.inner.borrow_mut()`.
7. `instance.rs:102,104` — `*item` on a `Val` no longer compiles; and the logic is wrong anyway (§2.11).
8. Decide the `wasmi` feature's fate (`Cargo.toml` + zero `cfg`s in `src/`).

### P1 — outright missing, spec-mandatory surface

9. `WebAssembly.Instance`, `.Memory`, `.Table`, `.Global`, `.Tag`, `.Exception`,
   `.CompileError`, `.LinkError`, `.RuntimeError` are not on the namespace (`lib.rs:120-134`);
   `WebAssembly.Module` is not a constructor (`lib.rs:127`).
10. `Table.get` / `set` / `grow` / `length` — none exist (`table.rs` has only a constructor).
11. `Global.value` getter, `Global.value` setter, `Global.valueOf()` — none exist (`global.rs`).
12. `Memory.buffer` — `memory.rs:75` throws `"TODO"`. Without this, wasm memory is unusable from JS.
13. `Memory.grow` must return the previous page count (`memory.rs:99-107` returns `()`).
14. `Module.customSections` — `module.rs:80-83` throws "not implemented"; requires keeping
    `[[Bytes]]` and a `wasmparser` walk.
15. The three error classes must be real `Error` subclasses (`error.rs`, §2.9).
16. Instance `exports` must be a **frozen, null-prototype, computed-once** object
    (`instance.rs:229-353`).

### P2 — implemented but wrong (silent misbehaviour)

17. `utils.rs:13-14` — F32/F64 exposed as raw bit patterns. Any float crossing the boundary is
    garbage.
18. `utils.rs:12` — `i64` returned as a lossy `Number`; spec requires `BigInt`.
19. `utils.rs:21-34` — `FromJs` is not type-directed; needs a `ToWebAssemblyValue(v, &ValType)`
    signature threaded through `instance.rs:96-97,108,251` and `global.rs`, `table.rs`.
20. `instance.rs:99-105` — F32 results written as `Val::F64`.
21. `instance.rs:248-252` — no missing-argument padding.
22. `instance.rs:270` — 0-result calls return `null` instead of `undefined`.
23. `instance.rs:81-109` — host-function result adaptation ignores the iterator protocol and
    mis-handles arity 1 with an array return.
24. `table.rs:63-68` — `"anyfunc"` tables initialised with `Ref::Any(None)`; must be `Ref::Func(None)`.
25. `global.rs:97` — descriptor accepts `"anyref"` (not a `ValueType`) and rejects `"anyfunc"`
    (required).
26. `global.rs:47-65` — constructor pattern-matches JS types instead of coercing.
27. `instance.rs:39` — missing import namespace silently skipped instead of `TypeError`.
28. `instance.rs:220` + `store.rs:24-27` — WASI (with inherited host stdio **and env**) is injected
    into every instance unconditionally. Conformance break *and* a default-on sandbox escape.
29. `instance.rs:143-146` — the imported-table type check is commented out; `:157-160` only
    `warn!`s on a memory type mismatch instead of raising `LinkError`.
30. Error-type mapping: replace every `Exception::throw_internal` at `module.rs:32,82`;
    `instance.rs:83,176,201,224,264,289,310,341`; `memory.rs:56,65,104`; `table.rs:70,78`;
    `global.rs:35,57,79,99` with the correct `TypeError` / `RangeError` / `CompileError` /
    `LinkError` / `RuntimeError`.
31. Bare-string throws: `instance.rs:258,305`; `memory.rs:30,75`; `table.rs:25`;
    `global.rs:53,54,55,110`.
32. Panics reachable from JS: `lib.rs:70`, `module.rs:30` (`as_bytes().unwrap()`);
    `lib.rs:82,92`, `module.rs:25`, `instance.rs:217,231`, `memory.rs:62,73,100`, `table.rs:88`,
    `global.rs:45` (`userdata().unwrap()`); `instance.rs:166` (`unreachable!()`);
    `instance.rs:220` (`.unwrap()` on `add_to_linker_sync`); `global.rs:32` (`unreachable!()`);
    `engine.rs:26` (`Engine::new(&config).unwrap()`).
33. Re-entrancy: `Arc<RefCell<Store>>` (`store.rs:13`) panics on wasm → JS → wasm re-entry
    (`instance.rs:262`, and every other `borrow_mut`).

### P3 — conformance polish / extensions

34. JS object caches (§2.13) — required for `===` identity of memories, tables, globals, tags,
    exported functions, and for `externref` round-tripping.
35. `WebAssembly.JSTag` + the JS-exception-through-wasm propagation rule (§2.8, §2.11).
36. `Memory.toFixedLengthBuffer()` / `toResizableBuffer()` + `HostResizeArrayBuffer` (§2.4).
37. `AddressValue`/`AddressType` (memory64) support in the Memory/Table descriptors and in
    `grow`/`length`/`get`/`set`.
38. `compileStreaming` / `instantiateStreaming` (§2.14).
39. `WebAssemblyCompileOptions` (`builtins`, `importedStringConstants`) and the `wasm:js-string`
    builtin set.
40. js-types `type()` methods and the `type` member on module descriptors (§2.17) — browser
    parity, not ED-mandatory.
41. `[EnforceRange]`/`ToNumber` argument conversion discipline at every numeric entry point.
42. §8 implementation-defined limits.
43. Exported GC Objects (§5.8) — only once GC types can cross the boundary.

---

## 4. Suggested build order (each step independently testable)

1. **P0 1–8** — get it compiling. Do not "fix" behaviour yet.
2. **Errors first** (P1 15): the three `Error` subclasses + a small
   `fn throw_compile/link/runtime(ctx, msg)` helper. Every later step needs them.
3. **Coercion core** (P2 17–19): replace `WasmValueConverter` with
   `to_js_value(&Val, &Ctx)` and `to_wasm_value(&Value, &ValType, &Ctx)`. This is the load-bearing
   change; put a table-driven unit test behind it (i32/i64/f32/f64 round-trips, BigInt, NaN,
   `undefined`, out-of-range, `v128` → `TypeError`).
4. **Namespace + constructors** (P1 9).
5. **Instance exports done properly** (P1 16 + P3 34): build once, frozen, null proto, cached.
6. **Memory buffer** (P1 12, 13 + P3 36) — external ArrayBuffer + detach-on-grow.
7. **Table / Global members** (P1 10, 11).
8. **Import reading rewritten** to the spec algorithm, dropping `Linker` for a positional
   `Vec<Extern>` and dropping the automatic WASI injection (P2 27, 28, 29).
9. **customSections** (P1 14).
10. Tag/Exception/JSTag (P3 35), then streaming (P3 38), then the rest.

## 5. Open questions for the maintainer

- Should WASI stay wired in at all? If yes it must be opt-in (a `den:wasi` import namespace the
  user passes explicitly), not injected into every `Instance` (`instance.rs:220`).
- `Exported Function.name`: spec says the function **index** as a string. Browsers comply.
  den currently uses the export name, which is friendlier for stack traces. Deliberate divergence,
  or match the spec?
- Is the `wasmi` backend a real goal? Today it is a feature flag with zero implementation; keeping
  it means every one of the above items has to be written twice behind a trait.
- `compileStreaming`/`instantiateStreaming` need a `Response`. Do we take a dependency from
  `den-stdlib-wasm` on `den-stdlib-whatwg-fetch`, invert it, or define a narrow trait in
  `den-stdlib-core`?

---

## 6. Verification log

Completeness/accuracy review, 2026-08-22. Everything below was checked by reading the local crate
sources under `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` and by running
`cargo check`, not from memory.

### 6.1 Build reproduced

`cargo check -p den-stdlib-core` → clean. `cargo check -p den-stdlib-wasm --message-format short`
→ **30 errors, 1 warning**, matching §1's headline exactly. Error-for-error the reproduction
matches §1.2 (`preview1` ×2, `getset` ×2, `wabt` ×1), §1.3 (`JsClass` field ×2, `borrow_mut`/
`borrow` ×10, one of them reported inside `core/borrow.rs:207`, `ref` binding ×2), §1.4 (`E0004 ExternType::Tag(_) not covered` at `module.rs:87`),
§1.5 (`E0614` at `instance.rs:102,104`).

A second run after the P0-2/P0-6 fixes had been applied in the working tree returned **17 errors**,
and §1.1 then materialised as `error[E0477]: the type `(WasiP1Ctx, rquickjs::Ctx<'js>)` does not
fulfill the required lifetime` at `store.rs:9` and `store.rs:13` plus nine cascading lifetime /
`E0521` errors. §1.1 is therefore confirmed, and §1's error-count narrative has been corrected to
say so.

### 6.2 Claims checked against source — confirmed

| Claim | Verified at |
|---|---|
| `pub struct Store<T: 'static>` | `wasmtime-48.0.0/src/runtime/store.rs:196` ✓ |
| `Instance::exports<'a, T: 'static>` | `.../runtime/instance.rs:390` ✓ |
| `ExternType` has 5 variants incl. `Tag(TagType)`, not `#[non_exhaustive]` | `.../runtime/types.rs:1445` ✓ |
| `Extern` has 6 variants incl. `SharedMemory` and `Tag` | `.../runtime/externals.rs:22` ✓ |
| `Val::F32(u32)` / `Val::F64(u64)`; accessors do `from_bits` | `.../runtime/values.rs:37,43` and `:369-370` ✓ |
| `Val::default_for_ty` supersedes `utils.rs:36-54` | `.../runtime/values.rs:131-146` ✓ |
| `impl From<f64> for Val` yields `Val::F64` | `.../runtime/values.rs:561-566` ✓ |
| `Memory::grow -> Result<u64>`, `data_ptr`, `data_size`, `SharedMemory::new` | `.../runtime/memory.rs:637,479,506,875` ✓ |
| `Tag::new(store, &TagType)` | `.../runtime/externals/tag.rs:36` ✓ |
| no `Module::custom_sections` in wasmtime 48 | grep over `.../runtime/module.rs` → 0 hits ✓ |
| `WASM3` default feature set; `GC_TYPES`/`EXCEPTIONS` on `gc`, `THREADS` on `threads` | `wasmtime-48.0.0/src/config.rs:2532`, `:2550-2552`; both are in `default` in `wasmtime-48.0.0/Cargo.toml` ✓ |
| `wat::parse_str(impl AsRef<str>) -> Result<Vec<u8>>` | `wat-1.257.1/src/lib.rs:193` ✓ |
| `wasmtime_wasi::p1` replaces `preview1` | `wasmtime-wasi-48.0.0/src/lib.rs:41`; `p1` is in `default` ✓ |
| `Ctx::as_raw` / `Ctx::from_raw` | `rquickjs-core-0.12.2/src/context/ctx.rs:512,442` ✓ |
| `UserDataGuard: Deref<Target = U>` | `rquickjs-core-0.12.2/src/runtime/userdata.rs:180` ✓ |
| `ArrayBufferSource` / `from_source` / `from_source_shared` / `detach` | `rquickjs-core-0.12.2/src/value/array_buffer.rs:29,141,153,259` ✓ |
| "using a `JsClass` type directly as a class field is not supported" is the real message | `rquickjs-core-0.12.2/src/class/impl_.rs:102-106` (a `#[diagnostic::on_unimplemented]`, not a macro error) ✓ |
| `#[rquickjs::module]` turns public `use` into module exports | `rquickjs-macro-0.12.2/src/module/mod.rs:225-234` ✓ |
| every den `file:line` cited in §1 and §2 | spot-checked all of `lib.rs`, `store.rs`, `utils.rs`, `module.rs`, `error.rs`, `tag.rs`, `engine.rs`, `instance.rs`, `memory.rs`, `table.rs`, `global.rs` — accurate ✓ |

### 6.3 Claims corrected

1. **§1.1 causation.** The doc blamed the six `str is not Sized` errors on `Store<T: 'static>`.
   They are actually downstream of §1.3's `borrow_mut` resolution failure and disappear when
   `store.inner` is named; and there are five of them, not six, plus an `E0599` at
   `instance.rs:285` and an `E0308` at `:352` the doc did not list. Rewritten with the measured
   diagnostics.
2. **§1.1 escape hatch was incomplete and would not compile.** `NonNull<qjs::JSContext>` is
   `'static` but not `Send`, while `wasmtime_wasi::p1::add_to_linker_sync<T: Send + 'static>`
   (`wasmtime-wasi-48.0.0/src/p1.rs:847`) — called at `instance.rs:220` — demands `Send`.
   The *current* `(WasiP1Ctx, Ctx<'js>)` does satisfy `Send`
   (`rquickjs-core-0.12.2/src/context/ctx.rs:103` has an unconditional `unsafe impl Send for Ctx`),
   so the recommended fix trades one error for another. Added the trap and the two ways out.
3. **§1.2 `getset` has three removal sites, not two** — `module.rs:5`, `:10` (the `#[derive(...)]`
   entry) and `:14`. Missing `:10` leaves `error: cannot find derive macro`.
4. **§1.3 `global.rs:70` is `borrow()`, not `borrow_mut()`** — same root cause, different method;
   the compiler emits a separate `E0599`.
5. **§2.7 `Tag::ty` is at `tag.rs:45`, not `:46`**, and takes `&self, store`.
6. **§2.0 said "six interface constructors" then listed seven.** Fixed to seven.
7. **§2.9's recommended JS snippet did not satisfy §2.9's own requirements.** `class extends
   Error` is not callable without `new` (the section itself requires it), `constructor(m, o)` gives
   `length === 2` (spec: 1), `this.name = n` puts `name` on the instance instead of the prototype,
   and `E.prototype.name` was left as `"Error"`. Replaced with an ordinary-function version that
   meets all of the listed bullets.

### 6.4 Gaps filled (API den needs that the doc never mentioned)

- **§1.7 (new)** — the doc names `TypeError`/`RangeError`/`CompileError`/… on nearly every line but
  never said how to throw them. Added the rquickjs 0.12 map (`Exception::throw_type` `:131`,
  `throw_range` `:171`, `throw_syntax` `:111`, `throw_reference` `:151`, `throw_internal` `:191`,
  `throw_message` `:105`) plus the `Constructor::construct` + `Ctx::throw` route for the three
  WebAssembly errors, which have no helper.
- **§1.6** — deleting the `wasmi` feature also breaks `den-core/Cargo.toml:86` and the workspace
  root's `wasm-wasmi`; all three manifests move together.
- **§2.4** — added the wasmtime 48 memory-type API table (`MemoryType::new`/`new64`,
  `MemoryTypeBuilder::{min,max,memory64,shared,page_size_log2,build}` with line refs,
  `MemoryType::minimum/maximum` returning `u64` regardless of address type). Without `new64` /
  `memory64` there is no way to implement the `AddressType` handling §2.4 spends a page on.
- **§2.4 buffer note** — the `from_source` bound is `S: ArrayBufferSource + ParallelSend +
  'static`; den enables rquickjs' `parallel`, so `ParallelSend: Send`
  (`markers.rs:24-34`) and the view type needs an `unsafe impl Send`. Also recorded that
  `ArrayBuffer::from_external` (`:185`) is private, so `from_source` with a custom source impl is
  the only public route, and that `from_source_immutable` is unusable (JS writes throw).
- **§2.5** — added exact `Table::{new,ty,get,set,size,grow}` signatures (all `u64` indices, `Ref`
  elements, `get` returning `Option<Ref>`), `TableType::new` vs `new64`, and the `Ref` variant list
  that the `Ref::Any(None)` bug comes from.
- **§2.6** — added `Global::{new,ty,get,set}` signatures. The section demanded a `value` getter and
  setter without naming the primitives; also noted `Global::get` needs `AsContextMut` (feeds the
  re-entrancy hazard) and that wasmtime's `set` error must not be used as the immutable-global
  `TypeError`.
- **§2.0** — namespace/interface property attributes: WebIDL namespace members are
  non-enumerable, but `IndexMap: IntoJs` makes them enumerable, so `Object.keys(WebAssembly)` is
  observably wrong today; plus the missing `@@toStringTag`s.
- **§1 / §3** — ordering advice: §1.1 is masked until §1.2 is fixed, and the error count rises
  in visibility (30 → 17 but with the hard blocker newly exposed) rather than falling smoothly.

### 6.5 Not verified / caveats

- The WebIDL and algorithm text in §2 is quoted from the spec snapshots in §0 and was not
  re-derived here; only the crate-API and den-source claims were checked.
- §2.16's implementation-defined limit values were not cross-checked against the spec text.
- The den `file:line` references were accurate at the time of review, but a concurrent
  implementation pass has begun modifying `den-stdlib-wasm/src/` (`store.rs`, `module.rs`,
  `lib.rs`, `instance.rs` already changed). Treat all `den-stdlib-wasm/src/*.rs` line numbers in
  this document as pinned to commit `4975f63` + the manifest bump, and re-grep before relying on
  them.
- Two stale cross-references fixed while here: §2.3 and §2.7 pointed at "§2.14" for the object
  caches; that is §2.13 (§2.14 is streaming).
