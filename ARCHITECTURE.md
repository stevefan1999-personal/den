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
    ├── den-stdlib-canvas        ImageData, ImageBitmap (`den:canvas`)
    ├── den-stdlib-console       console
    ├── den-stdlib-core          atob, btoa and gc
    ├── den-stdlib-crypto        Web Crypto
    ├── den-stdlib-ffi           den:ffi, capability-gated
    ├── den-stdlib-fs            den:fs
    ├── den-stdlib-http          den:http, cleartext HTTP/1 and HTTP/2
    ├── den-stdlib-intl          Intl over ICU4X (`den:intl`)
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
    ├── den-stdlib-webgpu        headless WebGPU compute (`den:webgpu`)
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
workers, WebAssembly and WebGPU. WHATWG is evaluated after workers because its
APIs extend worker-owned event classes. WebGPU is evaluated after workers so
`navigator.gpu` attaches to the existing `Navigator` instance.

`den:webgpu` is a native rquickjs slice of WebGPU (adapter, device, buffers,
textures, samplers, query sets, WGSL compute and render pipelines, bind groups,
command encoding, mapping and error scopes). Canvas, surfaces and Deno's BYOW
window-handle bridge are out of scope: den has no host-owned surface.

The crate is a thin wrapper over the safe `wgpu` crate, shaped after
[deno_webgpu](https://github.com/gfx-rs/wgpu/tree/trunk/deno_webgpu). wgpu and
wgpu-core own every device-timeline validation; den performs only the
content-timeline checks the specification places in the binding layer, which
are the WebIDL coercions, the `TypeError`s for feature-gated formats and
malformed dictionaries, the `RangeError` for a `mappedAtCreation` size that is
not a multiple of four, and the `OperationError`s for `mapAsync`,
`getMappedRange` and the `writeBuffer` / `setImmediates` data windows. Objects
whose creation failed are kept and used exactly like valid ones: wgpu registers
an invalid handle and reports a validation error on every later use, so den has
no fallback or dummy resources.

Errors reach script through one scope stack in the crate. den never pushes a
wgpu error scope; the `on_uncaptured_error` handler, installed before any
resource exists on the device, routes each error to the innermost matching
`pushErrorScope`, and anything uncaptured is dispatched as a
`GPUUncapturedErrorEvent` after the JS call returns. Mapped buffers are
copy-in/copy-out `ArrayBuffer`s detached on `unmap`; `ArrayBuffer::new` is
never used because it double-frees under this rquickjs fork.

wgpu is pinned to a trunk revision rather than the 30.0.1 release. That release
panics through `handle_error_fatal` when `RenderBundleEncoder::finish` fails,
resolves render-bundle commands from ids that the JS garbage collector may
already have freed, and ships a naga `ImmediateSlots::from_range` shift
overflow. Trunk fixes all three: bundle commands hold `Arc` references taken at
call time, which is why den can record bundles natively while retaining the
handles it passes for the encoder's lifetime.

Two behaviours are limited by the safe wgpu API rather than by den.
`RenderBundleEncoder` exposes no debug-group methods even though wgpu-core
records and validates them, so the bundle debug methods are inert. And
`set_vertex_buffer` panics on a zero-length slice, so a zero-size vertex
binding unbinds the slot instead of binding an empty range.

Official WebGPU CTS files live in [`vendor/cts`](vendor/cts) (`src/webgpu/**/*.spec.ts`).
The harness is [`den-stdlib-webgpu/tests/cts.rs`](den-stdlib-webgpu/tests/cts.rs):
one nextest test per official spec file, sources never rewritten. TypeScript is
transpiled into `target/cts-js/`. Canvas/DOM/worker suites are registered then
`#[ignore]`d with a skip reason, matching the other official suites. Run CTS
with `cargo nextest run --profile cts` so spec files execute one at a time:
each file owns a wgpu instance, and running the whole tree in parallel exhausts
host memory. Measure against
[`cts_runner/test.lst`](https://github.com/gfx-rs/wgpu/blob/trunk/cts_runner/test.lst)
on a real adapter; the `noop` backend performs no copies and cannot run
indirect draws, so it is a smoke test, not conformance parity.

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
- [`den:webgpu`](den-stdlib-webgpu/src/lib.rs) is a headless WebGPU slice. It
  installs `navigator.gpu` and the GPU constructors as globals. Canvas,
  surfaces and Deno's BYOW bridge are excluded.
- [`den:canvas`](den-stdlib-canvas/src/lib.rs) is canvas phase 0: `ImageData`,
  `ImageBitmap` and `createImageBitmap` over `ImageData` and `ImageBitmap`
  sources. All of it is CPU pixel work on straight RGBA8, so the crate carries
  no image codec; `Blob` sources, `OffscreenCanvas` and any rendering context
  are excluded until there is a decoder or a context to want them.
- [`den:intl`](den-stdlib-intl/src/lib.rs) is ECMA-402 phase 1: the `Intl`
  namespace, `Intl.getCanonicalLocales` and `Intl.Locale`, over ICU4X's
  likely-subtags, alias, week and calendar-preference data. The constructors
  that need formatting data — `Collator`, `DateTimeFormat`, `NumberFormat`,
  `PluralRules`, `ListFormat`, `RelativeTimeFormat`, `Segmenter`,
  `DisplayNames`, `DurationFormat` — and `Intl.supportedValuesOf` are absent
  rather than stubbed, and quickjs-ng's own `toLocaleString` and
  `localeCompare` are left alone.
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
WPT, and WebGPU CTS (`cargo nextest run -p den-stdlib-webgpu --test cts`).
WPT uses the vendored sparse checkout and the official `wptserve` process on
ports 8000–8002; [the workflow](.github/workflows/wpt.yml) owns that server
lifecycle.

Closed investigation notes stay available in Git history. The remaining
[research index](docs/research/README.md) points back here.
