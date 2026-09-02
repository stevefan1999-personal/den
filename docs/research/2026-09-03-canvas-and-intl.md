# den-stdlib-canvas and Intl on ICU4X

Date: 2026-09-03. Status: research complete, scope proposed, nothing implemented.

Sources: `docs.deno.com/api/web/canvas`, the local Deno checkout
`~/git/github.com/denoland/deno` at `1aa88c03` (`ext/canvas`, `ext/image`,
`ext/webgpu/canvas.rs`), ICU4X 2.3 docs, and den itself.

## Canvas

Deno's canvas surface is small, headless and almost entirely CPU-bound. There
is **no 2D context anywhere**. `OffscreenCanvas.getContext` accepts exactly
`bitmaprenderer` and `webgpu` and returns null for anything else.

The whole documented surface is: `ImageData` (which lives in Deno's `ext/web`,
not `ext/canvas`), `ImageBitmap` and `createImageBitmap`, `OffscreenCanvas`,
`ImageBitmapRenderingContext`, and `GPUCanvasContext`. Plus the unstable
`Deno.UnsafeWindowSurface`, which needs a real window handle and stays out of
scope for den.

`OffscreenCanvas` is just an `Rc<RefCell<DynamicImage>>`.
`transferToImageBitmap` swaps that buffer and `convertToBlob` runs an encoder.

**getContext('webgpu') never touches a `wgpu::Surface`.** The offscreen variant
configures a plain texture descriptor, `getCurrentTexture` is a
`create_texture`, and presentation is a readback: copy texture to buffer,
submit, map, poll, strip the 256-byte row padding. den already calls every one
of those. Two Deno bugs are worth not copying: the readback hardcodes 4 bytes
per pixel, so `rgba16float` reads back garbage, and it never swizzles, so a
`bgra8unorm` canvas comes out channel-swapped.

Five places where den can be more correct than Deno for free: throw
`InvalidStateError` on a second `getContext` with a different id (Deno returns
null, contradicting the HTML spec); ASCII-lowercase the `convertToBlob` MIME
type; ignore rather than clamp an out-of-range quality; do real WebIDL overload
resolution in the `ImageData` constructor; accept `imageOrientation: 'none'`.

What den already has: `Blob` and `File` are full native classes, and rquickjs
models `Uint8ClampedArray` as `TypedArray<'js, U8Clamped>`, which is exactly
`ImageData.data`. What it lacks: native classes cannot cross `postMessage` at
all today (`Blob` already fails), so `ImageBitmap` transferability is a
separate, generic piece of work.

Dependencies: the workspace has no image library at all. Phase 0 (`ImageData`,
`ImageBitmap` from `ImageData`) needs **zero** new dependencies. Only
`createImageBitmap(Blob)` and `convertToBlob` need codecs. Note `lcms2` is a
non-decision: `moxcms`, pure Rust, is already a mandatory dependency of
`image` 0.25, so ICC costs about 38 KB and no extra crates once `image` is in.

Testing: WPT is not a scoreboard here. `html/canvas` is 4,737 files of which
1,471 are `.html` and 1,019 are `.worker.js` that nearly all call
`getContext('2d')`; only 3 are `.any.js`, and den's WPT harness only collects
`.any.js`. Hand-written tests are the honest option.

Deno ships `DENO_DISABLE_OFFSCREEN_CANVAS` because exposing `OffscreenCanvas`
without a 2D context breaks feature-detecting libraries. If den exposes it,
it needs the same opt-out from the first commit.

## Intl

quickjs-ng has no Intl at all, so every object must be written natively against
ICU4X. ICU4X 2.3 is already in `Cargo.lock` via `temporal_rs`.

**The measuring stick already exists and is mostly free.** `vendor/test262`
carries 3,357 `intl402` files, and 2,029 of them live under `intl402/Temporal`.
Measured directly rather than assumed:

| slice | files | needs an Intl constructor in code |
|---|---|---|
| intl402/Temporal | 2029 | 45 |
| intl402 non-Temporal | 1328 | 1230 |

So about 1,984 files test non-ISO calendars in Temporal and need no Intl object
at all. Running every twelfth one of them through the current release binary,
**141 of 167 pass, 84%**, so pointing the harness at that directory scores
roughly 1,600 tests immediately, before a line of Intl is written. The failures
in that sample are real Temporal bugs worth fixing on their own: eras rejected
with `TypeError` where the spec wants `RangeError`, `Invalid PlainDate fields`
on era boundaries in `islamic-tbla`, `japanese` and `roc`, and `toLocaleString`
returning an ISO string because it currently ignores its arguments.

They are invisible today because the harness
(`den-stdlib-temporal/tests/test262.rs`) roots only at
`test/built-ins/Temporal` and never walks `test/intl402`. Pointing it at that
second subtree needs no new crate, no new Cargo stanza and no nextest change,
because `binary(test262)` is already in all three profiles.

Note the feature gate at `should_skip` skips any file whose `features:` starts
with `Intl`. Within `test/built-ins/Temporal` that hides only 10 files, all
tagged `Intl.Era-monthcode`, which is a calendar feature, not an Intl object.
The gate matters much more once `intl402` is walked, where 1,556 files carry
that tag.

Costs, measured under den's own `min-size-release` profile against a 39 MB
binary: about +4.8 MiB for the stable ICU4X component set, about +6.2 MiB with
`icu_experimental`. ICU4X compiled data is **not iterable**, so
`supportedLocalesOf` cannot be answered from it and den needs its own
available-locale list.

Intl is not only new globals. It replaces methods quickjs-ng already defines
natively (`toLocaleString` on Number, Date, Array, TypedArray, Object;
`localeCompare` and `toLocaleUpperCase` on String) and must take over den's own
Temporal `toLocaleString`, which today ignores its arguments and aliases
`toString`. Everything installed at realm creation is also paid per worker,
since each den Worker builds a full second engine.

Known ICU4X gaps: three of the six `DisplayNames` types have no data marker,
`NumberFormat` `style: "unit"` covers only some unit categories, collator
`usage: "search"` is missing, and `DurationFormat` needs Temporal duration
plumbing first. `DateTimeFormat` is a shape mismatch, not an effort problem:
ICU4X 2.x exposes semantic skeletons rather than arbitrary component bags.

## Proposed order

1. Point the existing test262 harness at `test/intl402` and fix the feature
   gate. No new crate. Gives a real scoreboard before any Intl code exists.
2. Canvas phase 0 with zero new dependencies: `ImageData`, then `ImageBitmap`
   and `createImageBitmap` from `ImageData` and `ImageBitmap`.
3. Intl phase 1 with no formatter data: `Intl.Locale`,
   `Intl.getCanonicalLocales`, the `Intl` namespace, and the shared
   `GetOption`/`ResolveLocale` plumbing every later constructor reuses.
4. Codecs (`image`) only when `createImageBitmap(Blob)` or `convertToBlob` is
   actually wanted.
5. `OffscreenCanvas` and `GPUCanvasContext` only when something needs
   `getContext('webgpu')`; the arrow is canvas depends on webgpu, optional.

Deferred with reasons: `ImageBitmap` transferability (needs generic structured
clone for native classes first), BYOW and any 2D context (permanently),
`DisplayNames` and `DurationFormat` (ICU4X data gaps).
