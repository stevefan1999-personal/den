# wasmi 1.1 API surface — second backend for den's WebAssembly JS-API

Status: research, 2026-08-22. Everything below was read out of local crate sources; every claim carries a
`file:line`. Nothing here is from memory.

> **Audited 2026-08-22** — a second pass re-opened every source referenced below, corrected two wrong
> claims, and added five den call sites the first pass missed. See [Verification log](#verification-log)
> at the end for exactly what was checked, corrected and added.

## Sources read

| What | Path |
|---|---|
| wasmi 1.1.0 | `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasmi-1.1.0/src/` |
| wasmi_core 1.1.0 | `.../wasmi_core-1.1.0/src/` |
| wasmi_ir 1.1.0 | `.../wasmi_ir-1.1.0/` (bytecode IR, no embedder API — not relevant) |
| wasmi_collections 1.1.0 | `.../wasmi_collections-1.1.0/` (internal maps/arenas — not relevant) |
| wasmtime 48.0.0 | `.../wasmtime-48.0.0/src/runtime/` |
| wasmtime 27.0.0 | `.../wasmtime-27.0.0/src/runtime/` (for the `T: 'static` delta) |
| wasmi_wasi 1.1.0 | not vendored locally; downloaded from crates.io, extracted to scratchpad |
| wasi-common 36.0.0 | ditto |

No `CHANGELOG.md` is vendored in the `wasmi` crate (`wasmi-1.1.0/` contains only `benches Cargo.lock Cargo.toml
Cargo.toml.orig README.md src tests`). The `README.md` proposal-support table
(`wasmi-1.1.0/README.md:59-73`) was used and cross-checked against
`wasmi-1.1.0/src/engine/config.rs:59-78`.

A **compile-verified probe crate** was written that exercises every wasmi call in this document with den's actual
shapes (`Store<(u32, JsCtx<'js>)>` behind `Rc<RefCell<_>>`, a `!Send` captured value force-wrapped, dynamic import
resolution, export walking, memory/table/global/module/func). It compiles clean against wasmi 1.1.0. See
[Appendix A](#appendix-a--compile-verified-probe).

---

## TL;DR

* wasmi 1.1 is a deliberate Wasmtime-API clone (`wasmi-1.1.0/src/lib.rs:1-5`, `README.md:31` "Loosely mirrors the
  Wasmtime API to act as drop-in replacement"). ~85% of den's existing calls port with a rename or an argument
  shuffle.
* **The seven things that actually hurt:** no exception-handling/`Tag` (and wasmi *panics* if it ever sees a tag),
  no `RefType`/`HeapType`/GC types, no shared memory, `Val`/`ValType` are structurally different enums,
  `Linker::define` has no store argument, **`wasmi::Val` is not `Copy` while `wasmtime::Val` is**
  ([§2.10.1](#2101-wasmival-is-not-copy--wasmtimes-is)), and **`ValType::is_i64()`/`is_v128()` do not exist
  on wasmi while `PartialEq` does not exist on wasmtime's `ValType`**
  ([§3.14.1](#3141-valtypeis_i64--is_v128-do-not-exist-on-wasmi)).
* **Before any of this compiles:** `--features wasm-wasmi` currently turns on *both* backends because
  `den-core/Cargo.toml:42` omits `default-features = false` ([§1.1 item 5](#11-pre-existing-breakage-you-must-fix-regardless-of-backend)).
* **wasmi is strictly better for den in one important way:** it has **no `T: 'static` bound** on `Linker`/`Instance`,
  so `Store<(WasiCtx, Ctx<'js>)>` — den's current design — works on wasmi but is *illegal* on wasmtime 48
  (`wasmtime-48.0.0/src/runtime/linker.rs:377`).
* Recommended abstraction: **cfg-gated type aliases + a thin shim module**, not a trait, not an enum. Argued in
  [§7](#7-recommended-abstraction-design).

---

## 1. What den has today (and its current state)

`den-stdlib-wasm` is wired into `den-core` behind `wasm` / `wasm-wasmtime` / `wasm-wasmi`
(`den-core/Cargo.toml:84-86`), and `den-stdlib-wasm/Cargo.toml:23,34` already declares the `wasmi` feature with
`wasmi = { version = "1.1.0", optional = true }` — **zero code behind it**, confirmed: `grep -rn wasmi
den-stdlib-wasm/src/` returns nothing.

The classes and where the backend leaks into them:

| File | rquickjs class | backend handle |
|---|---|---|
| `den-stdlib-wasm/src/engine.rs:6-11` | `Engine` | `wasmtime::Engine` |
| `den-stdlib-wasm/src/store.rs:7-14` | `Store<'js>` | `Arc<RefCell<wasmtime::Store<(WasiP1Ctx, Ctx<'js>)>>>` |
| `den-stdlib-wasm/src/module.rs:12-16` | `Module` | `wasmtime::Module` |
| `den-stdlib-wasm/src/instance.rs:22-25` | `Instance` | `wasmtime::Instance` |
| `den-stdlib-wasm/src/memory.rs:9-12` | `Memory` | `wasmtime::Memory` |
| `den-stdlib-wasm/src/table.rs:43-46` | `Table` | `wasmtime::Table` |
| `den-stdlib-wasm/src/global.rs:9-12` | `Global` | `wasmtime::Global` |
| `den-stdlib-wasm/src/tag.rs:4` | `Tag` | *(empty stub, no backend type)* |
| `den-stdlib-wasm/src/utils.rs:6` | `WasmValueConverter` | `wasmtime::Val` |

### 1.1 Pre-existing breakage you must fix regardless of backend

These are not caused by wasmi; they are already broken on `master` and will block any build:

1. `den-stdlib-wasm/src/module.rs:5` — `use getset::Getters;` but `getset` is **not** a dependency of
   `den-stdlib-wasm` (`Cargo.lock:1149-1163` lists `anyhow, den-stdlib-core, derive_more, either, indexmap,
   rquickjs, tokio, tracing, typed-builder, wasmi, wasmtime, wasmtime-wasi, wat`). `E0432`.
2. ~~`den-stdlib-wasm/src/lib.rs:103` — `wabt::wat2wasm(source)`~~ — **retracted, this claim was wrong.**
   `lib.rs:102-111` already reads `wat::parse_str(source)`, and `wat = "1.257.1"` is a declared *non-optional*
   dependency (`den-stdlib-wasm/Cargo.toml:22`, `Cargo.lock:1162`). `grep -rn wabt` over the whole repo matches
   only `docs/research/*.md`. **Nothing to fix here on either backend.**
   (wasmi's own optional `wat` dep is `1.239.0` (`wasmi-1.1.0/Cargo.toml`), semver-compatible with den's
   `1.257.1`, so the two unify — no duplicate `wat` in the graph.)
3. `den-stdlib-wasm/src/module.rs:86-93` — `extern_type_to_str` matches 4 `ExternType` variants, but wasmtime 48's
   `ExternType` has **5**, including `Tag(TagType)` (`wasmtime-48.0.0/src/runtime/types.rs:1445-1456`). Non-exhaustive
   match, `E0004`.
4. `den-stdlib-wasm/src/store.rs:11-14` — `wasmtime::Store<(WasiP1Ctx, Ctx<'js>)>` with a non-`'static` `T`. Every
   `Linker` method on wasmtime 48 requires `T: 'static` (`linker.rs:377` `define`, `:387` `func_insert`, `:416`
   `func_new`, `:1096` `instantiate`), as does `Instance::exports` (`instance.rs:390`) and `Func::new`
   (`func.rs:374`). wasmtime **27** had no such bound (`wasmtime-27.0.0/src/runtime/linker.rs:350-361`,
   `instance.rs:394` `T: 'a`). So the current source targets wasmtime 27 semantics while `den-stdlib-wasm/Cargo.toml:24` pins 48.
5. **`--features wasm-wasmi` enables *both* backends today.** `den-core/Cargo.toml:42` declares
   `den-stdlib-wasm = { version = "*", path = "../den-stdlib-wasm", optional = true }` — **without
   `default-features = false`** — and `den-stdlib-wasm/Cargo.toml:32` has `default = ["wasmtime"]`. So
   `wasm-wasmi = ["wasm", "den-stdlib-wasm?/wasmi"]` (`den-core/Cargo.toml:86`) resolves to
   `wasmtime + wasmi`, dragging cranelift in anyway and tripping the `compile_error!` guard proposed in
   [§7.2](#72-module-layout). Required companion change:

   ```toml
   # den-core/Cargo.toml:42
   den-stdlib-wasm = { version = "*", path = "../den-stdlib-wasm", optional = true, default-features = false }
   # den-core/Cargo.toml:84 — `wasm` alone must still pick a backend
   wasm = ["wasm-wasmtime"]
   ```
   The workspace root needs no change: it already declares
   `den-core = { version = "*", path = "den-core", default-features = false }` (`Cargo.toml:82`), routes
   `wasm* = ["den-core/wasm*"]` (`Cargo.toml:115-117`), and its `default` names `wasm-wasmtime` explicitly
   (`Cargo.toml:96`). The leak is entirely at the `den-core` → `den-stdlib-wasm` edge.

**This matters for the abstraction:** wasmi 1.1 has *no* `T: 'static` bound anywhere
(`wasmi-1.1.0/src/linker.rs:268,286,320,430`; `instance/mod.rs:322` is `T: 'ctx`; `store/mod.rs:61-71`), which I
verified by compiling `Store<(u32, JsCtx<'a>)>` + `Linker<…>::func_new` + `Func::new` — see Appendix A. Any shared
abstraction must therefore be written against the *stricter* wasmtime constraint (`StoreData: 'static`) or the two
backends will not be able to share the class definitions.

---

## 2. Side-by-side symbol table

`wasmtime` column = wasmtime 48.0.0. `wasmi` column = wasmi 1.1.0. All wasmi paths relative to
`wasmi-1.1.0/src/`; all wasmtime paths relative to `wasmtime-48.0.0/src/runtime/`.

### 2.1 Engine / Config

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Engine` (`Clone`, `Arc`-backed) | `Engine` — `engine/mod.rs:112`, `Clone` | same semantics |
| `Engine::new(&Config) -> Result<Engine>` | `Engine::new(&Config) -> Self` — `engine/mod.rs:144` | **infallible** in wasmi; drop the `.unwrap()` at `engine.rs:26` |
| `Engine::default()` | `Engine::default()` — `engine/mod.rs:132` | same |
| `Engine::same(&a,&b) -> bool` | `Engine::same(&a,&b) -> bool` — `engine/mod.rs:163` | same |
| — | `Engine::weak() -> EngineWeak` — `engine/mod.rs:151`; `EngineWeak::upgrade()` — `:126` | wasmi-only |
| `Engine::config()` (not public in 48) | `Engine::config() -> &Config` — `engine/mod.rs:158` | wasmi-only |
| `Config::new()` | `Config::default()` — `engine/config.rs:43` | wasmi has **no** `Config::new()` |
| `Config::async_support(bool)` | *(none)* | wasmi is sync-only; no async host funcs, no fibers |
| `Config::consume_fuel(bool)` | `Config::consume_fuel(bool)` — `engine/config.rs:334` | same |
| `Config::wasm_simd/…/wasm_multi_memory(…)` | `Config::wasm_mutable_global` `:149`, `wasm_sign_extension` `:161`, `wasm_saturating_float_to_int` `:174`, `wasm_multi_value` `:187`, `wasm_multi_memory` `:199`, `wasm_bulk_memory` `:211`, `wasm_reference_types` `:223`, `wasm_tail_call` `:236`, `wasm_extended_const` `:248`, `wasm_custom_page_sizes` `:260`, `wasm_memory64` `:272`, `wasm_wide_arithmetic` `:282`, `wasm_simd` `:293` *(cfg `simd`)*, `wasm_relaxed_simd` `:304` *(cfg `simd`)*, `floats` `:312` | **no** `wasm_threads`, **no** `wasm_exceptions`, **no** `wasm_gc`, **no** `wasm_function_references` |
| `Config::cranelift_opt_level(…)` etc. | `Config::compilation_mode(CompilationMode)` — `engine/config.rs:369`; `CompilationMode::{Eager, LazyTranslation (default), Lazy}` — `:28-41` | interpreter, no codegen knobs |
| — | `Config::ignore_custom_sections(bool)` — `engine/config.rs:349` | wasmi-only; **leave `false`** or `Module::custom_sections()` yields nothing |
| — | `Config::set_max_recursion_depth/min_stack_height/max_stack_height/max_cached_stacks` — `:87,:104,:123,:137` | wasmi-only |
| — | `Config::enforced_limits(EnforcedLimits)` — `:386` | wasmi-only parse/compile limits |

### 2.2 Store / contexts

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Store<T>` | `Store<T>` — `store/mod.rs:32` | not `Clone` in either; den keeps the `Rc`/`Arc<RefCell<…>>` wrapper |
| `Store::new(&Engine, T)` | `Store::new(&Engine, T)` — `store/mod.rs:63` | identical |
| `Store::data()/data_mut()/into_data()` | `:80 / :85 / :90` | identical |
| `Store::engine()` | `:75` | identical |
| `Store::limiter(f)` | `Store::limiter(impl FnMut(&mut T) -> &mut dyn ResourceLimiter + Send + Sync + 'static)` — `:97` | same shape |
| `Store::set_fuel/get_fuel` | `:195 / :182` | identical |
| `Store::call_hook(f)` + `CallHook` | `:254`, `CallHook` — `:349` | identical 4 variants |
| `Store::epoch_deadline_*` | *(none)* | use fuel instead |
| `AsContext { type Data; fn as_context(&self) -> StoreContext<'_, Self::Data> }` | identical — `store/context.rs:4-10` | same trait, same assoc type name |
| `AsContextMut` | identical — `store/context.rs:13-16` | same |
| `StoreContext<'a,T>` / `StoreContextMut<'a,T>` | `store/context.rs:24` / `:80` | same; `From<&'a T>`/`From<&'a mut T>` blanket impls at `:53,:60,:67` |
| — | `PrunedStore` — `store/mod.rs:12`, `store/pruned.rs` | wasmi-only type-erased store; ignore |
| `Store<T>: Send` iff `T: Send` | same; `store/mod.rs:369-378` asserts `Store<()>: Send + Sync` | see [§6.5](#65-send--static) |

### 2.3 Linker

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Linker::new(&Engine)` (`linker.rs`) | `Linker::new(&Engine)` — `linker.rs:243` | same |
| `Linker::define(store, module, name, item) -> Result<&mut Self>` — `linker.rs:369`, `T: 'static` | **`Linker::define(module, name, item) -> Result<&mut Self, LinkerError>`** — `linker.rs:268` | ⚠️ **no store argument**, and error is `LinkerError` not `anyhow::Error` |
| `Linker::func_new(module, name, FuncType, Fn(Caller<'_,T>,&[Val],&mut [Val]) -> Result<()> + Send+Sync+'static) -> Result<&mut Self, anyhow::Error>` — `linker.rs:408` | same shape — `linker.rs:286-296` — but **two** error types: the method returns `Result<&mut Self, LinkerError>`, the closure returns `Result<(), wasmi::Error>` | closure bound is **identical** (`Send + Sync + 'static`). ⚠️ *Correction:* an earlier revision of this doc said `func_new`'s error is `wasmi::Error`; only the **closure**'s is. `From<LinkerError> for Error` (`error.rs:336`) so `?` inside a `-> Result<_, wasmi::Error>` fn still works. This matters for den: `instance.rs:67-171` stores `func_new`'s and `define`'s results in the same `Option<Result<…>>` — on wasmi both are `LinkerError`, so that unification still type-checks |
| `Linker::func_wrap(module, name, impl IntoFunc<T,P,R>) -> Result<&mut Self>` | `linker.rs:320`, returns `Result<&mut Self, LinkerError>` | same |
| `Linker::get(store, module, name) -> Option<Extern>` | `linker.rs:340` | same; **panics** if engines differ (`linker.rs:365-368`) |
| `Linker::instance(store, name, Instance)` | `linker.rs:387` | same |
| `Linker::alias_module(a, b)` | `linker.rs:415` | same |
| `Linker::instantiate(store, &Module) -> Result<Instance>` — `linker.rs:1090` | **`Linker::instantiate_and_start(store, &Module) -> Result<Instance, Error>`** — `linker.rs:430` | rename. Both run the wasm `start` function |
| `Linker::instantiate_pre(&Module) -> InstancePre<T>` | *(none)* | no pre-instantiation |
| `Linker::allow_shadowing(bool)` | `linker.rs:258` | same |
| `Linker::module(…)`, `Linker::define_unknown_imports_as_traps(…)` | *(none)* | write it yourself if needed |
| `LinkerError` | `errors::LinkerError` — `linker.rs:30`, variants `DuplicateDefinition{import_name}`, `MissingDefinition{name,ty}`, `InvalidTypeDefinition{name,expected,found}` | wasmi's is a concrete enum |

### 2.4 Module

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Module::new(&Engine, impl AsRef<[u8]>)` (binary or `.wat` with `wat` feature) | `Module::new(&Engine, impl AsRef<[u8]>) -> Result<Self, Error>` — `module/mod.rs:226`; `wat::parse_bytes` at `:228` when `wat` feature on (**default on**, `Cargo.toml` `default = ["std","wat"]`) | same footgun: `Module::new` silently accepts WAT text |
| `Module::from_binary(&Engine, &[u8])` | **none** | ⚠️ den uses `from_binary` (`module.rs:31`). Closest safe replacement: `Module::validate(engine, bytes)?` then `Module::new`. `Module::validate` is binary-only (`module/mod.rs:288-291`) so it rejects WAT, giving the spec behaviour |
| `Module::validate(&Engine, &[u8]) -> Result<()>` | `Module::validate(&Engine, &[u8]) -> Result<(), Error>` — `module/mod.rs:288` | identical |
| `Module::deserialize/serialize` | *(none)* | no AOT cache |
| — | `unsafe Module::new_unchecked(&Engine, &[u8])` — `module/mod.rs:253` | wasmi-only; skips validation, UB if invalid |
| `Module::engine()` | `module/mod.rs:259` | same |
| `Module::imports() -> impl ExactSizeIterator<Item = ImportType>` | `Module::imports() -> ModuleImportsIter<'_>` — `module/mod.rs:328`; `ExactSizeIterator` at `:507` | `.len()` works, so `den/instance.rs:199` ports |
| `ImportType::module()/name()` | `module/mod.rs:544 / :549` | same |
| `ImportType::ty() -> ExternType` (by value, `types.rs:3579`) | **`ImportType::ty() -> &ExternType`** — `module/mod.rs:554` | ⚠️ returns a **reference**; needs `.clone()` to own |
| `Module::exports() -> impl ExactSizeIterator<Item = ExportType>` | `Module::exports() -> ModuleExportsIter<'_>` — `module/mod.rs:396` | ⚠️ **not** `ExactSizeIterator` (`module/export.rs:145` impls only `Iterator`) |
| `ExportType::name()/ty()` | `module/export.rs:125 / :130` | `ty()` returns `&ExternType` here too |
| `Module::get_export(name) -> Option<ExternType>` | `module/mod.rs:407` | by value here |
| `Module::customs()` / `Module::custom_section(name)` | **`Module::custom_sections() -> CustomSectionsIter<'_>`** — `module/mod.rs:451`; `CustomSection::name() -> &str` (`module/custom_section.rs:91`), `CustomSection::data() -> &[u8]` (`:97`) | ✅ **wasmi exposes custom sections and wasmtime 48 essentially does not** — this is the one place wasmi is *ahead*. See [§6.4](#64-custom-sections) |
| `Module::name()` | *(none)* | wasmi drops the name section |

### 2.5 Instance / Extern / ExternType

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Instance::new(store, &Module, &[Extern]) -> Result<Instance>` | `Instance::new(store, &Module, &[Extern]) -> Result<Instance, Error>` — `instance/mod.rs:185` | identical; runs `start` |
| `Instance::exports(store: impl Into<StoreContextMut<'a,T>>)`, `T: 'static` — `instance.rs:390` | `Instance::exports(store: impl Into<StoreContext<'ctx,T>>) -> ExportsIter<'ctx>`, `T: 'ctx` — `instance/mod.rs:322` | ⚠️ **shared** context in wasmi, mutable in wasmtime. Collect into a `Vec` before you touch the store mutably (den already does this at `instance.rs:235-240`) |
| `Instance::get_export(store, name) -> Option<Extern>` | `instance/mod.rs:229` | same |
| `Instance::get_func/get_global/get_table/get_memory(store, name)` | `instance/mod.rs:246 / :287 / :299 / :311` | identical, all take `impl AsContext` (shared) |
| `Instance::get_typed_func::<P,R>(store, name)` | `instance/mod.rs:264` | same |
| `Export<'a>::name()/into_extern()/ty(ctx)` | `instance/exports.rs:206 / :220 / :215` | plus `into_func/table/memory/global` at `:225-242` |
| `Extern::{Global,Table,Memory,Func}` (+ `SharedMemory`, `Tag` in wasmtime) | `Extern::{Global,Table,Memory,Func}` — `instance/exports.rs:21-32`, `Copy` | ⚠️ only 4 variants — matches are exhaustive with 4 arms |
| `Extern::into_func/into_global/…` | `instance/exports.rs:62-97` | same |
| `Extern::ty(ctx) -> ExternType` | `instance/exports.rs:104` | same |
| `ExternType::{Func,Global,Table,Memory,Tag}` — `types.rs:1445` | `ExternType::{Global,Table,Memory,Func}` — `instance/exports.rs:118-127`, `Clone` not `Copy` | ⚠️ **no `Tag`** |
| `ExternType::func()/global()/table()/memory()` | `instance/exports.rs:155-184` (return `Option<&T>`) | same |

### 2.6 Func / Caller / FuncType

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Func::new::<T>(store, FuncType, Fn(Caller<'_,T>,&[Val],&mut [Val]) -> anyhow::Result<()> + Send+Sync+'static) -> Func`, `T: 'static` — `func.rs:374` | `Func::new<T>(ctx, FuncType, Fn(Caller<'_,T>,&[Val],&mut [Val]) -> Result<(), wasmi::Error> + Send+Sync+'static) -> Func` — `func/mod.rs:354-367` | identical bounds; only the error type differs and **no `T: 'static`** |
| `Func::wrap::<T,P,R>(store, impl IntoFunc<T,P,R>)`, `T: 'static` — `func.rs:809` | `Func::wrap<T,P,R>(ctx, impl IntoFunc<T,P,R>)` — `func/mod.rs:370` | same |
| `Func::call(store, &[Val], &mut [Val]) -> Result<()>` — `func.rs:961` | `Func::call<T>(ctx, &[Val], &mut [Val]) -> Result<(), Error>` — `func/mod.rs:414` | ⚠️ wasmi **pre-validates and rewrites** `outputs` to typed defaults (`func/mod.rs:420` → `verify_and_prepare_inputs_outputs` → `FuncType::prepare_outputs`, `func/ty.rs:129-137`), so `outputs.len()` must match exactly — same as wasmtime, but wasmi will also overwrite each slot |
| `Func::ty(ctx) -> FuncType` | `func/mod.rs:394` | same |
| `Func::typed::<P,R>(ctx)` | `func/mod.rs:512` | same |
| — | `Func::call_resumable(ctx, …) -> Result<ResumableCall, Error>` — `func/mod.rs:454`; `ResumableCall`, `TypedResumableCall` re-exported at `lib.rs:166-172` | wasmi-only. Non-standard; do not depend on it |
| `Caller<'a,T>` | `Caller<'a,T>` — `func/caller.rs:7` | `data()` `:38`, `data_mut()` `:43`, `engine()` `:48`, `get_export(name)` `:32`, `get_fuel/set_fuel` `:59/:70`; impls `AsContext`/`AsContextMut` at `:75/:84` |
| `FuncType::new(params, results)` | `FuncType::new<P,R>(P, R)` where the iterators are `ExactSizeIterator<Item = ValType>` — `func/ty.rs:33-45` | ⚠️ **panics** (not `Result`) on out-of-bounds arity: `func/ty.rs:42` |
| `FuncType::params()/results() -> impl ExactSizeIterator<Item = ValType>` | `FuncType::params()/results() -> &[ValType]` — `func/ty.rs:48 / :53` | ⚠️ **slices**, not iterators. `.len()` still works; `den/instance.rs:245,253` needs `.iter().copied()` |
| `IntoFunc`, `WasmTy`, `WasmRet`, `WasmParams`, `WasmResults`, `TypedFunc` | all present — `lib.rs:175-186`, `func/typed_func.rs:22` | same names |
| `FuncError` | `errors::FuncError` — `func/error.rs:8`, 5 variants | wasmi-only concrete enum |

### 2.7 Memory

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Memory::new(store, MemoryType) -> Result<Memory>` — `memory.rs:269` | `Memory::new(ctx, MemoryType) -> Result<Self, Error>` — `memory/mod.rs:49` | same |
| `Memory::ty(store) -> MemoryType` | `memory/mod.rs:84` | same |
| `Memory::size(store) -> u64` (pages) | `memory/mod.rs:114` | same |
| `Memory::grow(store, delta: u64) -> Result<u64>` — `memory.rs:637` | `Memory::grow(ctx, additional: u64) -> Result<u64, MemoryError>` — `memory/mod.rs:130` | ⚠️ error type is `MemoryError` not `anyhow::Error` |
| `Memory::data(store) -> &[u8]` / `data_mut(store) -> &mut [u8]` | `Memory::data<'a,T>(impl Into<StoreContext<'a,T>>) -> &'a [u8]` — `memory/mod.rs:146`; `data_mut<'a,T>(impl Into<StoreContextMut<'a,T>>) -> &'a mut [u8]` — `:155` | same |
| `Memory::data_and_store_mut(store)` | `memory/mod.rs:165` | same |
| `Memory::data_ptr(store) -> *mut u8` | `memory/mod.rs:178` | same |
| `Memory::data_size(store) -> usize` | `memory/mod.rs:189` | same |
| `Memory::read/write` | `memory/mod.rs:207 / :230` | same |
| — | `Memory::new_static(ctx, ty, &'static mut [u8])` — `memory/mod.rs:65` | wasmi-only |
| `MemoryType::new(min: u32, max: Option<u32>)` | `MemoryType::new(min: u32, max: Option<u32>)` — `memory/ty.rs:19` | same, **panics** on bad limits (`:23` `.unwrap()`) |
| `MemoryType::new64(min,max)` | `memory/ty.rs:36` | same |
| `MemoryType::shared(min,max)` — `types.rs:3433` | **none** | ⚠️ see [§6.3](#63-shared-memory--threads) |
| `MemoryTypeBuilder::{min,max,shared,page_size_log2,memory64}.build()` | `MemoryTypeBuilder::{new,memory64,min,max,page_size_log2}` — `memory/ty.rs:103,114,122,132,148`; `build() -> Result<MemoryType, MemoryError>` — `:158` | ⚠️ **no `.shared()`** |
| `MemoryType::minimum()/maximum()/is_64()` | `memory/ty.rs:62 / :69 / :52` | same. **No `is_shared()`** |
| `SharedMemory` type | *(none)* | — |

### 2.8 Table

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Table::new(store, TableType, init: Ref) -> Result<Table>` — `externals/table.rs:98` | `Table::new(ctx, TableType, init: Val) -> Result<Self, Error>` — `table/mod.rs:49` | ⚠️ init is a **`Val`**, not a `Ref` |
| `Table::ty(store)` | `table/mod.rs:64` | same |
| `Table::size(store) -> u64` | `table/mod.rs:94` | same |
| `Table::grow(store, delta: u64, init: Ref) -> Result<u64>` | `Table::grow(ctx, delta: u64, init: Val) -> Result<u64, TableError>` — `table/mod.rs:114` | `Val` init, `TableError` |
| `Table::get(store, idx: u64) -> Option<Ref>` | `Table::get(ctx, index: u64) -> Option<Val>` — `table/mod.rs:135` | returns `Val` |
| `Table::set(store, idx: u64, Ref) -> Result<()>` | `Table::set(ctx, index: u64, value: Val) -> Result<(), TableError>` — `table/mod.rs:154` | `Val` |
| `Table::copy(store, dst, di, src, si, len)` | `table/mod.rs:189` | same |
| `Table::fill(store, dst, val, len)` | `table/mod.rs:234` | same, `Val` |
| `TableType::new(element: RefType, min: u32, max: Option<u32>)` — `types.rs:3076` | **`TableType::new(element: ValType, min: u32, max: Option<u32>)`** — `table/ty.rs:18` | ⚠️ `ValType` (`FuncRef`/`ExternRef`), not `RefType` |
| `TableType::new64(RefType, u64, Option<u64>)` | `table/ty.rs:34` (with `ValType`) | same shape |
| `TableType::element() -> &RefType` | `TableType::element() -> ValType` — `table/ty.rs:52` | ⚠️ by value, `ValType` |
| `TableType::minimum()/maximum()` | `table/ty.rs:57 / :64` (`u64` / `Option<u64>`) | same |

### 2.9 Global

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Global::new(store, GlobalType, Val) -> Result<Global>` — `externals/global.rs:99` | **`Global::new(ctx, initial_value: Val, mutability: Mutability) -> Self`** — `global.rs:51` | ⚠️ **no `GlobalType` argument, and infallible.** Type is derived from the `Val` |
| `Global::ty(store) -> GlobalType` | `global.rs:63` | same |
| `Global::get(store) -> Val` | `global.rs:90` | same |
| `Global::set(store, Val) -> Result<()>` | `Global::set(ctx, Val) -> Result<(), GlobalError>` — `global.rs:77` | `GlobalError::{ImmutableWrite, TypeMismatch}` (`wasmi_core/src/global.rs:7-12`) |
| `GlobalType::new(ValType, Mutability)` | `wasmi_core::GlobalType::new(ValType, Mutability)` — `wasmi_core/src/global.rs:59` | same (re-exported `wasmi::GlobalType`, `lib.rs:215`) |
| `GlobalType::content() -> &ValType` | `GlobalType::content() -> ValType` — `wasmi_core/src/global.rs:66` | ⚠️ by value |
| `GlobalType::mutability() -> Mutability` | `wasmi_core/src/global.rs:71` | same |
| `Mutability::{Const, Var}` | `wasmi_core/src/global.rs:27-32` | **same variant names** |

### 2.10 Values and types

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `Val::{I32(i32), I64(i64), F32(u32), F64(u64), V128(V128), FuncRef(Option<Func>), ExternRef(Option<Rooted<ExternRef>>), AnyRef(…), ExnRef(…), ContRef(…)}` — `values.rs:23-64` | **`Val::{I32(i32), I64(i64), F32(F32), F64(F64), V128(V128), FuncRef(Ref<Func>), ExternRef(Ref<ExternRef>)}`** — `value.rs:65-80` | ⚠️ name is `Val` in **both**. But: wasmi floats are `F32`/`F64` newtypes (not raw bits), refs are `Ref<T>` (not `Option<T>`), and there are **no** `AnyRef`/`ExnRef`/`ContRef` |
| `Val::I32(i)`, `Val::F32(f.to_bits())` | `Val::F32(F32::from_float(1.0f32))` or `Val::from(1.0f32)` (`value.rs:174-186`) | `F32::from_float/to_float/from_bits/to_bits` — `wasmi_core/src/float.rs:13,19,25,31`; `From<f32> for F32` / `From<F32> for f32` at `:35,:42` |
| `Val::null_func_ref()` / `null_extern_ref()` / `null_any_ref()` | `Val::FuncRef(Ref::Null)` / `Val::ExternRef(Ref::Null)`; no any-ref | `Ref<T>::{Val(T), Null}` — `reftype.rs:14-20`, `Default = Null` |
| `Val::ty(store) -> Result<ValType>` | **`Val::ty(&self) -> ValType`** — `value.rs:99` | ⚠️ **no store, infallible.** den's `global.rs:70` `value.ty(store.borrow().as_context()).unwrap()` becomes `value.ty()` |
| `Val::i32()/i64()/f32()/f64()` | `value.rs:112,119,127,135` (`f32()` returns `Option<F32>`) | same names |
| — | `Val::default(ValType) -> Val` — `value.rs:85` | ✅ **replaces den's hand-rolled `get_default_value_for_val_type`** (`utils.rs:36-53`) |
| `ValType::{I32,I64,F32,F64,V128,Ref(RefType)}` — `types.rs:88-104` | **`ValType::{I32,I64,F32,F64,V128,FuncRef,ExternRef}`** — `wasmi_core/src/value.rs:9-24` | ⚠️ flat enum, no `Ref(RefType)` nesting |
| `ValType::FUNCREF/EXTERNREF/ANYREF` consts; `ValType::matches(&other)` `types.rs:295`; `ValType::eq(a,b)` `types.rs:320`; `is_i32/is_i64/is_f32/is_f64/is_v128/is_ref/is_funcref/is_externref` `types.rs:187-235`; derives `Clone, Hash` only (`types.rs:87`) | `ValType::is_num()` / `is_ref()` — `wasmi_core/src/value.rs:30,38`; derives `Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash` (`wasmi_core/src/value.rs:8`) | ⚠️ **opposite derive sets, and only 2 of the 8 predicates survive.** wasmi's is `Copy + PartialEq`, wasmtime's is neither; wasmtime has `is_i64()`/`is_v128()`, wasmi has neither. den uses `is_i64()`/`is_v128()` at `instance.rs:118,123,127` — see [§3.14.1](#3141-valtypeis_i64--is_v128-do-not-exist-on-wasmi) |
| `RefType`, `RefType::FUNCREF/EXTERNREF/ANYREF`, `RefType::heap_type()` — `types.rs:412` | **none** | ⚠️ den's `table.rs:59,65` and `utils.rs:43-51` use these |
| `HeapType::{Func,Extern,Any,I31,Struct,Array,Exn,…}` — `types.rs:721` | **none** | — |
| `Ref` (wasmtime's `Ref::{Func(Option<Func>), Extern(…), Any(…)}`) | `Ref<T>` (generic, `reftype.rs:14`) | ⚠️ **same name, completely different type.** wasmi's `Ref` is `Option`-like over one `T` |
| `ExternRef` (GC-rooted, `Rooted<ExternRef>`) | `ExternRef` — `reftype.rs:99`, `Copy`, store-indexed | `ExternRef::new(ctx, T)` `:113` where `T: 'static + Any + Send + Sync`; `ExternRef::data(ctx) -> &dyn Any` `:128` |
| `AnyRef`, `ExnRef`, `StructRef`, `ArrayRef`, `I31`, `Rooted<T>`, `RootScope` | **none** | see [§6.2](#62-gc-types) |
| `V128` (`types::V128`, `.as_u128()`) | `wasmi_core::V128` — `wasmi_core/src/value.rs:501`, `From<u128>` `:503`, `as_u128()` `:510` | present in the enum **always**, but conversions `unimplemented!()`/`panic!()` without the `simd` feature — `value.rs:35,52,245-253` |
| `Tag`, `TagType` — `externals/tag.rs`, `types.rs:3028` | **none** | see [§6.1](#61-exception-handling--tag) |

#### 2.10.1 `wasmi::Val` is not `Copy` — wasmtime's is

This is the single most invasive mechanical difference in den's code and it is easy to miss.

| | derive |
|---|---|
| `wasmtime::Val` | `#[derive(Debug, Clone, Copy)]` — `wasmtime-48.0.0/src/runtime/values.rs:22` ("we inline the `enum Ref` variants into `enum Val` here as a size optimization") |
| `wasmi::Val` | `#[derive(Clone, Debug)]` — `wasmi-1.1.0/src/value.rs:64` — **no `Copy`** |

Compile-verified: `fn f(v: wasmi::Val) -> (wasmi::Val, wasmi::Val) { (v, v) }` fails with
`E0382: use of moved value`.

Every wasmi *handle* type (`Func`, `Memory`, `Table`, `Global`, `Instance`, `Extern`, `ExternRef`,
`Ref<T>`) **is** `Copy`; only `Val` (and `FuncType`, `ExternType`, which are `Clone`-only on both backends)
is not. Sites in den that break:

| den site | today | on wasmi |
|---|---|---|
| `utils.rs:5` | `#[derive(Clone, Copy, From, Into, Deref, DerefMut)] pub struct WasmValueConverter(wasmtime::Val);` | `E0204: the trait Copy cannot be implemented for this type`. **Drop `Copy`** from the derive list |
| `instance.rs:78` | `params.iter().map(\|x\| WasmValueConverter::from(*x))` | `*x` moves out of a `&Val`. → `params.iter().cloned()` (or `.map(\|x\| WasmValueConverter::from(x.clone()))`) |
| `instance.rs:104` | `*result = *item;` (`item: WasmValueConverter`) | `*result = (*item).clone();` (via the `Deref` derive) — `item.0` is private outside `utils.rs` |
| `instance.rs:271` | `WasmValueConverter::from(results[0])` | cannot move out of index. → `results[0].clone()`, or `results.into_iter().next().unwrap()` since `results` is owned here |
| `instance.rs:99-105` | the `matches!(result, Val::F32(_)) && item.f64().is_some()` fixup | keep the branch but clone; note the fixup itself is unnecessary on wasmi (§3.12) |

Knock-on: with `Copy` gone, `WasmValueConverter` is `Clone`-only, so any `Vec<WasmValueConverter>`/slice
indexing in future code needs the same treatment. If you would rather not diverge, define the shim newtype
as `Clone`-only on **both** backends — `wasmtime::Val` being `Copy` is not load-bearing anywhere in den.

### 2.11 Errors

| wasmtime 48 | wasmi 1.1 | Notes |
|---|---|---|
| `anyhow::Error` everywhere | **`wasmi::Error`** — `error.rs:24`; 8 bytes (`error.rs:29-33`) | den can keep `format!("{e}")`-into-`Exception::throw_internal` unchanged: `impl Display for Error` at `error.rs:170` |
| `wasmtime::Trap` (`downcast_ref::<Trap>()`) | `Error::as_trap_code() -> Option<TrapCode>` — `error.rs:80`; `TrapCode` from `wasmi_core::trap` (`lib.rs:215`) | easier: no downcasting needed |
| `wasmtime::Error::downcast::<T>()` | `Error::downcast/downcast_ref/downcast_mut::<T: HostError>()` — `error.rs:121,95,108` | ⚠️ only works for `T: HostError` (`wasmi_core/src/host_error.rs:62`: `'static + Display + Debug + Any + Send + Sync`) |
| `anyhow!("msg")` | `Error::new(impl Into<String>)` — `error.rs:46` | for host-func failures |
| — | `Error::host(E: HostError)` — `error.rs:56`; `Error::i32_exit(i32)` — `error.rs:70` | wasmi-only |
| — | `ErrorKind` — `error.rs:179`, `#[non_exhaustive]`, 18 variants | inspectable |
| — | `errors::{EnforcedLimitsError, ErrorKind, FuncError, IrError, LinkerError, InstantiationError, ReadError}` + `wasmi_core::{FuelError, GlobalError, HostError, MemoryError, TableError}` — `lib.rs:148-158` | many small typed errors; `From<…> for Error` for all (`error.rs:331-347`) |
| `InstantiationError` (wasmtime bundles into anyhow) | `errors::InstantiationError` — `module/instantiate/error.rs:18`, variants `InvalidNumberOfImports`, `ImportsExternalsMismatch`, `GlobalTypeMismatch`, `FuncTypeMismatch`, `TableTypeMismatch`, `MemoryTypeMismatch`, … | ✅ maps cleanly onto `WebAssembly.LinkError` |

**Practical error mapping for the JS API** (`den-stdlib-wasm/src/error.rs`):

| JS error class | wasmi source |
|---|---|
| `WebAssembly.CompileError` | `Module::new`/`Module::validate` → `ErrorKind::{Wasm, Read, Translation, Limits, Wat}` |
| `WebAssembly.LinkError` | `LinkerError::*`, `InstantiationError::{InvalidNumberOfImports, ImportsExternalsMismatch, *TypeMismatch}` |
| `WebAssembly.RuntimeError` | `Error::as_trap_code().is_some()` (`error.rs:80`), plus `ErrorKind::{Host, Message, I32ExitStatus}` |

---

## 3. Concrete BEFORE/AFTER for den's existing code

### 3.1 `engine.rs` — Engine construction

```rust
// BEFORE (den-stdlib-wasm/src/engine.rs:22-28)
pub fn new() -> Self {
  let mut config = wasmtime::Config::new();
  Self { inner: wasmtime::Engine::new(&config).unwrap() }
}
```
```rust
// AFTER (wasmi)
pub fn new() -> Self {
  let mut config = wasmi::Config::default();      // no Config::new()
  config.compilation_mode(wasmi::CompilationMode::Eager); // optional; default is LazyTranslation
  Self { inner: wasmi::Engine::new(&config) }     // infallible, no unwrap
}
```
`wasmi::Engine` is `Arc`-backed and `Clone` (`engine/mod.rs:112-116`), so den's `#[derive(Clone)]` +
`#[qjs(skip_trace)]` pattern is unchanged.

### 3.2 `lib.rs` — `WebAssembly.validate`

```rust
// BEFORE (den-stdlib-wasm/src/lib.rs:72)
Ok(wasmtime::Module::validate(&engine, buf).is_ok())
```
```rust
// AFTER
Ok(wasmi::Module::validate(&engine, buf).is_ok())
```
Identical. Note wasmi's `validate` is binary-only by construction (`module/mod.rs:288-291`, it drives
`wasmparser::Parser` directly), which is what the JS spec wants.

### 3.3 `module.rs` — construction from a buffer

```rust
// BEFORE (den-stdlib-wasm/src/module.rs:31-33)
let inner = wasmtime::Module::from_binary(&engine, buf)
  .map_err(|x| Exception::throw_internal(ctx, &format!("wasm module creation error: {}", x)))?;
```
```rust
// AFTER — wasmi has no `from_binary`; `Module::new` would also accept WAT text,
// which `WebAssembly.Module(bufferSource)` must not. Validate first.
wasmi::Module::validate(&engine, buf)
  .map_err(|e| Exception::throw_internal(ctx, &format!("wasm compile error: {e}")))?;
let inner = wasmi::Module::new(&engine, buf)
  .map_err(|e| Exception::throw_internal(ctx, &format!("wasm module creation error: {e}")))?;
```

### 3.4 `module.rs` — `imports()` / `exports()` / `extern_type_to_str`

```rust
// BEFORE (module.rs:54-65, 86-93)
module.imports().map(|import| indexmap! {
  "module" => import.module(),
  "name"   => import.name(),
  "kind"   => extern_type_to_str(import.ty()),   // ExternType by value
})
fn extern_type_to_str(x: ExternType) -> &'static str {
  match x { ExternType::Func(_) => "function", /* … 4 arms; MISSING Tag */ }
}
```
```rust
// AFTER — wasmi's ImportType::ty() returns &ExternType (module/mod.rs:554)
module.imports().map(|import| indexmap! {
  "module" => import.module(),
  "name"   => import.name(),
  "kind"   => extern_type_to_str(import.ty()),   // now &ExternType
})
fn extern_type_to_str(x: &wasmi::ExternType) -> &'static str {
  match x {
    wasmi::ExternType::Func(_)   => "function",
    wasmi::ExternType::Global(_) => "global",
    wasmi::ExternType::Table(_)  => "table",
    wasmi::ExternType::Memory(_) => "memory",
  } // exhaustive: wasmi has exactly 4 variants (instance/exports.rs:118-127)
}
```

### 3.5 `module.rs` — `customSections` (currently `not implemented`)

```rust
// BEFORE (module.rs:80-83)
pub fn custom_sections<'js>(_module: &Module, ctx: Ctx<'js>) -> Result<Vec<Object<'js>>> {
  Err(Exception::throw_internal(&ctx, "not implemented"))
}
```
```rust
// AFTER (wasmi) — actually implementable
pub fn custom_sections<'js>(
  module: &Module, section_name: String, ctx: Ctx<'js>,
) -> Result<Vec<ArrayBuffer<'js>>> {
  module.inner.custom_sections()
    .filter(|s| s.name() == section_name)
    .map(|s| ArrayBuffer::new(ctx.clone(), s.data()))
    .collect()
}
```
`CustomSectionsIter` yields `CustomSection { name() -> &str, data() -> &[u8] }`
(`module/custom_section.rs:104,91,97`). Requires `Config::ignore_custom_sections(false)` — the default
(`engine/config.rs:49`).

### 3.6 `store.rs` — the store and its data

```rust
// BEFORE (den-stdlib-wasm/src/store.rs:7-33)
pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);
pub struct Store<'js> { inner: Arc<RefCell<wasmtime::Store<StoreData<'js>>>> }
let wasi_ctx = WasiCtxBuilder::new().inherit_stdio().inherit_env().build_p1();
let inner = wasmtime::Store::new(&engine, (wasi_ctx, ctx));
```
```rust
// AFTER (wasmi + wasmi_wasi)
pub type StoreData<'js> = (wasmi_wasi::WasiCtx, Ctx<'js>);
pub struct Store<'js> { inner: Arc<RefCell<wasmi::Store<StoreData<'js>>>> }

let wasi_ctx = wasmi_wasi::WasiCtxBuilder::new()
  .inherit_stdio()          // &mut Self  (wasi-common-36.0.0/src/sync/mod.rs:101)
  .inherit_env()            // Result<&mut Self, StringArrayError>  (:58)
  .expect("inherit env")
  .build();                 // WasiCtx     (:124)
let inner = wasmi::Store::new(&engine, (wasi_ctx, ctx));
```
This compiles on wasmi with a non-`'static` `'js` (verified, Appendix A). It does **not** compile on wasmtime 48.

### 3.7 `instance.rs` — the linker and host-function bridge

```rust
// BEFORE (den-stdlib-wasm/src/instance.rs:67-112, 170, 197, 221-222)
let mut linker = wasmtime::Linker::new(module.engine());
let wasm_func = linker.func_new(module, name, ty, move |caller, params, results| {
    let (_, ctx) = caller.data();
    /* … */
    Ok(())                                  // anyhow::Result<()>
});
external.map(|value| linker.define(store.as_context(), module, name, value));
let instance = linker.instantiate(store.borrow_mut().as_context_mut(), module)?;
```
```rust
// AFTER (wasmi)
let mut linker = wasmi::Linker::<StoreData<'js>>::new(module.engine());
// ⚠️ ImportType::ty() is a reference → clone the FuncType
let wasmi::ExternType::Func(ty) = module_import.ty().clone() else { /* … */ };
let wasm_func = linker.func_new(module, name, ty, move |caller, params, results| {
    let (_, ctx) = caller.data();
    /* … */
    Ok::<(), wasmi::Error>(())              // wasmi::Error, not anyhow
});
// ⚠️ no store argument
external.map(|value| linker.define(module, name, value));
// ⚠️ renamed
let instance = linker.instantiate_and_start(store.borrow_mut().as_context_mut(), module)?;
```

Converting an `rquickjs::Error` inside the host closure: `wasmi::Error` has no `From<E: std::error::Error>`
blanket, only `From<T: HostError>` via `Error::host` (`error.rs:56`) and `HostError` is a foreign trait
(`wasmi_core/src/host_error.rs:62`) so you cannot impl it for `rquickjs::Error`. Two options:

```rust
// (a) lossy but 3 lines — the JS exception is already pending on the ctx anyway
let res: Value = func.call_arg(args).map_err(|e| wasmi::Error::new(e.to_string()))?;

// (b) preserve it: den-owned HostError newtype, recoverable via Error::downcast_ref
#[derive(Debug)] struct JsThrew;
impl core::fmt::Display for JsThrew { /* "javascript host function threw" */ }
impl wasmi::errors::HostError for JsThrew {}
// … .map_err(|_| wasmi::Error::host(JsThrew))?
```
Prefer (b): it lets the export wrapper distinguish "JS threw" (rethrow the pending exception) from "wasm trapped"
(construct a `WebAssembly.RuntimeError`).

### 3.8 `instance.rs` — export enumeration

```rust
// BEFORE (instance.rs:235-241)
for (name, ext) in self.instance.exports(&mut *store)
    .map(|x| (x.name().to_string(), x.into_extern()))
    .collect::<Vec<(String, Extern)>>()
{
  let value = match ext.ty(&mut *store) { /* 4 arms */ };
```
```rust
// AFTER — Instance::exports takes a SHARED context in wasmi (instance/mod.rs:322)
for (name, ext) in self.instance.exports(&*store)
    .map(|x| (x.name().to_string(), x.into_extern()))
    .collect::<Vec<(String, wasmi::Extern)>>()
{
  let value = match ext.ty(&*store) { /* still exactly 4 arms */ };
```
The `.collect::<Vec<_>>()` is still mandatory: `ExportsIter<'ctx>` borrows the store
(`instance/exports.rs:247-249`).

Result defaults inside the exported-function wrapper:
```rust
// BEFORE (instance.rs:253-261 + utils.rs:36-53)
let mut results: Vec<Val> = func_type.results()
  .map(|ref x| get_default_value_for_val_type(x) /* den's own fn */)
  .collect::<Result<Vec<_>>>()?;
```
```rust
// AFTER — wasmi ships this; and results() is a slice, not an iterator (func/ty.rs:53)
let mut results: Vec<wasmi::Val> =
  func_type.results().iter().copied().map(wasmi::Val::default).collect();
```

### 3.9 `memory.rs`

```rust
// BEFORE (memory.rs:50-66, 99-106)
let ty = wasmtime::MemoryTypeBuilder::default()
  .min(opts.initial).max(opts.maximum).shared(opts.shared.unwrap_or(false))
  .build().map_err(…)?;
let inner = wasmtime::Memory::new(store.borrow_mut().as_context_mut(), ty)?;
self.inner.grow(store.borrow_mut().as_context_mut(), delta)?;
```
```rust
// AFTER — MemoryTypeBuilder has NO .shared() (memory/ty.rs:97-161)
if opts.shared.unwrap_or(false) {
  return Err(Exception::throw_internal(&ctx,
    "shared memory requires the threads proposal, unsupported by the wasmi backend"));
}
let mut b = wasmi::MemoryType::builder();
b.min(opts.initial);
b.max(opts.maximum);
let ty = b.build().map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
let inner = wasmi::Memory::new(store.borrow_mut().as_context_mut(), ty)
  .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
// grow returns Result<u64, MemoryError>
self.inner.grow(store.borrow_mut().as_context_mut(), delta)
  .map_err(|e| Exception::throw_internal(&ctx, &format!("{e}")))?;
```
Note `MemoryTypeBuilder` methods take `&mut self` and return `&mut Self`, and `build(self)` takes `self`
(`memory/ty.rs:114-158`) — so the fluent one-liner does not chain into `.build()`; bind the builder first (as
above), exactly like wasmtime's builder.

`Memory.buffer` (`memory.rs:71-97`, currently `TODO`): wasmi's backing store is a `Vec<u8>` whose base pointer
**moves on grow** (`wasmi_core/src/memory/buffer.rs:133-146` — `try_reserve` + `resize` +
`vec_into_raw_parts` reassigns `self.ptr`). So a zero-copy `ArrayBuffer` handed to JS must be **detached and
recreated on every `grow()`** — which is what the JS spec mandates anyway
(`WebAssembly.Memory.prototype.grow` detaches the old buffer). Same requirement on both backends; wasmi just makes
it non-optional.

### 3.10 `table.rs`

```rust
// BEFORE (table.rs:56-79)
let (ty, init) = match desc.element.as_str() {
  "externref" => (TableType::new(RefType::EXTERNREF, desc.initial, desc.maximum), Ref::Extern(None)),
  "anyfunc"   => (TableType::new(RefType::FUNCREF,   desc.initial, desc.maximum), Ref::Any(None)),
  x => return Err(…),
};
let inner = wasmtime::Table::new(store, ty, init)?;
```
```rust
// AFTER — no RefType; element is a ValType; init is a Val (table/ty.rs:18, table/mod.rs:49)
let element = match desc.element.as_str() {
  "externref" => wasmi::ValType::ExternRef,
  "anyfunc"   => wasmi::ValType::FuncRef,
  x => return Err(Exception::throw_internal(ctx,
        &format!("Either externref or anyfunc is accepted for element type, found {x}"))),
};
let ty = wasmi::TableType::new(element, desc.initial, desc.maximum);
let inner = wasmi::Table::new(store, ty, wasmi::Val::default(element))
  .map_err(|e| Exception::throw_internal(ctx, &format!("{e}")))?;
```
Note the BEFORE code has a latent bug worth not carrying over: `"anyfunc"` maps to `Ref::Any(None)` (an
*anyref* null) while the type is `FUNCREF` (`table.rs:63-68`). The wasmi version cannot express that mistake.

Also `den/instance.rs:325` uses `ty.element().heap_type().to_string()` to round-trip the element type — there is no
`heap_type()` in wasmi. Replace the whole `TableDescriptor` round-trip with a direct
`Table::new(store, ty, Val::default(ty.element()))` since you already hold the `TableType`.

### 3.11 `global.rs`

```rust
// BEFORE (global.rs:67-79)
let inner = wasmtime::Global::new(
  store.borrow_mut().as_context_mut(),
  GlobalType::new(
    value.ty(store.borrow().as_context()).unwrap(),
    if desc.mutable.unwrap_or(false) { Mutability::Var } else { Mutability::Const },
  ),
  value,
).map_err(…)?;
```
```rust
// AFTER — Global::new takes (Val, Mutability) and is infallible (global.rs:51)
let inner = wasmi::Global::new(
  store.borrow_mut().as_context_mut(),
  value,
  if desc.mutable.unwrap_or(false) { wasmi::Mutability::Var } else { wasmi::Mutability::Const },
);
```
And `Global::from_type` (`global.rs:15-38`):
```rust
// BEFORE
let val: Val = match ty.content() {                      // &ValType-ish
  ValType::I32 => (*Coerced::<i32>::from_js(ctx, v.clone())?).into(),
  /* … */
  x if x.matches(&ValType::FUNCREF) && v.is_null() => Val::null_func_ref(),
  x if x.matches(&ValType::EXTERNREF) && v.is_null() => Val::null_extern_ref(),
  _ => unreachable!(),
};
```
```rust
// AFTER — flat ValType, no `matches`, no null_* constructors
let val: wasmi::Val = match ty.content() {               // ValType by value
  wasmi::ValType::I32 => (*Coerced::<i32>::from_js(ctx, v.clone())?).into(),
  wasmi::ValType::I64 => (*Coerced::<i64>::from_js(ctx, v.clone())?).into(),
  wasmi::ValType::F32 => f32::from_js(ctx, v.clone())?.into(),
  wasmi::ValType::F64 => (*Coerced::<f64>::from_js(ctx, v.clone())?).into(),
  wasmi::ValType::FuncRef   if v.is_null() => wasmi::Val::FuncRef(wasmi::Ref::Null),
  wasmi::ValType::ExternRef if v.is_null() => wasmi::Val::ExternRef(wasmi::Ref::Null),
  wasmi::ValType::FuncRef   => return Err(Exception::throw_type(ctx, "not a valid func ref")),
  wasmi::ValType::ExternRef => return Err(Exception::throw_type(ctx, "not a valid extern ref")),
  wasmi::ValType::V128      => return Err(Exception::throw_type(ctx, "v128 is not representable in JS")),
};
```
Also `GlobalDescriptor::from_js` (`global.rs:95-98`) accepts `"anyref"` — wasmi has no anyref; reject it (and note
the current code accepts `"anyref"` in the validator but matches `"anyfunc"` in the constructor at `global.rs:55`,
another latent inconsistency).

### 3.12 `utils.rs` — value conversion

⚠️ First, `utils.rs:5` must lose `Copy` — `wasmi::Val` is not `Copy` (see [§2.10.1](#2101-wasmival-is-not-copy--wasmtimes-is)):
```rust
// BEFORE (utils.rs:5)
#[derive(Clone, Copy, From, Into, Deref, DerefMut)]
pub struct WasmValueConverter(wasmtime::Val);
// AFTER
#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct WasmValueConverter(backend::Val);
```

```rust
// BEFORE (utils.rs:10-18, 27-30)
match self.0 {
  wasmtime::Val::I32(x) => x.into_js(ctx),
  wasmtime::Val::F32(x) => x.into_js(ctx),      // x: u32 raw bits — BUG, this yields the bit pattern
  /* … */
}
rquickjs::Type::Uninitialized | Undefined | Null => Ok(Self(wasmtime::Val::null_any_ref())),
```
```rust
// AFTER — wasmi floats are F32/F64, .to_float() gives the real value
match self.0 {
  wasmi::Val::I32(x) => x.into_js(ctx),
  wasmi::Val::I64(x) => x.into_js(ctx),                     // BigInt
  wasmi::Val::F32(x) => x.to_float().into_js(ctx),          // f32
  wasmi::Val::F64(x) => x.to_float().into_js(ctx),          // f64
  wasmi::Val::FuncRef(wasmi::Ref::Null)
  | wasmi::Val::ExternRef(wasmi::Ref::Null) => Ok(Value::new_null(ctx.clone())),
  _ => Err(rquickjs::Exception::throw_type(ctx, "unsupported wasm value")),
}
// null/undefined -> there is no any-ref in wasmi; pick externref
rquickjs::Type::Uninitialized | Undefined | Null =>
  Ok(Self(wasmi::Val::ExternRef(wasmi::Ref::Null))),
```
(`wasmtime::Val::F32(u32)` holding raw bits is why `den/instance.rs:99-105` has the `matches!(result, Val::F32(_))
&& item.f64().is_some()` fixup. On wasmi that whole special case disappears — `F32::from_float` does the right
thing.)

### 3.13 `tag.rs`

`den-stdlib-wasm/src/tag.rs:4` is an empty `WebAssembly.Tag` stub with no backend type. It stays a stub on wasmi
and must stay a stub (see below); on wasmtime 48 it *could* be backed by `wasmtime::Tag`
(`wasmtime-48.0.0/src/runtime/externals/tag.rs`). Keep it backend-neutral: a stub whose constructor throws.

### 3.14 The remaining `den` call sites §3.1–3.13 do not cover

Added after a line-by-line sweep of all 1088 lines of `den-stdlib-wasm/src/`. Each of these is a hard
compile error on wasmi with no guidance elsewhere in this document.

#### 3.14.1 `ValType::is_i64()` / `is_v128()` do not exist on wasmi

`instance.rs:117-127` type-checks imported globals with wasmtime's `ValType` predicates:

```rust
// BEFORE (instance.rs:116-127)
let external: Option<Extern> = match module_import.ty() {
  wasmtime::ExternType::Global(ty) if ty.content().is_i64()  && v.is_number()      => None,
  wasmtime::ExternType::Global(ty) if !ty.content().is_i64() && v.as_big_int().is_some() => None,
  wasmtime::ExternType::Global(ty) if ty.content().is_v128() => None,
  /* … */
};
```

`wasmtime::ValType` has `is_i64()` (`types.rs:193`) and `is_v128()` (`types.rs:211`). **wasmi's `ValType`
has exactly two predicates — `is_num()` (`wasmi_core/src/value.rs:30`) and `is_ref()` (`:38`) — and
nothing else.** Compile-verified: both `t.is_i64()` and `t.is_v128()` fail with
`E0599: no method named … found for enum ValType`.

`wasmi::ValType` is a flat `Copy + PartialEq` enum (derive at `wasmi_core/src/value.rs:8`, enum at `:9-24`), so match on it
directly — this is *shorter* than the wasmtime version:

```rust
// AFTER — note `ty` is now `&GlobalType` because ImportType::ty() is a reference (§2.4);
// GlobalType is Copy (wasmi_core/src/global.rs:48) and content() returns ValType by value.
let external: Option<wasmi::Extern> = match module_import.ty() {
  wasmi::ExternType::Global(ty) if ty.content() == wasmi::ValType::I64 && v.is_number() => None,
  wasmi::ExternType::Global(ty) if ty.content() != wasmi::ValType::I64
                                && v.as_big_int().is_some() => None,
  wasmi::ExternType::Global(ty) if ty.content() == wasmi::ValType::V128 => None,
  /* … */
};
```
**There is no single expression that compiles on both**, because the two `ValType`s have opposite derive
sets — this is the one place wasmtime is the *less* ergonomic of the two:

| | derives | predicates | `content()` returns |
|---|---|---|---|
| `wasmtime::ValType` | `#[derive(Clone, Hash)]` — `types.rs:87`. **No `Copy`, no `PartialEq`** (there is only an *associated* `ValType::eq(a, b)` at `types.rs:320` and `matches()` at `:295`) | `is_i32/is_i64/is_f32/is_f64/is_v128/is_ref/is_funcref/is_externref` — `types.rs:187-235` | `&ValType` — `types.rs:2981` |
| `wasmi::ValType` | `#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]` — `wasmi_core/src/value.rs:8` | only `is_num()`, `is_ref()` | `ValType` **by value** — `wasmi_core/src/global.rs:66` |

So `t == ValType::I64` compiles only on wasmi and `t.is_i64()` compiles only on wasmtime. Add the two
predicates den actually needs to the shim surface ([§7.3](#73-the-shim-surface-the-whole-contract)) — two
one-liners per backend, no enum-tag machinery:

```rust
// backend/wasmi.rs
pub fn global_is_i64(ty: &GlobalType)  -> bool { ty.content() == wasmi::ValType::I64 }
pub fn global_is_v128(ty: &GlobalType) -> bool { ty.content() == wasmi::ValType::V128 }

// backend/wasmtime.rs
pub fn global_is_i64(ty: &GlobalType)  -> bool { ty.content().is_i64() }
pub fn global_is_v128(ty: &GlobalType) -> bool { ty.content().is_v128() }
```
The same asymmetry means `val_type_name`/`val_default`/`new_table` in §7.3 must take `ValType` **by value**
(both are at worst `Clone`) or `&ValType` — never assume `Copy`; only wasmi's is.

#### 3.14.2 `ImportType::ty() -> &ExternType` ripples through the whole `resolve_imports` match

§2.4 states the fact and §3.4 shows it for `module.rs`, but `instance.rs:48` and `instance.rs:116-166`
are the sites that actually hurt. Matching `&ExternType` binds `&GlobalType` / `&TableType` /
`&MemoryType` / `&FuncType`. Three of those are `Copy`, one is not:

| wasmi type | derive | fix at the use site |
|---|---|---|
| `GlobalType` | `Copy` — `wasmi_core/src/global.rs:48` (derive), struct at `:49` | `*ty` |
| `TableType` | `Copy` — `table/ty.rs:7` (derive), struct at `:8` | `*ty` |
| `MemoryType` | `Copy` — `memory/ty.rs:7` (derive), struct at `:8` | `*ty` |
| `FuncType` | **`Clone` only** — `func/ty.rs:22` (derive), struct at `:23` | `ty.clone()` (as §3.7 already shows) |

Concretely:
* `instance.rs:132` `Global::from_type(ty, …)` takes `GlobalType` by value → pass `*ty`.
* `instance.rs:157-158` `let actual_ty = memory.inner.ty(store.as_context()); if actual_ty != ty` →
  `if actual_ty != *ty` (`MemoryType: PartialEq`, `memory/ty.rs:7` (derive), struct at `:8`).
* `instance.rs:166` `_ => unreachable!()` stays legal (wasmi has 4 variants, the wildcard just becomes
  dead) — but delete it so a future variant is a compile error rather than a panic.

#### 3.14.3 The *second* `Global::new` site

§3.11 rewrites `global.rs:67`. There is another one at `instance.rs:300-307`, in the export walker, that
synthesises a global when the instance does not actually export one:

```rust
// BEFORE (instance.rs:299-314)
wasmtime::ExternType::Global(ty) => {
  let global = if let Some(global) = self.get_global(&mut *store, &name) { Ok(global) } else {
    let val = get_default_value_for_val_type(ty.content())…?;
    wasmtime::Global::new(&mut *store, ty, val)          // (store, GlobalType, Val) -> Result
  }.map_err(…)?;
```
```rust
// AFTER — wasmi derives the type from the Val, so the mutability must be carried separately,
// and the call is infallible (global.rs:51) so the surrounding Result/map_err collapses.
wasmi::ExternType::Global(ty) => {
  let global = self.get_global(&*store, &name).unwrap_or_else(|| {
    wasmi::Global::new(&mut *store, wasmi::Val::default(ty.content()), ty.mutability())
  });
```
Note this also drops den's `get_default_value_for_val_type` here — `Val::default(ValType)` (`value.rs:85`)
covers it, same as §3.8. `GlobalType::mutability()` is `wasmi_core/src/global.rs:71`.

#### 3.14.4 `Instance::get_*` take a **shared** context on wasmi

`instance.rs:243,300,319,335` call `self.get_func/get_global/get_table/get_memory(&mut *store, &name)`.
wasmi's are `(&self, store: impl AsContext, name: &str)` (`instance/mod.rs:246,287,299,311`) — shared, not
`AsContextMut`. `&mut Store<T>` still satisfies `AsContext` (`impl<T> AsContext for &'_ mut T`,
`store/context.rs:139-147`), so the existing `&mut *store` compiles unchanged; but since `Instance::exports`
already forces you to hold a shared borrow for the loop (§3.8), prefer `&*store` throughout and only take
`&mut *store` for the two constructors that need it (`Global::new`, `Memory::new`, `Table::new`).

#### 3.14.5 `wat` and the `wat2wasm` global

`lib.rs:102-111` calls `wat::parse_str` from den's own `wat` dependency — **not** through wasmi. It is
backend-independent and needs no change. Do *not* route it through `wasmi::Module::new`'s WAT support
(`module/mod.rs:228`): that returns a `Module`, not bytes, and it is exactly the footgun §2.4 warns about.

---

## 4. WASI

`wasmi_wasi` **1.1.0 exists** (crates.io: `max_stable_version = "1.1.0"`, `default_version = "1.1.0"`; the newer
`2.0.0-beta.10` line tracks wasmi 2.0-beta). It is **not** vendored in the local registry — add it explicitly.

```toml
# den-stdlib-wasm/Cargo.toml
wasmi_wasi = { version = "1.1.0", optional = true }
# …
wasmi = ["dep:wasmi", "dep:wasmi_wasi"]
```

API (`wasmi_wasi-1.1.0/src/lib.rs:1-13`, `src/sync/mod.rs:1-13`, `src/sync/snapshots/preview_1.rs:37-68`):

```rust
pub use wasi_common::{Error, WasiCtx, WasiDir, WasiFile};
pub use wasi_common::sync::*;                    // WasiCtxBuilder, Dir, ambient_authority, …
pub fn add_wasi_snapshot_preview1_to_linker<T, U>(   // re-exported as `add_to_linker`
  linker: &mut wasmi::Linker<T>,
  wasi_ctx: impl Fn(&mut T) -> &mut U + Send + Sync + Copy + 'static,
) -> Result<(), wasi_common::Error>
where U: WasiSnapshotPreview1;
```

BEFORE/AFTER for `den-stdlib-wasm/src/instance.rs:220`:
```rust
// BEFORE
wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |(wasi_ctx, _)| wasi_ctx).unwrap();
// AFTER — same shape, different crate; `Copy` is additionally required on the accessor closure
wasmi_wasi::add_to_linker(&mut linker, |(wasi_ctx, _): &mut StoreData| wasi_ctx).unwrap();
```
`WasiCtx: WasiSnapshotPreview1` via `wasi-common-36.0.0/src/snapshots/preview_1.rs:45`.

**Preview level: `wasip1` only.** No preview2, no components. Matches den's current
`wasmtime_wasi::preview1::WasiP1Ctx` usage, so nothing is lost.

**Dependency-bloat check (important, this was the point of adding wasmi):** `wasmi_wasi` depends on
`wasi-common = { version = "36.0.0", default-features = false, features = ["sync"] }` and
`wiggle = { version = "36.0.0", default-features = false }`
(`wasmi_wasi-1.1.0/Cargo.toml`). `wasi-common` 36's feature table is
`{"default":["trace_log","wasmtime","sync"], "sync":["dep:cap-fs-ext","dep:cap-time-ext","dep:fs-set-times","dep:system-interface","dep:io-lifetimes"], "wasmtime":["dep:wasmtime","wiggle/wasmtime"], …}` and `wiggle` 36's is
`{"default":["wiggle_metadata","wasmtime","wasmtime_async"], "wasmtime":["dep:wasmtime"], …}` (crates.io API).
Since both are pulled with `default-features = false` and neither `wasmtime` feature is enabled,
**`wasmi_wasi` does not drag in wasmtime or cranelift.** It does drag in the `cap-std` family, `rustix`, `witx`,
`async-trait` and `tokio`. Non-trivial but ~an order of magnitude smaller than cranelift.

If WASI is not wanted on the wasmi path, put it behind its own feature: `wasm-wasmi-wasi`.

---

## 5. wasmi-only capabilities worth knowing

| Feature | Where |
|---|---|
| `custom_sections()` on `Module` | `module/mod.rs:451` — **the only place wasmi beats wasmtime for den** |
| Fuel metering built in, no cranelift | `Config::consume_fuel` `engine/config.rs:334`, `Store::set_fuel` `store/mod.rs:195`, `Caller::set_fuel` `func/caller.rs:70` |
| Resumable calls after host traps / out-of-fuel | `Func::call_resumable` `func/mod.rs:454`, `ResumableCall*` `lib.rs:166-172` |
| `EnforcedLimits` on parse/compile | `engine/config.rs:386` |
| `StoreLimits`/`StoreLimitsBuilder` | `limits.rs:14`, `lib.rs:189` |
| `no_std` | `lib.rs:71` |
| Static-buffer memory | `Memory::new_static` `memory/mod.rs:65` |
| `PrunedStore` (type-erased store) | `store/pruned.rs`, `store/mod.rs:12` |

---

## 6. Critical gaps — the ones that decide the abstraction

### 6.1 Exception handling / `Tag`

**Not supported, and worse than "not supported".**

* `README.md:69` lists `exception-handling` as 📅 (tracking issue #1037), not ✅.
* `Config::default_features()` (`engine/config.rs:59-78`) starts from `WasmFeatures::empty()` and never sets
  `EXCEPTIONS`. There is no `Config::wasm_exceptions(bool)`.
* There is **no `Tag`, `TagType`, `ExnRef`, or `ExternType::Tag`** anywhere in the crate.
* If a tag *does* reach the module parser, wasmi **panics**, not errors:
  * `module/import.rs:70-72`: `TypeRef::Tag(tag) => panic!("wasmi does not support the `exception-handling` Wasm proposal but found: {tag:?}")`
  * `module/export.rs:97-99`: `ExternalKind::Tag => panic!("wasmi does not support the `exception-handling` Wasm proposal")`

  Validation *should* reject the module first (the `EXCEPTIONS` feature bit is off, so `Module::validate`
  errors — `module/mod.rs:289` builds the validator from `engine.config().wasm_features()`). But
  `Module::new_unchecked` bypasses validation and would panic. **Never call `new_unchecked`.**

**Implication:** `WebAssembly.Tag` and `WebAssembly.Exception` cannot be implemented on the wasmi backend. They are
already unimplemented stubs in den (`tag.rs:4`, `error.rs:5`). Keep them stubs that throw on construction, on both
backends — do not give the abstraction a `Tag` slot.

### 6.2 GC types

**Not supported.** `README.md:67` marks `gc` 📅 and `README.md:66` `function-references` 📅. `ValType` has exactly
`{I32,I64,F32,F64,V128,FuncRef,ExternRef}` (`wasmi_core/src/value.rs:9-24`). There is no `anyref`, `i31ref`,
`structref`, `arrayref`, `eqref`, `nullref`, no `RefType`, no `HeapType`, no `Rooted<T>`/`RootScope`.

`Config::wasm_reference_types(true)` does set `WasmFeatures::GC_TYPES` (`engine/config.rs:225`) but the comment
at `:68` says it plainly: *"required by reference-types"* — it only unlocks wasmparser's subtyping machinery for
`funcref`/`externref`, not GC.

`ExternRef` in wasmi is a store-indexed handle (`reftype.rs:99`, `Copy`, 8 bytes per `reftype.rs:134-144`) that
wraps a `Box<dyn Any + Send + Sync>` (`reftype.rs:75-77`). **`Send + Sync` on the payload is a real problem for
den**: you cannot put a `rquickjs::Persistent<Value>` in an `ExternRef` unless it is `Send + Sync`. The
`DangerouslyImplementSync` wrapper den already defines at `instance.rs:54-57` is the escape hatch, but it must be
`'static + Any` too. (wasmtime's `ExternRef::new` has the same `Send + Sync + 'static` requirement, so this is not
a regression — it's just still unsolved.)

**Implication:** `WebAssembly.Global({value:"anyref"})` and `WebAssembly.Table({element:"anyref"})` must throw on
wasmi. den's `GlobalDescriptor::from_js` currently *accepts* `"anyref"` (`global.rs:97`) — that needs a
backend-aware allow-list.

### 6.3 Shared memory / threads

**Not supported.**
* `README.md:68`: `threads` 📅.
* No `THREADS`/`SHARED_EVERYTHING_THREADS` bit in `Config::default_features()` (`engine/config.rs:59-78`), no
  `Config::wasm_threads`.
* `MemoryTypeBuilder` has `memory64`, `min`, `max`, `page_size_log2` — and **no `shared`**
  (`memory/ty.rs:97-161`). `MemoryType` has `is_64()` but no `is_shared()` (`memory/ty.rs:52`).
* No `SharedMemory` type, no `Memory::atomic_notify`/`atomic_wait`.

**Implication:** `new WebAssembly.Memory({initial, maximum, shared: true})` must throw a `TypeError`/`RangeError`
on wasmi. den's `MemoryDescriptor` already carries `shared: Option<bool>` (`memory.rs:18`); the wasmi shim rejects
`Some(true)`.

### 6.4 Custom sections

**Supported — and this is the *only* capability where wasmi is ahead of wasmtime for den's purposes.**
`Module::custom_sections() -> CustomSectionsIter<'_>` (`module/mod.rs:451`), gated off by
`Config::ignore_custom_sections(true)` (`engine/config.rs:349`, default `false` at `:49`).
`CustomSection::{name, data}` at `module/custom_section.rs:91,97`.

`WebAssembly.Module.customSections(module, sectionName)` is therefore implementable on wasmi today
(`den-stdlib-wasm/src/module.rs:80-83` currently throws "not implemented"). Design the abstraction so that
`custom_sections` is a *shim function that may return an empty iterator*, not an unimplemented-everywhere stub.

### 6.5 `Send` / `'static`

**`Store<T>`:** wasmi asserts `Store<()>: Send + Sync` (`store/mod.rs:369-378`). `Store<T>` is `Send`/`Sync` iff
`T` is, since the only `T`-typed field is a `Box<T>` (`store/mod.rs:304`) and everything else is
`Box<dyn … + Send + Sync>` (`:326`, `:339`). Same as wasmtime.

**Host closures:** `Send + Sync + 'static` — identical to wasmtime.
* `Func::new` — `func/mod.rs:357`
* `Linker::func_new` — `linker.rs:291`
* `IntoFunc: Send + Sync + 'static` — `func/into_func.rs:23`
* stored as `Arc<dyn Fn(Caller<T>, FuncInOut) -> Result<FuncFinished, Error> + Send + Sync + 'static>` — `func/mod.rs:267-272`

So den's existing `DangerouslyImplementSync` + `Mutex` trick (`instance.rs:54-60`) transfers verbatim.

**`T: 'static`: wasmi does NOT require it; wasmtime 48 DOES.** This is the single biggest behavioural difference
for den's design.

| | wasmi 1.1 | wasmtime 48 | wasmtime 27 |
|---|---|---|---|
| `Store::new` | no bound (`store/mod.rs:61`) | no bound (`store.rs:698`) | no bound |
| `Linker::define` | no bound (`linker.rs:268`) | `T: 'static` (`linker.rs:377`) | no bound (`linker.rs:350`) |
| `Linker::func_new` | no bound (`linker.rs:286`) | `T: 'static` (`linker.rs:416`) | no bound |
| `Linker::instantiate*` | no bound (`linker.rs:430`) | `T: 'static` (`linker.rs:1096`) | no bound |
| `Instance::exports` | `T: 'ctx` (`instance/mod.rs:322`) | `T: 'static` (`instance.rs:390`) | `T: 'a` (`instance.rs:394`) |
| `Func::new` | no bound (`func/mod.rs:354`) | `T: 'static` (`func.rs:374`) | no bound |

Verified by compilation: a `Store<(u32, JsCtx<'a>)>` + `Linker<(u32, JsCtx<'a>)>::func_new` + `Func::new` compiles
against wasmi 1.1.0 (Appendix A).

**Design consequence:** if you want *one* set of `#[rquickjs::class]` structs shared by both backends, the
`StoreData` type must satisfy the *stricter* backend, i.e. it must be `'static`. Options:
1. Make `StoreData` `'static` on both backends (drop `Ctx<'js>` from the store; recover the context another way).
   Uniform, but you must solve "how does a host closure reach the JS context".
2. Keep `StoreData<'js>` and accept that the *wasmtime* backend needs a different store-data strategy (i.e. den's
   wasmtime 48 upgrade already has to solve this — presumably doc 01/02's job). The wasmi backend then keeps the
   simpler design.

I flag (1) vs (2) as an **open question**, not a recommendation — see [§8](#8-open-questions).

### 6.6 SIMD / v128

**Supported but off by default and feature-gated at *compile* time.**
* Crate feature `simd` (`wasmi-1.1.0/Cargo.toml`), off by default (`default = ["std","wat"]`).
* `Config::wasm_simd`/`wasm_relaxed_simd` only exist `#[cfg(feature = "simd")]` (`engine/config.rs:292,303`).
* `default_features()` sets `SIMD`/`RELAXED_SIMD` to `cfg!(feature = "simd")` (`engine/config.rs:75-76`).
* `ValType::V128` and `Val::V128(V128)` exist **unconditionally** in the enums (`wasmi_core/src/value.rs:19`,
  `wasmi-1.1.0/src/value.rs:75`), but the conversions **panic** without the feature:
  `value.rs:35` `unimplemented!("encountered unsupported ValType")`, `:52` same for `Val`, `:245-253`
  `panic!("`simd` crate feature is disabled")`.

**Implication:** every `match` on `wasmi::Val`/`ValType` must handle `V128` — it is not `cfg`'d out. Handle it by
throwing a JS `TypeError` ("v128 is not representable in JS"), which is what the spec says anyway. den's
`global.rs:53` already does exactly this for wasmtime. If you *do* want v128 support in wasm bodies (not at the JS
boundary), turn on `wasmi/simd`; it costs execution overhead per the crate docs (`lib.rs:66`).

### 6.7 Other differences worth a line each

* **No async.** No `Config::async_support`, no `Func::call_async`, no `func_new_async`, no fibers. den's
  `#[rquickjs::function] pub async fn instantiate` (`lib.rs:48`) stays "async in JS, sync in Rust" — unchanged from
  today, since `engine.rs:24` has `async_support` commented out.
* **No `InstancePre`.** No pre-instantiation caching.
* **No module serialization.** No `Module::serialize`/`deserialize`, no on-disk code cache.
* **No `Module::name()`.**
* **`Config::compilation_mode` defaults to `LazyTranslation`** (`engine/config.rs:32-33`), so translation errors can
  surface at *first call* rather than at `Module::new`. For a spec-faithful `WebAssembly.compile` (which must reject
  the promise at compile time) set `CompilationMode::Eager`.
* **Engine mismatch panics.** `Linker::get_definition` (`linker.rs:365`) and `instantiate_and_start`
  (`linker.rs:435`) `assert!(Engine::same(…))`. den stores the `Engine` and the `Store` as separate rquickjs
  userdata (`lib.rs:116-119`) — they are consistent today, but a `new WebAssembly.Engine()` from JS
  (`engine.rs:21-22` exposes a constructor!) would produce a second engine and panic the process. Worth removing
  the JS-visible `Engine` constructor; it's not in the spec anyway.

---

## 7. Recommended abstraction design

### 7.1 The three candidates, and why two lose

**(a) A `WasmBackend` trait (or a family of traits).**
Rejected. It would need `type Store`, `type StoreContextMut<'a>`, `type Val`, `type ValType`, `type Error`, …, i.e.
GATs plus `AsContextMut` supertrait plumbing, and then — because `#[rquickjs::class]` structs cannot be generic —
you would *still* need a `#[cfg]`-selected type alias `pub type Backend = WasmtimeBackend;` to name the single
concrete impl inside every class. You pay all the boilerplate and buy nothing: at any given compile there is
exactly **one** implementation. That is the textbook "interface with one implementation".
Second problem: `Val` cannot be abstracted usefully. wasmtime's has 10 variants including `AnyRef`/`ExnRef`/`ContRef`;
wasmi's has 7 with `F32(F32)` and `Ref<T>`. Any trait would force a den-owned neutral value enum plus two
conversions in each direction — which is exactly what a shim function gives you, without the trait.

**(b) An `enum Backend { Wasmtime(..), Wasmi(..) }`.**
Rejected harder. It requires **both** crates to compile in every build. wasmtime 48 pulls cranelift; the entire
point of the wasmi feature is to *not* pay that. It also adds a runtime match on every single call for a choice
that is fixed at compile time, and forces den to reconcile the two `Val` enums at runtime rather than at the source
level.

**(c) cfg-gated type aliases + a thin shim module. ← recommended.**
One `backend` module, two `#[cfg]` submodules that export the *same item names*. Zero traits, zero generics, zero
dynamic dispatch, zero runtime cost. den's classes name `backend::Memory`, `backend::Val`, etc. and are written
once. Where the two APIs genuinely disagree (nine places, enumerated in §3), the shim exposes one small free
function and each submodule implements it.

This is also the smallest diff from what exists: `engine.rs`, `store.rs`, `module.rs`, `instance.rs`, `memory.rs`,
`table.rs`, `global.rs`, `utils.rs` change their `use wasmtime::…` lines to `use crate::backend::…` and adjust the
handful of call sites in §3.

### 7.2 Module layout

```
den-stdlib-wasm/src/
  lib.rs                # unchanged rquickjs module; `mod backend;`
  backend/
    mod.rs              # #[cfg]-selects one submodule and re-exports it as `pub use`
    wasmtime.rs         # aliases + shims for wasmtime 48
    wasmi.rs            # aliases + shims for wasmi 1.1
  engine.rs             # #[rquickjs::class] Engine   { inner: backend::Engine }
  store.rs              # #[rquickjs::class] Store    { inner: Arc<RefCell<backend::Store>> }
  module.rs             # #[rquickjs::class] Module   { inner: backend::Module }
  instance.rs           # #[rquickjs::class] Instance { inner: backend::Instance }
  memory.rs  table.rs  global.rs   # ditto
  tag.rs                # stays a throwing stub on BOTH backends (§6.1)
  error.rs              # CompileError / LinkError / RuntimeError / Exception, backend-neutral
  utils.rs              # JS <-> backend::Val, delegating to backend::{val_to_js, val_from_js}
```

`den-stdlib-wasm/Cargo.toml`: make the two features mutually exclusive and fail loudly, because Cargo feature
unification *will* enable both if two dependents disagree:

```toml
[features]
default  = ["wasmtime"]
wasmtime = ["dep:wasmtime", "dep:wasmtime-wasi"]
wasmi    = ["dep:wasmi", "dep:wasmi_wasi"]
```
```rust
// backend/mod.rs
#[cfg(all(feature = "wasmtime", feature = "wasmi"))]
compile_error!("den-stdlib-wasm: enable exactly one of `wasmtime` or `wasmi`");
#[cfg(not(any(feature = "wasmtime", feature = "wasmi")))]
compile_error!("den-stdlib-wasm: enable exactly one of `wasmtime` or `wasmi`");

#[cfg(feature = "wasmtime")] mod wasmtime;
#[cfg(feature = "wasmtime")] pub use self::wasmtime::*;
#[cfg(feature = "wasmi")]    mod wasmi;
#[cfg(feature = "wasmi")]    pub use self::wasmi::*;
```

### 7.3 The shim surface (the whole contract)

Both `backend/wasmtime.rs` and `backend/wasmi.rs` export exactly these names.

```rust
// ---- direct type aliases (no wrapping, no cost) ----
pub type Engine      = wasmi::Engine;
pub type Config      = wasmi::Config;
pub type Store       = wasmi::Store<StoreData>;
pub type StoreData   = (wasmi_wasi::WasiCtx, JsHandle);   // see §8 open question
pub type Linker      = wasmi::Linker<StoreData>;
pub type Module      = wasmi::Module;
pub type Instance    = wasmi::Instance;
pub type Func        = wasmi::Func;
pub type FuncType    = wasmi::FuncType;
pub type Memory      = wasmi::Memory;
pub type MemoryType  = wasmi::MemoryType;
pub type Table       = wasmi::Table;
pub type TableType   = wasmi::TableType;
pub type Global      = wasmi::Global;
pub type GlobalType  = wasmi::GlobalType;
pub type Mutability  = wasmi::Mutability;
pub type Extern      = wasmi::Extern;
pub type ExternType  = wasmi::ExternType;
pub type Val         = wasmi::Val;
pub type ValType     = wasmi::ValType;
pub type Caller<'a>  = wasmi::Caller<'a, StoreData>;
pub type Error       = wasmi::Error;                       // Display on both backends
pub use wasmi::{AsContext, AsContextMut};                  // same trait names + same assoc type

/// Backend capability flags — read by the JS layer to throw spec-correct errors.
pub const SUPPORTS_SHARED_MEMORY: bool = false;   // wasmtime.rs: true
pub const SUPPORTS_TAGS:          bool = false;   // wasmtime.rs: true
pub const SUPPORTS_ANYREF:        bool = false;   // wasmtime.rs: true
pub const SUPPORTS_V128:          bool = cfg!(feature = "wasmi-simd");
pub const NAME: &str = "wasmi";

// ---- the nine shim functions where the APIs actually differ ----

/// wasmi: Engine::new is infallible.  wasmtime: returns Result.
pub fn new_engine() -> Engine;

/// wasmi: no `from_binary`; validate-then-new.  wasmtime: `from_binary`.
pub fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<Module, Error>;

/// wasmtime takes a store; wasmi does not.
pub fn linker_define(
  linker: &mut Linker, store: &mut Store, module: &str, name: &str, item: Extern,
) -> Result<(), Error>;

/// wasmtime: `Linker::instantiate`.  wasmi: `Linker::instantiate_and_start`.
pub fn linker_instantiate(
  linker: &Linker, store: &mut Store, module: &Module,
) -> Result<Instance, Error>;

/// wasmtime: `Global::new(store, GlobalType, Val) -> Result`.
/// wasmi:    `Global::new(store, Val, Mutability) -> Global`.
pub fn new_global(store: &mut Store, value: Val, mutable: bool) -> Result<Global, Error>;

/// wasmtime: `TableType::new(RefType, u32, Option<u32>)` + `Ref` init.
/// wasmi:    `TableType::new(ValType, u32, Option<u32>)` + `Val` init.
pub fn new_table(
  store: &mut Store, element: ValType, min: u32, max: Option<u32>,
) -> Result<Table, Error>;

/// wasmi rejects `shared == true`.
pub fn new_memory_type(min: u64, max: Option<u64>, shared: bool) -> Result<MemoryType, Error>;

/// `"i32"|"i64"|"f32"|"f64"|"v128"|"externref"|"anyfunc"|"anyref"` -> backend ValType.
/// wasmi returns None for "anyref".
pub fn val_type_from_str(s: &str) -> Option<ValType>;
pub fn val_type_name(t: ValType) -> &'static str;   // for Module.imports()/exports() "kind"

/// ValType predicates. Neither backend's spelling compiles on the other: wasmtime's `ValType`
/// has `is_i64()`/`is_v128()` but no `PartialEq`; wasmi's has `PartialEq` but no `is_i64()`.
/// See §3.14.1. Used by `instance.rs:118,123,127`.
pub fn global_is_i64(ty: &GlobalType)  -> bool;
pub fn global_is_v128(ty: &GlobalType) -> bool;

/// Global content type + mutability, for the two `Global::new` sites (§3.11, §3.14.3).
/// wasmtime: `content() -> &ValType`.  wasmi: `content() -> ValType` (Copy).
pub fn global_content(ty: &GlobalType) -> ValType;

/// Host-function error construction (anyhow::Error vs wasmi::Error).
pub fn host_error(msg: &str) -> Error;

/// Register a dynamic host import. Hides the closure's return type
/// (`anyhow::Result<()>` vs `Result<(), wasmi::Error>`).
pub fn linker_func_new<F>(
  linker: &mut Linker, module: &str, name: &str, ty: FuncType, f: F,
) -> Result<(), Error>
where F: Fn(Caller<'_>, &[Val], &mut [Val]) -> Result<(), Error> + Send + Sync + 'static;

/// WASI preview1.
pub fn add_wasi_to_linker(linker: &mut Linker) -> Result<(), Error>;

/// Custom sections: wasmi yields real data, wasmtime 48 yields nothing.
pub fn custom_sections<'m>(m: &'m Module, name: &str) -> impl Iterator<Item = &'m [u8]> + 'm;

// ---- value bridging lives per backend (utils.rs just forwards) ----
pub fn val_to_js<'js>(v: &Val, ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<rquickjs::Value<'js>>;
pub fn val_from_js<'js>(v: rquickjs::Value<'js>, want: Option<ValType>, ctx: &rquickjs::Ctx<'js>)
  -> rquickjs::Result<Val>;
pub fn val_default(t: ValType) -> Val;   // wasmi: Val::default(t); wasmtime: den's existing helper
```

Two things this buys immediately:
* Every `match ext.ty(..) { … }` in `instance.rs` becomes 4 arms on wasmi and 5 on wasmtime — so wrap the
  discrimination in `val_type_name`/`extern_kind_name` shims and the shared code never matches on `ExternType`
  directly. That kills the `E0004` from §1.1 permanently.
* `SUPPORTS_*` consts let the JS-facing code produce spec-correct `TypeError`s ("shared memory unsupported") with
  a plain `if !backend::SUPPORTS_SHARED_MEMORY { throw }`, no `cfg!` scattered through the class bodies.

### 7.4 Sketch of `backend/wasmi.rs` (the parts that aren't aliases)

```rust
pub fn new_engine() -> Engine {
  let mut config = wasmi::Config::default();
  config.compilation_mode(wasmi::CompilationMode::Eager); // spec: compile errors at compile time
  config.ignore_custom_sections(false);                   // default, but be explicit: we expose them
  wasmi::Engine::new(&config)
}

pub fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<Module, Error> {
  // Module::new would also accept WAT text (module/mod.rs:228); validate() is binary-only.
  wasmi::Module::validate(engine, bytes)?;
  wasmi::Module::new(engine, bytes)
}

pub fn linker_define(
  linker: &mut Linker, _store: &mut Store, module: &str, name: &str, item: Extern,
) -> Result<(), Error> {
  linker.define(module, name, item)?;   // LinkerError -> Error via From (error.rs:336)
  Ok(())
}

pub fn linker_instantiate(
  linker: &Linker, store: &mut Store, module: &Module,
) -> Result<Instance, Error> {
  linker.instantiate_and_start(&mut *store, module)
}

pub fn new_global(store: &mut Store, value: Val, mutable: bool) -> Result<Global, Error> {
  let m = if mutable { wasmi::Mutability::Var } else { wasmi::Mutability::Const };
  Ok(wasmi::Global::new(&mut *store, value, m))
}

pub fn new_table(
  store: &mut Store, element: ValType, min: u32, max: Option<u32>,
) -> Result<Table, Error> {
  wasmi::Table::new(&mut *store, wasmi::TableType::new(element, min, max),
                    wasmi::Val::default(element))
}

pub fn new_memory_type(min: u64, max: Option<u64>, shared: bool) -> Result<MemoryType, Error> {
  if shared {
    return Err(wasmi::Error::new(
      "shared memory requires the threads proposal, unsupported by the wasmi backend"));
  }
  let mut b = wasmi::MemoryType::builder();
  b.min(min);
  b.max(max);
  Ok(b.build()?)  // MemoryError -> Error (error.rs:334)
}

pub fn val_type_from_str(s: &str) -> Option<ValType> {
  Some(match s {
    "i32" => ValType::I32, "i64" => ValType::I64,
    "f32" => ValType::F32, "f64" => ValType::F64,
    "v128" => ValType::V128,
    "externref" => ValType::ExternRef,
    "anyfunc" | "funcref" => ValType::FuncRef,
    _ => return None,       // notably: "anyref" is NOT supported here
  })
}

pub fn val_type_name(t: ValType) -> &'static str {
  match t {
    ValType::I32 => "i32", ValType::I64 => "i64",
    ValType::F32 => "f32", ValType::F64 => "f64",
    ValType::V128 => "v128",
    ValType::FuncRef => "anyfunc", ValType::ExternRef => "externref",
  }
}

pub fn global_is_i64(ty: &GlobalType)  -> bool { ty.content() == ValType::I64 }
pub fn global_is_v128(ty: &GlobalType) -> bool { ty.content() == ValType::V128 }
pub fn global_content(ty: &GlobalType) -> ValType { ty.content() }   // Copy on wasmi

pub fn extern_kind_name(t: &ExternType) -> &'static str {
  match t {
    ExternType::Func(_)   => "function",
    ExternType::Global(_) => "global",
    ExternType::Table(_)  => "table",
    ExternType::Memory(_) => "memory",
  }
}

pub fn host_error(msg: &str) -> Error { wasmi::Error::new(msg) }

pub fn linker_func_new<F>(
  linker: &mut Linker, module: &str, name: &str, ty: FuncType, f: F,
) -> Result<(), Error>
where F: Fn(Caller<'_>, &[Val], &mut [Val]) -> Result<(), Error> + Send + Sync + 'static {
  linker.func_new(module, name, ty, f)?;
  Ok(())
}

pub fn add_wasi_to_linker(linker: &mut Linker) -> Result<(), Error> {
  wasmi_wasi::add_to_linker(linker, |(wasi, _): &mut StoreData| wasi)
    .map_err(|e| wasmi::Error::new(e.to_string()))
}

pub fn custom_sections<'m>(m: &'m Module, name: &str) -> impl Iterator<Item = &'m [u8]> + 'm {
  let name = name.to_owned();
  m.custom_sections().filter(move |s| s.name() == name).map(|s| s.data())
}

pub fn val_default(t: ValType) -> Val { wasmi::Val::default(t) }
```

`backend/wasmtime.rs` is the mirror image; the interesting arms are `new_global` (build a `GlobalType` from
`val.ty(&store)?`), `new_table` (`ValType::Ref(rt)` → `TableType::new(rt, …)` + `Ref` init), `new_memory_type`
(`MemoryTypeBuilder::shared(shared)`), `extern_kind_name` (add the `Tag(_) => "tag"` arm), `custom_sections`
(`core::iter::empty()`), `global_is_i64`/`global_is_v128` (`ty.content().is_i64()` / `.is_v128()`), and
`global_content` (`ty.content().clone()` — wasmtime's `ValType` is `Clone`-only, `types.rs:87`).

One alias needs a warning rather than a mirror: **`pub type Val`**. On wasmtime it is `Copy`, on wasmi it is
`Clone`-only ([§2.10.1](#2101-wasmival-is-not-copy--wasmtimes-is)). Shared code above the shim must
therefore treat `backend::Val` as `Clone`-only, or the wasmtime build will compile and the wasmi build will
not. Same for `backend::ValType`: `Copy + PartialEq` on wasmi, neither on wasmtime — shared code must use
*neither* property and go through `global_is_*` / `val_type_name`.

### 7.5 One known ceiling to mark, not fix now

`den-stdlib-wasm/src/instance.rs:262` calls `func.call(&mut *store.borrow_mut(), …)` from inside a JS callback.
If wasm calls a host import, and that host import's JS body calls an exported wasm function, the `RefCell` is
already mutably borrowed by the enclosing `instantiate`/`call` and this **panics**. It is a pre-existing bug on the
wasmtime path and ports verbatim.

The fix costs nothing extra while you are already touching this code: inside a host closure, use the `Caller` as
the store context (`wasmi::Caller` impls `AsContextMut` — `func/caller.rs:84`; so does wasmtime's) rather than
re-borrowing the `Rc<RefCell<Store>>`. Mark it if you defer:
```rust
// ponytail: single RefCell over the whole Store; wasm -> JS -> wasm re-entrancy panics.
// Fix by threading `Caller` (impls AsContextMut) instead of re-borrowing the Rc.
```

---

## 8. Open questions

1. **`StoreData: 'static` or not?** wasmi allows `Store<(WasiCtx, Ctx<'js>)>`; wasmtime 48 does not (§6.5). Either
   (a) both backends move to a `'static` store payload and host closures reach the JS context by some other route,
   or (b) the two backends carry different `StoreData` and the classes get a `'js` lifetime only on wasmi — which
   breaks the "one set of classes" goal. Needs a decision before any code is written; it determines whether
   `backend::StoreData` can be a plain alias or has to be a lifetime-parameterised alias. Note that if you go with
   (a), the obvious "capture `rquickjs::Context` and call `Context::with`" trick may re-enter the runtime lock from
   inside a host call — that needs to be checked against rquickjs before committing.
2. **Do we want `wasmi/simd`?** Off by default. Enabling it makes `v128` valid inside wasm bodies (still not
   representable at the JS boundary) at a documented execution cost (`wasmi-1.1.0/src/lib.rs:66`). Suggest: leave
   off, expose as `wasm-wasmi-simd`.
3. **Do we want `wasmi_wasi` at all?** It costs the `cap-std`/`rustix`/`witx`/`tokio` tree (but not wasmtime —
   verified in §4). If the wasmi backend exists to be *small*, WASI should be its own opt-in feature.
4. **`WebAssembly.Engine` is exposed to JS** (`den-stdlib-wasm/src/engine.rs:21-22` has `#[qjs(constructor)]`).
   It is not in the JS-API spec and a second engine will trip wasmi's `assert!(Engine::same(…))`
   (`linker.rs:435`) — process abort, not an exception. Recommend removing the constructor. Confirm nothing in
   den's JS depends on it.
5. **`WebAssembly.Memory.prototype.buffer`** is unimplemented on both paths (`memory.rs:75`). wasmi's buffer moves
   on grow (§3.9) so the detach-on-grow protocol is mandatory. Is a copying `ArrayBuffer` acceptable as a first
   cut, or must it be zero-copy from day one?
6. **`Module::imports()`/`exports()` are static methods in den** (`module.rs:53,67` use `#[qjs(static)]` and are
   exposed at `lib.rs:127-131`). The spec puts them on `WebAssembly.Module` as statics too, so that's right — but
   `customSections` takes a `sectionName` argument in the spec and den's signature (`module.rs:81`) does not.
   Worth fixing while implementing §3.5.

---

## Appendix A — compile-verified probe

Every wasmi call shown above was type-checked by compiling this against `wasmi 1.1.0` (default features:
`std`, `wat`), with a deliberately non-`'static` store payload. It builds clean; a deliberately-broken line was
inserted and removed to prove the check was real.

Location: `<scratchpad>/probe/src/lib.rs`. Highlights:

```rust
pub struct JsCtx<'js>(pub &'js str);
pub type StoreData<'js>  = (u32, JsCtx<'js>);
pub type SharedStore<'js> = Rc<RefCell<Store<StoreData<'js>>>>;

// non-'static T in Store + Linker + Func::new  ->  COMPILES on wasmi, would not on wasmtime 48
pub fn resolve_imports<'js>(
  module: &Module, store: &mut Store<StoreData<'js>>, linker: &mut Linker<StoreData<'js>>,
) -> Result<(), Error> {
  for import in module.imports() {
    let (m, n) = (import.module().to_string(), import.name().to_string());
    match import.ty().clone() {                       // &ExternType -> clone
      ExternType::Func(ty) => {
        let captured = Mutex::new(NotSync(0u32));     // den's DangerouslyImplementSync shape
        linker.func_new(&m, &n, ty, move |caller: Caller<'_, StoreData<'js>>, params, results| {
          let _ = caller.data().1 .0;
          let _ = captured.lock().unwrap().0;
          for (i, r) in results.iter_mut().enumerate() {
            *r = params.get(i).cloned().unwrap_or(Val::I32(0));
          }
          Ok(())
        })?;
      }
      ExternType::Global(ty) => {
        let g = Global::new(&mut *store, Val::default(ty.content()), ty.mutability());
        linker.define(&m, &n, g)?;                    // no store argument
      }
      ExternType::Table(ty)  => { let t = Table::new(&mut *store, ty, Val::default(ty.element()))?;
                                  linker.define(&m, &n, t)?; }
      ExternType::Memory(ty) => { let x = Memory::new(&mut *store, ty)?;
                                  linker.define(&m, &n, x)?; }
    }                                                  // exhaustive with 4 arms
  }
  Ok(())
}
```

Also verified in the same crate: `linker.instantiate_and_start`, `inst.exports(&*store)` with a **shared** borrow
followed by `f.call(&mut *store.borrow_mut(), …)`, `Memory::{new,data_mut,grow,ty,size}`,
`Table::{new,get,set,grow,size,ty().element()}`, `Global::{new,get,set,ty().mutability()}`,
`Module::{validate,new,imports,exports,custom_sections}`, `Func::new`, and
`Store::{as_context,as_context_mut}` producing `StoreContext`/`StoreContextMut`.

---

## Verification log

**2026-08-22 — completeness/accuracy audit.** Independent re-read of the local crate sources; every line
reference below was opened, not recalled. Crate roots:
`/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{wasmi-1.1.0,wasmi_core-1.1.0,wasmtime-48.0.0}`,
`wasmi_wasi-1.1.0` + `wasi-common-36.0.0` from the extracted `.crate` files in the session scratchpad,
den sources at `/home/steve/git/github.com/stevefan1999-personal/den/`.

### Claims checked and CONFIRMED

| Claim | Evidence |
|---|---|
| `Engine::new(&Config) -> Self`, infallible | `wasmi-1.1.0/src/engine/mod.rs:144`; `Default` at `:132`, `same` at `:163`, `weak` at `:151`, `config` at `:158` — all as documented |
| `Linker::define(module, name, item) -> Result<&mut Self, LinkerError>`, **no store arg** | `linker.rs:268-275` |
| `Linker::instantiate_and_start(ctx, &Module) -> Result<Instance, Error>` | `linker.rs:430-441`; `assert!(Engine::same(…))` at `:435` confirmed |
| `Linker::new/allow_shadowing/func_wrap/get/instance/alias_module` at `243/258/320/340/387/415` | `linker.rs`, all exact |
| `Global::new(ctx, Val, Mutability) -> Self` — no `GlobalType`, infallible | `global.rs:51`; `ty` `:63`, `set` `:77` (`Result<(), GlobalError>`), `get` `:90` |
| `TableType::new(element: ValType, min: u32, max: Option<u32>)`; `element() -> ValType` by value | `table/ty.rs:18`, `:52`; `Table::new(ctx, ty, init: Val)` at `table/mod.rs:49`, `get/set/grow` at `:135/:154/:114` |
| `MemoryTypeBuilder` has `new/memory64/min/max/page_size_log2/build` and **no `shared`** | `memory/ty.rs:103,114,122,132,148,158`; `MemoryType::{new,new64,builder,is_64,minimum,maximum}` at `:19,36,45,52,62,69`, **no `is_shared`** |
| `FuncType::params()/results() -> &[ValType]`; `FuncType::new` **panics** | `func/ty.rs:48,53`; `panic!("failed to create function type: {error}")` at `:42` |
| `Val` has exactly 7 variants with `F32(F32)/F64(F64)/FuncRef(Ref<Func>)/ExternRef(Ref<ExternRef>)` | `value.rs:65-80` |
| `Val::default(ValType) -> Val` and `Val::ty(&self) -> ValType` (no store) | `value.rs:85`, `:99` |
| `Ref<T>::{Val(T), Null}`, `#[derive(Debug, Default, Copy, Clone)]` | `reftype.rs:13-20` |
| `ExternRef::new(ctx, T)` requires `T: 'static + Any + Send + Sync`; `data(ctx) -> &dyn Any` | `reftype.rs:113-117`, `:128` |
| `Module::{new,new_unchecked,validate,engine,imports,exports,get_export,custom_sections}` at `226,253,288,259,328,396,407,451` | `module/mod.rs` |
| `ImportType::{module,name,ty}` at `544/549/554`; `ty()` returns **`&ExternType`** | `module/mod.rs` |
| `ModuleImportsIter: ExactSizeIterator` (`module/mod.rs:507`) but `ModuleExportsIter` is `Iterator`-only | `module/export.rs:145-155` — no `ExactSizeIterator` impl anywhere in the file |
| `ExportType::{name,ty}` at `module/export.rs:125,130`; `ty()` returns `&ExternType` | ✓ |
| `CustomSection::{name,data}` at `module/custom_section.rs:91,97` | ✓ |
| `Instance::exports<'ctx, T: 'ctx>(store: impl Into<StoreContext<'ctx,T>>) -> ExportsIter<'ctx>` — **shared** ctx | `instance/mod.rs:322-327`. Note `ExportsIter` *is* `ExactSizeIterator` (`instance/exports.rs:276`) — unlike `ModuleExportsIter` |
| `Instance::{new,get_export,get_func,get_typed_func,get_global,get_table,get_memory}` at `185,229,246,264,287,299,311`, all `impl AsContext` | ✓ |
| `Extern` has exactly 4 variants and is `Copy`; `ExternType` has exactly 4 and is `Clone` | `instance/exports.rs:20-32`, `:114-127` |
| `Func::{new,wrap,ty,call,call_resumable,typed}` at `354,370,394,414,454,512`; `call` pre-validates via `verify_and_prepare_inputs_outputs` | `func/mod.rs:414-424` |
| `Memory::{new,new_static,ty,size,grow,data,data_mut,data_and_store_mut,data_ptr,data_size,read,write}` at `49,65,84,114,130,146,155,165,178,189,207,230`; `grow -> Result<u64, MemoryError>` | `memory/mod.rs` |
| wasmi memory buffer pointer **moves on grow** | `wasmi_core-1.1.0/src/memory/buffer.rs:132-146` — `try_reserve` + `resize` + `(self.ptr, self.len, self.capacity) = vec_into_raw_parts(…)` |
| `Store::{new,engine,data,data_mut,into_data,limiter,get_fuel,set_fuel,call_hook}` at `63,75,80,85,90,97,182,195,254`; `CallHook` at `:349` | `store/mod.rs` |
| `AsContext`/`AsContextMut` trait shape + `From` impls at `store/context.rs:53,60,67` | ✓ verbatim |
| `Error::{new,host,i32_exit,as_trap_code,downcast_ref,downcast_mut,downcast}` at `46,56,70,80,95,108,121`; `Display` at `:170`; `ErrorKind` at `:179` | `error.rs` |
| `Config` method line numbers (all 20 of them, `87`→`386`) | `engine/config.rs` — every single one matched |
| `Config::default_features()` starts from `WasmFeatures::empty()`, sets `GC_TYPES` "required by reference-types", sets `SIMD`/`RELAXED_SIMD` from `cfg!(feature = "simd")`, **never sets `EXCEPTIONS`/`THREADS`** | `engine/config.rs:59-80` |
| `CompilationMode::{Eager, LazyTranslation (#[default]), Lazy}` | `engine/config.rs:28-42` |
| Crate features `default = ["std","wat"]`, `simd` off by default | `wasmi-1.1.0/Cargo.toml` `[features]`; docs table at `src/lib.rs:64-69`; `#![no_std]` at `:71` |
| README proposal table: `function-references`/`gc`/`threads`/`exception-handling` all 📅 at lines 66/67/68/69; "Loosely mirrors the Wasmtime API" at `:31` | `wasmi-1.1.0/README.md` |
| wasmi **panics** on tags | `module/import.rs:70-72`, `module/export.rs:97-99` — both verbatim |
| wasmtime 48 `T: 'static` on `define`/`func_insert`/`func_new`/`instantiate` | `wasmtime-48.0.0/src/runtime/linker.rs:377,387,416,1096` — all four confirmed |
| wasmtime 48 `ExternType` has 5 variants incl. `Tag(TagType)` | `wasmtime-48.0.0/src/runtime/types.rs:1445-1456` |
| `getset` is **not** a dependency of `den-stdlib-wasm` though `module.rs:5` imports it | `den-stdlib-wasm/Cargo.toml` `[dependencies]`; `Cargo.lock:1147-1163` |
| `wasmi_wasi` 1.1.0 API: `pub use wasi_common::{Error, WasiCtx, WasiDir, WasiFile}`, `pub use sync::*`, `add_wasi_snapshot_preview1_to_linker` re-exported as `add_to_linker`, accessor closure `Fn(&mut T) -> &mut U + Send + Sync + Copy + 'static` | `wasmi_wasi-1.1.0/src/lib.rs:5-13`, `src/sync/mod.rs:7-13`, `src/sync/snapshots/preview_1.rs:60-68` |
| `wasmi_wasi` pulls `wasi-common`/`wiggle` 36 with `default-features = false` (so **no wasmtime, no cranelift**), and `wasmi` with `features = ["std"]` only | `wasmi_wasi-1.1.0/Cargo.toml` `[dependencies.*]` |
| `WasiCtxBuilder::{inherit_env, inherit_stdio, build}` at `58/101/124`; `build(&mut self) -> WasiCtx` so the fluent chain in §3.6 does type-check | `wasi-common-36.0.0/src/sync/mod.rs`; `impl WasiSnapshotPreview1 for WasiCtx` at `src/snapshots/preview_1.rs:45` |

### Claims CORRECTED

1. **§1.1 item 2 — `wabt::wat2wasm` was never in the tree.** `den-stdlib-wasm/src/lib.rs:103` reads
   `wat::parse_str(source)`; `wat = "1.257.1"` is a declared non-optional dep (`Cargo.toml:22`,
   `Cargo.lock:1162`); `grep -rn wabt` over the repo matches only `docs/research/*.md`. The item was
   retracted in place and the false "pre-existing `E0433`" removed. (Docs 01, 05 and 07 repeat the same
   stale claim and should be corrected too — out of scope for this file.)
2. **§2.3 — `Linker::func_new`'s error type is `LinkerError`, not `wasmi::Error`.** `linker.rs:286-296`
   returns `Result<&mut Self, LinkerError>`; only the *closure* returns `Result<(), wasmi::Error>`. Row
   rewritten to name both. Same fix applied to the `func_wrap` row (`linker.rs:320`). Consequence noted:
   den's `instance.rs:67-171` unifies `func_new`'s and `define`'s results into one `Option<Result<…>>`,
   which still type-checks on wasmi precisely *because* both are `LinkerError`.

### Gaps FILLED (present in den, absent from the previous revision)

3. **§2.10.1 (new) — `wasmi::Val` is not `Copy`; `wasmtime::Val` is.**
   `wasmi-1.1.0/src/value.rs:64` `#[derive(Clone, Debug)]` vs `wasmtime-48.0.0/src/runtime/values.rs:22`
   `#[derive(Debug, Clone, Copy)]`. Compile-verified against wasmi 1.1.0:
   `fn f(v: wasmi::Val) -> (wasmi::Val, wasmi::Val) { (v, v) }` → `E0382: use of moved value: v`.
   Breaks `utils.rs:5` (`E0204`, the `Copy` derive on `WasmValueConverter`) plus `instance.rs:78`, `:104`
   and `:271`. All four sites listed with fixes. §3.12 and §7.4 updated.
4. **§3.14.1 (new) — `wasmi::ValType` has no `is_i64()`/`is_v128()`.** Compile-verified: both calls fail
   with `E0599: no method named … found for enum ValType`. wasmi has only `is_num()`/`is_ref()`
   (`wasmi_core/src/value.rs:30,38`). den calls them at `instance.rs:118`, `:123`, `:127`. Also recorded
   the *reverse* asymmetry, which the previous revision did not have and which defeats the obvious
   workaround: **wasmtime 48's `ValType` derives only `Clone, Hash` (`types.rs:87`) — no `Copy`, no
   `PartialEq`** (only an associated `ValType::eq(a,b)` at `:320`). So `t == ValType::I64` compiles on
   wasmi only and `t.is_i64()` on wasmtime only; two one-line shim fns (`global_is_i64`, `global_is_v128`)
   added to the §7.3 contract and to the §7.4 sketch.
5. **§3.14.2 (new) — the `&ExternType` ripple through `instance.rs:48,116-166`.** Matching a reference
   binds `&GlobalType`/`&TableType`/`&MemoryType`/`&FuncType`. Recorded which are `Copy` (`GlobalType`
   `wasmi_core/src/global.rs:48`, `TableType` `table/ty.rs:7`, `MemoryType` `memory/ty.rs:7`) and which is
   not (`FuncType`, `func/ty.rs:22` — `Clone` only). Named the two concrete deref sites:
   `instance.rs:132` (`Global::from_type(*ty, …)`) and `instance.rs:157-158`
   (`actual_ty != *ty`; `MemoryType: PartialEq` confirmed).
6. **§3.14.3 (new) — the second `Global::new` call site at `instance.rs:299-314`**, which §3.11 missed.
   Needs `Global::new(store, Val::default(ty.content()), ty.mutability())` and loses its `Result`.
7. **§3.14.4 (new) — `Instance::get_func/get_global/get_table/get_memory` take a shared context** on
   wasmi (`instance/mod.rs:246,287,299,311`), so `instance.rs:243,300,319,335` keep compiling via
   `impl<T> AsContext for &'_ mut T` (`store/context.rs:139-147`), but the whole export loop reads better
   with `&*store`.
8. **§3.14.5 (new)** — explicit note that `lib.rs:102-111`'s `wat::parse_str` is backend-independent, so
   nobody "fixes" it into `wasmi::Module::new`'s WAT path.
9. **§1.1 item 5 (new) — the feature-unification bug that breaks the whole plan.**
   `den-core/Cargo.toml:42` declares `den-stdlib-wasm` **without `default-features = false`**, and
   `den-stdlib-wasm/Cargo.toml:32` has `default = ["wasmtime"]`. So `--features wasm-wasmi` resolves to
   *both* backends: cranelift is still pulled, and the `compile_error!` guard proposed in §7.2 would fire
   on the very command it is meant to protect. Fix (`default-features = false` on the dep +
   `wasm = ["wasm-wasmtime"]`) recorded inline. Root `Cargo.toml` verified clean (`:82`, `:96`, `:115-117`).
10. **§7.4 tail + §7.3** — added `global_is_i64`/`global_is_v128`/`global_content` to the shim contract and
    both sketches, plus an explicit warning that `backend::Val` must be treated as `Clone`-only and
    `backend::ValType` as neither `Copy` nor `PartialEq` in code above the shim.

### Not verified (flagged, not claimed)

* `wasmi_wasi` 1.1.0's crates.io metadata (`max_stable_version`, the `2.0.0-beta.10` line) — the
  `.crate` tarball was inspected, the registry index was not re-queried this session. The *API* claims in
  §4 were all read out of the extracted source.
* The `wasi-common` 36 / `wiggle` 36 **feature tables** quoted in §4 came from the crates.io API in the
  original session. The `default-features = false` half of the argument — the part the conclusion rests
  on — was re-read directly from `wasmi_wasi-1.1.0/Cargo.toml` and holds.
* Appendix A's probe crate was not rebuilt end-to-end; only the two new negative checks in §2.10.1 and
  §3.14.1 were compiled (against the same `probe/` crate, wasmi 1.1.0, default features).
