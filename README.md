# Den: One word less than Deno!

Just another Rust hobbyist project to learn how to make a JS runtime!

Made during the Easter holiday of 2023.

## Features

- QuickJS (via [rquickjs](https://github.com/DelSkayn/rquickjs)) on a Tokio multi-thread runtime
- TypeScript and JSX transpiled by [oxc](https://github.com/oxc-project/oxc) — parse → semantic →
  transform → codegen, wired into the file and HTTP module loaders
- A standard library exposed both as `den:*` modules and as globals: `console`, `atob`/`btoa`/`gc`,
  `TextEncoder`/`TextDecoder`, timers, `fetch` (`Headers`/`Request`/`Response`), `crypto.subtle.digest`,
  `den:assert` (including Insta-backed named snapshots), `den:fs`,
  `den:path` (portable/POSIX/Windows lexical paths),
  `den:networking` (TCP/UDP/Unix/TLS), `den:process`,
  `Temporal` (`temporal_rs`)
- Optional bundled SQLite as `den:sqlite` (`--features stdlib-sqlite`)
- Optional C FFI as `den:ffi` (`--features stdlib-ffi`, off by default; a script still needs
  a `--allow-ffi[=PATH,...]` grant at run time)
- WinterTC / WHATWG web platform APIs: `AbortController`, `Blob`/`File`/`FileReader`/`FormData`,
  `XMLHttpRequest`, `EventSource`, `URLPattern`, `CompressionStream`, `WebSocket`,
  `performance.now`, `navigator.userAgentData` — all native Rust classes, no JS preludes
- Official suites, one nextest test per vendored file (sources are never rewritten):
  test262 Temporal (`cargo nextest run -p den-stdlib-temporal --test test262`),
  WebAssembly spec (`cargo nextest run -p den-stdlib-wasm --test spec_core`),
  WPT (`cargo nextest run -p den-core --test wpt --features stdlib`).
  All three: `cargo nextest run --profile official --build-jobs 8`
- Import maps and import attributes (`json` / `text` / `bytes`)
- The WebAssembly JS API on wasmtime 48, with a `jit` feature (native Cranelift)
  and Pulley for no-JIT / unsupported hosts (App Store, hardened runtime, iOS)
- Optional WASI preview1 imports as `den:wasm`'s `wasiImports` (`--features wasi`)
- Web Workers: `Worker` (classic and module), `MessageChannel`/`MessagePort`, `BroadcastChannel`,
  `EventTarget` and the event classes, `structuredClone` with transfer, and `reportError` — one OS
  thread and one QuickJS runtime per worker, with the spec's error chain and lifetime rules
- Everything above is a cargo feature, so you can compile most of it away

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit together, and
[docs/research/](docs/research/) for the dependency/API research notes behind them.

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

Add the optional SQLite module when it is needed:

```bash
$ cargo build --features stdlib-sqlite
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

An embedder stops a realm by dropping it, plus its own flag in
`runtime.set_interrupt_handler` for a script spinning in bytecode — the
compiled recipe is the rustdoc on `den_core::engine::Engine`. See
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
    --features stdlib,typescript,react,wasm
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
updates with `cargo insta review` (from the separate `cargo-insta` CLI), or use
`INSTA_UPDATE` with ordinary `cargo test`; see the
[Insta documentation](https://docs.rs/insta/latest/insta/).

# TODO LIST

Note that this project is still in its pre-alpha and subjects to major re-architect. Den *can* run for now but it is not
yet functional and reliable. I expect this to be at least yearlong to come and I hope I have enough free time to spend
on it.

There are still a lot of bugs that needs to be addressed before it can be deemed functional:

- [ ] MAKE SOME UNIT TESTS AND INTEGRATION TESTS
    - [x] Unit tests for the transpiler, the engine and the standard library
    - [x] Integration tests for the WebAssembly JS API, driven as JS through a real QuickJS context
      on wasmtime (native Cranelift and Pulley)
    - [x] Unit and integration tests for Web Workers, including the thread boundary, the error
      chain and the process-lifetime rule
    - [ ] End-to-end tests that actually run the `den` binary (the worker suite spawns a child
      *test* process to capture stderr, which is as close as it gets today)
- [x] Detect when the task list is empty and is safe to shutdown (like Node)
    - `AsyncRuntime::idle()` is the whole rule: den exits when nothing is spawned. A worker only
      keeps the process alive while it is doing work or something is still listening to it — see
      [ARCHITECTURE.md](ARCHITECTURE.md) §7.5. The process still exits 0 on an uncaught error,
      which is the next thing to fix.
- [x] Make it easily embeddable to other Rust projects
    - [x] Remove the need for the global state. The last of it was the "global cancellation token":
      an `Engine` now carries no realm-wide cancellation at all — stopping one is dropping it, and a
      host that has to interrupt a running script installs its own interrupt flag. See
      [docs/research/16](docs/research/16-cancellation-without-tokens.md) and
      [17](docs/research/17-graceful-shutdown-and-external-stop.md).
    - This is also important because we can reuse it to test the standard library
    - Better yet, integrate some crates and libraries to upstream rquickjs so everybody can enjoy
- [ ] Finish up the standard libraries
    - [ ] Rewrite [RegExp](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/RegExp)
      using [rust-lang/regex](https://github.com/rust-lang/regex)
    - [ ] Rewrite [BigInt](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/BigInt),
      BigFloat and BigDecimal using [rust-num/num](https://github.com/rust-num/num)
        - Although I hardly doubt it will work because bignum is a language-level construct
- [x] Mark more parts of the code as features and let user to selective include them
- [ ] Filling up comments and documentations once it is stable in the future (I hope so)
- [x] Figure out how to expose Rust modules as one big module. You don't want to cherrypick each exposed Rust rquickjs
  module in one big Rust module
- [ ] Add GH Actions manifests to automate CI/CD workflow such as linting, testing and build release
    - [x] Linting: `cargo clippy` and `cargo fmt --check` (`.github/workflows/lint.yml`)
    - [x] Docs: `cargo doc --no-deps`
    - [x] Testing: `cargo nextest run` as a matrix over `jit` and Pulley
    - [ ] Release builds and artifact publishing
- [ ] Add GH Workspace config or Nix to have consistent build environment
    - There is a `devfile.yaml` for Che/OpenShift Dev Spaces, but no devcontainer and no Nix flake
- [ ] Add [tracing](https://docs.rs/tracing/latest/tracing/) support and also instruments
    - [x] `tracing-subscriber` installed in `main` with `RUST_LOG` filtering, and `console.*` routed
      to `tracing` instead of `println!`
    - [x] Optional `tokio-console` support behind the `tokio-console` feature (needs
      `--cfg tokio_unstable`)
    - [ ] Actual `#[instrument]` spans on the runtime's own code paths

# Contributors

#
