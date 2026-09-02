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
window-handle bridge are out of scope: den has no host-owned surface. The
public `wgpu` crate owns validation; mapped buffers are copy-in/copy-out
`ArrayBuffer`s detached on `unmap`. `DEN_WEBGPU_BACKEND` (or
`DENO_WEBGPU_BACKEND`) selects backends; `noop` is the hermetic test backend.

Official WebGPU CTS files live in [`vendor/cts`](vendor/cts) (`src/webgpu/**/*.spec.ts`).
The harness is [`den-stdlib-webgpu/tests/cts.rs`](den-stdlib-webgpu/tests/cts.rs):
one nextest test per official spec file, sources never rewritten. TypeScript is
transpiled into `target/cts-js/`. Canvas/DOM/worker suites are registered then
`#[ignore]`d with a skip reason, matching the other official suites. Run CTS
with `cargo nextest run --profile cts` so spec files execute one at a time:
each file owns a wgpu instance, and AllFeaturesMaxLimits cases would otherwise
allocate adapter-max textures in parallel and OOM the host. Allocations above
a 2 GiB host budget become `GPUOutOfMemoryError` without asking the driver for
the full size. `createTexture` preflights WebGPU validation (MSAA vs
`STORAGE_BINDING`, size limits, mip counts, format/dimension) and never passes
an invalid descriptor to wgpu: Vulkan SIGSEGVs on combos such as sampleCount 4
plus `STORAGE_BINDING` instead of returning `GPUValidationError`. Invalid or
over-budget textures become a 1×1 `rgba8unorm` dummy; the JS `GPUTexture`
still reports the requested descriptor. Invalid (`is_dummy`) textures, views and
buffers inject `GPUValidationError` on `createView` / `createBindGroup`. Destroyed
objects stay valid JS handles: those calls must not emit validation (CTS
`resource_state` / `createView.texture_state` only wrap `invalid`), so bind-group
creation substitutes a live dummy wgpu resource instead of the destroyed handle.
Each `GPUDevice` owns one shared fallback sampled/MSAA/storage texture, one
uniform/storage buffer, one empty bind group, one dummy shader module, and one
dummy compute/render pipeline. Invalid GPUTextures and invalid pipelines clone
those handles; `destroy()` on a dummy must not call `inner.destroy()`. Per-object
dummies made `createBindGroup` RSS climb by tens of GiB.
The CTS runner allocates one `Logger` per case, sets
`maxSubcasesInFlight` to 1, and replaces the pooled device every 16 cases.
It hides the suite's `gc()` (CTS cadence retains too much) and instead
calls the runtime `gc()` after every case so QuickJS drops GPUBindGroup
and bundle objects; without that drain, `setBindGroup` cartesian
render-bundle cases SIGSEGV in `free_object`. The harness also sets a
512 MiB QuickJS heap
limit so a cartesian spec fails that test instead of OOMing the host. Do not
raise the GC threshold: native wgpu textures live until JS GC, and a 32 MiB
threshold let RSS climb by tens of GiB during `createBindGroup`.
CTS defaults to 100 in-flight subcases; that cartesian fan-out retained
every `createTexture` subcase and SIGSEGV'd QuickJS `free_object`. Explicit
`gc()` plus thousands of `GPUTexture` class instances asserted in
`free_zero_refcount`. Never batch `createTexture` / `createView` with other
specs: those files alone can pin gigabytes of dummy textures.
Native `setPipeline` / `setVertexBuffer` / `draw` run only for a live
(non-dummy) pipeline that needs no bind groups and no immediate data:
wgpu `set_bind_group` of `GPUBindGroup.inner` corrupts QuickJS, and
`RenderBundleEncoder::finish` fatals on a worker thread if the native
encoder is invalid. Valid bundles record those commands in software and
replay them onto the render pass at `executeBundles` so pass scissor,
stencil reference and viewport apply (bundles have no setters for those).
Software still injects bind-group / immediate /
occlusion-query validation (`beginOcclusionQuery` without a queryset,
wrong type, OOB index, nesting, duplicate index, unbalanced end).
Native occlusion begin/end run only when the pass has a live occlusion
queryset. Destroyed query sets stay encode-valid and fail at submit.
A command encoder encodes at most 256 native compute/render passes:
radv rejects a command stream with tens of thousands of timestamp
passes (`timestampQuery` 65536). Extra passes stay software-valid.
Pipeline creation preflights WGSL entry points, pipeline-overridable
constants, bind-group layout vs shader binding class (including texture
sample type/dimension and storage-texture access), render-pipeline
state that naga/wgpu would reject after the JS call (`@builtin(frag_depth)`
without a depth aspect, depth bias on non-triangle topology, strip index
format, unclipped depth, vertex-buffer limits, inter-stage locations),
and skips wgpu when the descriptor is already invalid: delayed
`on_uncaptured_error` from naga/wgpu otherwise leaks after the JS call
returns. Auto-layout pipelines whose WGSL uses `texture_storage_` skip
wgpu create (and skip native bind-group-layout synthesis): wgpu leftover
`create_bind_group_layout` for `read_write` non-r32 formats and vertex
writable storage otherwise fires after the JS call. wgpu 30 has no `Snorm10_10_10_2`; `snorm10-10-10-2` maps to the
packed unorm layout of the same size so CTS validation does not TypeError. Constant record keys
are read as JS strings (NUL-preserving); rquickjs `Atom::to_string`
truncates at embedded NUL and would treat `'c0\0'` as `c0`. `@id(N)
override name` is keyed only by `N`, not `name`. NaN/Inf constant values
throw `TypeError`. `beginComputePass` / `beginRenderPass` timestampWrites
that use an invalid query set, a non-timestamp query, a mismatched device,
or an OOB/duplicate index mark the encoder invalid and omit the writes
from the native pass; the `GPUValidationError` is injected at
`encoder.finish()`, which is what CTS wraps. Beginning a second pass
while one is open also marks the encoder invalid and returns a dummy pass
without calling wgpu. Ending a pass after the parent encoder is finished
must not `Drop` the native pass (that double-ends and leftover-errors).
`clearBuffer` records mapping liveness and injects usage/range errors at
`finish()`, destroyed buffers at `queue.submit()`. `setPipeline` with an
invalid or device-mismatched pipeline, `dispatchWorkgroups` over the
per-dimension limit, and `dispatchWorkgroupsIndirect` with an invalid
buffer/usage/offset mark the encoder invalid and skip wgpu; destroyed
indirect buffers fail at `queue.submit()`. The same skip-and-inject-at-finish
pattern applies to `setVertexBuffer` / `setIndexBuffer` (slot, usage,
alignment, range, device mismatch) and `copyTextureToTexture` (invalid
at finish, destroyed at submit). Indexed draws that overflow `u32` or
exceed the bound index range skip wgpu. `drawIndirect` /
`drawIndexedIndirect` are software-validated and skip native wgpu:
wgpu 30 panics `Cannot get non-existent resource RenderPipelineId`
on a worker thread when a pipeline handle is already gone, which
leaves extra JS error scopes. No-op fragment shaders (`@fragment fn main() {}`)
skip wgpu pipeline creation for the same reason (CTS `createNoOpRenderPipeline`).
`GPURenderBundleEncoder` never records native commands (`with_encoder` is
a no-op). wgpu 30 `RenderBundleEncoder::finish` calls `handle_error_fatal`
on a worker thread, so finish drops the native encoder without calling
wgpu finish. `executeBundles` software-checks device, color formats,
depth format, sample count, and depth/stencil readonly, then skips native
wgpu. Per-case runtime `gc()` is required so bundle/bind-group JS objects
are actually dropped. Do not restore CTS `gc()` between subcases: that
SIGSEGVs QuickJS `free_zero_refcount` across encoding specs. `draw.spec.ts`
still asserts `free_zero_refcount` during the first cartesian case
(`unused_buffer_bound`); isolate it and never batch it.
`setBindGroup` software-validates invalid/device-mismatched groups,
`index >= maxBindGroups`, and dynamic offsets at encode time (count,
alignment, and `bind.offset + dynamicOffset + bindingSize <= buffer.size`).
Dynamic-offset windows live in a thread-local table keyed by the
fingerprint buffer pointer so `GPUBindGroup`'s JS-class layout stays
the empirically safe field set. Extra fields, `Rc<[T]>`, nested extras,
and large inline arrays on that class corrupt QuickJS (`free_object`).
Native wgpu `set_bind_group` is never called. Destroyed resources are tracked for
`queue.submit()`, and at draw/dispatch checks
pipeline layout compatibility (empty groups ignored; auto layouts only
match bind groups from the same auto pipeline; explicit layouts compare
entries including visibility). Vertex-buffer OOB is CPU-validated for
`draw` and instance-step `drawIndexed`. `setImmediates` throws
`OperationError` for out-of-bounds or unaligned content bytes and
injects range errors at encode. Draw/dispatch software-checks that every
4-byte slot of each statically used `var<immediate>` (whole struct, not
just the accessed member) was written; `executeBundles` clears the filled
mask. Native draw/dispatch is skipped whenever the pipeline uses
immediates, even after software `setImmediates` fills every slot, so
wgpu leftover "missing immediate ranges" cannot fire. Filled slots
must not mark the encoder invalid: CTS
`pipeline_immediate:required_slots_set` expects a valid compute pass
when every required slot was written. Pipeline layout vs shader binding
types are checked only for resources statically used by that pipeline
stage (naga global use on the matching entry point). A shared module
with compute+vertex+fragment entries must not invalidate the vertex
stage for fragment-only bindings. An unused `@group` declaration does
not invalidate `create*Pipeline`. Color-target formats that require an
unenabled feature are a WebIDL `TypeError`, not `GPUValidationError`.
`createView({ usage })` must be a subset of the texture's usage.
Buffer↔texture copies validate mapped buffers at encode, destroyed
buffers at submit, and skip native wgpu for invalid/destroyed/mapped
handles. Invalid `create*PipelineAsync` yields once, then resolves
with a dummy if the device was lost in the meantime (CTS device_lost).
`GPURenderBundleEncoder.finish` on an already-finished encoder injects
validation and returns a dummy bundle; it must not throw
`InvalidStateError` (CTS device_lost double-finish).
Auto-layout bind groups include only naga-statically-used resources.
Render/compute passes track bind-group texture subresource usage
(via the bind-group TLS extras table, not extra JS-class fields) by
kind (attachment / sampled / storage) and aspect. Depth-stencil
attachments record depth and stencil separately so a read-only aspect
may share a subresource with a sampled bind group of that aspect.
wgpu 30 still records `DEPTH_STENCIL_WRITE` unless both aspects are
read-only, so native `setBindGroup` is skipped when that leftover
would fire even if the sampled aspect is spec-valid. Overlapping
read+write, attachment+storage, or compute storage write+write
aliasing still invalidate the encoder at pass end.
`GPUAdapter`/`GPUDevice` limits grant at least the WebGPU core defaults
(`Limits::or_better_values_from`): wgpu's Vulkan backend reports 15
`maxInterStageShaderVariables` after subtracting `@builtin(position)`,
while core default is 16. Native wgpu is capped to the adapter and
skipped when a pipeline is valid at the granted limit but over wgpu's
native inter-stage count. WGSL `texture_external` is valid at
shader and pipeline creation: skip wgpu (naga has no external textures)
and return a live dummy, without injecting. Local CTS runs should kill
the spec process if RSS exceeds 20 GiB so the host OOM killer cannot
take the whole tmux session.
`createView` passes wgpu `format: None` when the JS descriptor omits
`format`, so depth/stencil `aspect: "depth-only"` / `"stencil-only"`
views resolve to wgpu's aspect-specific format. Passing the combined
texture format with a single aspect leftover-errors
(`Depth24PlusStencil8` is not an aspect-specific view format).
Pipeline-overridable constants without defaults are required only for
the entry point that uses them: a shared vertex+fragment module may
leave fragment-only overrides out of `vertex.constants`.
Fragment builtin / color-IO preflight is entry-point-specific so a
module that also contains `@builtin(sample_mask)` outputs does not
dummy an alpha-to-coverage pipeline that uses a different entry.
Attachment `view` / `resolveTarget` accept `GPUTexture` or
`GPUTextureView` (WebGPU allows both). Timestamp query sets without
the `timestamp-query` feature throw `TypeError`. naga 30 panics on
immediate structs larger than 64 4-byte slots (`1 << lo` overflow);
those shaders skip native wgpu and size-check in software.
Transient textures keep `viewFormats` empty.

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
