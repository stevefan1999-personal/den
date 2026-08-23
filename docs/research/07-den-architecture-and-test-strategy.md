# 07 — den architecture map & test strategy

Status: research note for the rquickjs 0.8→0.12 / wasmtime 27→48 / swc→oxc migration.
Every claim below was read out of the working tree or out of the vendored crate source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. File:line references are to the
**working tree as it stands now** (dirty: dependency bumps applied to every `Cargo.toml`,
`den-transpiler-swc` renamed to `den-transpiler-oxc`, `scratchpad.rs` + `transpile.rs` staged for
deletion — see `git status`).

**Read this first, before touching any crate.** The single most important fact: *the workspace
does not compile right now*. The `Cargo.toml`s were bumped, the sources were not. Section 6 is
the inventory of what is broken and what is simply dead.

**And the second most important fact: this is not a three-dependency migration.** The title says
rquickjs / wasmtime / oxc; the actual `git diff` on the manifests bumps ~30 crates, three of which
(`derive_more`, `rand`, plus the `edition = "2024"` switch) break den's sources on their own.
Section 0 is the full inventory. Section 6.4–6.7 is the verified, per-file compile-error list.

---

## 0. The actual dependency diff (`git diff -- '**/Cargo.toml'`)

Everything below is a *breaking* change den's sources actually hit. Bumps that turned out to be
source-compatible (`tokio`, `tokio-util`, `tracing`, `thiserror`, `serde`, `serde_json`, `either`,
`indexmap`, `futures`, `url`, `uuid`, `cfg-if`, `mime`, `clap`, `color-eyre`, `console-subscriber`,
`mimalloc`, `pcre2`, `colored 2→3`, `matchit 0.8→0.9`, `relative-path 1→2`, `typed-builder
0.20→0.23`, `rustyline 15→18`, `reqwest 0.12→0.13`, `rusqlite 0.32→0.39`, `base64 0.22→0.23`,
`fmmap 0.3→0.5` incl. its `tokio-async`→`tokio` feature rename, `getset 0.1.3→0.1.7`,
`delegate-attr 0.3.0→0.3.1`, `vc-ltl`, `tracing-subscriber`) are listed only so you know they were
checked — `cargo check` is clean on every crate that does not appear in §6.4–§6.7.

| Change | Where declared | Breaks |
|---|---|---|
| `edition = "2021"` → **`"2024"`**, new `rust-version = "1.97"` | root `Cargo.toml` `[workspace.package]`, inherited by all 16 members | §6.5 |
| `derive_more 1.0.0` → **`2.1.1`** | `[workspace.dependencies]` | §6.6 — 5 call sites |
| `rand 0.8.5` → **`0.10.2`** | `den-stdlib-crypto/Cargo.toml` | §6.7 |
| `rquickjs 0.8.1` → **`0.12.2`** | `[workspace.dependencies]` | §3.2 (Loader/Resolver), §6.4 (`NotAJsClassField`), §6.8 (`async_with!` deprecated) |
| `wasmtime`/`wasmtime-wasi 27.0.0` → **`48.0.0`** | `den-stdlib-wasm/Cargo.toml` | §6.3 item 6, §6.4 |
| `wabt 0.10.0` **removed**, `wat 1.257.1` added | `den-stdlib-wasm/Cargo.toml` | §6.3 item 4 |
| `getset` **removed** from `den-stdlib-wasm` (kept in two other crates) | `den-stdlib-wasm/Cargo.toml` | §6.3 item 5 |
| `wasmi 0.40` → **`1.1.0`** (still optional, still unused) | `den-stdlib-wasm/Cargo.toml` | §6.9 — the `wasmi` feature has never compiled |
| all `swc_*` + `sourcemap` **removed**, `oxc_* 0.146.0` + `oxc_sourcemap 8.1.2` + `trie-match` added | `den-transpiler-oxc/Cargo.toml` | §6.3 item 1, §3.3 |
| `anyhow` removed from `den-transpiler-swc`, still declared (unused) in `den-stdlib-wasm` | | §6.2 |
| `phf`, `log` removed from `[workspace.dependencies]` | root | nothing references them |

`den-stdlib-wasm` also gained `[dev-dependencies]` (`rquickjs` with `macro`/`futures`/`array-buffer`,
`tokio` with `macros`/`rt-multi-thread`) — the §5.2 harness needs no further manifest edits.

---

## 1. Crate-by-crate map

### 1.1 Dependency graph (intra-workspace edges only)

```
den (bin)                       src/{main,app,repl}.rs
 ├── den-core ───────────────────────────────────────────────┐
 └── den-utils (declared, UNUSED — no `den_utils` in src/)   │
                                                             │
den-core                                                     │
 ├── den-stdlib-console      (opt, feature stdlib-console)   │
 ├── den-stdlib-core         (opt, feature stdlib-core)      │
 ├── den-stdlib-crypto       (opt, feature stdlib-crypto)    │
 ├── den-stdlib-fs           (opt, feature stdlib-fs)        │
 ├── den-stdlib-networking   (opt, feature stdlib-networking)│──> den-stdlib-io
 ├── den-stdlib-sqlite       (opt, feature stdlib-sqlite)    │
 ├── den-stdlib-text         (opt, feature stdlib-text)      │
 ├── den-stdlib-timer        (opt, feature stdlib-timer)     │──> den-stdlib-core, den-utils(UNUSED)
 ├── den-stdlib-wasm         (opt, feature wasm)             │──> den-stdlib-core (UNUSED)
 ├── den-stdlib-whatwg-fetch (opt, feature stdlib-whatwg-fetch)──> den-utils (used: SerdeJsonValue)
 ├── den-transpiler-oxc      (opt, feature transpile)        │
 └── den-utils               (declared, UNUSED)              │
                                                             │
den-stdlib-regex   ← NOTHING depends on it (workspace member only)
```

Verified by `grep -oE '^den-[a-z-]+' */Cargo.toml` and `grep -rn 'den_stdlib_\|den_utils' --include='*.rs'`.

**Direction is strictly one-way: `den-core` → `den-stdlib-*`.** No stdlib crate depends on
`den-core`. This is the constraint that shapes the whole test strategy (§5).

### 1.2 What each crate owns

| Crate | Owns | JS-visible surface | Key files |
|---|---|---|---|
| `den` (root bin) | CLI (`clap`), REPL wiring, ctrl-c, tracing/color-eyre init, mimalloc | — | `src/main.rs:24`, `src/app.rs:23`, `src/repl.rs:14` |
| `den-core` | The `Engine`: `AsyncRuntime` + `AsyncContext`, module resolver/loader stack, stdlib registration, transpile-on-eval, `CancellationToken` shutdown | — | `den-core/src/engine.rs:36` (`Engine::new`), `:309` (`run_file`), `:344` (`eval`) |
| `den-stdlib-console` | `console` global backed by `tracing`, node-ish value formatter | `console.{debug,log,warn,error}` | `den-stdlib-console/src/lib.rs:296` (module), `:49` (`Formatter::format`) |
| `den-stdlib-core` | `atob`/`btoa`/`gc`, `CancellationToken` class | `atob`, `btoa`, `gc`, `CancellationToken` | `den-stdlib-core/src/lib.rs:45`, `cancellation.rs:9` |
| `den-stdlib-crypto` | `crypto.getRandomValues`, `crypto.randomUUID` | `crypto.*` global + module exports | `den-stdlib-crypto/src/lib.rs:42` |
| `den-stdlib-fs` | async fs ops over `tokio::fs` (5 of 17 module fns are `not implemented`: `metadata`, `readDir`, `readLink`, `setPermissions`, `symlinkMetadata` — `lib.rs:59,67,72,107,112`) | module `den:fs` only, no globals | `den-stdlib-fs/src/lib.rs:1` |
| `den-stdlib-io` | `AsyncReadWrapper` / `AsyncWriteWrapper` — thin `Arc<RwLock<dyn AsyncRead/Write>>` adapters that convert to/from `TypedArray`/`String`. **Pure Rust glue, no `#[rquickjs::module]`, no classes.** Its only consumer is `den-stdlib-networking` (`socket.rs:3`) | none directly | `den-stdlib-io/src/lib.rs:11`, `:37` |
| `den-stdlib-networking` | `TcpStream` / `TcpListener` / `SocketAddr` / `IpAddr` classes | module `den:networking` only | `den-stdlib-networking/src/socket.rs:17`, `:71` |
| `den-stdlib-regex` | **nothing** — `src/lib.rs` is a single newline (`od -c` → `\n`). Pulls `pcre2` + `colored` for no reason | none | `den-stdlib-regex/src/lib.rs:1` |
| `den-stdlib-sqlite` | `Connection` class over `rusqlite`, JS↔SQL value conversion | module `den:sqlite` only | `den-stdlib-sqlite/src/lib.rs:16`, `:251` |
| `den-stdlib-text` | `TextEncoder`/`TextDecoder` over `encoding_rs` | globals + module | `den-stdlib-text/src/lib.rs:13`, `:110`, `:150` |
| `den-stdlib-timer` | `setTimeout`/`setInterval`/`clear*` returning a `CancellationToken` | globals + module | `den-stdlib-timer/src/lib.rs:1` |
| `den-stdlib-wasm` | The WebAssembly JS API: `Engine`, `Store`, `Module`, `Instance`, `Memory`, `Table`, `Global`, `Tag`, error classes, JS↔`wasmtime::Val` conversion | `WebAssembly` global + module `den:wasm` | see §1.3 |
| `den-stdlib-whatwg-fetch` | `fetch()` + `Response` over `reqwest` | `fetch`, `Response` globals + module | `den-stdlib-whatwg-fetch/src/lib.rs:148`, `:159` |
| `den-transpiler-oxc` | TS/JSX → JS transpile + syntax inference. **Currently still 100% swc code inside an oxc-named crate** | — | `den-transpiler-oxc/src/lib.rs:24` (`EasySwcTranspiler`), `:169`, `:204` |
| `den-utils` | `SerdeJsonValue` (`serde_json::Value` ↔ JS), feature-gated | — | `den-utils/src/serde_json.rs:6` |

### 1.3 `den-stdlib-wasm` public surface (the crate under test)

```
den-stdlib-wasm/src/
  lib.rs        pub mod wasm  → #[rquickjs::module]  ⇒ generates `pub struct js_wasm`
                ResultObject{module,instance}, instantiate(), validate(), compile(), wat2wasm()
                #[qjs(evaluate)] installs Store+Engine userdata and the `WebAssembly` global
  engine.rs     Engine  — newtype over wasmtime::Engine, stored as ctx userdata
  store.rs      Store<'js> — Arc<RefCell<wasmtime::Store<(WasiP1Ctx, Ctx<'js>)>>>, ctx userdata
                ⚠ illegal under wasmtime 48 (`Store<T: 'static>`) — see §6.4a
  module.rs     Module  — newtype over wasmtime::Module + static imports/exports/customSections
  instance.rs   Instance — linker construction, import resolution, `exports` getter
  memory.rs     Memory + MemoryDescriptor
  table.rs      Table + TableDescriptor
  global.rs     Global + GlobalDescriptor
  tag.rs        Tag — empty placeholder class
  error.rs      Exception, CompileError, LinkError, RuntimeError — all empty placeholder classes
  utils.rs      WasmValueConverter (JS ↔ wasmtime::Val), get_default_value_for_val_type
```

Two *different* JS surfaces come out of this crate, and they are not the same set:

1. **The `WebAssembly` global** (`lib.rs:120-134`), built from an `indexmap!` literal:
   `WebAssembly.{instantiate, validate, compile, wat2wasm}` and
   `WebAssembly.Module.{imports, exports, customSections}` — where `WebAssembly.Module` is a plain
   object holding three static functions, **not a constructor**.
   `WebAssembly.Memory`, `.Table`, `.Global`, `.Instance`, `.CompileError`, `.LinkError`,
   `.RuntimeError`, `.Tag` **do not exist**.
2. **The ES module `den:wasm`**. The `pub use crate::{...}` at `lib.rs:22-30` is not an ordinary
   re-export: `rquickjs-macro-0.12.2/src/module/mod.rs:119-146` (`export_use`) turns every `pub use`
   name inside a `#[rquickjs::module]` into
   `Class::<T>::create_constructor(&ctx)?.expect("…did not define a constructor")` and exports it.
   So `import { Memory, Table, Global, Instance, Module, Tag, CompileError, LinkError,
   RuntimeError, WasmException } from "den:wasm"` **does** give you constructors, and
   `pub fn` items in the module (`instantiate`, `validate`, `compile`, `wat2wasm`) plus the
   `pub struct ResultObject` are exported too (module macro `mod.rs:225-320`).

That asymmetry is itself a bug (spec says the constructors live on `WebAssembly`) and it is the
first thing the new tests should pin down.

---

## 2. Module registration flow

### 2.1 The three-step dance in `Engine::new` (`den-core/src/engine.rs:36-307`)

A `#[rquickjs::module] pub mod foo` expands to a unit struct `js_foo: ModuleDef`
(`rquickjs-macro/src/module/mod.rs:445-460`). Getting that struct into JS takes three independent
registrations, and **each one uses a string name that must match**:

| Step | Code | Purpose |
|---|---|---|
| 1. Resolver | `BuiltinResolver::with_module("den:x")` — `engine.rs:47-89` | makes the specifier `"den:x"` *resolvable*. `BuiltinResolver::resolve` returns `Err(Error::new_resolving(..))` for anything not in its `HashSet` (`rquickjs-core-0.12.2/src/loader/builtin_resolver.rs:35-59`). |
| 2. Loader | `ModuleLoader::with_module("den:x", den_stdlib_x::js_x)` — `engine.rs:122-172` | maps the resolved name to `Module::declare_def::<js_x>` (`rquickjs-core/src/loader/module_loader.rs:22-24`). Note `load()` does `self.modules.remove(path)` — a native module is loadable exactly once per runtime (fine: QuickJS caches the module afterwards). |
| 3. Eager eval | `Module::evaluate_def::<js_x, _>(ctx.clone(), "den:x")` — `engine.rs:237-298` | declares **and evaluates** the module immediately at context creation so its `#[qjs(evaluate)]` body runs, which is what installs the *globals* (`console`, `atob`, `TextEncoder`, `setTimeout`, `fetch`, `crypto`, `WebAssembly`). Returns `(Module<Evaluated>, Promise)` (`rquickjs-core/src/value/module.rs:323`); den discards both. |

Steps 1+2 give you `import … from "den:x"`. Step 3 gives you the global. A module can have one
without the other — and three of them deliberately do:

| Specifier | Resolver+Loader | `evaluate_def` | Effect |
|---|---|---|---|
| `den:core` | yes (`:51`,`:126`) | yes (`:249`) | import + globals |
| `den:console` | yes (`:55`,`:131`) | yes (`:241`) | import + globals |
| `den:text` | yes (`:63`,`:142`) | yes (`:257`) | import + globals |
| `den:timer` | yes (`:67`,`:147`) | yes (`:265`) | import + globals |
| `den:crypto` | yes (`:83`,`:166`) | yes (`:281`) | import + globals |
| `den:wasm` | yes (`:87`,`:170`) | yes (`:289`) | import + globals |
| `den:networking` | yes (`:59`,`:136`) | **no** | import-only (by design) |
| `den:fs` | yes (`:71`,`:152`) | **no** | import-only (by design) |
| `den:sqlite` | yes (`:75`,`:157`) | **no** | import-only (by design) |
| `den:whatcg-fetch` / `den:whatwg-fetch` | yes, misspelled | yes, correctly spelled | **BUG, see §2.2** |

Side note that trips people up: the `rename = "…"` argument on `#[rquickjs::module]`
(e.g. `den-stdlib-fs/src/lib.rs:2` uses `rename = "den:fs"`, everyone else uses
`rename = "camelCase"`) is a **no-op**. `ModuleConfig::rename` is written at
`rquickjs-macro/src/module/config.rs:38` and never read anywhere in `module/mod.rs`. Only
`rename_vars` and `rename_types` do anything. The JS specifier is *only* the string passed in
`engine.rs`.

### 2.2 BUG — `den:whatcg-fetch` vs `den:whatwg-fetch` (confirmed)

```rust
// den-core/src/engine.rs:77-80  — RESOLVER
#[cfg(feature = "stdlib-whatwg-fetch")]
{
    resolver = resolver.with_module("den:whatcg-fetch");   // ← "whatcg"
}

// den-core/src/engine.rs:159-163 — LOADER
#[cfg(feature = "stdlib-whatwg-fetch")]
{
    loader = loader
        .with_module("den:whatcg-fetch", den_stdlib_whatwg_fetch::js_whatwg);   // ← "whatcg"
}

// den-core/src/engine.rs:271-277 — EAGER EVALUATION
#[cfg(feature = "stdlib-whatwg-fetch")]
{
    let _ = Module::evaluate_def::<den_stdlib_whatwg_fetch::js_whatwg, _>(
        ctx.clone(),
        "den:whatwg-fetch",   // ← "whatwg"
    )?;
}
```

**Why it is a real bug and not cosmetic.** QuickJS resolves an import by calling the *normalize*
(resolver) hook **first**, and only then looks in the already-loaded module table:

```c
/* rquickjs-sys-0.12.2/quickjs/quickjs.c:30001-30040, js_host_resolve_imported_module */
cname = rt->normalize_u.module_normalize_func2(ctx, base_cname, cname1, attributes, opaque);
if (!cname) return NULL;                       /* ← resolver error aborts the import */
...
/* first look at the loaded modules */
m = js_find_loaded_module(ctx, module_name);
```

So:

* `import { fetch } from "den:whatwg-fetch"` — the correctly spelled specifier, and the name under
  which the module actually exists in the context thanks to `evaluate_def` — **throws**, because
  `BuiltinResolver` rejects it before `js_find_loaded_module` is ever reached (`HttpResolver` then
  rejects the non-http scheme, `FileResolver` finds no file).
* `import { fetch } from "den:whatcg-fetch"` — the typo — **works**, but declares and evaluates a
  *second, independent* `js_whatwg` module instance, re-running `#[qjs(evaluate)]`
  (`den-stdlib-whatwg-fetch/src/lib.rs:175-181`) and re-setting the `fetch` / `Response` globals.

Fix: one-character change at `engine.rs:79` and `engine.rs:162` (`whatcg` → `whatwg`). Add the
regression test `import_den_whatwg_fetch_specifier_resolves` from §5.6.

### 2.3 What `den:wasm`'s `evaluate` actually does

```rust
// den-stdlib-wasm/src/lib.rs:114-137
#[qjs(evaluate)]
pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
    let engine = crate::engine::Engine::new();
    let store  = crate::store::Store::new(&engine, ctx.clone());
    ctx.store_userdata(store)?;      // Store<'js> holds a clone of Ctx<'js> (!)
    ctx.store_userdata(engine)?;
    ctx.globals().set("WebAssembly", indexmap!{ … })?;
    Ok(())
}
```

Consequences to keep in mind while writing tests:

* the `wasmtime::Engine` and the **single** `wasmtime::Store` are per-`AsyncContext` userdata, not
  per-`Instance`. Every `Memory`/`Table`/`Global`/`Instance` in a context shares one store.
* `ctx.userdata::<Store>()` / `::<Engine>()` is `.unwrap()`ed in **ten** places
  (`lib.rs:82`, `lib.rs:92`, `module.rs:25`, `memory.rs:62`, `memory.rs:73`, `memory.rs:100`,
  `table.rs:88`, `global.rs:45`, `instance.rs:217`, `instance.rs:231`). If the module def was never
  evaluated in that context, these panic → the whole test binary aborts instead of failing one test.
* `Store<'js>` stores `Ctx<'js>` inside userdata *of that same context* — a reference cycle. Per-test
  runtimes make it moot; a long-lived embedder should care.

---

## 3. Loader / Resolver stack and every transpiler call site

### 3.1 The stack, in resolution order

`runtime.set_loader(resolver_tuple, loader_tuple)` (`engine.rs:223`). rquickjs tries tuple members
left-to-right.

**Resolvers** (`engine.rs:44-117`):
1. `BuiltinResolver` — the `den:*` specifiers (§2.1).
2. `HttpResolver` (`den-core/src/resolver/http.rs:11`) — joins base+path as `Url`, applies optional
   `matchit::Router` allow/deny lists, accepts only `http`/`https` schemes. The allow/deny lists are
   `pub(crate)` with **no constructor to set them** — dead flexibility today.
3. `FileResolver` with patterns `{}.js`, `{}.mjs`, `+{}.jsx`, `+{}.mjsx` (feature `react`),
   `+{}.ts`, `+{}.tsx` (feature `typescript`).

**Loaders** (`engine.rs:118-222`):
1. `BuiltinLoader` (empty).
2. `ModuleLoader` — the native `den:*` modules.
3. `HttpLoader` (`den-core/src/loader/http.rs:24`) — `reqwest::get`, MIME sniffing
   (`text|application` × `javascript|typescript`), transpile, `Module::declare`.
4. `MmapScriptLoader` (`den-core/src/loader/mmap_script.rs:39`) — `fmmap::AsyncMmapFile`,
   extension allow-list, transpile, `Module::declare`.

Both custom loaders bridge async→sync with
`tokio::task::block_in_place(|| Handle::current().block_on(task))`
(`http.rs:107`, `mmap_script.rs:79`). **That means any test that imports a file or URL must run on
`#[tokio::test(flavor = "multi_thread")]`** — `block_in_place` panics on the current-thread runtime.

### 3.2 rquickjs 0.12 breaks both traits (independent of the oxc swap)

`rquickjs-core-0.12.2/src/loader.rs:64-101`:

```rust
fn resolve<'js>(&mut self, ctx: &Ctx<'js>, base: &str, name: &str,
                attributes: Option<ImportAttributes<'js>>) -> Result<String>;

fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str,
             attributes: Option<ImportAttributes<'js>>) -> Result<Module<'js, Declared>>;
```

den's three impls still use the 0.8 three-argument signatures:
`resolver/http.rs:12`, `loader/http.rs:25`, `loader/mmap_script.rs:40`. All three need the extra
`_attributes: Option<ImportAttributes<'js>>` parameter. (`ImportAttributes` also unlocks
`import x from "y" with { type: "json" }` — out of scope, just note the door is open.)

### 3.3 Every swc→oxc call site (exhaustive; `grep -rn 'transpil\|Syntax\|IsModule\|SourceMap' src den-core`)

The transpiler is consumed through **five** symbols:
`EasySwcTranspiler`, `EasySwcTranspilerError`, `Syntax`, `IsModule`, `SourceMap`, plus the two free
functions `infer_transpile_syntax_by_extension` and `get_best_transpiling`, plus the (dead) error
type `InferTranspileSyntaxError`.

| # | Site | Current code | What must change |
|---|---|---|---|
| 1 | `den-core/src/engine.rs:11-18` | `use den_transpiler_swc::{get_best_transpiling, infer_transpile_syntax_by_extension, EasySwcTranspiler, EasySwcTranspilerError, IsModule, SourceMap, Syntax}` | crate path `den_transpiler_oxc`, and whichever names survive the oxc rewrite |
| 2 | `den-core/src/engine.rs:27-28` | `pub transpiler: Arc<EasySwcTranspiler>` | new type name; **oxc's `Allocator` is not `Sync`** — if the oxc transpiler holds an arena it cannot live in a `Clone + Send` `Engine`; allocate per-call instead (see §7 Q1) |
| 3 | `den-core/src/engine.rs:37-38` | `let transpiler = Arc::new(EasySwcTranspiler::default());` | ditto |
| 4 | `den-core/src/engine.rs:174-199` | `builder.transpiler(transpiler.clone())` for `HttpLoader` and `MmapScriptLoader` | field type of both loaders |
| 5 | `den-core/src/engine.rs:334-342` | `pub fn transpile(&self, src, syntax: Syntax, module: IsModule) -> Result<(String, Option<SourceMap>), EasySwcTranspilerError>` | public API of `Engine` — pick the oxc equivalents (`SourceType` for `Syntax`; oxc has no `IsModule::Unknown`, see §7 Q2) |
| 6 | `den-core/src/engine.rs:348-357` | `infer_transpile_syntax_by_extension(get_best_transpiling()).unwrap_or_default()` then `transpile(src, syntax, IsModule::Unknown)` inside `Engine::eval` | `IsModule::Unknown` (swc: "sniff module vs script") has no oxc analogue |
| 7 | `den-core/src/engine.rs:380-390` | `EngineError::{EasySwcTranspiler, InferTranspileSyntaxError}` | rename; **`InferTranspileSyntaxError` is never constructed** (`infer_transpile_syntax_by_extension` returns `Option`) → delete the variant |
| 8 | `den-core/src/loader/http.rs:7-11, 21, 79-94` | import block, `transpiler: Arc<EasySwcTranspiler>` field, `.transpile(&body, infer_…(extension).unwrap_or_default(), IsModule::Bool(true), false)` | same three edits |
| 9 | `den-core/src/loader/mmap_script.rs:7-11, 21, 57-71` | identical shape, but the input is `std::str::from_utf8(src.as_slice())?` from an mmap | same three edits |

Nothing else in the workspace mentions the transpiler. The root binary never touches it.

Shape of the change at the two loaders (identical in both files):

```rust
// BEFORE — den-core/src/loader/mmap_script.rs:57-71
#[cfg(feature = "transpile")]
{
    let (src, _) = self
        .transpiler
        .transpile(
            std::str::from_utf8(src.as_slice())?,
            infer_transpile_syntax_by_extension(extension).unwrap_or_default(),
            IsModule::Bool(true),
            false,
        )
        .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;
    let module = Module::declare(ctx.clone(), path, src)?;
    Ok(module)
}

// AFTER (shape only — exact oxc types come from doc 0x-transpiler)
#[cfg(feature = "transpile")]
{
    let src = self
        .transpiler
        .transpile(std::str::from_utf8(src.as_slice())?, source_type_for(extension))
        .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;
    Ok(Module::declare(ctx.clone(), path, src.code)?)
}
```

Keep the seam narrow: if `den-transpiler-oxc` exposes the *same* three items
(`Transpiler::transpile(&self, src, syntax) -> Result<Output, Error>`,
`infer_transpile_syntax_by_extension`, `get_best_transpiling`), sites 8 and 9 shrink to a rename
and sites 5–7 to a signature tweak.

---

## 4. Current test reality

* Test directories: **zero** (`find -maxdepth 2 -name tests` → nothing).
* Tests in-tree: **two**, both in `den-core/src/engine.rs:392-425` — `my_test` (console.log +
  `null ?? "123"` + `null ?? 123`) and `my_test2` (`export const hello = "world"`). Both
  `#[tokio::test(flavor = "multi_thread")]`, both named after nothing.
* CI (`.github/workflows/lint.yml`) runs `cargo +nightly clippy --no-deps`, `cargo +nightly fmt
  --check` and `cargo +nightly doc --no-deps` on `[self-hosted]` runners, triggered by
  `workflow_dispatch` + `pull_request` filtered to `paths: ["**/*.rs", ".github/workflows/lint.yml"]`.
  **There is no `cargo test` job**, and the path filter means a manifest-only commit runs nothing
  at all (§6.9d). Adding both is part of this work.
* `README.md:90` — "MAKE SOME UNIT TESTS AND INTEGRATION TESTS" is the first unchecked TODO.
* Dev-deps are already in place: `den-core` has `tokio{rt,rt-multi-thread,macros}` + `color-eyre`;
  `den-stdlib-wasm` has `tokio{macros,rt-multi-thread}` + `rquickjs{macro,futures,array-buffer}`
  (`den-stdlib-wasm/Cargo.toml:27-29`). `wat = "1.257.1"` is a **normal** dependency, so it is
  available to tests without adding anything.

---

## 5. Test strategy for the WebAssembly JS API

### 5.1 Why `den-stdlib-wasm` needs its own harness

`den-core::Engine::eval::<T>` is the natural "eval a snippet, assert on the result" harness, but
`den-stdlib-wasm` **must not** depend on `den-core`: `den-core/Cargo.toml:42` already declares
`den-stdlib-wasm`, so the reverse edge is a cycle and cargo will refuse it. Confirmed in §1.1.

So there are two harnesses, one per layer:

* **`den-stdlib-wasm`** — build a bare `AsyncRuntime`/`AsyncContext`, evaluate `js_wasm` by hand,
  eval JS. No resolver, no loader, no transpiler, no stdlib. Fast (a fresh `wasmtime::Engine` per
  context is a few ms) and hermetic.
* **`den-core`** — the real `Engine`, exercising registration + `WebAssembly` reachability from
  transpiled TS, i.e. the wiring, not the semantics.

### 5.2 The harness (`den-stdlib-wasm/tests/common/mod.rs`)

Put it in `tests/common/mod.rs`, not behind `#[cfg(test)]` in `src/`: integration tests cannot see
`#[cfg(test)]` items, and exposing a `testing` feature just to share a 30-line helper is not worth
it. In-crate `#[cfg(test)]` modules stay for pure-Rust unit tests (§5.4).

```rust
// den-stdlib-wasm/tests/common/mod.rs
use rquickjs::{
    async_with, context::EvalOptions, AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module,
    Object, Promise, TypedArray,
};

/// Compile WebAssembly Text at test time — no .wasm binaries checked into the repo.
/// `wat` is already a normal dependency of the crate (Cargo.toml:22).
pub fn wat(source: &str) -> Vec<u8> {
    wat::parse_str(source).expect("test fixture is not valid WAT")
}

/// One `AsyncRuntime` + `AsyncContext` per call.
///
/// Isolation is per-context because `den:wasm` keeps exactly one `wasmtime::Store` in the
/// context userdata (den-stdlib-wasm/src/lib.rs:116-119), so tests must not share a context.
///
/// `bytes`, if given, is exposed to the snippet as a `Uint8Array` named `WASM`.
/// The snippet may use top-level `await`; its completion value is returned as `T`.
/// Any JS exception (or Rust error) comes back as `Err(String)` so the caller can assert on it.
pub async fn eval<T>(bytes: Option<Vec<u8>>, source: &str) -> Result<T, String>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    let runtime = AsyncRuntime::new().expect("runtime");
    let context = AsyncContext::full(&runtime).await.expect("context");

    async_with!(context => |ctx| {
        let run = async {
            // Installs Store + Engine userdata and the `WebAssembly` global.
            let _ = Module::evaluate_def::<den_stdlib_wasm::js_wasm, _>(ctx.clone(), "den:wasm")?;

            if let Some(bytes) = bytes {
                ctx.globals().set("WASM", TypedArray::new(ctx.clone(), bytes)?)?;
            }

            // `promise: true` == JS_EVAL_FLAG_ASYNC: the result is a promise of `{ value: … }`,
            // which is how den-core/src/engine.rs:359-366 does it too.
            let mut options = EvalOptions::default();   // EvalOptions is #[non_exhaustive]:
            options.global = true;                      // assign fields, no struct literal
            options.promise = true;
            options.strict = true;

            ctx.eval_with_options::<Promise, _>(source, options)?
                .into_future::<Object>()
                .await?
                .get::<_, T>("value")
        };
        run.await.catch(&ctx).map_err(|err| err.to_string())
    })
    .await
}
```

Notes that make this work (all verified against rquickjs 0.12.2 source):

* `WithFuture::poll` (`src/context/async/future.rs:66-158`) drains the scheduler **and** runs
  pending QuickJS jobs while holding the runtime lock, so `async_with!` alone resolves promises —
  no separate `runtime.drive()` task is needed for these tests.
* `EvalOptions` is `#[non_exhaustive]` (`src/context/ctx.rs:28`) with `Default` at `:67`; you must
  mutate fields.
* `CaughtError` implements `Display` (`src/result.rs:541`), hence `err.to_string()`; it is not
  `Send`, which is why the conversion happens inside the closure.
* `TypedArray::new(ctx, impl Into<Vec<T>>) -> Result<Self>` (`src/value/typed_array.rs:128`) needs
  the `array-buffer` feature — already enabled in both `[dependencies]` and `[dev-dependencies]`.
* `#[tokio::test]` (current-thread) is fine here; only `den-core` tests that hit the file/HTTP
  loaders need `flavor = "multi_thread"` (§3.1).
* **`async_with!` is `#[deprecated]` in 0.12** (§6.8b). Write the harness as
  `context.async_with(async |ctx| { … }).await` instead — same semantics, no warning:

  ```rust
  context.async_with(async |ctx| {
      let run = async { /* … as above … */ };
      run.await.catch(&ctx).map_err(|err| err.to_string())
  }).await
  ```

Usage:

```rust
mod common;
use common::{eval, wat};

const ADD: &str = r#"
    (module
      (func (export "add") (param i32 i32) (result i32)
        local.get 0 local.get 1 i32.add))
"#;

#[tokio::test]
async fn exported_function_add_returns_sum_of_two_i32_arguments() {
    let sum: i32 = eval(Some(wat(ADD)), r#"
        const { instance } = await WebAssembly.instantiate(WASM);
        instance.exports.add(20, 22)
    "#).await.unwrap();
    assert_eq!(sum, 42);
}
```

### 5.3 Test fixtures without binaries

Never check in `.wasm`. Three tiers, in order of laziness:

1. `wat::parse_str` in Rust (the `wat()` helper above) — default choice, gives `Vec<u8>` to hand to
   JS as `WASM`.
2. `WebAssembly.wat2wasm("(module …)")` from inside the snippet — den already exposes it
   (`lib.rs:101-112`), so a test can be pure JS. **It is broken today** (calls `wabt::wat2wasm`;
   `wabt` is not a dependency, `wat` is) — fixing it to `wat::parse_str(source)` is a two-line
   change and then this tier becomes the most readable one.
3. Hand-written byte arrays only for *deliberately invalid* input:
   `new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0])` (empty but valid module),
   `new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9])` (bad version → `CompileError`),
   `new Uint8Array([1, 2, 3])` (not wasm at all).

### 5.4 File layout

```
den-stdlib-wasm/
├── src/
│   ├── utils.rs           + #[cfg(test)] mod tests   ← pure Rust: WasmValueConverter,
│   │                                                    get_default_value_for_val_type
│   └── module.rs          + #[cfg(test)] mod tests   ← pure Rust: extern_type_to_str
└── tests/
    ├── common/mod.rs      ← the harness above
    ├── namespace.rs       ← WebAssembly global shape, validate, compile, wat2wasm
    ├── module.rs          ← Module ctor + imports/exports/customSections statics
    ├── instance.rs        ← instantiate, imports resolution, exports, calling, coercion
    ├── memory_table_global.rs
    └── errors.rs          ← CompileError / LinkError / RuntimeError / traps

den-core/
└── tests/
    ├── engine_stdlib_modules.rs   ← every `den:*` specifier resolves; globals present
    ├── engine_wasm.rs             ← WebAssembly reachable through the real Engine
    └── engine_transpile.rs        ← TS/JSX file loading via MmapScriptLoader
        fixtures/{hello.ts,hello.tsx,hello.js}
```

Rule of thumb: **semantics of the JS API → `den-stdlib-wasm/tests/`; wiring → `den-core/tests/`.**
Do not re-test `instance.exports.add(1,2)` at the `den-core` level; one smoke test there is enough.

### 5.5 Test cases — `den-stdlib-wasm`

"Today" = expected result against the code as written (after it is made to compile again).
`FAIL` cases are the spec gaps this suite is meant to document; land them `#[ignore = "…"]`
or as `assert!(result.is_err())` pinning current behaviour, and flip them as the API is completed.

**A. Namespace & module shape — `tests/namespace.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 1 | `evaluating_den_wasm_installs_webassembly_global` | `typeof WebAssembly === "object"` and it has `instantiate`/`validate`/`compile` | PASS |
| 2 | `webassembly_namespace_exposes_module_memory_table_global_constructors` | `typeof WebAssembly.Module === "function" && typeof WebAssembly.Memory === "function"` … | **FAIL** — `WebAssembly.Module` is a plain object of statics, the other three are absent (`lib.rs:120-133`) |
| 3 | `den_wasm_module_exports_memory_table_global_instance_constructors` | `import { Memory, Table, Global, Instance } from "den:wasm"` are functions | PASS (via `export_use`) |
| 4 | `wat2wasm_compiles_wat_text_to_a_valid_module` | `WebAssembly.validate(WebAssembly.wat2wasm("(module)"))` is `true` | **FAIL** — `wabt` is not a dependency (`lib.rs:103`) |
| 5 | `validate_returns_true_for_a_minimal_valid_module` | `WebAssembly.validate(WASM) === true` | PASS |
| 6 | `validate_returns_false_for_non_wasm_bytes` | `WebAssembly.validate(new Uint8Array([1,2,3])) === false` | PASS |
| 7 | `validate_accepts_both_uint8array_and_arraybuffer` | same bytes as `Uint8Array` and as `.buffer` both return `true` | PASS (`Either<TypedArray,ArrayBuffer>`, `lib.rs:77`) |
| 8 | `compile_resolves_to_a_module_instance` | `(await WebAssembly.compile(WASM)) instanceof Module` | PASS |
| 9 | `compile_rejects_with_compile_error_for_invalid_bytes` | rejection is a `CompileError` | partial — it throws `CompileError` (`lib.rs:96`) but the class carries no message and is not an `Error` subclass |
| 10 | `validate_throws_type_error_for_a_non_buffer_argument` | `WebAssembly.validate("nope")` throws | PASS (rquickjs `Either` conversion failure) |

**B. Module statics — `tests/module.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 11 | `module_constructor_compiles_bytes_synchronously` | `new Module(WASM)` (imported from `den:wasm`) does not throw | PASS |
| 12 | `module_constructor_throws_compile_error_on_truncated_binary` | throws, message mentions the wasm error | partial — generic `Exception::throw_internal` (`module.rs:31-33`), not a `CompileError` |
| 13 | `module_imports_lists_module_name_and_kind_for_each_import` | `[{module:"env", name:"log", kind:"function"}]` | PASS (`module.rs:54-65`) |
| 14 | `module_exports_lists_name_and_kind_for_each_export` | `[{name:"add", kind:"function"}, {name:"mem", kind:"memory"}]` | PASS (`module.rs:68-78`) |
| 15 | `module_custom_sections_returns_matching_section_payloads` | `customSections(mod, "name")` → array of `ArrayBuffer` | **FAIL** — `throw_internal("not implemented")` (`module.rs:81-83`) |
| 16 | `module_statics_are_callable_from_the_webassembly_namespace` | `WebAssembly.Module.imports(mod)` works | PASS |

**C. Instantiation & imports — `tests/instance.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 17 | `instantiate_from_bytes_resolves_to_module_and_instance_pair` | result has both `.module` and `.instance` | **BLOCKED** — `ResultObject` (`lib.rs:32-39`) does not compile under rquickjs 0.12: `JsClass` types cannot be class fields, they must be `Class<'js, T>` (§6.8a). PASS once that is fixed |
| 18 | `instantiate_from_module_object_resolves_to_bare_instance` | per spec, passing a `Module` resolves to an `Instance`, not a pair | **FAIL** — always returns `ResultObject` (`lib.rs:52-58`) |
| 19 | `instantiate_without_import_object_throws_when_module_declares_imports` | throws "import object is not an object" | PASS (`instance.rs:199-202`) |
| 20 | `instantiate_with_import_object_missing_the_member_throws_type_error` | `{env:{}}` for an `env.log` import throws `TypeError` | PASS (`instance.rs:41-46`) |
| 21 | `instantiate_with_a_non_function_for_a_function_import_throws_link_error` | `{env:{log: 42}}` throws `LinkError` | PASS (`instance.rs:51`, `:181`) |
| 22 | `instantiate_ignores_extra_unused_members_of_the_import_object` | extra keys do not throw | PASS (loop is driven by `module.imports()`) |
| 23 | `two_instances_of_the_same_module_have_independent_globals` | mutating instance A's exported mutable global does not affect B | risky — one shared `Store`; write it to pin whichever behaviour is real |
| 24 | `instantiate_returns_a_promise` | `WebAssembly.instantiate(WASM) instanceof Promise` | PASS (async `#[rquickjs::function]`) |

**D. Exports & value coercion — `tests/instance.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 25 | `exported_function_add_returns_sum_of_two_i32_arguments` | `add(20,22) === 42` | PASS |
| 26 | `exported_function_length_matches_wasm_parameter_count` | `instance.exports.add.length === 2` | PASS (`instance.rs:284`) |
| 27 | `exported_function_name_matches_the_export_name` | `instance.exports.add.name === "add"` | PASS (`instance.rs:285`) |
| 28 | `exported_function_with_no_results_returns_undefined` | spec: `undefined` | **FAIL** — returns `null` (`instance.rs:270`) |
| 29 | `exported_function_with_multiple_results_returns_an_array` | `[1,2]` | PASS (`instance.rs:272-280`) |
| 30 | `exported_function_returning_i64_returns_a_bigint` | `typeof f() === "bigint"` | **FAIL** — `Val::I64` → `i64::into_js` → i32-or-f64 Number, never BigInt (`utils.rs:13`; `rquickjs-core/src/value/convert/into.rs:483-486`). Lossy past 2^53. |
| 31 | `exported_function_accepts_a_bigint_argument_for_an_i64_parameter` | `f(9007199254740993n)` round-trips | PASS on the way in (`utils.rs:30`), fails on the way out — pair with #30 |
| 32 | `exported_function_coerces_a_js_boolean_argument_to_i32` | `add(true, 1) === 2` | PASS (`utils.rs:27`) |
| 33 | `exported_function_rejects_a_float_argument_for_an_i32_parameter` | `add(1.5, 1)` throws | PASS but for the wrong reason — `Type::Float` → `Val::F64` (`utils.rs:29`), wasmtime then reports a type mismatch; spec says truncate toward zero |
| 34 | `exported_memory_is_exposed_as_a_memory_object` | `instance.exports.mem instanceof Memory` | PASS (`instance.rs:334-347`) |
| 35 | `exported_global_is_exposed_as_a_global_object` | `instance.exports.g instanceof Global` | PASS (`instance.rs:299-317`) |

**E. Host (imported) functions — `tests/instance.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 36 | `imported_host_function_receives_wasm_arguments_as_js_numbers` | host sees `(1, 2)` | PASS (`instance.rs:76-80`) |
| 37 | `imported_host_function_return_value_populates_the_single_result` | wasm gets `7` back | PASS (`instance.rs:107-109`) |
| 38 | `imported_host_function_returning_an_array_fills_multiple_results` | `[1,2]` → two results | PASS (`instance.rs:81-106`) |
| 39 | `imported_host_function_returning_an_array_of_the_wrong_length_throws` | message names expected vs actual | PASS (`instance.rs:83-93`) |
| 40 | `imported_host_function_that_throws_propagates_the_exception_to_the_caller` | JS exception surfaces at the `instance.exports.f()` call site | needs pinning — the trap path goes through `wasmtime` and `Exception::throw_internal` (`instance.rs:263-268`) |
| 41 | `imported_host_function_calling_back_into_an_export_does_not_panic` | re-entrancy | **HAZARD** — `store.borrow_mut()` (`instance.rs:262`) while the outer call already holds the borrow ⇒ `RefCell` panic ⇒ process abort, not a JS error. Write this test; if it aborts, switch to `try_borrow_mut()` + `Exception::throw_internal`. |
| 42 | `imported_host_function_arity_shorter_than_the_wasm_signature_is_accepted` | JS fn with 1 param, wasm passes 2 | PASS (`Args::push_args` + JS ignores extras) |

**F. Memory / Table / Global — `tests/memory_table_global.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 43 | `memory_constructor_allocates_the_requested_initial_pages` | `new Memory({initial: 1})` succeeds | PASS (`memory.rs:49-69`) |
| 44 | `memory_buffer_returns_an_arraybuffer_of_initial_size_bytes` | `mem.buffer.byteLength === 65536` | **FAIL** — `buffer` unconditionally throws `"TODO"` (`memory.rs:72-76`); the `JS_NewArrayBuffer` implementation is commented out at `:77-96` |
| 45 | `memory_grow_returns_the_previous_page_count` | `mem.grow(1) === 1` | **FAIL** — returns `()`/undefined (`memory.rs:99-107`) |
| 46 | `memory_grow_past_maximum_throws_range_error` | `new Memory({initial:1,maximum:1}).grow(1)` throws | partial — throws, but an internal error, not `RangeError` |
| 47 | `memory_buffer_is_detached_after_grow` | old buffer `byteLength === 0` | blocked by #44 |
| 48 | `memory_descriptor_without_initial_throws` | `new Memory({})` throws | PASS-ish — `this.get("initial")?` fails (`memory.rs:25`) |
| 49 | `table_constructor_creates_an_anyfunc_table_with_the_requested_length` | `new Table({element:"anyfunc", initial:2})` succeeds | suspicious — `"anyfunc"` builds a `FUNCREF` table but initialises it with `Ref::Any(None)` (`table.rs:63-68`), which wasmtime should reject as a type mismatch. Test pins it. |
| 50 | `table_constructor_rejects_an_unknown_element_type` | `{element:"i32"}` throws | PASS (`table.rs:69-74`) |
| 51 | `table_length_reports_the_current_size` | `table.length === 2` | **FAIL** — `Table` has no `length` accessor at all |
| 52 | `table_get_returns_null_for_an_uninitialised_slot` | `table.get(0) === null` | **FAIL** — no `get` |
| 53 | `table_set_stores_an_exported_function_and_get_returns_it` | round-trip | **FAIL** — no `set` |
| 54 | `table_grow_increases_length_and_returns_the_previous_length` | | **FAIL** — no `grow` |
| 55 | `global_constructor_creates_an_immutable_i32_global` | `new Global({value:"i32"}, 42)` succeeds | PASS (`global.rs:44-82`) |
| 56 | `global_value_getter_returns_the_current_value` | `g.value === 42` | **FAIL** — no `value` accessor on `Global` |
| 57 | `global_value_setter_updates_a_mutable_global` | `g.value = 7` | **FAIL** — no setter |
| 58 | `global_value_setter_throws_on_an_immutable_global` | | **FAIL** — no setter |
| 59 | `global_constructor_rejects_a_value_of_the_wrong_type` | `new Global({value:"i64"}, 1)` (Number, not BigInt) throws "mismatched type" | PASS (`global.rs:56-64`) |
| 60 | `global_descriptor_rejects_an_unknown_value_type` | `{value:"i128"}` throws | PASS (`global.rs:95-102`); note the accepted list contains `anyref` while the constructor matches on `anyfunc` (`global.rs:97` vs `:55`) — a latent inconsistency, assert it |

**G. Errors & streaming — `tests/errors.rs`**

| # | Test name | Asserts | Today |
|---|---|---|---|
| 61 | `compile_link_and_runtime_errors_are_constructible_from_the_module` | `new CompileError()` etc. from `den:wasm` | PASS-ish — **they are not uniform**: `CompileError::new()` returns `Self` (`error.rs:28-30`, it has to — `lib.rs:96` constructs one in Rust), while `Exception::new()` (`error.rs:10`), `LinkError::new()` (`:40`) and `RuntimeError::new()` (`:50`) return `()`. Assert what `new X()` actually yields for each, separately |
| 62 | `error_types_are_subclasses_of_error_with_the_spec_names` | `new CompileError() instanceof Error`, `.name === "CompileError"` | **FAIL** — plain rquickjs classes, no `Error` inheritance, no `message` |
| 63 | `unreachable_trap_throws_a_runtime_error` | calling an export that traps throws `RuntimeError` | **FAIL** — surfaces as a generic internal exception (`instance.rs:263-268`) |
| 64 | `instantiate_streaming_is_not_implemented` | `WebAssembly.instantiateStreaming === undefined` | PASS as a gap-documenting test; flip to a real streaming test when `Response` (den-stdlib-whatwg-fetch) can feed bytes |
| 65 | `compile_streaming_is_not_implemented` | ditto | same |

That is 65 named cases; the ~35 marked PASS are the ones that can land green immediately, the rest
are the executable to-do list for the JS API.

### 5.6 Test cases — `den-core/tests/`

All `#[tokio::test(flavor = "multi_thread")]` (the loaders use `block_in_place`, §3.1).
`cargo nextest run` runs with cwd = the package root, so fixtures are addressed as `tests/fixtures/x.ts`.

| # | File | Test name | Asserts |
|---|---|---|---|
| 66 | `engine_stdlib_modules.rs` | `every_registered_den_specifier_resolves_and_imports` | table-driven over `den:core`, `den:console`, `den:networking`, `den:text`, `den:timer`, `den:fs`, `den:sqlite`, `den:crypto`, `den:wasm` — each `await import("…")` succeeds |
| 67 | | `import_den_whatwg_fetch_specifier_resolves` | **regression test for §2.2** — currently fails, passes after the one-char fix |
| 68 | | `eager_modules_install_their_globals` | `typeof console.log`, `atob`, `TextEncoder`, `setTimeout`, `crypto.randomUUID`, `fetch`, `WebAssembly` are all defined without any import |
| 69 | | `import_only_modules_do_not_leak_globals` | `typeof TcpListener === "undefined"` before importing `den:networking` |
| 70 | `engine_wasm.rs` | `webassembly_global_is_reachable_from_engine_eval` | `typeof WebAssembly.instantiate === "function"` |
| 71 | | `wasm_add_module_runs_end_to_end_through_engine_eval` | one smoke test: wat bytes handed in via `globalThis`, instantiate, call, assert `42` |
| 72 | | `each_engine_gets_its_own_wasm_store` | two `Engine::new()`s, a `Memory` grown in one is unchanged in the other |
| 73 | `engine_transpile.rs` | `typescript_file_import_is_transpiled_and_evaluated` | `run_file("tests/fixtures/hello.ts")` returns the exported value |
| 74 | | `tsx_file_import_is_transpiled_when_react_feature_is_on` | same for `.tsx` |
| 75 | | `eval_transpiles_typescript_annotations_in_a_snippet` | `Engine::eval::<i32>("const x: number = 1; x")` — exercises `IsModule::Unknown` (§3.3 site 6) |
| 76 | | `unknown_extension_import_fails_with_a_loading_error` | `.txt` is rejected by `MmapScriptLoader` (`mmap_script.rs:46-51`) |

Also rename the two existing tests while you are in there:
`my_test` → `eval_returns_coerced_primitive_values`,
`my_test2` → `eval_accepts_a_module_level_export_statement` (`den-core/src/engine.rs:399`, `:414`).

### 5.7 What makes this codebase hard to test, and the workaround for each

| Obstacle | Where | Workaround |
|---|---|---|
| **One `Store` per context, shared by everything** | `lib.rs:116-118` | fresh `AsyncRuntime` + `AsyncContext` per test (the harness does this). Never `#[tokio::test]` two wasm scenarios in one context. **Note this whole design is up for grabs — wasmtime 48 forbids `Store<(WasiP1Ctx, Ctx<'js>)>` (§6.4a) and option 3 there is a per-`Instance` store, which changes what tests #23/#41/#72 even mean.** |
| **`RefCell<Store>` re-entrancy panics** | `instance.rs:219`, `:234`, `:262`, `memory.rs:64`, `table.rs:88`, `global.rs:68` | test #41 pins it; fix by `try_borrow_mut()` → `Exception::throw_internal`, which turns an abort into a catchable JS error and makes the failure testable |
| **`ctx.userdata::<…>().unwrap()`** in 10 places | see §2.3 | harness always evaluates `js_wasm` first; longer-term return a JS error so a misuse fails one test instead of aborting the binary |
| **`WasiCtxBuilder::inherit_stdio().inherit_env()`** hard-coded | `store.rs:24-27` | today a WASI module writes straight to the test runner's stdout and can read the CI environment. Make `Store::new` take the pipes (or read an optional `StoreConfig` from userdata) so tests can inject `wasmtime_wasi::p2::pipe::MemoryOutputPipe` via `.stdout(pipe.clone())` and assert `pipe.contents()`. `inherit_env()` in particular should not be the default — it hands every env var to guest wasm. |
| **`block_in_place` in both loaders** | `http.rs:107`, `mmap_script.rs:79` | any `den-core` test touching files/URLs must use `flavor = "multi_thread"` |
| **`HttpLoader` does real network I/O** | `http.rs:27` | no test should import an `http://` URL. If HTTP loading needs coverage, bind a `tokio::net::TcpListener` on `127.0.0.1:0` and serve one canned response; otherwise skip the layer. |
| **`run_file` interpolates the path into a JS template literal** | `engine.rs:322` | forward slashes only; a Windows path or a backtick in a fixture name breaks the eval |
| **Wasm engine has `async_support` disabled** | `engine.rs:24` (commented out) | host imports are synchronous; a JS import returning a Promise cannot be awaited by the guest. Document it; do not write tests that assume otherwise. |
| **No `cargo nextest run` in CI** | `.github/workflows/lint.yml` | add a `test` job mirroring the `clippy` job: `cargo nextest run --workspace`, plus one `--no-default-features` build to keep the feature gates honest — but fix §6.9b and §6.9c first, they are exactly what that build catches. Widen `paths:` to include `**/Cargo.toml` and `Cargo.lock` (§6.9d) or the job will not run on dependency commits. |

---

## 6. Dead or broken — delete, do not migrate

### 6.1 Already staged for deletion — confirm and move on

* **`den-stdlib-wasm/scratchpad.rs`** (`git show HEAD:den-stdlib-wasm/scratchpad.rs`, 28 lines).
  A private `async fn wasm()` that builds a wasmtime engine, compiles `vec![]` as a module, and
  looks up a `"test"` function. It is not in `lib.rs`'s module tree (so it never compiled), it
  imports `color_eyre` which is not a dependency of the crate, and half of it is commented-out WASI
  code. **Delete** (already `D` in the index). Its only value — "here is how you stand up a
  wasmtime store by hand" — is superseded by the test harness in §5.2.
* **`den-transpiler-oxc/src/transpile.rs`** (`RD` in the index). Verbatim duplicate of
  `infer_transpile_syntax_by_extension` / `InferTranspileSyntaxError` / `get_best_transpiling`,
  which already live in `den-transpiler-oxc/src/lib.rs:169-211`. **Delete** (done).

### 6.2 Dead code that is still in the tree

* **`den-stdlib-regex`** — `src/lib.rs` is one newline. No dependents (§1.1). It drags in `pcre2`
  (a C library build) and `colored` for nothing. **Delete the crate and its entry at
  `Cargo.toml:11`.** The README TODO ("rewrite RegExp using rust-lang/regex", `README.md:97`)
  survives fine without an empty crate holding its place.
* **`EngineError::InferTranspileSyntaxError`** (`den-core/src/engine.rs:387-389`) — never
  constructed; `infer_transpile_syntax_by_extension` returns `Option`, and every call site does
  `.unwrap_or_default()`. Delete the variant during the oxc swap.
* **`Module::new2`** (`den-stdlib-wasm/src/module.rs:38-43`) — a one-line pass-through to
  `new_inner`, called from exactly two places. Three constructors (`new`, `new2`, `new_inner`) for
  one operation; collapse to two (`new` for JS, `new_inner` for Rust).
* **`HttpResolver::{allowlist, denylist}`** (`den-core/src/resolver/http.rs:7-8`) — `pub(crate)`
  fields with no way to populate them; the matching code at `:34-47` can never fire. Either expose
  a builder (it is the only sandboxing knob den has) or drop the fields and the `matchit`
  dependency.
* **Unused dependencies** (each is a `Cargo.toml` line to delete):
  `den-stdlib-wasm` → `anyhow`, `den-stdlib-core`; `den-stdlib-timer` → `den-utils`;
  `den-stdlib-crypto` → `den-utils`; `den-core` → `den-utils`; `den` (root) → `den-utils`;
  `den-stdlib-console` → `indexmap`, `colored`. (Verified by grepping each crate's `src/` for the
  corresponding `use`.)
* **Placeholder constructors** — `pub fn new() {}` returning `()` under `#[qjs(constructor)]`:
  `error.rs:10` (`Exception`), `:40` (`LinkError`), `:50` (`RuntimeError`) — but **not**
  `CompileError`, whose `new()` returns `Self` (`error.rs:28-30`) — plus
  `tag.rs:9`, `lib.rs:44` (`ResultObject`), plus
  `den-stdlib-core/cancellation.rs:18`, `den-stdlib-networking/socket.rs:26,80`,
  `den-stdlib-sqlite/lib.rs:24`, `den-stdlib-whatwg-fetch/lib.rs:19`. A JS `new X()` whose Rust
  constructor returns unit does not produce a Rust-backed instance. Test #61 pins the real
  behaviour; the classes that are genuinely not constructible from JS should drop
  `#[qjs(constructor)]` instead of faking one.
* **`den-stdlib-wasm/src/tag.rs`** — an empty `Tag` class exported as a constructor, with no
  exception-handling proposal support behind it. Delete until there is an implementation.

### 6.3 Currently broken — will not compile as-is

These are migration work items, not deletions, but every one of them blocks `cargo test`:

1. `den-transpiler-oxc/src/lib.rs` is **entirely swc code** (`swc_common`, `swc_ecma_*`,
   `swc_compiler_base`, `sourcemap`) while its `Cargo.toml:17-25` declares only `oxc_*` +
   `trie-match`. Nothing here compiles. The crate has to be rewritten against oxc.
2. `den-core` imports `den_transpiler_swc` in three files (`engine.rs:13`, `loader/http.rs:9`,
   `loader/mmap_script.rs:9`) but depends on `den-transpiler-oxc` (`Cargo.toml:44`).
3. rquickjs 0.12 `Loader`/`Resolver` trait signatures changed (§3.2) — three impls to update.
4. `den-stdlib-wasm/src/lib.rs:103` calls `wabt::wat2wasm`; the dependency is now `wat` — switch to
   `wat::parse_str(source)` (`wat-1.257.1/src/lib.rs:193`).
5. `den-stdlib-wasm/src/module.rs:5` `use getset::Getters` — `getset` was removed from that crate's
   dependencies. The derive is only used for a `#[getset(get)] inner` that nothing calls; delete
   the derive and the import.
6. `den-stdlib-wasm/src/store.rs:5` `wasmtime_wasi::preview1::{WasiP1Ctx}` and
   `instance.rs:220` `wasmtime_wasi::preview1::add_to_linker_sync` — in wasmtime-wasi 48 the module
   is **`p1`** (`wasmtime-wasi-48.0.0/src/lib.rs:41`, `WasiP1Ctx` at `src/p1.rs:142`,
   `add_to_linker_sync` at `src/p1.rs:847`). `WasiCtxBuilder::build_p1` (`src/ctx.rs:480`) and
   `inherit_stdio`/`inherit_env` (`src/ctx.rs:135`, `:219`) still exist.
   Note the bound tightened too: 27 had `add_to_linker_sync<T: Send>`
   (`wasmtime-wasi-27.0.0/src/preview1.rs:804`), 48 has `<T: Send + 'static>` — see §6.4.
7. Remaining wasmtime 27→48 API drift in `store.rs` / `memory.rs` / `table.rs` / `global.rs` /
   `instance.rs` — **enumerated in §6.4**. There is no separate wasmtime migration note in
   `docs/research/`; do not go looking for one.

### 6.4 wasmtime 27 → 48 — the exhaustive drift list

Produced by applying §6.3 items 4–6 locally and re-running `cargo check -p den-stdlib-wasm`.
These are the errors that remain once the three trivial ones are out of the way.

**(a) `Store<T>` became `Store<T: 'static>`.** This is the single biggest item in the whole
migration and it forces a redesign, not a rename.

```
wasmtime-27.0.0/src/runtime/store.rs:176   pub struct Store<T> {
wasmtime-48.0.0/src/runtime/store.rs:196   pub struct Store<T: 'static> {
                                     :237  pub struct StoreInner<T: 'static> {
```

den's store data is *not* `'static`:

```rust
// den-stdlib-wasm/src/store.rs:7,13
pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);
pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData<'js>>>>,
```

Nine errors fall out of that one fact:

| Site | Error |
|---|---|
| `store.rs:13` | `E0477` the type `(WasiP1Ctx, Ctx<'js>)` does not fulfill the required lifetime |
| `store.rs:9` (the `Deref` derive) | `E0477`, same cause |
| `store.rs:9` / `:30` | `lifetime may not live long enough` — `'js` must outlive `'static` |
| `global.rs:45` | `ctx.userdata::<Store>()` — `'js` must outlive `'static` (`Store<'js>` is invariant in `'js`) |
| `memory.rs:62`, `:73`, `:100` | same |
| `instance.rs:157` | `memory.inner.ty(store.as_context())` — `'js` must outlive `'static` |
| `instance.rs:219` | `E0521` `import_object` escapes the associated function body |
| `table.rs:88` | `E0521` `ctx` escapes the associated function body |

The `Ctx<'js>` is in the store data for exactly one reason: the host-function trampoline at
`instance.rs:71-74` recovers it from `caller.data()` in order to `Persistent::restore(ctx)` the JS
callback. Three ways out, pick one **before** writing any of the §5.5 tests, because #23, #41 and
#72 all depend on the answer:

1. `StoreData = WasiP1Ctx` (no lifetime) and hand the trampoline a context handle that *is*
   `'static` — e.g. store the `AsyncContext`/`rquickjs::Context` clone instead of `Ctx<'js>` and
   enter it inside the callback. Cleanest, but the callback is called from inside an already-held
   runtime lock, so re-entering must be a no-op borrow, not a re-lock.
2. Keep the `Ctx` but launder the lifetime with the same `DangerouslyImplementSync` trick already
   used at `instance.rs:54-57`, i.e. a `'static`-transmuted context handle. Smallest diff, most
   unsound; if you take it, say so in a comment.
3. Give each `Instance` its own `wasmtime::Store` and drop the context-wide userdata store
   altogether (also fixes the re-entrancy hazard, §5.7). Biggest change to the ownership model.

**(b) `ExternType` gained a `Tag(TagType)` variant**
(`wasmtime-48.0.0/src/runtime/types.rs:1445-1456`, absent in `wasmtime-27.0.0/…/types.rs:1151-1160`).

* `den-stdlib-wasm/src/module.rs:85` — `E0004: ExternType::Tag(_) not covered` in `extern_type_to_str`.
* `den-stdlib-wasm/src/instance.rs:241` — `E0004`, the `match ext.ty(&mut *store)` in `exports`.
* `den-stdlib-wasm/src/instance.rs:116-166` — this one *compiles*, because the match already ends in
  `_ => unreachable!()`. A module importing a tag now panics the process instead of erroring.
  Turn that arm into an `Exception::throw_type` while you are here.

Nothing else in `memory.rs` / `table.rs` / `global.rs` breaks on wasmtime 48:
`MemoryTypeBuilder::default().min().max().shared().build()`, `TableType::new(RefType, u32,
Option<u32>)`, `Global::new`, `Linker::{new,define,func_new,instantiate}`, `Val::null_*`,
`RefType::{FUNCREF,EXTERNREF,ANYREF}` and `AsContext`/`AsContextMut` are all unchanged.

### 6.5 Edition 2021 → 2024

The manifests also switch `edition` to `2024` (and set `rust-version = "1.97"`). One source break
today, in `den-stdlib-wasm`:

```
error: cannot explicitly borrow within an implicitly-borrowing pattern:
       explicit `ref` binding modifier not allowed when implicitly borrowing
  --> den-stdlib-wasm/src/lib.rs:67:26   Either::Left(ref x)  => x.as_bytes(),
  --> den-stdlib-wasm/src/lib.rs:68:27   Either::Right(ref x) => x.as_bytes(),
```

`validate_inner` matches on `buffer_source: &Either<…>`, so edition 2024 match ergonomics
(RFC 3627) already bind `x` by reference; the explicit `ref` is now an error. Fix: delete both
`ref`s. The visually identical arms at `module.rs:26-29` are fine — there `buffer_source` is taken
by value, so `ref` is still meaningful.

Nothing else trips: den's only `unsafe` blocks (`instance.rs:56-57`, `den-stdlib-crypto/src/lib.rs:31`,
the commented-out `qjs::JS_NewArrayBuffer` in `memory.rs:77-96`) are already outside `unsafe fn`,
and no identifier named `gen` exists in the workspace.

### 6.6 derive_more 1.0 → 2.x — the trait re-exports moved

From `derive_more-2.1.1/CHANGELOG.md`, "2.0.0 — Breaking changes":

> `use derive_more::SomeTrait` now imports macro only. Importing macro with its trait along is
> possible now via `use derive_more::with_trait::SomeTrait`.

Concretely: in 1.0 the crate root re-exported `core::ops::Deref` *and* the derive under the same
name (`derive_more-1.0.0/src/lib.rs:146-170`, `re_export_traits!`); in 2.x that whole block moved
under `pub mod with_trait` (`derive_more-2.1.1/src/lib.rs:164-188`). Any file that imports `Deref`
from the crate root and then **calls** `.deref()` loses the trait from scope:

| File:line | Code | Error |
|---|---|---|
| `den-stdlib-core/src/cancellation.rs:20` | `#[delegate(self.deref())]` on `cancel` | `E0599` no method named `deref` |
| `den-stdlib-networking/src/socket.rs:84` | `Ok(self.deref().local_addr()?.into())` | `E0599` |
| `den-stdlib-networking/src/socket.rs:88` | `let (stream, addr) = self.deref().accept().await?;` | `E0599` |
| `den-stdlib-sqlite/src/lib.rs:51` | `if let Some(conn) = self.conn.borrow().deref()` | `E0599` |
| `den-stdlib-sqlite/src/lib.rs:78` | same, in `query_rows` | `E0599` |

Fix, one line per file: add `use std::ops::Deref;`. That coexists with the existing
`use derive_more::{Deref, DerefMut, …}` because the derive lives in the macro namespace and the
trait in the type namespace. (`use derive_more::with_trait::{Deref, DerefMut}` also works and is
closer to the old behaviour.) Verified: all four crates check clean afterwards.

Files that import from `derive_more::derive::{…}` — all of `den-stdlib-wasm`, `den-stdlib-io` —
are unaffected; that submodule was always macro-only.

**Do not confuse this with a rquickjs problem.** Before the `deref` fix lands,
`den-stdlib-core` does not build, which makes `den-stdlib-wasm` and `den-core` fail with a wall of
cascading `UserDataGuard`/`str`-unsized nonsense that has nothing to do with rquickjs 0.12.
Fix §6.6 and §6.7 **first**, then read the real errors.

### 6.7 rand 0.8 → 0.10 (`den-stdlib-crypto`)

```
error[E0432]: unresolved import `rand::RngCore`  --> den-stdlib-crypto/src/lib.rs:1:5
error[E0425]: cannot find function `thread_rng` in crate `rand`  --> den-stdlib-crypto/src/lib.rs:32:15
```

In rand 0.10 the byte-source trait is `rand_core::Rng` (`rand_core-0.10.1/src/lib.rs:49`, with
`fn fill_bytes(&mut self, dst: &mut [u8])` at `:62`), re-exported as `rand::Rng`
(`rand-0.10.2/src/lib.rs:59`); what used to be called `Rng` is now `RngExt` (`:72`); and
`thread_rng()` is now `rand::rng()` (`rand-0.10.2/src/lib.rs:70`). Fix:

```rust
-use rand::RngCore;                       // lib.rs:1
+use rand::Rng;
-        rand::thread_rng().fill_bytes(dest);   // lib.rs:32
+        rand::rng().fill_bytes(dest);
```

Verified: `cargo check -p den-stdlib-crypto` is clean afterwards.

### 6.8 rquickjs 0.12 breakage beyond the Loader/Resolver traits (§3.2)

**(a) `#[rquickjs::class]` now rejects a field whose type is itself a `JsClass`.**

```
error[E0277]: using a `JsClass` type directly as a class field is not supported
  --> den-stdlib-wasm/src/lib.rs:33:5
   | `module::Module` implements `JsClass` — wrap the field in `Class<'js, T>` instead
   = note: nested mutations are lost because the generated getter clones the value
```

Two errors, one per field of `ResultObject` (`lib.rs:32-39`: `module: crate::module::Module`,
`instance: crate::instance::Instance`). The check is new — `NotAJsClassField` /
`JsClassFieldCheck` live at `rquickjs-core-0.12.2/src/class/impl_.rs:88-125` and the symbol does
not exist anywhere in `rquickjs-core-0.8.1`.

Fix: `ResultObject` grows a lifetime and holds `Class<'js, Module>` / `Class<'js, Instance>`,
built with `Class::instance(ctx.clone(), …)?` inside `instantiate` (`lib.rs:53-58`). Adding `'js`
means the `JsLifetime` derive no longer applies — write it by hand exactly like `Store` does
(`store.rs:16-18`). **§5.5 test #17 cannot be written until this lands.**

**(b) `async_with!` is deprecated.** `rquickjs-core-0.12.2/src/context/async.rs:71` carries
`#[deprecated]`; the macro now expands to nothing but
`AsyncContext::async_with(&$context, async |$ctx| { … })` (`:73-77`), since async closures are
stable. Three warnings today: `den-core/src/engine.rs:5` (the import), `:313` (`run_file`),
`:359` (`eval`). The direct call is

```rust
// rquickjs-core-0.12.2/src/context/async.rs:218-224
pub fn async_with<F, R>(&self, f: F) -> WithFuture<F, R>
where
    F: for<'js> AsyncFnOnce(Ctx<'js>) -> R + ParallelSend,
    R: ParallelSend;
```

so `async_with!(self.context => |ctx| { … }).await` becomes
`self.context.async_with(async |ctx| { … }).await`. Not a build breaker (CI does not pass
`-D warnings`), but write the §5.2 harness the new way rather than baking in a deprecated macro.

### 6.9 Feature-gate landmines

**(a) `den-stdlib-wasm`'s `wasmi` feature is fiction.** The manifest declares
`default = ["wasmtime"]`, `wasmtime = ["dep:wasmtime", "dep:wasmtime-wasi"]`, `wasmi = ["dep:wasmi"]`,
and `den-core` (`wasm-wasmtime` / `wasm-wasmi`) and the root bin faithfully mirror them — but
`grep -rn 'cfg(feature' den-stdlib-wasm/src` returns **nothing**. All 11 source files name
`wasmtime::` unconditionally, so `--features wasmi --no-default-features` has never compiled and
`--no-default-features` alone fails too. Either gate the backend for real or delete the `wasmi`
feature from all three manifests. (`docs/research/03-wasmi-1.1-api.md` documents the API for a
backend that does not exist yet.)

**(b) `cargo check -p den-core` in isolation fails on `block_in_place`**
(`loader/http.rs:107`, `loader/mmap_script.rs:79`): `den-core/Cargo.toml:28` is
`tokio.workspace = true` with **no features**, and `block_in_place` needs `rt-multi-thread`. It has
only ever built because feature unification supplies it from the root bin (`Cargo.toml:88`) or from
den-core's own dev-dependency (`Cargo.toml:49`). Pre-existing, not caused by the bump — but it will
break the `--no-default-features` CI build proposed in §5.7. Fix:
`tokio = { workspace = true, features = ["rt-multi-thread"] }`.

**(c) The `not(transpile)` arm of `MmapScriptLoader` does not compile.**
`mmap_script.rs:74-75` reads `let module = Module::declare(ctx.clone(), path, src.as_slice());
Ok(module)` → `E0308: expected Result<Module<'_>, Error>, found Result<Result<Module<'_>, Error>, _>`.
The `transpile` arm (`:69-70`) is fine because it uses `Module::declare(…)?`. Only visible with
`--no-default-features`; fix it while editing the file for §3.3 site 9.

**(d) CI never runs on this change.** `.github/workflows/lint.yml:10-13` filters
`paths: ["**/*.rs", ".github/workflows/lint.yml"]`, so a manifest-only commit — which is exactly
what the first half of this migration is — triggers no jobs. Add `**/Cargo.toml` and `Cargo.lock`
when you add the `test` job.

### 6.10 Verified compile status, crate by crate

`cargo check --offline -p <crate>` against the working tree this doc describes. Use it as the
burn-down list.

| Crate | Status | Errors |
|---|---|---|
| `den-utils` | clean | — |
| `den-stdlib-io` | clean | — |
| `den-stdlib-text` | clean | — |
| `den-stdlib-console` | clean | — |
| `den-stdlib-fs` | clean | — |
| `den-stdlib-regex` | clean | (the file is one newline) |
| `den-stdlib-whatwg-fetch` | clean | — |
| `den-stdlib-timer` | blocked | only by `den-stdlib-core` |
| `den-stdlib-core` | **1** | `cancellation.rs:20` → §6.6 |
| `den-stdlib-crypto` | **2** | `lib.rs:1`, `:32` → §6.7 |
| `den-stdlib-networking` | **2** | `socket.rs:84`, `:88` → §6.6 |
| `den-stdlib-sqlite` | **2** | `lib.rs:51`, `:78` → §6.6 |
| `den-transpiler-oxc` | **15–18** | every `swc_*` / `sourcemap` / `anyhow` path → §6.3 item 1 |
| `den-stdlib-wasm` | **4**, then **17** | §6.3 items 4–6 first, then §6.4 (11) + §6.5 (2) + §6.8a (2) + §6.3 item 4 (1) + `module.rs` Tag (1) |
| `den-core` (`--no-default-features --features stdlib`) | **5** | 3× Loader/Resolver arity → §3.2; 2× `block_in_place` → §6.9b |
| `den` (`--no-default-features`) | **4** | 3× arity; `mmap_script.rs:79` → §6.9c |

Full `den-stdlib-wasm` residue after §6.3 items 4–6 are applied (17 errors, 2 warnings):

```
store.rs:13, store.rs:9 (×2), store.rs:30      E0477 / lifetime — §6.4a
global.rs:45                                    lifetime — §6.4a
memory.rs:62, :73, :100                         lifetime — §6.4a
instance.rs:157                                 lifetime — §6.4a
instance.rs:219, table.rs:88                    E0521 escape — §6.4a
module.rs:85, instance.rs:241                   E0004 ExternType::Tag — §6.4b
lib.rs:33 (×2)                                  E0277 JsClass field — §6.8a
lib.rs:67, :68                                  edition-2024 `ref` — §6.5
```

---

## 7. Open questions for the implementer

1. **Can the oxc transpiler live in a `Clone + Send + Sync` `Engine`?** `Engine` is `#[derive(Clone)]`
   and holds `Arc<EasySwcTranspiler>` (`engine.rs:25-32`); `oxc_allocator::Allocator` is not `Sync`.
   If the oxc wrapper must allocate an arena per call, `Engine::transpile` becomes `&self` +
   local arena and the `Arc` field can hold just the config. Decide before touching the loaders,
   because their `transpiler` field type follows.
2. **What replaces `IsModule::Unknown`?** (`engine.rs:354`) swc sniffs module-vs-script; oxc's
   `SourceType` requires you to pick. Options: always parse as module (breaks REPL snippets using
   `with`/`arguments`?), or try module then fall back to script. This affects `Engine::eval`, i.e.
   the REPL and test #75.
3. **Should `WebAssembly.{Module,Memory,Table,Global,Instance,CompileError,…}` be added to the
   global namespace object?** Everything needed is already there
   (`Class::<T>::create_constructor(ctx)`); it is ~10 lines in `lib.rs:120-134` and it unblocks
   tests #2, #43-#60 written the way the spec (and every other runtime) expresses them.
4. **i64 ↔ BigInt** (test #30): fixing `WasmValueConverter::into_js` to emit
   `BigInt::from_i64(ctx, x)` for `Val::I64` is correct per spec but changes existing behaviour for
   any script that treats i64 results as Numbers. Do it, or gate it? (Recommend: do it — the
   current behaviour is silently lossy above 2^53.)
5. **`Store` re-entrancy** (test #41): `try_borrow_mut` + JS exception is the cheap fix; a
   per-instance store is the real one but changes the `Instance`/`Memory`/`Table` ownership model.
   Which? — **and note this is no longer optional**: wasmtime 48's `Store<T: 'static>` (§6.4a)
   already forces `StoreData` to change, so decide re-entrancy and store ownership in one pass.
   Of the three options in §6.4a, only (3) also closes the re-entrancy hole.
6. **Does `den-stdlib-io` deserve to exist** as a crate, given its single consumer and 66 lines?
   Folding it into `den-stdlib-networking` removes a workspace member; keeping it is justified only
   if `den:fs` is going to grow file handles. (Not urgent — no action needed for this migration.)
7. **CI**: add a `test` job to `.github/workflows/lint.yml`? Without it the new suite rots. The
   runners are self-hosted, so wasmtime build time is a real cost — consider `cargo nextest run
   --workspace --no-fail-fast` on PRs plus a nightly full-feature-matrix run. Widen `paths:` too
   (§6.9d).
8. **Does the `wasmi` backend exist or not?** (§6.9a) Three manifests advertise a `wasmi` feature
   that has never had a single `#[cfg(feature = "wasmi")]` behind it. Delete the feature, or gate
   `den-stdlib-wasm` properly and make CI build both backends. Deleting is the honest default until
   someone writes it — and it decides whether `docs/research/03-wasmi-1.1-api.md` is live research
   or an artefact.

---

## Verification log

Second pass, done as a completeness/accuracy audit against the vendored crate sources under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` and against `cargo check --offline`
run per workspace member. The working tree was patched temporarily to get past the first wave of
errors and then restored with `git checkout --` — no source file in the repo was modified by this
pass, only this document.

### Claims checked and CONFIRMED

| Claim | Where verified |
|---|---|
| `Resolver::resolve` / `Loader::load` gained `attributes: Option<ImportAttributes<'js>>` | `rquickjs-core-0.12.2/src/loader.rs:64-70`, `:98-103`; 0.8.1 equivalents at `:60`, `:67`. Reproduced as 3 real `E0050` errors at `resolver/http.rs:12`, `loader/http.rs:25`, `loader/mmap_script.rs:40` — exactly the three sites §3.2 names |
| `BuiltinResolver::resolve` errors for unknown specifiers | `builtin_resolver.rs:35-59` (doc said `:34-58`, corrected) |
| `ModuleLoader::with_module` → `Module::declare_def`, and `load()` does `self.modules.remove(path)` | `module_loader.rs:22-24`, `:49` |
| `Module::evaluate_def` returns `(Module<Evaluated>, Promise)` | `value/module.rs:323-333` |
| `export_use` turns `pub use` into `Class::<T>::create_constructor(&ctx)?.expect(…)` | `rquickjs-macro-0.12.2/src/module/mod.rs:119`, `:133-136` |
| `ModuleConfig::rename` is written and never read | written `module/config.rs:38`; `module/mod.rs` only ever reads `rename_vars` / `rename_types` (`:126`, `:360`, `:375`, `:387`) |
| `EvalOptions` is `#[non_exhaustive]` with a `Default` | `context/ctx.rs:28`, `impl Default` at `:67` |
| `WithFuture::poll` drains jobs under the runtime lock | `context/async/future.rs:60-66` |
| `CaughtError: Display` | `result.rs:541`; `CatchResultExt` at `:616` |
| `i64 → Number` (i32-or-f64), never BigInt | `value/convert/into.rs:483-486` (`i32 f64 => i64 u32 u64 usize isize`) — test #30 stands |
| `wat::parse_str` is the `wabt::wat2wasm` replacement | `wat-1.257.1/src/lib.rs:193` |
| `wasmtime_wasi::preview1` → `p1` | `wasmtime-wasi-48.0.0/src/lib.rs:41`, `WasiP1Ctx` `src/p1.rs:142`, `add_to_linker_sync` `src/p1.rs:847`, `build_p1` `src/ctx.rs:480`, `inherit_stdio` `:135`, `inherit_env` `:219` |
| The `den:whatcg-fetch` / `den:whatwg-fetch` mismatch | `engine.rs:79`, `:162` vs `:275` — read verbatim, §2.2 is correct including the fix locations |
| Every `engine.rs` line reference in §1.2, §2.1, §3.1, §3.3, §5.6, §5.7 | read `den-core/src/engine.rs` in full; all sites land within ±1 line, three off-by-ones corrected (`den:text`, `den:timer`, `den:sqlite` resolver lines) |
| Every `den-stdlib-wasm` line reference in §2.3, §5.5, §5.7 | read all 11 source files; `userdata().unwrap()` really is in 10 places, the `RefCell` sites, `set_length`/`set_name`, the `null` return at `instance.rs:270`, the `anyfunc`/`Ref::Any` mismatch at `table.rs:63-68`, the `anyref` vs `anyfunc` inconsistency at `global.rs:97` vs `:55` — all correct |
| All six "unused dependency" claims in §6.2 | `grep -rn` per crate: `den-stdlib-wasm`→`anyhow`/`den_stdlib_core` 0 hits, `den-stdlib-timer`/`den-stdlib-crypto`/`den-core`/root-`src`→`den_utils` 0 hits, `den-stdlib-console`→`indexmap`/`colored` 0 hits. (`typed_builder` in `den-stdlib-wasm` *is* used — 3 hits — so it is correctly absent from that list.) |
| `den-stdlib-regex/src/lib.rs` is one newline | `od -c` → `\n` |
| CI has no test job | `.github/workflows/lint.yml` |

### Claims CORRECTED

1. **§1.2 `den-stdlib-fs`** — "7 of 17 declared fns are `not implemented`" → it is **5** of 17
   (`metadata`, `readDir`, `readLink`, `setPermissions`, `symlinkMetadata`).
2. **§5.5 test #61 and §6.2** — "all three error classes have a `new()` returning `()`" is wrong.
   `CompileError::new()` returns `Self` (`error.rs:28-30`); it has to, because `lib.rs:96`
   constructs one from Rust. Only `Exception` (`:10`), `LinkError` (`:40`) and `RuntimeError`
   (`:50`) return `()`. The placeholder-constructor line refs `error.rs:10,39,48` were also off.
3. **§5.5 test #17** — marked PASS; it is **blocked**: `ResultObject` does not compile under
   rquickjs 0.12 (§6.8a).
4. **§6.3 item 7** — pointed at "the wasmtime migration note", which does not exist in
   `docs/research/` (only `03-wasmi-1.1-api.md`, `04-swc-to-oxc-transpiler.md`,
   `05-webassembly-js-api-spec.md`, and this file). Replaced with the actual list, §6.4.
5. Line-reference fixes: `builtin_resolver.rs:34-58`→`:35-59`; `wasmtime-wasi/src/lib.rs:39`→`:41`;
   `memory.rs:26`→`:25`; `den-stdlib-sqlite/src/lib.rs:250`→`:251`; resolver lines for `den:text`
   (`:62`→`:63`), `den:timer` (`:66`→`:67`), `den:sqlite` (`:74`→`:75`).

### Gaps FILLED (things the doc did not mention at all)

New §0 (full dependency diff) and §6.4–§6.10. In severity order:

1. **§6.4a — wasmtime 48 `Store<T: 'static>`.** `den-stdlib-wasm/src/store.rs` cannot exist in its
   current shape; nine lifetime errors across five files. This is the largest single item in the
   migration and the doc had zero words on it.
2. **§6.6 — derive_more 1.0→2.x moved the std trait re-exports to `with_trait`.** Five `E0599`s in
   three crates (`den-stdlib-core`, `den-stdlib-networking`, `den-stdlib-sqlite`) that are
   *load-bearing*: until they are fixed, `den-stdlib-wasm` and `den-core` emit a cascade of
   misleading `UserDataGuard` / unsized-`str` errors that look like rquickjs problems and are not.
3. **§6.8a — rquickjs 0.12 rejects `JsClass` types as class fields** (`NotAJsClassField`, new in
   0.12). Breaks `ResultObject` and therefore `WebAssembly.instantiate`'s return value.
4. **§6.7 — rand 0.8→0.10** breaks `den-stdlib-crypto` (`RngCore` → `Rng`, `thread_rng()` → `rng()`).
5. **§6.5 — edition 2021→2024** (the manifests switch it; the doc never said so). Breaks
   `den-stdlib-wasm/src/lib.rs:67-68` on RFC 3627 match ergonomics.
6. **§6.4b — `ExternType::Tag`** is a new wasmtime 48 variant: two `E0004`s plus one silent
   `unreachable!()` at `instance.rs:166`.
7. **§6.8b — `async_with!` is deprecated** in 0.12, including in the §5.2 harness the doc proposes.
8. **§6.9a — the `wasmi` feature has never compiled**: no `cfg(feature = …)` anywhere in
   `den-stdlib-wasm/src`, yet three manifests advertise it.
9. **§6.9b/c — two `--no-default-features` breakages** (`den-core`'s tokio missing
   `rt-multi-thread`; the double-wrapped `Result` at `mmap_script.rs:74-75`), which matter because
   §5.7 proposes exactly that CI build.
10. **§6.9d — CI `paths:` filter** excludes `Cargo.toml`, so this migration's first commits run no
    checks.
11. **§6.10 — a per-crate `cargo check` burn-down table** so the implementer can work the list
    instead of guessing.

### Not verified / left alone

* `den-transpiler-oxc`'s oxc-side design (owned by `04-swc-to-oxc-transpiler.md`); only confirmed
  that the crate is still 100% swc and produces 15–18 errors.
* Runtime behaviour of any §5.5 test case — nothing in the workspace executes yet.
* wasmi 1.1 API surface (`03-wasmi-1.1-api.md`).
