# den:webgpu versus deno_webgpu

Date: 2026-09-02. Status: decision taken, rewrite in progress.

Reference implementation: `gfx-rs/wgpu/deno_webgpu` at wgpu trunk
`c5ab27023` (v30.0.0-387). den before this work: wgpu 30.0.1 from crates.io,
`den-stdlib-webgpu` at 11.4k lines after an uncommitted 8.4k-line
software-validation layer (checkpointed as `6790207`).

## Method

Nine parallel readers over deno_webgpu, wgpu / wgpu-core (registry 30.0.1 and
trunk), den's crate, and the CTS expectation lists, plus a claims audit of
ARCHITECTURE.md against source with throwaway Rust repros, plus a 22-file CTS
baseline on radv and on the noop backend.

## What deno_webgpu does

- Thin wrapper over `wgpu-core`: WebIDL conversion, JS object lifecycle, two
  cross-thread bridges (uncaptured-error event, device-lost promise). Zero
  device-timeline validation of its own. Invalid objects are whatever wgpu-core
  hands back; later use yields `InvalidResource` validation errors.
- Error scopes are wgpu-core's per-thread stack; `create_*` errors go through
  `Device::handle_error_nolabel`. Uncaptured errors are delivered from the
  wgpu callback, hopped to the isolate thread, then `dispatchEvent`.
- Content-timeline checks it keeps (the only ones den needs too): `TypeError`
  for feature-gated formats, required features not on the adapter, bind group
  layout entries with not exactly one kind, swizzle syntax; `RangeError` for
  `mappedAtCreation` size not a multiple of 4; `OperationError` for
  `mapAsync` mode, `writeBuffer`/`setImmediates` data ranges, and
  `popErrorScope` on an empty stack.
- Render bundles record natively; `finish` yields an invalid bundle on error.
- Single threaded. `mapAsync`/`onSubmittedWorkDone` poll the device in an async
  loop.

## What den did instead, and why it was wrong

| den workaround (checkpoint `6790207`) | Verified cause |
|---|---|
| Software-recorded render bundles, `with_encoder` no-op, bundle replay at `executeBundles` | wgpu 30.0.1 `RenderBundleEncoder::finish` really is `handle_error_fatal` (`wgpu_core.rs:3933`). Fixed on trunk. |
| "wgpu panics `Cannot get non-existent resource RenderPipelineId` on a worker thread" | den's own `static_ref` (`715bfd7` render.rs:347) erased lifetimes into an id-keyed 30.0.1 bundle; the JS GC freed the pipeline before `finish`. Trunk bundles hold `Arc`s. "Worker thread" is libtest-mimic's runner thread. |
| "class field layout corrupts QuickJS", `free_zero_refcount` / `gc_decref_child` aborts, per-case `gc()` rules | quickjs-ng 0.15.1 engine bug, see below. rquickjs boxes class data behind `JS_SetOpaque`; layout is invisible to QuickJS. |
| `createTexture` preflight, 1x1 dummy textures, "Vulkan SIGSEGVs on sampleCount 4 + STORAGE_BINDING" | wgpu-core rejects that descriptor before wgpu-hal (`resource.rs:1572`); repro on radv returns a validation error. |
| "delayed `on_uncaptured_error` leaks after the JS call" | wgpu invokes the callback synchronously inside the failing call; den queued it and only drained inside `flushed` wrappers. |
| naga immediates `1 << lo` overflow shim | Real in naga 30.0.1 (debug only, `64 - hi`), fixed on trunk. |
| Skip native draw with immediates, `MAX_NATIVE_PASSES`, skip `setBindGroup`, dummy pipelines | All duplicate wgpu-core validation or work around the panics above. Side effect: valid pipelines with storage textures, immediates or depth-only fragments never drew. |

Still true and kept: no `Snorm10_10_10_2` in wgpu-types; rquickjs
`Atom::to_string` truncates at NUL; wgpu records `DEPTH_STENCIL_WRITE` unless
both aspects are read-only; radv rejects 65536 timestamp passes in one command
buffer; `Device::poll` is fatal on a lost device (keep the `catch_unwind`).

## The QuickJS crashes are an engine bug

valgrind on `api/validation/encoding/cmds/render/draw.spec.ts` shows the cycle
collector (`gc_free_cycles`) freeing a promise's resolving function that a
pending reaction job still references. An instrumented GC dump found no Rust
class holding an edge to the freed object; the only holders were suspended
async-function frames. A pure-JS reproducer with no WebGPU crashes den:

```js
(function () {
  async function step() {
    const d = Promise.withResolvers();
    globalThis.leaked = () => d;
    await d.promise;
  }
  step();
})();
gc();
leaked().resolve;
```

quickjs-ng 0.15.1: a closure capturing a local of a suspended coroutine holds
an open `JSVarRef` into the coroutine's heap frame. Open var_refs are not GC
objects and hold no refcount on the async-function data, so a coroutine
reachable only through such a closure looks like an unreachable cycle and is
freed. Upstream fix: quickjs-ng commit `7955cfd` (v0.16.0). The CTS framework
hits exactly this shape in `RunCaseSpecific.run` (`resolvePromiseBlockingSubcase`),
so any spec file can abort whenever GC runs at the wrong moment.

Bumping to quickjs-ng 0.16 is not a drop-in: `JS_NewArrayBuffer` gained a
`max_len` and a realloc callback, so the bundled bindings for every target and
rquickjs-core's ArrayBuffer code would need regenerating. Instead the
rquickjs fork branch `patch-coroutine-closure-gc` carries the upstream commit
as `sys/patches/coroutine-closure-gc.patch`, applied by the sys build script
to the `OUT_DIR` copy of `quickjs.c` (a `diffy` apply, mirroring the fork's
older `patch-qjs-patch-without-gnu` branch). Delete the patch file when the
submodule moves past v0.16.0.

## Baseline (22 sampled spec files, checkpoint + `--features den-core/stdlib-webgpu`)

| backend | pass | fail | timeout | crash |
|---|---|---|---|---|
| radv | 13 | 7 | 1 | 1 |
| noop | 10 | 9 | 1 | 2 |

As checked in, 0/22: the checkpoint had `stdlib-webgpu` commented out of
den-core's `stdlib` feature, so the harness had no `navigator.gpu`.

Distinct failure signatures: QuickJS refcount aborts
(`api/validation/encoding/cmds/render/draw`, `api/operation/compute/basic`);
`getMappedRange` of size 0 raising `OperationError`; mapped `ArrayBuffer`s not
detached on `device.destroy()`; spurious validation errors from
`copyBufferToBuffer` and `queue.writeBuffer`; uncaptured errors dispatched
three times; vertex attributes reading back zero on radv; noop backend cannot
run indirect draws (wgpu-core `indirect_validation` alignment assert).

The noop backend is not CI parity with deno: cts_runner never runs on noop,
and noop performs no copies, so `api,operation,*` content checks fail there.

## Decision

1. Pin wgpu to trunk `c5ab27023` until wgpu 31 (done, `023bd97`). Four
   compile fixes, no behaviour change.
2. Keep the safe `wgpu` crate. A `wgpu-core` port buys nothing for error
   semantics: with no wgpu error scopes pushed, every error reaches
   `on_uncaptured_error` synchronously on the JS thread, and den's own scope
   stack routes it exactly like wgpu-core's. The safe crate cannot inject
   content-timeline errors into wgpu's stack, so den keeps one small router
   for both.
3. Delete the software-validation layer, fallback resources, dummy flags,
   WGSL scanners, software bundles, `MAX_NATIVE_PASSES`, `catch_gpu`. Keep
   WebIDL coercion, the buffer mapping state machine with copy-in/copy-out
   `ArrayBuffer`s, `supported.rs`, EventTarget wiring, error classes.
4. Restore native render bundles. Trunk wgpu-core records `Arc` references at
   call time, so the encoder's `'a` borrow is a leftover API constraint; den
   holds handle clones for the bundle's lifetime and extends the borrow in one
   documented `unsafe`.
5. Measure against `cts_runner/test.lst` on radv, not on noop.

## Gaps in the safe wgpu API (found while rewriting, worth filing upstream)

Both are places where wgpu-core can express a WebGPU behaviour but the safe
`wgpu` wrapper cannot, so den cannot be fully spec-correct without them.

- **Render bundle debug groups.** `CommandEncoderInterface`,
  `ComputePassInterface` and `RenderPassInterface` all declare
  `push_debug_group` / `pop_debug_group` / `insert_debug_marker`;
  `RenderBundleEncoderInterface` declares none, and the public
  `RenderBundleEncoder` exposes none. wgpu-core's bundle encoder does record
  those commands and does validate unbalanced groups at finish. den's
  `GPURenderBundleEncoder` debug methods are therefore no-ops, and the CTS
  cases covering unbalanced bundle debug groups cannot pass.
- **Zero-length vertex and index buffer bindings.** `RenderPass::set_vertex_buffer`
  documents "Panics if the buffer slice length is 0", and `check_buffer_bounds`
  panics on an out-of-range slice. WebGPU permits a zero-size binding. den has
  to pre-check the range and skip the call, which unbinds the slot instead of
  binding an empty range, so a following zero-count draw reports a missing
  vertex buffer rather than succeeding.

## Result

Same 22 files, same host, after the rewrite (21 commits, crate down from 11,400
to about 5,000 lines):

| | pass | fail | timeout | crash |
|---|---|---|---|---|
| checkpoint `6790207` | 13 | 7 | 1 | 1 |
| after | 19 | 3 | 0 | 0 |

Both the timeout and the crash are gone, and native render bundles, indirect
draws and immediates now actually execute instead of being skipped.

The three residual failures are not one thing:

- `api/validation/createBindGroup` fails on destroyed-resource state, which is
  wgpu issue 7881 and is 0% for deno as well.
- `api/validation/encoding/render_bundle` fails
  `depth_stencil_readonly_mismatch`, deno's documented "readonly flag
  normalization mismatch", and also `empty_bundle_list`, which deno does not
  list.
- `api/validation/encoding/cmds/render/draw` fails `vertex_buffer_OOB`, which
  is the zero-length `set_vertex_buffer` limit above, and `unused_buffer_bound`,
  which is unexplained.

`empty_bundle_list` and `unused_buffer_bound` are the honest next targets.

## Measuring stick

`cts_runner/test.lst` has 472 selectors touching 253 of den's 696 spec files
(166 whole-file, 87 partial). Plan: copy it verbatim as
`den-stdlib-webgpu/tests/cts.lst`, filter cases in `cts_run.js` with the CTS's
own `compareQueries`, honour `fails-if(<backend>)`, print per-selector
results. Deno's residual failures live in `fail.lst`; anything den still fails
outside that list is a den bug.
