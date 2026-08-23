# WinterTC surface vs den, mapped onto txiki.js

Snapshot written 2026-08-23 against den `959abfd` and a local checkout of
[saghul/txiki.js](https://github.com/saghul/txiki.js) at
`/home/steve/git/github.com/saghul/txiki.js`. Not a living document.

txiki does not vendor a WPT/WinterTC checkout. Its "WinterTC suite" is the
`tests/test-*.js` files plus JS polyfills under `src/js/polyfills/`. Those are
the behavior source for this work. rquickjs 0.12 already threads
`ImportAttributes` through `Resolver`/`Loader`
(`rquickjs-core-0.12.2/src/loader.rs`).

## What den already has

EventTarget/Event family, structuredClone, workers, fetch GET, TextEncoder/Decoder,
console, timers, atob/btoa, crypto.getRandomValues/randomUUID, wasm JS API,
TCP connect/listen, a path-oriented `den:fs` (several methods still throw),
sqlite, HTTP+file module load, in-process TS via oxc.

`DOMException` is a QuickJS-ng intrinsic (`JS_AddIntrinsicAToB`). Do not
reinstall it.

## Implementation shape (copy txiki's split, not its C stack)

- **JS prelude** (like `den-stdlib-worker`) for anything that `extends EventTarget`
  or is spec'd as a JS class: AbortController, Blob, File, FileReader, FormData,
  XMLHttpRequest, EventSource, Headers/Request extras, CompressionStream.
- **Rust natives** for bytes-in/bytes-out: `subtle.digest` (sha2), flate2
  compressor, tokio fs/process/net/signal, `url`/`urlpattern` crates, tungstenite
  WebSocket.
- Do **not** vendor libuv, libwebsockets, mbedtls, WAMR, ada, tweetnacl, miniz.

## File ownership for parallel worktrees

| Slice | Owns | txiki sources |
|---|---|---|
| abort-nav | `den-stdlib-worker` prelude + natives | `polyfills/abort-controller.js`, `navigator.js`, `performance.js`; tests `test-abort-controller.js`, `test-navigator-useragentdata.js`, `test-performance.js` |
| crypto-fs | `den-stdlib-crypto`, `den-stdlib-fs` | `polyfills/crypto/digest.js`; fs tests `test-fs.js`, `test-fs-stat.js`, `test-fs-readdir.js` |
| process | new `den-stdlib-process` | `test-env.js`, `test-args.js`, `test-pid-ppid.js`, `test-signal.js`, `test-lookup.js`, `test-exec.js` (spawn only, skip `exec()` image replace) |
| import | `den-core` loader/resolver | `core/import-map.js`; `test-import-map.js`, `test-import-json.js`, `test-import-text.js`, `test-import-bytes.js` |
| net | `den-stdlib-networking` | `test-udp.js`, `test-pipe.js`, `test-tls-echo.js` |
| whatwg | new `den-stdlib-whatwg` + fetch crate | blob/file/file-reader/form-data/xhr/eventsource/compression-streams/ws.js, fetch/{headers,request,body,fetch}.js |

Engine wiring (`den-core/src/engine.rs`, workspace `Cargo.toml`) is shared: add
only your own `cfg` block / member, unique names, no drive-by edits.

## Test translation

txiki tests `import assert from 'tjs:assert'`. den tests eval JS through
`Engine` or a crate-local realm (see `den-stdlib-worker/src/lib.rs` `realm()`)
and return a comma-separated list of failed check names, like
`den-core/tests/stdlib.rs`. Do not add a `den:assert` crate.
