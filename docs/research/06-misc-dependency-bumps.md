# 06 — Misc dependency bumps + Rust edition 2021 → 2024

Scope: every dependency bump in the den workspace **except** `rquickjs` (doc 01), `wasmi` (doc 03),
`swc`→`oxc` (doc 04) and the WebAssembly JS API rework (doc 05). Plus the workspace-wide
edition 2021 → 2024 migration.

State at time of writing: all `Cargo.toml` files already carry the new versions, `cargo fetch`
resolves, `rustc 1.97.1` / `cargo 1.97.1`, **no `.rs` file has been touched yet**.

Every claim below is backed by a path+line in
`/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` (abbreviated `$REG` from here on)
or in the den tree. Where a claim was non-obvious it was verified by compiling a minimal repro under
edition 2024 with the real crate — those are marked **[verified by compile]**.

---

## 0. TL;DR — what actually breaks

| # | Where | What | Severity |
|---|---|---|---|
| 1 | `den-stdlib-networking/src/socket.rs:84,88`, `den-stdlib-core/src/cancellation.rs:20`, `den-stdlib-sqlite/src/lib.rs:51,78` | `derive_more` 2.0 no longer re-exports traits from the crate root → `.deref()` is unresolved | **build break** |
| 2 | `den-stdlib-crypto/src/lib.rs:1,32` | `rand::RngCore` gone (renamed `Rng`), `rand::thread_rng()` gone (renamed `rand::rng()`) | **build break** |
| 3 | `den-stdlib-wasm/src/lib.rs:103` | `wabt` removed from the manifest; replace with `wat::parse_str` | **build break** |
| 4 | `den-stdlib-wasm/src/module.rs:5,10,14` | `getset` was dropped from `den-stdlib-wasm/Cargo.toml` but `use getset::Getters;` remains | **build break** |
| 5 | `den-stdlib-wasm/src/lib.rs:67,68` | edition 2024 match ergonomics: explicit `ref` under an implicit borrow | **build break** |
| 6 | `den-core/src/loader/mmap_script.rs:53` | `fmmap` 0.5 made every file-backed constructor `unsafe`; `AsyncMmapFile::open` is now `pub async unsafe fn` | **build break** |
| 7 | `src/repl.rs:19-27` | rustyline 18: `Config` lost `Copy`, `SQLiteHistory::{open,with_config}` take `&Config`, `Configurer::set_behavior` removed | **build break** |
| 8 | `den-core/Cargo.toml:20-26`, `den-stdlib-whatwg-fetch/Cargo.toml:24-33` | reqwest 0.13 remapped `default-tls` from native-tls to **rustls** → den now builds rustls *and* aws-lc-rs on top of native-tls | build bloat / C toolchain |
| 9 | `den-stdlib-wasm/src/lib.rs:102-112` | `wat::parse_str` does **not** validate the module (wabt did) | behaviour regression |
| 10 | `den-stdlib-console`, `den-stdlib-regex`, `den-stdlib-crypto`, `den-stdlib-whatwg-fetch` | declared-but-unused deps (`colored`, `pcre2`, `getset`, `indexmap`, …) | cleanup |
| 11 | `den-core/Cargo.toml:28` | `tokio` declared with **no features** while `den-core` calls `block_in_place`/`Handle` → `cargo check -p den-core` alone fails (masked by the `den` bin's features) — §4.9 | **build break** (isolated builds) |
| 12 | `den-stdlib-core/src/lib.rs:10,25` | `base64` + `base64-simd` features are mutually exclusive (two `#[cfg]` tail blocks) → `--all-features` never builds — §6.3 | verification hazard |

Everything else in the bump list (`rusqlite`, `base64`, `typed-builder`, `uuid`, `indexmap`,
`matchit`, `relative-path`, `url`, `clap`, `tracing-subscriber`, `color-eyre`, `console-subscriber`,
`mimalloc`, `vc-ltl`, `encoding_rs`, `tokio`, `tokio-util`, `futures`, `serde`, `serde_json`,
`getset`, `colored`, `delegate-attr`, `cfg-if`, `either`) is **source-compatible** — evidence per
crate below.

Resolved versions (from `Cargo.lock`, verified with `awk` over the lockfile):

```
derive_more 2.1.1   rand 0.10.2 / rand_core 0.10.1   rusqlite 0.39.0 / libsqlite3-sys 0.37.0
reqwest 0.13.4      rustyline 18.0.1 / rustyline-derive 0.12.0   colored 3.1.1   base64 0.23.1
typed-builder 0.23.2  getset 0.1.7   uuid 1.24.1   indexmap 2.14.0   matchit 0.9.2
relative-path 2.0.1   fmmap 0.5.0    url 2.5.8      clap 4.6.6       tracing-subscriber 0.3.23
color-eyre 0.6.5      console-subscriber 0.5.0      mimalloc 0.1.52  vc-ltl 5.3.1  pcre2 0.2.11
encoding_rs 0.8.35    tokio 1.53.1   tokio-util 0.7.19   futures 0.3.34  serde 1.0.229
serde_json 1.0.151    wat 1.257.1    wabt <absent>   derivative 2.2.0
```

---

## 1. Workspace-wide: `derive_more` 1.0 → 2.1.1

### 1.1 The one breaking change that matters

`$REG/derive_more-2.1.1/CHANGELOG.md:88-91` (2.0.0, "Breaking changes"):

> `use derive_more::SomeTrait` now imports macro only. Importing macro with its trait along is
> possible now via `use derive_more::with_trait::SomeTrait`.

Mechanically:

* derive_more **1.0.0** `src/lib.rs:169-227` runs a `re_export_traits!` macro that re-exports the
  real std traits from the crate root — `re_export_traits!("deref", deref_traits, core::ops, Deref);`
  (`:169`), `…DerefMut` (`:170`), `core::error::Error` (`:186`), `core::convert::From` (`:190`),
  `core::convert::Into` (`:198`), `core::fmt::Display` (`:171-173`).
* derive_more **2.1.1** `src/lib.rs:123` is just `pub use derive_more_impl::*;` — **macros only**.
  The traits moved to `pub mod with_trait` (`src/lib.rs:139`).

`From`, `Into` and `Debug` are in the std prelude, so losing them is invisible. **`Deref`,
`DerefMut`, `Display` and `Error` are not**, and den calls `.deref()` at **five** call sites in three
files: `den-stdlib-networking/src/socket.rs:84,88`, `den-stdlib-core/src/cancellation.rs:20`,
`den-stdlib-sqlite/src/lib.rs:51,78`.

### 1.2 `derive_more::derive` survives

`$REG/derive_more-2.1.1/src/lib.rs:128-134`:

```rust
/// Module containing derive definitions only, without their corresponding traits.
pub mod derive {
    pub use derive_more_impl::*;
}
```

So every `use derive_more::derive::{…}` in den (`den-stdlib-wasm/src/{store,memory,engine,global,instance,utils}.rs`,
`den-stdlib-wasm/src/{table,module}.rs` partially, `den-stdlib-whatwg-fetch/src/lib.rs:5`,
`den-stdlib-sqlite/src/lib.rs:4-7`) is **unchanged**. Those sites were already macro-only and are
now the correct idiom.

### 1.3 Feature names unchanged

`$REG/derive_more-2.1.1/Cargo.toml` `[features]` vs `derive_more-1.0.0/Cargo.toml` `[features]`:
`deref_mut`, `deref`, `from`, `into`, `display`, `error`, `debug` all still exist, `default = ["std"]`
in both. The workspace declaration in `Cargo.toml:36-45` needs no change.

### 1.4 Attribute syntax unchanged for den

`Display` / `Error` attribute syntax did **not** change between 1.0 and 2.x. The 1.0 release already
moved to `#[display("…", args)]`; 2.1.0 only *added* `#[display(rename_all = "…")]`
(`CHANGELOG.md:19-21`). den's two error enums carry no `#[display]`/`#[error]` attributes at all:

* `den-core/src/engine.rs:380-390` — `#[derive(Display, From, Error, Debug)] pub enum EngineError`
  with three `#[from]` single-field variants.
* `den-transpiler-oxc/src/lib.rs:159-167` and `:199-202`.
* `den-stdlib-sqlite/src/lib.rs:244-249` — `#[derive(Error, Display, From, Debug)] enum QueryRowError`.

`Error`'s `source()` inference rule ("It's a tuple struct/variant and there's exactly one field that
is not used as the `backtrace`" — `$REG/derive_more-impl-2.1.1/doc/error.md:21-23`) is identical to
1.0, so all three keep the same generated `source()`.

Implicit `Display` for unit variants (`InferTranspileSyntaxError::InvalidExtension`) still works —
2.1.0 added `rename_all` specifically to customise "implicit naming of unit enum variants"
(`CHANGELOG.md:19-21`), which presupposes it exists.

### 1.5 Per-site fixes

**[verified by compile]** — a scratch crate on edition 2024 + derive_more 2.1.1 reproduced exactly:

```
error[E0599]: no method named `deref` found for reference `&Wrap` in the current scope
  = help: items from traits can only be used if the trait is in scope
help: trait `Deref` which provides `deref` is implemented but not in scope
  |
1 + use std::ops::Deref;
```

#### `den-stdlib-networking/src/socket.rs`

BEFORE (`:1-13`, `:84`, `:88`):
```rust
use std::sync::Arc;

use den_stdlib_io::{AsyncReadWrapper, AsyncWriteWrapper};
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
…
        Ok(self.deref().local_addr()?.into())
…
        let (stream, addr) = self.deref().accept().await?;
```
AFTER:
```rust
use std::{ops::Deref, sync::Arc};

use den_stdlib_io::{AsyncReadWrapper, AsyncWriteWrapper};
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
```
(call sites unchanged)

#### `den-stdlib-core/src/cancellation.rs`

BEFORE (`:1-5`, `:20`):
```rust
use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
…
    #[delegate(self.deref())]
    pub fn cancel(&self) {}
```
AFTER — add the trait import; `delegate-attr` expands `self.deref()` verbatim into the method body,
so the trait must be in scope at the definition site:
```rust
use std::ops::Deref;

use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
```

#### `den-stdlib-sqlite/src/lib.rs`

BEFORE (`:1-12`, `:51`, `:78`):
```rust
use std::{cell::RefCell, sync::Arc};
…
        if let Some(conn) = self.conn.borrow().deref() {
```
AFTER:
```rust
use std::{cell::RefCell, ops::Deref, sync::Arc};
```

Alternative (one less import, arguably clearer): `if let Some(conn) = &*self.conn.borrow() {`.
Both are fine; the import is the smaller diff across the two call sites.

#### Everything else

`den-stdlib-io/src/lib.rs:3`, `den-stdlib-text/src/lib.rs:2`, `den-stdlib-networking/src/{ip_addr,socket_addr}.rs:5`,
`den-utils/src/serde_json.rs:2`, `den-core/src/engine.rs:3`, `den-transpiler-oxc/src/lib.rs:5`,
`den-stdlib-wasm/src/error.rs:1`: these import only `From`/`Into`/`Deref`/`DerefMut`/`Debug`/`Display`/`Error`
as **derives**, never as traits. Verified by grep: den contains **zero** occurrences of `dyn Error`,
`: Display`, `+ Display`, `impl Display`, `.source()` or `Box<dyn …>`.
`den-stdlib-wasm/src/error.rs:17` spells it out fully — `impl std::error::Error for CompileError {}`.
**No change needed at those sites.**

---

## 2. `derivative` 2.2.0 — UNMAINTAINED (RUSTSEC-2024-0388), optional removal

`derivative` is still pinned at 2.2.0 in `Cargo.toml:35` and pulled by six member crates. It is
unmaintained (proc-macro, syn 1). It is **not** required for the bump — it still compiles under
rustc 1.97 / edition 2024 (proc-macro crates keep their own edition). Removal is a separate,
mechanical, *optional* commit. Per-site mapping:

| Site | Current | Replacement | Notes |
|---|---|---|---|
| `den-stdlib-core/src/cancellation.rs:7-13` | `#[derive(… Derivative …)] #[derivative(Clone, Debug)]` on `{ token: CancellationToken }` | `#[derive(Clone, Debug)]` | `CancellationToken: Clone + Debug` |
| `den-stdlib-networking/src/ip_addr.rs:8-14` | `#[derivative(Clone, Debug)]` on `{ addr: IpAddr }` | `#[derive(Clone, Debug)]` | trivially derivable |
| `den-stdlib-networking/src/socket_addr.rs:10-16` | `#[derivative(Clone, Debug)]` on `{ addr: SocketAddr }` | `#[derive(Clone, Debug)]` | trivially derivable |
| `den-stdlib-networking/src/socket.rs:15-21` | `#[derivative(Clone, Debug)]` on `{ stream: Arc<RwLock<TcpStream>> }` | `#[derive(Clone, Debug)]` | `tokio::sync::RwLock<T>: Debug where T: Debug`; `TcpStream: Debug` |
| `den-stdlib-networking/src/socket.rs:69-76` | `#[derivative(Clone, Debug)]` on `{ listener: Arc<TcpListener> }` | `#[derive(Clone, Debug)]` | |
| `den-stdlib-sqlite/src/lib.rs:14-20` | `#[derivative(Debug, Clone)]` on `{ conn: Arc<RefCell<Option<rusqlite::Connection>>> }` | `#[derive(Debug, Clone)]` | `impl fmt::Debug for Connection` at `$REG/rusqlite-0.39.0/src/lib.rs:1095` |
| `den-stdlib-whatwg-fetch/src/lib.rs:8-14` | `#[derivative(Clone, Debug)]` on `{ inner: Arc<RefCell<Option<reqwest::Response>>> }` | `#[derive(Clone, Debug)]` | `impl fmt::Debug for Response` at `$REG/reqwest-0.13.4/src/async_impl/response.rs:440` |
| `den-stdlib-text/src/lib.rs:10-20` | `#[derivative(Clone, Debug)]` + `#[derivative(Debug = "ignore")]` on `encoding: &'static Encoding` | `#[derive(Clone)]` + `derive_more::Debug` with `#[debug(skip)]` — **or** plain `#[derive(Clone, Debug)]` | `impl core::fmt::Debug for Encoding` exists at `$REG/encoding_rs-0.8.35/src/lib.rs:3480`, so the `ignore` is not actually needed; dropping it only changes the printed output |
| `den-stdlib-text/src/lib.rs:107-111` | `#[derivative(Clone, Debug)]` on `struct TextEncoder {}` | `#[derive(Clone, Debug)]` | |
| `den-core/src/loader/mmap_script.rs:13-22` | `#[derivative(Debug)]` + `#[derivative(Default(new = "true"))]` + `#[derivative(Debug = "ignore")]` on `transpiler: Arc<EasySwcTranspiler>` | see below — **needs real work** | the transpiler type is genuinely not `Debug` |
| `den-core/src/loader/http.rs:13-22` | `#[derivative(Default(new = "true"))]` + `#[derivative(Default(value = "true"))]` on `check_mime` | see below — **needs real work** | non-`Default` default value |

The two loaders are the only non-mechanical ones. `derive_more`'s `Debug` supports skipping
(`$REG/derive_more-impl-2.1.1/doc/debug.md:5` — `#[debug(skip)]` (or `#[debug(ignore)]`)), but there
is no `derive_more` equivalent of `derivative(Default(value = …))` or `Default(new = "true")`.

`den-core/src/loader/mmap_script.rs` BEFORE:
```rust
#[derive(Derivative, TypedBuilder)]
#[derivative(Debug)]
#[derivative(Default(new = "true"))]
pub struct MmapScriptLoader {
    #[builder(default)]
    extensions: Vec<String>,
    #[derivative(Debug = "ignore")]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
}
```
AFTER:
```rust
#[derive(Debug, Default, TypedBuilder)]
pub struct MmapScriptLoader {
    #[builder(default)]
    extensions: Vec<String>,
    #[debug(skip)]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
}

impl MmapScriptLoader {
    pub fn new() -> Self { Self::default() }
}
```
with `use derive_more::Debug;` replacing `use derivative::Derivative;` (note: this shadows the std
`Debug` derive in that module, which is what we want).

`den-core/src/loader/http.rs` BEFORE:
```rust
#[derive(Derivative, TypedBuilder)]
#[derivative(Default(new = "true"))]
pub struct HttpLoader {
    #[derivative(Default(value = "true"))]
    #[builder(default)]
    check_mime: bool,
    #[derivative(Debug = "ignore")]     // inert: nothing derives Debug here
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
}
```
AFTER:
```rust
#[derive(TypedBuilder)]
pub struct HttpLoader {
    #[builder(default = true)]
    check_mime: bool,
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
}

impl Default for HttpLoader {
    fn default() -> Self {
        Self {
            check_mime: true,
            #[cfg(feature = "transpile")]
            transpiler: Default::default(),
        }
    }
}

impl HttpLoader {
    pub fn new() -> Self { Self::default() }
}
```
(`typed-builder` supports `#[builder(default = <expr>)]`; note the 0.23 semantics change in §7.)

---

## 3. Root binary crate (`den`) — `src/`

### 3.1 rustyline 15.0.0 → 18.0.1 — `src/repl.rs` — **BREAKING**

There is no CHANGELOG in the published crate; the diffs below are from the sources.

| API | 15.0.0 | 18.0.1 |
|---|---|---|
| `Config` derives | `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` — `src/config.rs:6` | `#[derive(Clone, Debug, PartialEq, Eq)]` — `src/config.rs:6` — **`Copy` removed** |
| `SQLiteHistory::with_config` | `(config: Config)` — `src/sqlite_history.rs:36` | `(config: &Config)` — `src/sqlite_history.rs:36` |
| `SQLiteHistory::open` | `(config: Config, path: &P)` — `src/sqlite_history.rs:44` | `(config: &Config, path: &P)` — `src/sqlite_history.rs:44` |
| `Configurer::set_behavior` | trait method — `src/config.rs:564` | **removed**; only `pub(crate) fn Config::set_behavior` (`src/config.rs:182`) and `Builder::behavior` (`src/config.rs:485`) remain. The source comment at `:486` says *"cannot be touched after editor / terminal creation"* |
| `Editor::readline` | `(&mut self, prompt: &str)` — `src/lib.rs:637` | `<P: Prompt + ?Sized>(&mut self, prompt: &P)` — `src/lib.rs:643`; `impl Prompt for str` at `src/prompt.rs:19`, so `readline("> ")` still compiles |
| `Editor::with_history` | `(config: Config, history: I)` — `src/lib.rs:611` | unchanged — `(config: Config, history: I)` — `src/lib.rs:621` |
| `add_history_entry`, `set_helper` | `src/lib.rs:816,837` | unchanged — `src/lib.rs:831,852` |
| derive macros | rustyline-derive 0.11 | rustyline-derive 0.12 — identical `proc_macro_derive` set (`Completer`/`Helper`/`Highlighter`/`Hinter`/`Validator`, `attributes(rustyline)`); `#[rustyline(Validator)]` unchanged |
| `with-sqlite-history` feature | present | present — now pins `rusqlite 0.39.0` with `bundled` (`Cargo.toml:157-160`), i.e. the **same** `rusqlite` den uses → single `libsqlite3-sys 0.37.0` in the graph |
| `ReadlineError::{Eof, Interrupted}` | present | present — `src/error.rs:18,20` |
| `Behavior::{Stdio, PreferTerm}` | present | present — `src/config.rs:349-361` |
| `Config::{builder, build}` | present | present — `src/config.rs:52,537` |

Because `Config` is no longer `Copy`, the current expression moves `config` into
`Editor::with_history` **before** the inner `SQLiteHistory::open(&config, …)` borrow is evaluated
(arguments evaluate left-to-right) — so the borrows must be hoisted out.

`src/repl.rs` BEFORE (`:1-28`):
```rust
use rustyline::{
    config::Configurer, error::ReadlineError, sqlite_history::SQLiteHistory,
    validate::MatchingBracketValidator, Behavior, Completer, Config, Editor, Helper, Highlighter,
    Hinter, Validator,
};
…
    let config = Config::default();
    let mut rl = Editor::with_history(
        config,
        SQLiteHistory::open(config, "history.db")
            .or(SQLiteHistory::with_config(config))
            .unwrap(),
    )
    .unwrap();
    rl.set_behavior(Behavior::PreferTerm);
    rl.set_helper(Some(h));
```
AFTER:
```rust
use rustyline::{
    error::ReadlineError, sqlite_history::SQLiteHistory,
    validate::MatchingBracketValidator, Behavior, Completer, Config, Editor, Helper, Highlighter,
    Hinter, Validator,
};
…
    // `Behavior` can only be set on the Config before the terminal is created in rustyline 18
    let config = Config::builder().behavior(Behavior::PreferTerm).build();
    let history = SQLiteHistory::open(&config, "history.db")
        .or_else(|_| SQLiteHistory::with_config(&config))
        .unwrap();
    let mut rl = Editor::with_history(config, history).unwrap();
    rl.set_helper(Some(h));
```

Note the removal of `config::Configurer` from the import list — with `set_behavior` gone, den calls
no other `Configurer` method, so keeping the import is an `unused_imports` warning.
`.or(…)` → `.or_else(…)` is not required but avoids eagerly opening a second in-memory DB.

### 3.2 clap 4.5.23 → 4.6.6 — `src/main.rs:4,13-22,40` — no change

`$REG/clap-4.6.6/Cargo.toml` `[features]` is byte-for-byte the same set as `clap-4.5.23`
(`cargo`, `color`, `debug`, `default = ["std","color","help","usage","error-context","suggestions"]`,
`deprecated`, `derive`, `env`, `error-context`, `help`, `std`, `string`, `suggestions`, `unicode`,
`unstable-*`, `usage`, `wrap_help`) plus one addition (`unstable-markdown`). den uses
`#[derive(Parser)]`, `#[command(author, version, about, long_about = None)]`, `#[arg]`,
`#[arg(long, default_value_t = …)]`, `Cli::parse()` — all stable 4.x surface.
4.6.6 is `edition = "2024"`, `rust-version = "1.85"` (`Cargo.toml:13-14`).

### 3.3 color-eyre 0.6.3 → 0.6.5 — `src/main.rs:25,30` — no change

`pub fn install() -> Result<(), crate::eyre::Report>` at `$REG/color-eyre-0.6.5/src/lib.rs:458`,
identical to 0.6.3. `[features]` identical in both: `default = ["track-caller","capture-spantrace"]`,
`capture-spantrace`, `track-caller`, `issue-url`. den's `Cargo.toml:101`
(`tracing = ["color-eyre/track-caller", "color-eyre/capture-spantrace"]`) still resolves.

### 3.4 tracing-subscriber 0.3.19 → 0.3.23 — `src/main.rs:7,31-38` — no change

`$REG/tracing-subscriber-0.3.23/CHANGELOG.md:1-56` — 0.3.20 escaped ANSI sequences in logs and
`impl Clone for EnvFilter`; 0.3.21/0.3.22 bumped `tracing` to 0.1.43; 0.3.23 made ANSI sanitisation
switchable. No API removals. `tracing_subscriber::fmt()`, `.with_env_filter()`, `.pretty()`,
`.init()`, `EnvFilter::builder().with_default_directive(…).from_env_lossy()`, `filter::LevelFilter`
all unchanged. (Cosmetic: log output containing ANSI is now escaped by default since 0.3.20.)

### 3.5 console-subscriber 0.4.1 → 0.5.0 — `src/main.rs:28` — no change

`$REG/console-subscriber-0.5.0/CHANGELOG.md:6-17`: the only breaking change is the public `tonic`
dependency (0.12 → 0.13 → 0.14). `pub fn init()` is still at `src/builder.rs:713`. den's
`#[cfg(all(feature = "tokio-console", tokio_unstable))] console_subscriber::init();` is unaffected.

### 3.6 mimalloc 0.1.43 → 0.1.52 — `src/main.rs:9-11` — no change

`pub struct MiMalloc;` at `$REG/mimalloc-0.1.52/src/lib.rs:46`. `#[global_allocator] static GLOBAL:
mimalloc::MiMalloc = mimalloc::MiMalloc;` unchanged. (Note: this is a `static`, not a `static mut`,
so the edition-2024 `static_mut_refs` hard error does not apply.)

### 3.7 vc-ltl 5.1.1 → 5.3.1 — no change

`diff $REG/vc-ltl-5.1.1/src/lib.rs $REG/vc-ltl-5.3.1/src/lib.rs` → **identical**.
`diff` of the two `build.rs` → **identical**. Only the vendored `TargetPlatform/` binaries changed.
On non-Windows the build script early-returns with a `cargo:warning=VC-LTL only supports Windows
host` (`$REG/vc-ltl-5.3.1/build.rs:14-18`) — that warning is pre-existing, not a regression.

### 3.8 futures 0.3.31 → 0.3.34 — `src/app.rs:2` — no change

Only `.then()` from `futures::prelude::*` is used (`src/app.rs:84,116`). 0.3.x is semver-stable.
See §9.7 for the edition-2024 prelude interaction (also a non-issue).

---

## 4. `den-core`

### 4.1 fmmap 0.3.3 → 0.5.0 — `den-core/src/loader/mmap_script.rs` — **BREAKING**

**Feature rename** (already applied in `den-core/Cargo.toml:13`):
`$REG/fmmap-0.3.3/Cargo.toml` had `tokio-async = ["dep:fs4", "fs4?/tokio-async", "async-trait", …]`
plus a *separate* bare `tokio = ["dep:tokio"]`. `$REG/fmmap-0.5.0/Cargo.toml` has a single
`tokio = ["dep:tokio","fs4/tokio","dep:pin-project-lite","tokio?/io-std","tokio?/io-util","tokio?/fs","tokio?/rt"]`.
`async-std` / `std-async` support was dropped entirely (`src/mmap_file/async_std_impl.rs` exists in
0.3.3, gone in 0.5.0).

**`async-trait` dropped.** 0.3.3 `src/mmap_file/tokio_impl.rs:5` is `use async_trait::async_trait;`
and the trait is declared `#[async_trait] #[enum_dispatch] pub trait AsyncMmapFileExt`
(`src/mmap_file.rs:431-433`). 0.5.0 uses native AFIT and adds a supertrait:
`#[enum_dispatch] pub trait AsyncMmapFileExt: Sync` (`src/mmap_file.rs:817`).
Consequence for den: none — `AsyncMmapFile` is `Sync`, and den only calls `as_slice()`.

**The actual break — every file-backed constructor is now `unsafe`.**
`$REG/fmmap-0.5.0/src/mmap_file.rs:1957` (inside `declare_and_impl_async_mmap_file!`):

```rust
pub async unsafe fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
    Ok(Self::from(AsyncDiskMmapFile::open(path).await?))
}
```

vs 0.3.3 `src/mmap_file.rs:1508`: `pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self>`.

The crate-level safety contract (`$REG/fmmap-0.5.0/src/lib.rs:37-46`):

> The caller must ensure, by means outside this crate, that no other process or thread will mutate
> or truncate the file for as long as this mapping (and any borrowed slices it has yielded) is
> alive. Calling the constructor otherwise is undefined behavior.
>
> fmmap auto-acquires a `flock`-style file lock on every file-backed open (exclusive for writable
> mappings, shared for read-only) … But `flock` is **advisory**.

`as_slice()` itself is unchanged: `fn as_slice(&self) -> &[u8];`
(0.3.3 `src/mmap_file.rs:443`, 0.5.0 `src/mmap_file.rs:828`). `slice()`/`bytes()` gained overflow
checks; den does not use them.

`den-core/src/loader/mmap_script.rs` BEFORE (`:53-55`):
```rust
            let src = AsyncMmapFile::open(path)
                .await
                .map_err(|_| Error::new_loading(path))?;
```
AFTER:
```rust
            // SAFETY: fmmap 0.5 marks every file-backed constructor `unsafe` because an external
            // writer truncating the file while the mapping is live is UB (SIGBUS on read).
            // den maps user-supplied script files read-only for the duration of one `load()` call;
            // fmmap takes a shared advisory flock for us. Same exposure as fmmap 0.3, now explicit.
            let src = unsafe { AsyncMmapFile::open(path) }
                .await
                .map_err(|_| Error::new_loading(path))?;
```

Note the `unsafe` block wraps only the call that *builds* the future — `.await` is safe and stays
outside. (`unsafe { AsyncMmapFile::open(path).await }` also compiles but widens the block for no
reason.) den has no `unsafe fn`, so the edition-2024 `unsafe_op_in_unsafe_fn` hard error does not
interact with this.

Lazy alternative worth a thought, since this is the only mmap in the tree and script files are
small: `tokio::fs::read(path)` removes the dependency, the `unsafe`, and the SIGBUS exposure
entirely. Out of scope for the bump, but the `unsafe` block is the moment to ask.

### 4.2 relative-path 1.9.3 → 2.0.1 — `den-core/src/loader/mmap_script.rs:3,42-44` — no change

`RelativePath::new<S: AsRef<str> + ?Sized>(s: &S) -> &RelativePath` at
`$REG/relative-path-2.0.1/src/lib.rs:675`; `pub fn extension(&self) -> Option<&str>` at
`:1271-1275` — byte-identical body to 1.9.3 `src/lib.rs:1582-1586`. Diffing the public item lists of
1.9.3 `src/lib.rs` against 2.0.1's whole `src/` shows **only additions**: `PathExt` trait,
`RelativeToError`, plus test fns. `edition = "2021"`, `rust-version = "1.66"`.

### 4.3 matchit 0.8.5 → 0.9.2 — `den-core/src/resolver/http.rs:1,35,43` — no change

* `pub struct Router<T>` — 0.8.5 `src/router.rs:9`, 0.9.2 `src/router.rs:9`
* `pub fn at<'path>(&self, path: &'path str) -> Result<Match<'_, 'path, &T>, MatchError>` —
  0.8.5 `:59`, 0.9.2 `:60`
* `pub enum MatchError { NotFound }` — 0.8.5 `src/error.rs:151-154`, 0.9.2 `src/error.rs:153-156`

0.9 is a *routing-semantics* release, not an API release: the README diff shows named parameters now
match "until the next static segment" rather than "until the next `/`", and prefix/suffix parameters
inside a segment (`/images/img-{id}.png`) are now supported. Irrelevant here — den never calls
`Router::insert`. In fact `HttpResolver::{allowlist, denylist}` (`den-core/src/resolver/http.rs:7-8`)
are `Option<Router<String>>` that are **never populated anywhere in the tree** (grep for
`allowlist`/`denylist` returns only the declaration and the two `if let Some(...)` reads), so both
branches are permanently dead. Deleting them would delete the `matchit` dependency outright.

The lockfile still has `matchit 0.8.4` alongside `0.9.2` — that one comes in transitively, not from den.

### 4.4 url 2.5.4 → 2.5.8 — `den-core/src/resolver/http.rs:3,13-14,18-27,49-50` — no change

`diff` of `pub fn` names between `$REG/url-2.5.4/src/lib.rs` and `$REG/url-2.5.8/src/lib.rs` →
**empty**. `impl From<Url> for String` still at `$REG/url-2.5.8/src/lib.rs:2803-2807`
(den relies on it at `:50`, `Ok(name.into())`). `ParseError::RelativeUrlWithoutBase`, `Url::parse`,
`Url::join`, `Url::scheme` unchanged.

### 4.5 reqwest 0.12.9 → 0.13.4 — `den-core/src/loader/http.rs`, `den-stdlib-whatwg-fetch/src/lib.rs`

#### API: nothing to change

| den call site | 0.13.4 |
|---|---|
| `reqwest::get(name)` — `loader/http.rs:27`, `whatwg-fetch/src/lib.rs:150` | `pub async fn get<T: IntoUrl>(url: T) -> crate::Result<Response>` — `src/lib.rs:325` |
| `reqwest::header::CONTENT_TYPE` — `loader/http.rs:3` | `pub use http::header;` — `src/lib.rs:279` |
| `.headers()` — `loader/http.rs:32` | `src/async_impl/response.rs:68` |
| `.text()` — `loader/http.rs:78`, `whatwg-fetch:70` | `src/async_impl/response.rs:163` |
| `.bytes()` — `whatwg-fetch:24,41` | `src/async_impl/response.rs:290` |
| `.json::<serde_json::Value>()` — `whatwg-fetch:58` | `src/async_impl/response.rs:269` |
| `.status()`, `.url()` — `whatwg-fetch:89,98,108,121,135` | `src/async_impl/response.rs:56,111` |

One behavioural footnote: `Response::text()` (`src/async_impl/response.rs:163-175`) is now explicitly
documented as "if the `charset` feature is disabled the method will only attempt to decode the
response as UTF-8". `charset` is a default feature and 0.12 behaved the same; it only bites if you
turn defaults off (see below).

#### **The TLS feature remap — this is the real change**

`native-tls` still exists in 0.13, but the mapping inverted:

| | reqwest 0.12.9 `Cargo.toml` | reqwest 0.13.4 `Cargo.toml` |
|---|---|---|
| `default` | `["default-tls","charset","http2","macos-system-configuration"]` | `["default-tls","charset","http2","system-proxy"]` |
| `default-tls` | `["dep:hyper-tls","dep:native-tls-crate","__tls","dep:tokio-native-tls"]` → **native-tls** | `["rustls"]` → **rustls** |
| `native-tls` | `["default-tls"]` (an alias) | `["__native-tls","__native-tls-alpn"]` (independent) |
| `rustls` | n/a (`rustls-tls-*` family) | `["__rustls-aws-lc-rs","dep:rustls-platform-verifier","__rustls"]` |
| compression | `async-compression` | `tower-http/decompression-*` |
| macOS proxy | `macos-system-configuration` | `system-proxy` (cross-platform, `hyper-util/client-proxy-system`) |

den asks for `native-tls` **without** `default-features = false`
(`den-core/Cargo.toml:20-26`, `den-stdlib-whatwg-fetch/Cargo.toml:24-33`), so it now gets *both*
stacks compiled. Confirmed against the real lockfile:

```
$ cargo tree --offline -i aws-lc-rs -e normal
aws-lc-rs v1.18.0
└── rustls v0.23.43
    ├── hyper-rustls v0.27.3
    │   └── reqwest v0.13.4
    │       ├── den-core v0.4.0
    │       └── den-stdlib-whatwg-fetch v0.4.0
    ├── rustls-platform-verifier v0.7.0 └── reqwest v0.13.4
    └── tokio-rustls v0.26.1
```

`aws-lc-rs` needs cmake + a C compiler (and NASM on Windows) — the exact class of problem that got
`wabt` deleted. Runtime behaviour is *not* affected: `impl Default for TlsBackend`
(`$REG/reqwest-0.13.4/src/tls.rs:620-635`) still prefers native-tls when both are on —
`#[cfg(all(feature = "__native-tls", not(feature = "http3")))] { TlsBackend::NativeTls }`.
So this is pure build cost, but it is large.

Fix — set `default-features = false` and re-add what 0.12's default gave you:

`den-core/Cargo.toml` BEFORE:
```toml
reqwest = { workspace = true, features = [
    "stream",
    "deflate",
    "gzip",
    "brotli",
    "native-tls",
] }
```
AFTER:
```toml
reqwest = { workspace = true, default-features = false, features = [
    "stream",
    "deflate",
    "gzip",
    "brotli",
    "native-tls",
    "charset",       # was a reqwest default; Response::text() charset sniffing
    "http2",         # was a reqwest default
    "system-proxy",  # was `macos-system-configuration` in 0.12's default
] }
```
Same treatment for `den-stdlib-whatwg-fetch/Cargo.toml` (keep its extra
`multipart`, `json`, `cookies`).

**[verified by compile]** A scratch crate with exactly
`default-features = false, features = ["native-tls","charset","http2","system-proxy","stream","multipart","json","deflate","gzip","cookies","brotli"]`
plus den's actual call shape (`reqwest::get`, `headers().get(CONTENT_TYPE)`,
`status().canonical_reason()`, `url().to_string()`, `text()`) compiles clean, and
`cargo tree -i rustls` reports "nothing to print" / `aws-lc-rs` is absent from the graph. Only
`hyper-tls`/`native-tls`/`tokio-native-tls` are built.

Alternatively — if you'd rather ride the ecosystem default — drop `"native-tls"` and let reqwest use
rustls. That deletes the OpenSSL/Schannel dependency instead, at the cost of aws-lc-rs's C build.
Either choice is fine; building *both* is the thing to avoid.

### 4.6 typed-builder 0.20.0 → 0.23.2 — no change

`$REG/typed-builder-0.23.2/CHANGELOG.md:17-31` lists two breaking changes for 0.23.0:
1. `default` expressions that reference previous fields now receive them **as references**.
2. The `Optional` trait was removed (internal detail).

den uses only bare `#[builder(default)]` — `den-core/src/loader/mmap_script.rs:17`,
`den-core/src/loader/http.rs:17` — and undecorated fields in
`den-stdlib-wasm/src/{memory.rs:14, global.rs:85, table.rs:9}`. Neither breaking change applies.
(If you adopt the `#[builder(default = true)]` suggestion in §2, note that literal defaults are
unaffected by change 1 — only defaults that *read another field* are.)

0.23 is `edition = "2024"` (`CHANGELOG.md:19`), which is fine on rustc 1.97.

### 4.7 cfg-if 1.0.0 → 1.0.4 — `den-core/src/engine.rs:348` — no change (patch bump)

### 4.8 mime 0.3.17 — not bumped, no change

`den-core/src/loader/http.rs:2,41,45` (`mime::TEXT`, `mime::APPLICATION`, `mime::JAVASCRIPT`).

### 4.9 `den-core` declares `tokio` with **no features** but calls `rt` / `rt-multi-thread` APIs — **breaks every isolated build**

`den-core/Cargo.toml:28` is a bare `tokio.workspace = true`, and the workspace entry
(`Cargo.toml:56`) is `tokio = "1.53.1"` with no feature list. But `den-core` calls

```rust
tokio::task::block_in_place(move || Handle::current().block_on(task))
```

at `den-core/src/loader/http.rs:107` and `den-core/src/loader/mmap_script.rs:79`.
`block_in_place` is gated on tokio's `rt-multi-thread` feature and `runtime::Handle` on `rt`
(`$REG/tokio-1.53.1/src/task/blocking.rs:74`, inside `cfg_rt_multi_thread!` at `:3`; `Handle`
is re-exported at `src/runtime/mod.rs:614` inside `cfg_rt!` at `:538`).

It compiles today **only** because the root `den` package asks for
`tokio = { workspace = true, features = ["macros", "rt-multi-thread", "signal"] }`
(`Cargo.toml:88`) and Cargo unifies features across one build graph. Any build that does not pull
the `den` binary in fails — **observed, not theoretical**:

```
$ cargo check -p den-core --no-default-features
error[E0425]: cannot find function `block_in_place` in module `tokio::task`
  --> den-core/src/loader/http.rs:107:22
error[E0425]: cannot find function `block_in_place` in module `tokio::task`
  --> den-core/src/loader/mmap_script.rs:79:22
```

This is **pre-existing** (tokio has always gated these) and not caused by any bump — but it
invalidates the per-crate verification steps in §14, so fix it in the same manifest pass:

```toml
tokio = { workspace = true, features = ["rt", "rt-multi-thread"] }
```


---

## 5. `den-stdlib-console`

`colored 2.1.0 → 3.1.1` and `indexmap` are declared (`den-stdlib-console/Cargo.toml:15-16`) but
`den-stdlib-console/src/lib.rs` (311 lines) references **neither** — grep for `colored` across all
`*.rs` in the repo returns zero hits, grep for `indexmap` inside that file returns zero hits.
`FormatterBuilder` (`:200`) is hand-written, not `typed-builder`.

For the record, colored 3.0.0's only breaking change is the MSRV
(`$REG/colored-3.1.1/CHANGELOG.md:4-5`: *"**[BREAKING CHANGE]:** Upgrade MSRV to 1.80 and remove the
then unnecessary lazy_static dependency."*). So even if it were used, no source change.

**Action:** delete both lines from `den-stdlib-console/Cargo.toml`. Used deps are `rquickjs` and
`tracing` (`src/lib.rs:2,276,281,286,291`).

---

## 6. `den-stdlib-core`

### 6.1 base64 0.22.1 → 0.23.1 — `den-stdlib-core/src/lib.rs:12-17,30-37` — no change

`pub use crate::engine::general_purpose::STANDARD as BASE64_STANDARD;` at
`$REG/base64-0.23.1/src/prelude.rs:19` — `use base64::prelude::*; BASE64_STANDARD.encode(..)/.decode(..)`
is unchanged. `$REG/base64-0.23.1/RELEASE-NOTES.md:3-11` lists only additions for 0.23.0
(more preconfigured consts, richer `InvalidLastSymbol`, custom padding symbols, MSRV 1.71) plus:

> Added SIMD-accelerated engines behind the default-on `simd-unsafe` feature

`[features]` went from `default = ["std"]` (0.22.1) to `default = ["std", "simd-unsafe"]` (0.23.1).
Worth knowing: base64 now contains `unsafe` SIMD by default. `default-features = false` +
`features = ["std"]` restores the 0.22 behaviour if that matters. Also note den's separate
`base64-simd` feature (`den-stdlib-core/Cargo.toml:15` dep, `:25` feature) is now largely redundant with upstream's `simd-unsafe` —
candidate for deletion, but that's a judgement call, not a bump requirement.

### 6.2 `derive_more` `Deref` — see §1.5.

### 6.3 `base64` and `base64-simd` are mutually exclusive — `--all-features` does not build

Not a bump issue, but it breaks the feature-matrix commands in §14, so it belongs here.
`base64-simd` was **not** bumped (0.8.0 before and after); the API den uses is intact:
`base64_simd::STANDARD` (`$REG/base64-simd-0.8.0/src/lib.rs:133` — a free `const`, note it was an
*associated* const `Base64::STANDARD` in 0.7), `encode_to_string` (`:332`), `decode_to_vec` (`:343`).

The problem is den's own code shape. `den-stdlib-core/src/lib.rs` writes both back-ends as
`#[cfg]`-gated **tail blocks** in the same function (`:7-11` and `:12-17` in `btoa`, `:22-29` and
`:30-37` in `atob`). With exactly one feature on, the surviving block is the tail expression. With
**both** on, the first block becomes an expression statement and must be `()`:

```
$ cargo check -p den-stdlib-core --features base64-simd      # `base64` is a default feature
error[E0308]: mismatched types: expected `()`, found `Result<String, _>`
  --> den-stdlib-core/src/lib.rs:10:9
error[E0308]: mismatched types: expected `()`, found `Result<String, Error>`
  --> den-stdlib-core/src/lib.rs:25:9
```

So: never run `cargo check --all-features` on this workspace. To exercise the SIMD path use
`cargo check -p den-stdlib-core --no-default-features --features base64-simd`. (A real fix is
`#[cfg(all(feature = "base64-simd", not(feature = "base64")))]` on the first block, or picking one
back-end and deleting the other — see the §6.1 remark that `simd-unsafe` makes `base64-simd`
redundant anyway.)


---

## 7. `den-stdlib-crypto`

### 7.1 rand 0.8.5 → 0.10.2 — `den-stdlib-crypto/src/lib.rs:1,32` — **BREAKING**

Three renames across 0.9 and 0.10 hit this file:

* `$REG/rand-0.10.2/CHANGELOG.md:145` (0.9.0): *"Rename fn `rand::thread_rng()` to `rand::rng()` and
  remove from the prelude"*.
* `$REG/rand-0.10.2/CHANGELOG.md:44` (0.10.0): *"Rename `Rng` -> `RngExt` as upstream `rand_core` has
  renamed `RngCore` -> `Rng`"*.
* `$REG/rand-0.10.2/src/lib.rs:59`: `pub use rand_core::{CryptoRng, Rng, SeedableRng, TryCryptoRng, TryRng};`
  — **`RngCore` is not re-exported at the crate root at all**, so `use rand::RngCore;` is an
  unresolved import, not just a deprecation. (`rand_core::RngCore` does still exist at
  `$REG/rand_core-0.10.1/src/lib.rs:257` but is `#[deprecated(since = "0.10.0", note = "use `Rng` instead")]`.)
* `fn fill_bytes(&mut self, dst: &mut [u8])` now lives on `rand_core::Rng`
  (`$REG/rand_core-0.10.1/src/lib.rs:62`).
* `pub fn rng() -> ThreadRng` at `$REG/rand-0.10.2/src/rngs/thread.rs:201`; the `thread_rng` feature
  is on by default (`$REG/rand-0.10.2/Cargo.toml` `default = ["std","std_rng","sys_rng","thread_rng"]`).

BEFORE (`den-stdlib-crypto/src/lib.rs:1`, `:30-32`):
```rust
use rand::RngCore;
…
        let dest = array.as_bytes().unwrap();
        let dest = unsafe { core::slice::from_raw_parts_mut(dest.as_ptr() as *mut u8, dest.len()) };
        rand::thread_rng().fill_bytes(dest);
```

AFTER — the laziest form drops the import entirely.
`$REG/rand-0.10.2/src/lib.rs:314-316` gives a free function:
```rust
#[cfg(feature = "thread_rng")]
#[inline]
#[track_caller]
pub fn fill<T: Fill>(dest: &mut [T]) { Fill::fill_slice(dest, &mut rng()) }
```
and `impl Fill for u8 { fn fill_slice(this, rng) { rng.fill_bytes(this) } }`
(`$REG/rand-0.10.2/src/rng.rs:338-342`) — i.e. byte-for-byte the same work. So:
```rust
// (delete `use rand::RngCore;` from line 1)
…
        let dest = array.as_bytes().unwrap();
        let dest = unsafe { core::slice::from_raw_parts_mut(dest.as_ptr() as *mut u8, dest.len()) };
        rand::fill(dest);
```

If you prefer to keep the explicit RNG handle, the trait-based form is:
```rust
use rand::Rng;      // this is the OLD `RngCore`
…
        rand::rng().fill_bytes(dest);
```
**[verified by compile]** both forms build against rand 0.10.2 on edition 2024.

Panic semantics: `rand::fill`/`rng()` are `#[track_caller]` and panic if the OS RNG fails during
initial seeding (`$REG/rand-0.10.2/src/rngs/thread.rs:198-200`). Same as `thread_rng()` in 0.8.

### 7.2 uuid 1.11.0 → 1.24.1 — `den-stdlib-crypto/src/lib.rs:3,39` — no change

`pub fn new_v4() -> Uuid` at `$REG/uuid-1.24.1/src/v4.rs:33`. The `fast-rng` feature still exists
(`$REG/uuid-1.24.1/Cargo.toml`, `fast-rng = ["rng","dep:rand"]`) and is documented at
`src/lib.rs:108`. (1.24 also added `rng-rand`/`rng-getrandom` as more explicit spellings; `fast-rng`
is not deprecated.)

### 7.3 `den-stdlib-crypto` dead deps

`den-stdlib-crypto/src/lib.rs` imports exactly `rand`, `rquickjs`, `uuid` (lines 1-3) and `indexmap`
+ `rquickjs` inside the module (lines 44-45). The manifest additionally declares
`den-utils`, `derivative`, `derive_more`, `either`, `getset`, `tracing`, `typed-builder`, `tokio` —
all **unused**. Deleting them removes `getset` from the graph and makes this crate's build near-instant.

---

## 8. `den-stdlib-regex` — the whole crate is empty

`den-stdlib-regex/src/lib.rs` is **1 line** (blank). The manifest declares `colored 3.1.1`,
`pcre2 0.2.11` and `rquickjs`. `pcre2` drags in `pcre2-sys`, which vendors and compiles the PCRE2 C
library — a C-toolchain dependency for zero Rust code, in a crate that is not even a member
dependency of `den-core` (grep `den-stdlib-regex` in `den-core/Cargo.toml` → absent).

**Action:** either delete the crate from `Cargo.toml:11` `members`, or at minimum strip all three
deps. Nothing in the bump list needs `pcre2 0.2.11` or `colored 3.1.1` to be researched further.

---

## 9. `den-stdlib-sqlite` — rusqlite 0.32.1 → 0.39.0

**No source change required.** Verified item by item against the two sources (rusqlite ships no
CHANGELOG in the crate, so this is a direct signature diff):

| den call site | 0.32.1 | 0.39.0 | verdict |
|---|---|---|---|
| `Connection::open_in_memory()` `:29` | `src/lib.rs:457` `-> Result<Connection>` | `src/lib.rs:439` `-> Result<Self>` | same type |
| `Connection::open(path)` `:38` | `src/lib.rs:446` | `src/lib.rs:428` | same |
| `conn.prepare(&sql)` `:52,79` | `src/lib.rs:770` `-> Result<Statement<'_>>` | `src/lib.rs:772` | same |
| `conn.close()` `:98` | `src/lib.rs:795` `-> Result<(), (Connection, Error)>` | `src/lib.rs:803` `-> Result<(), (Self, Error)>` | same type; `\|(_, e)\|` destructure still fine |
| `stmt.parameter_count()` `:113,138` | `src/statement.rs:499` | `src/statement.rs:520` | same |
| `stmt.parameter_index(&str)` `:121` | `src/statement.rs:416` `-> Result<Option<usize>>` | `src/statement.rs:440` | same |
| `stmt.raw_bind_parameter(idx, v)` `:157-172` | `src/statement.rs:547` `<T: ToSql>(&mut self, one_based_col_index: usize, …)` | `src/statement.rs:567` `<I: BindIndex, T: ToSql>` | **generalised**, and `impl BindIndex for usize` at `src/bind.rs:23-29` → den's `usize` indices still resolve with no annotation |
| `stmt.raw_execute()` `:65` | `src/statement.rs:572` | `src/statement.rs:592` | same |
| `stmt.raw_query()` `:187` | `src/statement.rs:589` `-> Rows<'_>` | `src/statement.rs:609` | same |
| `stmt.column_count()` `:184` | `src/column.rs:55` | `src/column.rs:97` | same |
| `rows.next()` `:188` | `src/row.rs:39` `-> Result<Option<&Row<'stmt>>>` | `src/row.rs:39` | same |
| `row.get_ref(i)` `:193` | `src/row.rs:319` `-> Result<ValueRef<'_>>` | `src/row.rs:320` | same |
| `ValueRef::{data_type,as_i64,as_f64,as_str,as_blob}` `:211-239` | `src/types/value_ref.rs:26,41,63,85,111` | `src/types/value_ref.rs:26,41,63,85,109` | identical signatures |
| `rusqlite::types::Type` `:212-235` | `src/types/mod.rs:115-126` — `Null/Integer/Real/Text/Blob` | `src/types/mod.rs:113-124` — same five | no new variants |
| `rusqlite::types::Null` `:171` | present | `src/types/mod.rs:108` `pub struct Null;` | same |
| `rusqlite::types::FromSqlError` `:247` | `src/types/from_sql.rs:8` | `src/types/from_sql.rs:10` — gained `Utf8Error(Utf8Error)` | den never matches on it (only `#[derive(From)]` into `QueryRowError`) → fine |

Build-side: `libsqlite3-sys` 0.30.1 → 0.37.0 with `bundled` (newer amalgamation). rustyline 18
pins the same `rusqlite 0.39.0` + `bundled` (`$REG/rustyline-18.0.1/Cargo.toml:157-163`), so there
is exactly one `libsqlite3-sys` in the graph — confirmed in `Cargo.lock` (`libsqlite3-sys 0.37.0`,
single entry).

`derive_more` `Deref` import fix — §1.5. Edition-2024 `if let` temporary scope — §11.5.

---

## 10. `den-stdlib-wasm`

### 10.1 `wabt 0.10` → `wat 1.257.1` — `den-stdlib-wasm/src/lib.rs:102-112` — **BREAKING**

`wabt::wat2wasm<S: AsRef<[u8]>>(source: S) -> Result<Vec<u8>, wabt::Error>`
(`$REG/wabt-0.10.0/src/lib.rs:1038-1041`). `wabt` is no longer in `Cargo.lock` and
`den-stdlib-wasm/Cargo.toml:22` now has `wat = "1.257.1"`.

Replacement API (`$REG/wat-1.257.1/src/lib.rs`):

```
pub fn parse_str(wat: impl AsRef<str>) -> Result<Vec<u8>>     // :193
pub fn parse_bytes(bytes: &[u8]) -> Result<Cow<'_, [u8]>>     // :147  (accepts binary OR text)
pub fn parse_file(file: impl AsRef<Path>) -> Result<Vec<u8>>  // :104
pub type Result<T> = std::result::Result<T, Error>;           // :369
pub struct Error { kind: Box<ErrorKind> }                     // :381
impl fmt::Display for Error                                   // :424
impl std::error::Error for Error                              // :445
```

`Error` is `Debug + Display + std::error::Error`. Its `Display` includes the source snippet, because
`parse_str` routes through `Error::cvt` which calls `err.set_text(contents)` (`:398-407`).

BEFORE (`den-stdlib-wasm/src/lib.rs:101-112`):
```rust
    #[rquickjs::function]
    pub fn wat2wasm(source: String, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        match wabt::wat2wasm(source) {
            Ok(data) => TypedArray::new(ctx, data),
            Err(e) => {
                Err(Exception::throw_internal(
                    &ctx,
                    &format!("wat2wasm error: {e}"),
                ))
            }
        }
    }
```
AFTER:
```rust
    #[rquickjs::function]
    pub fn wat2wasm(source: String, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        match wat::parse_str(source) {
            Ok(data) => TypedArray::new(ctx, data),
            Err(e) => {
                Err(Exception::throw_internal(
                    &ctx,
                    &format!("wat2wasm error: {e}"),
                ))
            }
        }
    }
```

**[verified by compile + run]** against `wat 1.257.1` on edition 2024. Rendered errors:

```
wat2wasm error: expected `)`
     --> <anon>:1:27
      |
    1 | (module (func (export "f")
      |                           ^
```
```
wat2wasm error: duplicate func identifier
     --> <anon>:1:25
      |
    1 | (module (func $a) (func $a))
      |                         ^
```

i.e. the JS exception message becomes **multi-line**. That's strictly better diagnostics but if the
single-line shape matters, use `format!("wat2wasm error: {e:?}")`… actually no — `Debug` is derived
and unhelpful. Keep `{e}`.

#### Behaviour regression: `wat` does not validate

`wabt`'s doc for `wat2wasm` (`$REG/wabt-0.10.0/src/lib.rs:1044-1046`) says: *"If wasm source is valid
wasm binary will be returned in the vector. Returned binary is **validated** and can be executed."*
`wat` only parses, resolves names, and encodes.

**[verified by run]** `wat::parse_str("(module (func (export \"f\") i32.const 1))")` — a module whose
function leaves an `i32` on the stack with no declared result — returns `Ok(33 bytes)`. wabt would
have rejected it.

If `WebAssembly.wat2wasm()` should keep rejecting invalid modules, den already has the validator it
needs one screen up — `wasmtime::Module::validate(&engine, buf)` at `den-stdlib-wasm/src/lib.rs:72`
inside `validate_inner`. Minimal patch:

```rust
        let engine = ctx.userdata::<crate::engine::Engine>().unwrap();
        match wat::parse_str(source) {
            Ok(data) if wasmtime::Module::validate(&engine, &data).is_ok() => {
                TypedArray::new(ctx, data)
            }
            Ok(_) => Err(Exception::throw_internal(&ctx, "wat2wasm error: invalid module")),
            Err(e) => Err(Exception::throw_internal(&ctx, &format!("wat2wasm error: {e}"))),
        }
```
Or accept the regression and let `WebAssembly.compile()`/`instantiate()` reject it downstream —
which is arguably closer to how the real toolchain behaves. Flag for the maintainer; it's a
one-line decision either way.

`wat 1.257.1` is pure Rust (`[dependencies.wast] version = "257.0.1", features = ["wasm-module"],
default-features = false`), `edition = "2024"`, `rust-version = "1.85.0"`, `default = ["component-model"]`.
No cmake, no C++ — which was the point of the swap.

### 10.2 `getset` was removed from the manifest but is still used — **BREAKING**

`den-stdlib-wasm/Cargo.toml` no longer lists `getset` (it was `getset = "0.1.3"` before the bump),
but `den-stdlib-wasm/src/module.rs:5` is `use getset::Getters;` and `:10,:14` use it:

```rust
#[derive(Trace, JsLifetime, Getters, Deref, DerefMut, From, Into, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    #[getset(get)]
    pub(crate) inner: wasmtime::Module,
}
```

The generated accessor is `fn inner(&self) -> &wasmtime::Module`. **It is never called** — grep for
`.inner()` across the whole tree returns zero hits, and `Module::imports()`/`exports()` at
`module.rs:56,70` reach `wasmtime::Module` through the `Deref` derive, not through `inner()`.

**Fix (lazy, and the right one): delete the derive.**
BEFORE:
```rust
use std::clone::Clone;

use derive_more::{derive::DerefMut, Deref, From, Into};
use either::Either;
use getset::Getters;
use indexmap::{indexmap, IndexMap};
…
#[derive(Trace, JsLifetime, Getters, Deref, DerefMut, From, Into, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    #[getset(get)]
    pub(crate) inner: wasmtime::Module,
}
```
AFTER:
```rust
use std::clone::Clone;

use derive_more::{derive::DerefMut, Deref, From, Into};
use either::Either;
use indexmap::{indexmap, IndexMap};
…
#[derive(Trace, JsLifetime, Deref, DerefMut, From, Into, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Module,
}
```

(If you'd rather keep it: re-add `getset = "0.1.7"`. getset 0.1.3 → 0.1.7 is purely additive —
diffing the `proc_macro_derive` lists shows 0.1.7 *adds* `CloneGetters`
(`$REG/getset-0.1.7/src/lib.rs:312`) and `WithSetters` (`:372`); `Getters` with `attributes(get,
with_prefix, getset)` at `:297` is unchanged. But re-adding a dep for an uncalled accessor is
backwards.)

### 10.3 `derivative` was removed from `den-stdlib-wasm/Cargo.toml` — correctly

grep for `derivative`/`Derivative` under `den-stdlib-wasm/` → zero hits. Nothing to do.

### 10.4 `anyhow` is declared but unused here

`den-stdlib-wasm/Cargo.toml:14` has `anyhow = "1.0.104"`. The only `anyhow` reference in the tree is
`den-transpiler-oxc/src/lib.rs:162` (`SwcParse(anyhow::Error)`), and *that* crate's new manifest does
**not** declare `anyhow`. Both sides are wrong; the transpiler side belongs to doc 04. From this
doc's perspective: delete `anyhow` from `den-stdlib-wasm/Cargo.toml`.

### 10.5 The `unsafe impl Send/Sync` at `den-stdlib-wasm/src/instance.rs:56-57` — assessment

```rust
#[derive(Clone, Copy, From, Deref, DerefMut)]
struct DangerouslyImplementSync<T>(T);
unsafe impl<T> Send for DangerouslyImplementSync<T> {}
unsafe impl<T> Sync for DangerouslyImplementSync<T> {}
```

**Edition 2024 does not touch this.** The 2024 unsafe-related changes are:
`unsafe_op_in_unsafe_fn` (bodies of `unsafe fn`), `unsafe extern` blocks, `unsafe` attributes
(`#[unsafe(no_mangle)]` et al.), and `static_mut_refs`. `unsafe impl Trait for Type` is unchanged
syntax and unchanged semantics. Compile-wise this is a no-op for the migration.

Soundness-wise it is unconditionally unsound as written (`T` is unbounded, so this asserts *any* `T`
is `Send + Sync`), and it is only "fine" because of the surrounding contract the comment at `:62-65`
describes: the value is a `Persistent<Function>` restored on the same thread that owns the JS
context, wrapped in a `Mutex` purely to satisfy wasmtime's `Send + Sync` closure bound. That is a
pre-existing design decision, orthogonal to every bump in this doc. If it gets revisited, the
narrow fix is to make the type non-generic (`struct SendPersistentFunction(Persistent<Function>)`)
so the assertion is scoped to the one type it was reasoned about. **Out of scope; do not change it
as part of this migration.**

### 10.6 Edition-2024 match-ergonomics break — `den-stdlib-wasm/src/lib.rs:66-70` — see §11.2.

---

## 11. Edition 2021 → 2024

`Cargo.toml:19-21` now has `edition = "2024"` / `rust-version = "1.97"`; every member inherits via
`edition.workspace = true` + `rust-version.workspace = true`. Toolchain is `rustc 1.97.1`,
`cargo 1.97.1`, `rust-toolchain.toml` channel `stable`.

The list below is exhaustive for den — each item is either "found, here's the fix" or "confirmed
absent, here's the grep".

### 11.1 `unsafe_op_in_unsafe_fn` is now a hard error — **not applicable**

den has **no `unsafe fn`** anywhere. `grep -rn "unsafe" --include='*.rs'` over the whole tree yields
exactly five live hits and one comment:

```
den-stdlib-crypto/src/lib.rs:31   unsafe { core::slice::from_raw_parts_mut(...) }   // unsafe block
den-stdlib-text/src/lib.rs:140    unsafe { core::slice::from_raw_parts_mut(...) }   // unsafe block
den-stdlib-wasm/src/memory.rs:77  // let val = unsafe {                             // commented out
den-stdlib-wasm/src/store.rs:16   unsafe impl<'js> JsLifetime<'js> for Store<'js>
den-stdlib-wasm/src/instance.rs:56  unsafe impl<T> Send for DangerouslyImplementSync<T> {}
den-stdlib-wasm/src/instance.rs:57  unsafe impl<T> Sync for DangerouslyImplementSync<T> {}
```

Two `unsafe { }` blocks in safe fns (unaffected) and three `unsafe impl`s (unaffected).
The one *new* `unsafe` block the migration introduces is the fmmap one in §4.1.

### 11.2 Match ergonomics reservations (RFC 3627) — **1 real break** 🔴

Rust 2024 forbids explicit `ref` / `ref mut` / `mut` binding modifiers inside a pattern whose default
binding mode is not `move` — i.e. when matching a reference against a non-reference pattern.

**[verified by compile]** — a repro of den's exact shape on edition 2024 with rustc 1.97.1:

```
error: cannot explicitly borrow within an implicitly-borrowing pattern
 --> src/lib.rs:6:14
  |
6 |         E::L(ref x) => x.as_slice(),
  |              ^^^ explicit `ref` binding modifier not allowed when implicitly borrowing
  |
note: matching on a reference type with a non-reference pattern implicitly borrows the contents
help: remove the unnecessary binding modifier
6 -         E::L(ref x) => x.as_slice(),
6 +         E::L(x) => x.as_slice(),
```

All eleven `ref` patterns in den (10 in `match` arms + 1 in a closure), triaged by scrutinee type:

| Site | Scrutinee | Verdict |
|---|---|---|
| **`den-stdlib-wasm/src/lib.rs:67,68`** | `buffer_source: &Either<TypedArray<'js,u8>, ArrayBuffer<'js>>` (`:62`) — **a reference** | 🔴 **hard error** |
| `den-stdlib-wasm/src/module.rs:27,28` | `buffer_source: Either<…>` by value (`:22`) | ✅ ok |
| `den-stdlib-io/src/lib.rs:46,47,48` | `buf: Either<String, Either<Vec<u8>, TypedArray<'js,u8>>>` by value (`:43`) | ✅ ok |
| `den-stdlib-text/src/lib.rs:74,75` | `buffer: Either<…>` by value (moved out of `Option` at `:66-67`) | ✅ ok |
| `den-core/src/loader/http.rs:40` | `mime_type: Option<Mime>` — owned local | ✅ ok |
| `den-stdlib-wasm/src/instance.rs:255` | `.map(\|ref x\| …)` over an iterator of owned `ValType` | ✅ ok (redundant, but legal) |

**[verified by compile]** the "ok" shape (owned scrutinee + `ref`) builds clean on edition 2024.

`den-stdlib-wasm/src/lib.rs` BEFORE (`:61-73`):
```rust
    fn validate_inner<'js>(
        buffer_source: &Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: &crate::engine::Engine,
    ) -> Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        Ok(wasmtime::Module::validate(&engine, buf).is_ok())
    }
```
AFTER:
```rust
    fn validate_inner<'js>(
        buffer_source: &Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: &crate::engine::Engine,
    ) -> Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(x) => x.as_bytes(),
            Either::Right(x) => x.as_bytes(),
        }
        .unwrap();

        Ok(wasmtime::Module::validate(&engine, buf).is_ok())
    }
```
(`x` binds as `&TypedArray<'js, u8>` / `&ArrayBuffer<'js>` exactly as before — the implicit borrow
gives the identical type, which is why the compiler calls the modifier "unnecessary".)

The other half of RFC 3627 — `&`/`&mut` patterns may not match against an *inherited* reference —
does not fire: den's only reference pattern is `.find(|&e| extension == e)`
(`den-core/src/loader/mmap_script.rs:50`), where the scrutinee is a real `&&String` from
`Iterator::find`, not an inherited one.

### 11.3 RPIT lifetime capture (`impl Trait` captures all in-scope lifetimes) — **not applicable**

`grep -rn -- "-> impl"` over `**/*.rs` in the den tree returns **zero** hits (the only matches are
inside `docs/research/*.md`). den's `Loader`/`Resolver` trait impls
(`den-core/src/loader/http.rs:25`, `den-core/src/loader/mmap_script.rs:40`,
`den-core/src/resolver/http.rs:12`) are **synchronous** — they return
`rquickjs::Result<Module<'js, Declared>>` / `Result<String>` and drive their inner futures with
`tokio::task::block_in_place(move || Handle::current().block_on(task))`
(`loader/http.rs:107`, `loader/mmap_script.rs:79`). No `impl Future`, no RPIT, nothing to add
`+ use<>` to.

`async fn` in inherent impls (den has ~35 of them) already captured all in-scope lifetimes in
edition 2021 — that rule did not change.

### 11.4 `gen` keyword reservation — **not applicable**

`grep -rnE "\bgen\b" --include='*.rs'` → zero hits. (Relevant to `rand` only in that upstream renamed
`Rng::gen` → `Rng::random` for this reason — see `$REG/rand-0.10.2/CHANGELOG.md:151`.)

### 11.5 Tail-expression / `if let` temporary scope — **audit, no change expected**

Rust 2024 drops temporaries in a block's tail expression *before* the block's locals, and drops
`if let` scrutinee temporaries *before* the `else` branch.

The one shape in den that involves a scope guard in a scrutinee is
`den-stdlib-sqlite/src/lib.rs:51` and `:78`:

```rust
        if let Some(conn) = self.conn.borrow().deref() {
            …
        } else {
            Err(Exception::throw_internal(&ctx, "already closed"))
        }
```

Under 2024 the `Ref<'_, Option<rusqlite::Connection>>` temporary is dropped at the end of the
then-block (it must live that long — `conn` borrows from it) and before the `else` block. The
`else` block does not re-borrow the `RefCell`, and the function's return value
(`Result<usize>` / `Result<Option<Array>>`) does not borrow the guard, so behaviour is identical.
It's strictly safer under 2024 — an `else` branch that took `borrow_mut()` would have panicked in
2021 and now would not.

**[verified by compile]** the exact shape (`Arc<RefCell<Option<String>>>` + `if let Some(x) =
self.0.borrow().deref() { … } else { … }`) builds clean on edition 2024.

`den-stdlib-sqlite/src/lib.rs:97` (`if let Some(conn) = self.conn.borrow_mut().take()`) takes the
value out, so no guard is held across the branch at all.

### 11.6 `unsafe extern` blocks — **not applicable**

`grep -rn 'extern "C"'` → zero hits. den links no C directly (`libsqlite3-sys`, `rquickjs-sys`,
`pcre2-sys`, `mimalloc` do it in their own crates, at their own editions).

### 11.7 `unsafe` attributes (`#[unsafe(no_mangle)]`, `#[unsafe(export_name)]`, `#[unsafe(link_section)]`) — **not applicable**

`grep -rnE "no_mangle|export_name|link_section"` → zero hits.

### 11.8 `static mut` references — **not applicable**

`grep -rn "static mut"` → zero hits. The only `static` is
`src/main.rs:11` `static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;` (immutable).

### 11.9 Never-type fallback (`!` instead of `()`) — **audit, no change expected**

The candidate shape in den is `den-stdlib-wasm/src/instance.rs:83-93`:

```rust
                                            if array.len() != results.len() {
                                                Err(Exception::throw_internal(
                                                    ctx,
                                                    &format!(…),
                                                ))?
                                            }
```

Here `Err(_)?` has an unconstrained `Ok` type, so fallback decides it. Under 2024 it becomes `!`
rather than `()`, and `!` coerces to the `()` the `if` block requires — no trait bound is involved,
so neither `dependency_on_unit_never_type_fallback` (warn since 1.83) nor
`never_type_fallback_flowing_into_unsafe` (deny in 2024) applies.

**[verified by compile]** the exact shape
(`if n > 3 { Err(std::fmt::Error)? } Ok(())` in a `-> Result<(), std::fmt::Error>` fn) builds clean
on edition 2024 with no warnings.

`den-stdlib-sqlite/src/lib.rs:164` (`unimplemented!("missing case for number")` as a match arm) is a
`!`→concrete coercion with a known target type, not fallback. Fine.

### 11.10 New prelude (`Future`, `IntoFuture`) — **not applicable**

Edition 2024 adds `core::future::{Future, IntoFuture}` to the prelude. Ambiguity only arises when a
glob import brings a *different* item of the same name. den's glob imports:

* `src/app.rs:2` — `use futures::prelude::*;`. `$REG/futures-0.3.34/src/lib.rs:232` re-exports
  `crate::future::{self, Future, TryFuture}`, and `futures::future::Future` **is**
  `core::future::Future` (re-exported, not redefined) → same item, no ambiguity. `IntoFuture` is not
  in `futures::prelude`.
* `rquickjs::prelude::*` — `den-stdlib-text/src/lib.rs:7`, `den-stdlib-sqlite/src/lib.rs:10`,
  `den-stdlib-wasm/src/{memory.rs:3, instance.rs:6, utils.rs:2}`.
  `$REG/rquickjs-core-0.12.2/src/lib.rs:76-95` exports `Ctx`, `Coerced`, `FromAtom`, `FromIteratorJs`,
  `FromJs`, `IntoAtom`, `IntoJs`, `IteratorJs`, `List`, `Exhaustive`, `Flat`, `Func`, `FuncArg`,
  `IntoArg`, `IntoArgs`, `MutFn`, `OnceFn`, `Opt`, `Rest`, `This`, `CatchResultExt`,
  `ThrowResultExt`, `JsLifetime`, `Async`, `Promise`, `Promised`, `MultiWith`. **No `Future`, no
  `IntoFuture`.**
* `base64::prelude::*` — `den-stdlib-core/src/lib.rs:14,32`. No `Future`.

No file imports two conflicting globs.

### 11.11 `Box<dyn Error>` — **not applicable**

`grep -rn "Box<dyn"` → zero hits.

### 11.12 Cargo-side edition-2024 effects

* **Resolver.** The root `Cargo.toml` has both `[workspace]` and `[package]`, so the workspace
  resolver is inferred from the root package's edition → **resolver "3"** (MSRV-aware). With
  `rust-version = "1.97"` set on every member and a committed `Cargo.lock`, this changes nothing
  today; it will make future `cargo update` refuse crates that demand a newer rustc. `cargo tree`
  runs clean with no resolver warning.
* **`dep:` / weak-dep syntax.** den already uses `dep:base64`, `dep:base64-simd`, `dep:mimalloc`,
  `dep:console-subscriber`, `dep:den-transpiler-oxc`, `dep:wasmtime`, `dep:wasmi`, and weak deps
  `den-transpiler-oxc?/typescript` (`den-core/Cargo.toml:56-62`). That syntax is stable since Cargo
  1.60 and is **unrelated** to the edition — no change.
* **rustfmt.** `.rustfmt.toml:1` was changed to `edition = "2024"`. rustfmt derives
  `style_edition` from `edition` when the former is unset, so **the 2024 style edition is now
  active** and `cargo fmt` will produce a repo-wide reformat (notably `use` ordering and
  version-sort). Expect a large mechanical diff — land it as its own commit *after* the code fixes,
  not interleaved. Also note the config uses nightly-only options
  (`format_strings`, `group_imports`, `imports_granularity`, `format_macro_matchers`,
  `normalize_comments`, `wrap_comments`, `reorder_impl_items`, `struct_field_align_threshold`,
  `inline_attribute_width`, `format_code_in_doc_comments`), so formatting still requires
  `cargo +nightly fmt`.

---

## 12. Dead / mis-declared dependencies

Verified by grepping every `*.rs` under the workspace (excluding `target/`).

### 12.1 Confirmed dropped, zero usage — nothing to do

| Dep | Old declaration | Usage |
|---|---|---|
| `phf` | was `phf = "0.11.2"` in `[workspace.dependencies]` | `grep -rn "phf" --include='*.rs'` → **0 hits**. Correctly removed. (`phf 0.14.0` still appears in `Cargo.lock` — transitive, not den's.) |
| `log` | was `log = "0.4.22"` in `[workspace.dependencies]` | `grep -rnE "\blog::\|use log\b\|log!\("` → **0 hits**. den logs through `tracing` (`den-stdlib-console/src/lib.rs:276-291`). Correctly removed. (`log 0.4.33` in `Cargo.lock` is transitive.) |
| `wabt` | was `wabt = "0.10.0"` in `den-stdlib-wasm` | replaced — §10.1. Absent from `Cargo.lock`. |
| `sourcemap` / `swc_*` | `den-transpiler-swc` | doc 04 |

### 12.2 Still declared, still unused — recommend deleting

| Crate | Unused dep | Evidence |
|---|---|---|
| `den-stdlib-console` | `colored 3.1.1`, `indexmap` | `src/lib.rs` (311 lines) references neither |
| `den-stdlib-regex` | `colored 3.1.1`, `pcre2 0.2.11`, `rquickjs` | `src/lib.rs` is 1 blank line |
| `den-stdlib-crypto` | `den-utils`, `derivative`, `derive_more`, `either`, `getset 0.1.7`, `tracing`, `typed-builder`, `tokio` | `src/lib.rs` imports only `rand`, `rquickjs`, `uuid`, `indexmap` |
| `den-stdlib-whatwg-fetch` | `getset 0.1.7`, `indexmap`, `tracing`, `typed-builder`, `tokio`, `either` | `src/lib.rs:1-6` imports only `den_utils`, `derivative`, `derive_more`, `rquickjs`; plus `reqwest`, `serde_json` |
| `den-stdlib-wasm` | `anyhow 1.0.104` | §10.4 |
| workspace root | `thiserror 2.0.20` in `[workspace.dependencies]` | no member declares `thiserror.workspace = true`, and `grep -rn "thiserror" --include='*.rs'` → 0 hits. den uses `derive_more::{Display, Error, From}` instead |

Deleting the `den-stdlib-regex` deps alone removes the `pcre2-sys` C build from the workspace.

### 12.3 Declared-nowhere but used — must be fixed

| Crate | Missing dep | Site |
|---|---|---|
| `den-stdlib-wasm` | `getset` | `src/module.rs:5` — §10.2 (recommendation: delete the usage, not re-add the dep) |
| `den-transpiler-oxc` | `anyhow`, `sourcemap` | `src/lib.rs:6,162` — the file is still verbatim SWC code; belongs to doc 04 |

---

## 13. No-change confirmations (grouped, for completeness)

| Dep | Bump | Why nothing changes |
|---|---|---|
| `indexmap` | 2.7.0 → 2.14.0 | `$REG/indexmap-2.14.0/RELEASES.md:1-60` — additive only (`pop_if`, `insert_sorted_by`, `extract_if`, `get_disjoint_mut`, more `const` `Slice` methods) + MSRV 1.85. den uses `indexmap!` and `IndexMap` only |
| `either` | 1.13.0 → 1.18.0 | semver-stable 1.x; den uses `Either::{Left,Right}` construction/matching only |
| `delegate-attr` | 0.3.0 → 0.3.1 | `diff` of `src/lib.rs`: a syn-2 `ImplItem` modifiers refactor (`defaultness` → `modifiers`, adds `polarity`). No user-facing surface |
| `cfg-if` | 1.0.0 → 1.0.4 | patch; `cfg_if::cfg_if!` at `den-core/src/engine.rs:348` |
| `tokio` | 1.42.0 → 1.53.1 | `$REG/tokio-1.53.1/Cargo.toml:13-14` still `edition 2021`, `rust-version 1.71`. No breaking entries in `CHANGELOG.md` for anything den uses (`block_in_place`, `Handle::block_on`, `spawn`, `signal::ctrl_c`, `mpsc::unbounded_channel`, `yield_now`, `RwLock`, `#[tokio::main]`, `#[tokio::test]`) |
| `tokio-util` | 0.7.13 → 0.7.19 | `CancellationToken::{child_token, cancelled, run_until_cancelled}` at `$REG/tokio-util-0.7.19/src/sync/cancellation_token.rs:204,300` (and `cancelled` nearby) — unchanged |
| `futures` | 0.3.31 → 0.3.34 | patch line; only `.then()` used |
| `serde` / `serde_json` | 1.0.216 → 1.0.229 / 1.0.133 → 1.0.151 | patch line. serde 1.0.229 splits out `serde_core` internally; no source effect. `den-utils/src/serde_json.rs` uses `Value`, `Map`, `json!` |
| `encoding_rs` | 0.8.35 (**not bumped**) | `den-stdlib-text/Cargo.toml:16` is unchanged from before; `Encoding::for_label`, `new_decoder{,_without_bom_handling}`, `DecoderResult` all as-is |
| `tracing` | 0.1.41 → 0.1.44 | patch; only the `debug!`/`info!`/`warn!`/`error!` macros are used |
| `mime` | 0.3.17 (not bumped) | unchanged |
| `getset` | 0.1.3 → 0.1.7 | additive (`CloneGetters`, `WithSetters`); and den should stop using it — §10.2 |
| `colored` | 2.1.0 → 3.1.1 | 3.0.0's only breaking change is MSRV 1.80 (`CHANGELOG.md:4-5`); and den doesn't use it — §5 |
| `pcre2` | 0.2.9 → 0.2.11 | `den-stdlib-regex/src/lib.rs` is empty — §8 |

---

## 14. Suggested commit order

1. **`derive_more` trait imports** — 3 files, `use std::ops::Deref;`. Smallest, unblocks nothing else.
2. **`rand` in `den-stdlib-crypto`** — 2 lines.
3. **`wat` in `den-stdlib-wasm`** + delete the `Getters` derive + drop `anyhow`.
4. **fmmap `unsafe { … }`** in `den-core/src/loader/mmap_script.rs`.
5. **rustyline** rewrite of `src/repl.rs:19-27`.
6. **Edition-2024 `ref` pattern** in `den-stdlib-wasm/src/lib.rs:67-68`.
7. **reqwest `default-features = false`** in the two manifests (build-cost only; separable).
8. **Dead-dep pruning** (§12.2) + **`den-core` tokio features** (§4.9) — pure manifest commit.
9. *(optional, separate)* **derivative removal** (§2).
10. *(last)* `cargo +nightly fmt` for the 2024 style edition.

Verification after each:

```bash
cargo check --workspace --all-targets
cargo check -p den-stdlib-wasm --no-default-features --features wasmi   # wasm backend matrix
cargo check -p den --no-default-features                                # feature-gate matrix
cargo check -p den-core --no-default-features                           # needs the §4.9 tokio fix
cargo check -p den-stdlib-core --no-default-features --features base64-simd   # NOT --all-features, see §6.3
cargo tree -i aws-lc-rs                                                 # must be empty after step 7
cargo clippy --workspace --all-targets -- -D warnings                   # see §15.2 first
```

**Do not use `--all-features`** anywhere in this workspace — §6.3.

Note that steps 1-8 will **not** produce a green `cargo check` on their own: `den-transpiler-oxc`
(doc 04), `rquickjs 0.12` (doc 01) and `wasmtime 48` / `wasmi 1.1` (docs 03/05) all still need their
own migrations. Work these changes in, but expect the first green build only after those land.

---

## 15. Measured build state (`cargo check`, rustc 1.97.1, edition 2024, offline)

Everything above is a source-level claim. This section is the *observed* compiler output on the
tree as it stands (manifests bumped, no `.rs` touched). It is what "done" looks like for this doc.

### 15.1 Per-crate results — before any fix

| Crate | `cargo check -p …` | Errors | Owner |
|---|---|---|---|
| `den-utils`, `den-stdlib-io`, `den-stdlib-fs`, `den-stdlib-timer`, `den-stdlib-text`, `den-stdlib-console`, `den-stdlib-regex` | ✅ clean | — | — |
| `den-stdlib-whatwg-fetch` | ✅ clean | — | confirms §4.5 "reqwest 0.13: nothing to change" |
| `den-stdlib-crypto` | ❌ | `E0432` unresolved import `rand::RngCore` (`:1`), `E0425` `rand::thread_rng` not found (`:32`) | §7.1 |
| `den-stdlib-core` | ❌ | `E0599` no method `deref` (`cancellation.rs:20`) | §1.5 |
| `den-stdlib-sqlite` | ❌ | `E0599` no method `deref` ×2 (`:51`, `:78`) | §1.5 |
| `den-stdlib-networking` | ❌ | `E0599` no method `deref` ×2 (`socket.rs:84`, `:88`) | §1.5 |
| `den-stdlib-wasm` | ❌ | ours: `wabt` + `getset` unresolved crates (§10.1, §10.2) and `lib.rs:67:26`/`:68:27` `cannot explicitly borrow within an implicitly-borrowing pattern` (§11.2, error text reproduced in-tree). With those three fixed, **25** errors remain: `wasmtime_wasi::preview1`, `UserDataGuard::borrow_mut`, `ExternType::Tag(_)` non-exhaustive, `JsClass` fields needing `Class<'js, T>` | rest: docs 02/03/05 + doc 01 |
| `den-transpiler-oxc` | ❌ 18 errors | still verbatim SWC source (`swc_common`, `sourcemap`, `anyhow` unlinked) | doc 04 |
| `den-core` | ❌ | `E0425 block_in_place` ×2 (§4.9), `E0050` `Loader::load` 3≠4 params ×2, `E0050` `Resolver::resolve` 4≠5 params | §4.9 + doc 01 |

After applying §1.5, §7.1, §10.1, §10.2, §11.2 and §4.1, the crates
`den-stdlib-core`, `den-stdlib-crypto`, `den-stdlib-networking` and `den-stdlib-sqlite`
check **clean**; `den-stdlib-wasm` is left with 25 errors, every one of them owned by docs 02/03/05 and doc 01;
`den-core` keeps only the rquickjs-0.12 signature errors (doc 01). Re-measured, not assumed.

### 15.2 The fmmap `unsafe` error is *invisible* until doc 01 lands

`den-core` never reports `E0133 call to unsafe function AsyncMmapFile::open` today, because rustc
runs THIR unsafety checking **after** type checking and `den-core` still has typeck errors
(`block_in_place`, `Loader::load`). Do not read the absence of that error as "fmmap 0.5 is fine" —
`pub async unsafe fn open` is right there at `$REG/fmmap-0.5.0/src/mmap_file.rs:1957`, and a scratch
crate with exactly den's call shape does fail with `E0133` and does pass with the §4.1 fix.

### 15.3 `-D warnings` will fail on **pre-existing** warnings

The `cargo clippy … -D warnings` step in §14 is not reachable until these are dealt with (none are
caused by a bump; all predate it):

| Warning | Site |
|---|---|
| `enum QueryRowError is never used` (dead_code) — declared, never constructed, never returned | `den-stdlib-sqlite/src/lib.rs:245` |
| `unused import: Module` | `den-core/src/engine.rs:8` |
| `unused variable: ctx` | `den-core/src/engine.rs:238` |
| `unused variable: extension` (only without `transpile`) | `den-core/src/loader/http.rs:30` |
| `variable does not need to be mutable` | `den-stdlib-wasm/src/engine.rs:23` |
| `unused import: Into` | `den-transpiler-oxc/src/lib.rs:5` (doc 04) |
| `use of deprecated macro async_with` ×3 | `den-core/src/engine.rs:5,313,359` (doc 01) |

### 15.4 Two manifest nits found while measuring

* `Cargo.toml:36-45` lists `"from"` **twice** in the workspace `derive_more` feature array. Harmless
  (Cargo dedupes) but delete the duplicate while you are in there.
* `den-stdlib-io/src/lib.rs:12,38` hold the tree's only trait objects
  (`Arc<RwLock<dyn AsyncRead + Unpin>>`, `…dyn AsyncWrite…`). §1.5/§11.11 say den has no `Box<dyn …>`
  and no `dyn Error` — both still true — but "zero `dyn`" would not be. Neither site is affected by
  the edition or by any bump; `.write()` reaches `RwLock` through auto-deref, which needs no
  `Deref` import.

---

## Verification log

Independent completeness/accuracy pass. Toolchain: `rustc 1.97.1 (8bab26f4f 2026-07-14)`,
`cargo 1.97.1`, `rust-toolchain.toml` channel `stable`. Sources read under
`$REG` = `/home/steve/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

### Claims checked against the real sources — all **confirmed**

| Claim | Where verified |
|---|---|
| derive_more 2.1.1 root is macros-only; `derive` and `with_trait` submodules | `$REG/derive_more-2.1.1/src/lib.rs:123, 128, 139`; 1.0.0's `re_export_traits!` at `:167-227` |
| derive_more 2.0.0 breaking-change wording | `$REG/derive_more-2.1.1/CHANGELOG.md:84-91` |
| `rand::rng()`, `rand::fill`, `rand_core::Rng`, deprecated `RngCore` | `$REG/rand-0.10.2/src/lib.rs:59,314`, `src/rngs/thread.rs:201`, `src/rng.rs:338`; `$REG/rand_core-0.10.1/src/lib.rs:49,62,256` |
| fmmap 0.5 `pub async unsafe fn open` + `fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt}` path + `AsyncMmapFileExt: Sync` + `as_slice` unchanged | `$REG/fmmap-0.5.0/src/mmap_file.rs:1957, 817, 828`; `src/lib.rs:1200-1209`; 0.3.3 `src/mmap_file.rs:1508` |
| rustyline 18: `Config` lost `Copy`; `SQLiteHistory::{with_config,open}(&Config)`; `Configurer::set_behavior` gone (only `pub(crate)` `:182` + `Builder::behavior` `:485`); `with_history(Config, I)` unchanged; `readline<P: Prompt + ?Sized>`; `impl Prompt for str` | `$REG/rustyline-18.0.1/src/{config.rs:6,178,182,485; sqlite_history.rs:36,44; lib.rs:621,643,831,852; prompt.rs:19}` vs 15.0.0 |
| rustyline 18 pins `rusqlite 0.39.0` + `bundled` | `$REG/rustyline-18.0.1/Cargo.toml:157-163` |
| wat 1.257.1 `parse_str/parse_bytes/parse_file`, `Result`, `Error`, edition 2024, `default = ["component-model"]` | `$REG/wat-1.257.1/src/lib.rs:193,147,104,369,381`; `Cargo.toml:13,36-38` |
| reqwest 0.13.4 `default-tls = ["rustls"]`, `default = [default-tls, charset, http2, system-proxy]`, `native-tls = [__native-tls, __native-tls-alpn]`, decompression via `tower-http` | `$REG/reqwest-0.13.4/Cargo.toml [features]`; 0.12.9 for the "before" column |
| `impl Default for TlsBackend` still prefers native-tls when both are on | `$REG/reqwest-0.13.4/src/tls.rs:621-635` |
| aws-lc-rs really is in den's graph today | `cargo tree --offline -i aws-lc-rs -e normal` (aws-lc-rs 1.18.0 ← rustls 0.23.43 ← hyper-rustls/reqwest) |
| rusqlite 0.39 `raw_bind_parameter<I: BindIndex, T: ToSql>` + `impl BindIndex for usize` | `$REG/rusqlite-0.39.0/src/statement.rs:567`, `src/bind.rs:23-28`; 0.32.1 `src/statement.rs:547` |
| matchit 0.9.2 `at`, `MatchError::NotFound` | `$REG/matchit-0.9.2/src/router.rs:60`, `src/error.rs:153-156` |
| typed-builder 0.23 breaking changes (ref-defaults, `Optional` removed) | `$REG/typed-builder-0.23.2/CHANGELOG.md:17-31` |
| base64 0.23.1 `default = ["std","simd-unsafe"]`, prelude `BASE64_STANDARD` | `$REG/base64-0.23.1/Cargo.toml`, `src/prelude.rs:19` |
| uuid 1.24.1 `new_v4`, `fast-rng = ["rng","dep:rand"]` | `$REG/uuid-1.24.1/src/v4.rs:33`, `Cargo.toml:104-107` |
| url 2.5.8 `impl From<Url> for String`; relative-path 2.0.1 `extension`; console-subscriber 0.5.0 `init`; mimalloc 0.1.52 `MiMalloc`; color-eyre 0.6.5 `install` | `url/src/lib.rs:2803`, `relative-path/src/lib.rs:1271`, `console-subscriber/src/builder.rs:713`, `mimalloc/src/lib.rs:46`, `color-eyre/src/lib.rs:458` |
| colored 3.0.0's only break is MSRV 1.80 | `$REG/colored-3.1.1/CHANGELOG.md:4-5` |
| lockfile versions in the header table | `Cargo.lock` (all 39 spot-checked names match; `wabt` absent) |
| `.rustfmt.toml` flipped to `edition = "2024"` | `git diff .rustfmt.toml` |

### Claims re-verified by **compiling**, not just reading

* derive_more 2.1.1 on edition 2024 with den's exact `EngineError` / `InferTranspileSyntaxError` /
  wrapper-struct shapes — builds; `Display` on attribute-free enums and `Error::source()` inference
  behave as §1.4 says.
* `derivative 2.2.0` + `delegate-attr 0.3.1` + `typed-builder 0.23.2` + `derive_more 2.1.1` together
  on **edition 2024**, with den's `#[derivative(Default(new = "true"))]`, `Default(value = "true")`,
  `Debug = "ignore"` and `#[delegate(self.deref())]` shapes — all build. §2's "derivative is not
  required for the bump" is correct, including at the macro-expansion level.
* §4.1's fmmap fix: `unsafe { AsyncMmapFile::open(path) }.await` builds; without `unsafe` it is
  `E0133`.
* §3.1's rustyline rewrite: transcribed verbatim into a scratch crate with
  `features = ["derive","with-sqlite-history"]` — builds clean, no unused-import warning.
* §11.2's `ref`-pattern error: reproduced both standalone and **in den itself** —
  `den-stdlib-wasm/src/lib.rs:67:26` and `:68:27`, `error: cannot explicitly borrow within an
  implicitly-borrowing pattern`. Removing `ref` clears both.

### Corrections applied to this document

1. §1.1 said den calls `.deref()` "in four places" — it is **five** call sites in three files
   (`socket.rs:84,88`, `cancellation.rs:20`, `sqlite/lib.rs:51,78`), which the TL;DR row 1 already
   listed correctly. Reworded.
2. §1.2 cited `derive_more-2.1.1/src/lib.rs:125-134` for `pub mod derive`; the item starts at
   **`:128`** (`:125-127` are doc comment). Corrected.
3. TL;DR row 8 and §4.5 cited `den-stdlib-whatwg-fetch/Cargo.toml:26-35` for the reqwest block; it is
   **`:24-33`**. Corrected in both places.
4. §6.1 cited `Cargo.toml:22-23` for the `base64-simd` feature; it is
   `den-stdlib-core/Cargo.toml:15` (dependency) and `:25` (feature). Corrected.
5. §11.2 said "all eight `ref` patterns"; there are **eleven** (`loader/http.rs:40`,
   `stdlib-io:46,47,48`, `wasm/module.rs:27,28`, `wasm/instance.rs:255`, `stdlib-text:74,75`,
   `wasm/lib.rs:67,68`). The triage table itself was already complete and correct. Corrected.

### Sections added

* **§4.9** — `den-core` declares `tokio` with no features while calling `block_in_place`/`Handle`.
  Not covered anywhere in the original doc, and it makes every per-crate verification command in
  §14 fail. Observed, with the exact `E0425` output and the one-line manifest fix.
* **§6.3** — `base64` and `base64-simd` cannot be enabled together (`E0308` ×2, observed); therefore
  `--all-features` is not a usable verification command on this workspace. Also records that
  `base64-simd` is *not* part of the bump (0.8.0 both sides) and that its API surface is intact.
* **§15** — measured `cargo check` state per crate, so the implementer can tell "my fix worked" from
  "someone else's doc still owns this error"; the fmmap-`E0133`-is-masked trap; the list of
  pre-existing warnings that block `-D warnings`; two manifest nits.
* TL;DR rows **11** and **12** for the two new build issues.

### Checked and found accurate — no change made

The reqwest feature-remap analysis (§4.5), the rusqlite 0.32→0.39 signature table (§9), the
edition-2024 audit (§11.1, .3, .4, .6, .7, .8, .10, .11), the dead-dependency inventory (§12) and
the "no change" table (§13) all survived spot-checking. §11.3's `-> impl` grep, §11.4's `gen` grep,
§11.6-11.8's `extern "C"` / `no_mangle` / `static mut` greps and §12.2's unused-dep greps were all
re-run and returned exactly what the doc claims.

### Not verified

* §10.1's "`wat` does not validate the module" behaviour claim and its rendered error snippets were
  taken on trust (they require running the parser; the source-level argument — `wat` only parses,
  resolves and encodes — is sound).
* §4.5's "[verified by compile]" reqwest `default-features = false` feature set was not re-built
  here; the feature arithmetic was re-derived from `$REG/reqwest-0.13.4/Cargo.toml` and is correct
  (`native-tls` no longer implies `default-tls`, so no `rustls` edge remains).
* Anything owned by docs 01-05 (rquickjs 0.12, wasmtime 48, wasmi 1.1, oxc) — out of scope.
