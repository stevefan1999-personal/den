# Den: One word less than Deno!

Just another Rust hobbyist project to learn how to make a JS runtime!

Made during the Easter holiday of 2023.

## Features

- QuickJS (via [rquickjs](https://github.com/DelSkayn/rquickjs)) on a Tokio multi-thread runtime
- A clap-derived CLI with default/explicit `run`, `eval`, `repl`, shell completions,
  script-argument forwarding, and feature-gated FFI grants
- TypeScript and JSX transpiled by [oxc](https://github.com/oxc-project/oxc) — parse → semantic →
  transform → codegen, wired into the file and HTTP module loaders
- A standard library exposed both as `den:*` modules and as globals: `console`, `atob`/`btoa`/`gc`,
  `TextEncoder`/`TextDecoder`, timers, `fetch` (`Headers`/`Request`/`Response`), `crypto.subtle.digest`,
  `den:assert` (including Insta-backed named snapshots), `den:fs`,
  `den:http` (cleartext HTTP/1 and HTTP/2 prior-knowledge server with a Fetch handler, graceful
  drain, and 16 MiB buffered-body limit),
  `den:kv` (durable byte CRUD and snapshot transactions over SurrealKV),
  `den:path` (portable/POSIX/Windows lexical paths),
  `den:networking` (TCP/UDP/Unix/TLS), `den:process`,
  `Temporal` (`temporal_rs`)
- Bundled SQLite as `den:sqlite` (selectable alone with
  `--no-default-features --features stdlib-sqlite`)
- C FFI as `den:ffi` (selectable alone with `--no-default-features --features stdlib-ffi`; a script still needs
  a `--allow-ffi[=PATH,...]` grant at run time)
- WinterTC / WHATWG web platform APIs: `AbortController`, `Blob`/`File`/`FileReader`/`FormData`,
  `XMLHttpRequest`, `EventSource`, `URLPattern`, `CompressionStream`, `WebSocket`,
  `performance.now`, `navigator.userAgentData` — all native Rust classes, no JS preludes
- Official suites, one nextest test per vendored file (sources are never rewritten):
  test262 Temporal (`cargo nextest run -p den-stdlib-temporal --test test262`),
  WebAssembly spec (`cargo nextest run -p den-stdlib-wasm --test spec_core`),
  WPT (`cargo nextest run -p den-core --test wpt --features stdlib`).
  All three: `cargo nextest run --profile official --build-jobs 8`
  WPT expects the official `vendor/wpt` server on ports 8000–8002; the CI workflow contains the
  matching startup and cleanup command.
- Import maps and import attributes (`json` / `text` / `bytes`)
- The WebAssembly JS API on wasmtime 48, with a `jit` feature (native Cranelift)
  and Pulley for no-JIT / unsupported hosts (App Store, hardened runtime, iOS)
- Optional WASI preview1 imports as `den:wasm`'s `wasiImports` (`--features wasi`)
- TLS uses ring by default. Without the `ring` feature Den leaves Rustls provider and
  cipher-suite selection to the embedding application.
- Web Workers: `Worker` (classic and module), `MessageChannel`/`MessagePort`, `BroadcastChannel`,
  `EventTarget` and the event classes, `structuredClone` with transfer, and `reportError` — one OS
  thread and one QuickJS runtime per worker, with the spec's error chain and lifetime rules
- Everything above is a cargo feature, so you can compile most of it away
- Host-side foundations for deny-by-default capability policies and bounded
  engines, plus a SeaORM-managed SQLite content-addressed package store using
  Resolvo for deterministic flat dependency solving. Hosts can hydrate an
  immutable package-module snapshot into `EngineBuilder`; registry fetching,
  lockfiles, and package commands are not wired into the CLI yet.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together. Closed
research remains available in Git history.

# Build instruction

## Steps

Run the following command to get a debug build:

```bash
$ cargo build
```

Run the following command to get an optimized release build:

```bash
$ cargo build --release
```

The checked-in Cargo config also makes the same command work for
`x86_64-unknown-linux-musl`. Plain musl-gcc sysroots omit Linux UAPI headers,
so two private compatibility headers keep libffi's static-trampoline hardening
enabled without mixing host headers into the musl build. Static musl cannot
`dlopen`, so `den:ffi` runtime loading requires a dynamically linked build;
CI exercises it with the pinned [musl nextest image](.github/musl-nextest.Dockerfile).

Build only the SQLite standard-library module when it is needed:

```bash
$ cargo build --no-default-features --features stdlib-sqlite
```

Or choose the `min-size-release` profile to get a size-favored build:

```bash
$ cargo build --profile min-size-release
```

On Linux/ELF, also remap build paths, disable QuickJS's native assertions, and
enable identical-code folding plus packed relocations:

```bash
$ ./scripts/min-size-linux
```

Add `--features wasi` to that command when preview1 imports are required.

The aggressive artifact is isolated under `target/<host-triple>/min-size-release`.

This profile stays on stable Rust. A nightly toolchain can go further by rebuilding
the standard library; see the [min-sized-rust `build-std` guide](https://github.com/johnthagen/min-sized-rust#optimize-libstd-with-build-std).

For stalled async tasks, leaked spawns, or I/O that never wakes, build with the
Tokio console instrumentation and attach the `tokio-console` client:

```bash
$ RUSTFLAGS="--cfg tokio_unstable" cargo run --features tokio-console -- app.ts
```

### Embedded bytecode modules

Applications embedding `den-core` can compile fixed ES modules into their Rust
binary with rquickjs's existing `embed!` macro:

```toml
rquickjs = { version = "0.12.2", features = ["loader", "macro", "phf"] }
```

```rust
use den_core::engine::Engine;
use rquickjs::{embed, loader::Bundle};

static MODULES: Bundle = {
    // Keep this dependency in the same item so JS-only edits rerun `embed!`.
    const _: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/js/main.js"));
    embed! {
        "app:/main.js": "js/main.js",
    }
};

async fn run() -> Result<(), den_core::engine::EngineError> {
    let engine = Engine::new_with_bundle(MODULES).await;
    engine.run_module("app:/main.js").await
}
```

The macro compiles on the build host, so embedded bytecode must use the same
QuickJS version and endianness as the target. It accepts JavaScript modules,
not classic scripts or TypeScript/JSX; bundle or transpile those before calling
the macro. List every module; static dependencies must precede their importers,
and cyclic static dependency graphs must be bundled first. Keep one
`include_bytes!` dependency inside the `MODULES` item for every embedded input,
otherwise a JS-only edit can reuse stale incremental bytecode.

**(WIP)** In addition, you can also install it as a binary:

```bash
$ cargo install den
```

## Stopping den

Ctrl-C is the kernel's by default: den installs no SIGINT handler, so the
process dies at the signal's default disposition, like Node, Deno and Bun. A
script that wants a graceful stop asks for the signal, and then owns
termination — nothing exits on its behalf:

```js
import { addSignalListener, removeSignalListener, exit } from "den:process";

const goodbye = async () => {
  removeSignalListener("SIGINT", goodbye); // first: a second Ctrl-C is kernel death again
  setTimeout(() => exit(130), 5000);       // the deadline for everything below
  closing = true;                          // stop taking new work
  await Promise.allSettled(inFlight);      // den stays alive while these are pending
  await conn.shutdown();                   // protocol goodbyes, not durability
  exit(0);                                 // mandatory: nothing else ends the process
};

addSignalListener("SIGINT", goodbye);
```

An embedder stops a realm by cancelling its program future, awaiting
`Engine::shutdown()`, then dropping it; its own `runtime.set_interrupt_handler`
flag stops a script spinning in bytecode. The compiled recipe is the rustdoc on
`den_core::engine::Engine`. See
[ARCHITECTURE.md](ARCHITECTURE.md) §2.

## Testing

A green `jit` run says nothing about Pulley: same JS-API layer, different compiler
target. Both have to pass:

```bash
# wasmtime + native Cranelift (the default feature set)
$ cargo nextest run --workspace --build-jobs 8

# den unit tests only (skip official vendor binaries)
$ cargo nextest run --profile compat --workspace --build-jobs 8

# wasmtime + Pulley (no JIT pages)
$ cargo nextest run --workspace --build-jobs 8 --no-default-features \
    --features stdlib,typescript,react,wasm,wasi,ring
```

`--no-default-features` is required for the Pulley run: cargo features are additive,
so leaving the defaults on keeps `jit` enabled. aarch64-apple-darwin *has*
Cranelift; App Store / hardened runtime / iOS builds omit `jit` on purpose.

For the fastest edit loop, test the owning crate instead of unifying every
workspace feature into every test binary:

```bash
$ cargo nextest run -p den-stdlib-worker --test integration --build-jobs 8
```

The test profile keeps line-number backtraces but omits dependency DWARF, which
materially reduces linker memory and target size. CI disables incremental
compilation, allowing a configured `sccache` runner to cache Rust dependencies;
local incremental builds remain enabled for quick rebuilds.

The REPL stores its bounded history in the `history.surrealkv` directory using
[SurrealKV](https://github.com/surrealdb/surrealkv) with immediate commits. SurrealKV locks a store to one process; a
second REPL in the same directory falls back to in-memory history instead of
failing to start.

### Snapshot assertions

`den:assert` exposes Insta-backed named snapshots:

```js
import { assertSnapshot } from "den:assert";

assertSnapshot(JSON.stringify(value), "stable_name");
```

Names are required because JavaScript calls share one Rust assertion site;
snapshots live under `./snapshots` in the process working directory. Review
updates with `cargo insta review` (from the separate `cargo-insta` CLI), or set
`INSTA_UPDATE` on a focused `cargo nextest run`; see the
[Insta documentation](https://docs.rs/insta/latest/insta/).
