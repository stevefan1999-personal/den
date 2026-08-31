# den architecture

This is the map and the invariants. Implementation details belong in rustdoc
and tests so they cannot drift into a second specification.

## Crate graph

```text
den                              CLI, REPL, signals and tracing
└── den-core                     embeddable QuickJS runtime, loaders, resolvers
    ├── den-capabilities         host policy values and attenuation
    ├── den-transpiler-oxc       optional TypeScript and JSX lowering
    ├── den-stdlib-assert        den:assert
    ├── den-stdlib-console       console
    ├── den-stdlib-core          atob, btoa and gc
    ├── den-stdlib-crypto        Web Crypto
    ├── den-stdlib-ffi           den:ffi, capability-gated
    ├── den-stdlib-fs            den:fs
    ├── den-stdlib-http          den:http, cleartext HTTP/1 and HTTP/2
    ├── den-stdlib-kv            den:kv over SurrealKV
    ├── den-stdlib-networking    TCP, UDP, Unix, TLS and WebSocket transport
    ├── den-stdlib-path          lexical paths
    ├── den-stdlib-process       process, signals and child processes
    ├── den-stdlib-sqlite        bundled SQLite
    ├── den-stdlib-temporal      Temporal
    ├── den-stdlib-text          TextEncoder and TextDecoder
    ├── den-stdlib-timer         timers
    ├── den-stdlib-whatwg        Fetch and WHATWG APIs
    ├── den-stdlib-wasm          WebAssembly through wasmtime
    └── den-stdlib-worker        workers, events and structured clone

den-config                       JSONC discovery and capability-policy conversion
den-package-store                SeaORM package store, solver and module snapshots
den-e2e                          file-based cross-crate runtime tests
```

The workspace manifest is the authoritative member and feature graph:
[Cargo.toml](Cargo.toml). Each standard-library crate owns one JS-facing
surface and must not depend on `den-core`; `den-core` composes them.

The CLI uses clap derive and advertises only implemented commands. It discovers
`den.json`/`den.jsonc` (or accepts `--config`) before building the engine.
Root preloads run before the entry; imports, policy metadata, stack/heap budgets
and an optional feature-gated package snapshot are inherited by workers. Downloaded
package bytes and metadata belong to `den-package-store`; its schema is created
through versioned SeaORM migrations, validated against the same SeaQuery table
definitions when opened, and package content is addressed by SHA-256.
Resolvo operates on a validated in-memory snapshot, never from inside QuickJS's
synchronous loader. A host solves and hydrates a `PackageModuleSnapshot`, then
passes it to `EngineBuilder::package_modules`; workers inherit that immutable
snapshot. Hydration is finite and solver-produced root/dependency edges define
which bare imports each module can see. The current solver is intentionally
flat (one version per registry/package key); optional, peer and nested
multi-version graphs are excluded until scoped instance identities exist.

## Runtime invariants

- [`Engine`](den-core/src/engine.rs) owns one `rquickjs::AsyncRuntime` and
  `AsyncContext`. JavaScript work runs on that context, never on an arbitrary
  Tokio worker.
- [`EngineBuilder`](den-core/src/builder.rs) owns stack, GC and optional heap
  limits together with the realm's capability policy and process arguments.
  Workers inherit the same settings; a child policy may only attenuate its
  parent. Builtin operations do not enforce this policy yet; hosts must call
  `Policy::check` at their own boundaries.
- `AsyncRuntime::idle()` is the event loop. Do not run a second driver beside
  it; two schedulers would compete for the same runtime lock.
- A host stops work by cancelling its program future, calling
  `Engine::shutdown()`, then dropping every engine clone. A QuickJS interrupt
  handler is still required for bytecode that never yields.
- `Engine::shutdown()` drains realm-owned resources, including workers and KV
  stores, before the context disappears.
- [`den_util::stack`](den-util/src/stack.rs) installs quickjs-ng's structured
  `Error.prepareStackTrace` hook before any module runs. Loaders register the
  generated source and OXC/`sourceMappingURL` maps before compilation; stack
  lookup performs no I/O. QuickJS exposes only the live synchronous frame
  chain, so den does not claim V8-style causal frames across `await`.
- Workers use one QuickJS runtime per OS thread. The
  [`WorkerHost`](den-stdlib-worker/src/host.rs) seam lets `den-core` build a
  worker engine without reversing the dependency edge.
- The REPL keeps durable history in `history.surrealkv`. Failure to acquire the
  store lock falls back to in-memory history rather than preventing startup.

## Module registration

Every `den:*` module enabled in [`engine.rs`](den-core/src/engine.rs) must be in
both the builtin resolver and native module loader. Modules that expose globals
must also be evaluated during context construction.

Import-only modules include `den:assert`, `den:ffi`, `den:fs`, `den:http`,
`den:kv`, `den:networking`, `den:path` and `den:sqlite`. Global-producing
modules include console, core, crypto, process, Temporal, text, timers, WHATWG,
workers and WebAssembly. WHATWG is evaluated after workers because its APIs
extend worker-owned event classes.

The loader chain is:

1. native builtins;
2. the optional package snapshot, then the embedded bytecode bundle;
3. HTTP modules through [`loader/http.rs`](den-core/src/loader/http.rs);
4. filesystem modules through
   [`loader/mmap_script.rs`](den-core/src/loader/mmap_script.rs).

The resolver chain is:

1. import maps in
   [`resolver/import_map.rs`](den-core/src/resolver/import_map.rs);
2. native builtins;
3. the package snapshot, then the embedded bundle;
4. HTTP URLs in [`resolver/http.rs`](den-core/src/resolver/http.rs);
5. absolute and relative files in
   [`resolver/file.rs`](den-core/src/resolver/file.rs).

Application import maps yield for `den-pkg:` parents, so package dependency
edges cannot be rewritten around the solved snapshot.

Import attributes are handled once by
[`loader/typed.rs`](den-core/src/loader/typed.rs): `json`, `text`, and `bytes`
produce synthetic modules; other types fail loading.

## Standard-library boundaries

- [`den:http`](den-stdlib-http/src/lib.rs) accepts Fetch `Request` objects and
  requires handlers to return `Response` objects. It supports HTTP/1 and
  cleartext HTTP/2 prior knowledge, bounds buffered bodies, and exposes
  explicit graceful drain through `Server.close()` and `Server.finished`.
- [`den:kv`](den-stdlib-kv/src/lib.rs) stores byte keys and values in
  SurrealKV. Resolved mutations use immediate durability; transactions are
  explicit and stores are closed during engine shutdown.
- [`den:whatwg`](den-stdlib-whatwg/src/lib.rs) owns Fetch together with the
  other WHATWG globals. `den:whatwg-fetch` remains an independently selectable
  module surface, not a separate crate.
- [`den:wasm`](den-stdlib-wasm/src/lib.rs) uses Cranelift under `jit` and Pulley
  otherwise. WASI is independently gated by the `wasi` feature.
- `den:ffi` is denied at runtime unless the host grants the requested library
  path, even when the crate is compiled in.

## Feature invariants

Root features pass through to `den-core`; focused `stdlib-*` features select
one surface, while `stdlib` selects the complete standard library. `react` and
`typescript` imply `transpile`. `wasi` implies `wasm`; `jit` only changes the
wasmtime execution engine.

Keep conditional registration synchronized across dependency declarations,
resolver entries, loader entries and eager evaluation. A feature that compiles
but leaves one of those lists out is broken.

## Verification

Use nextest, never `cargo test`:

```bash
cargo nextest run --workspace --profile official --build-jobs 8
cargo nextest run --workspace --profile official --build-jobs 8 \
  --no-default-features --features stdlib,typescript,react,wasm,wasi,ring
```

Focused conformance suites are Test262 Temporal, the WebAssembly spec runner,
and WPT. WPT uses the vendored sparse checkout and the official `wptserve`
process on ports 8000–8002; [the workflow](.github/workflows/wpt.yml) owns that
server lifecycle.

Closed investigation notes stay available in Git history. The remaining
[research index](docs/research/README.md) points back here.
