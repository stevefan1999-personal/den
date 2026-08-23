# Den: One word less than Deno!

Just another Rust hobbyist project to learn how to make a JS runtime!

Made during the Easter holiday of 2023.

## Features

- QuickJS (via [rquickjs](https://github.com/DelSkayn/rquickjs)) on a Tokio multi-thread runtime
- TypeScript and JSX transpiled by [oxc](https://github.com/oxc-project/oxc) — parse → semantic →
  transform → codegen, wired into the file and HTTP module loaders
- A standard library exposed both as `den:*` modules and as globals: `console`, `atob`/`btoa`/`gc`,
  `TextEncoder`/`TextDecoder`, timers, `fetch` (`Headers`/`Request`/`Response`), `crypto.subtle.digest`,
  `den:fs`, `den:networking` (TCP/UDP/Unix/TLS), `den:sqlite`, `den:process`
- WinterTC / WHATWG web platform APIs: `AbortController`, `Blob`/`File`/`FileReader`/`FormData`,
  `XMLHttpRequest`, `EventSource`, `URLPattern`, `CompressionStream`, `WebSocket`,
  `performance.now`, `navigator.userAgentData`
- Import maps and import attributes (`json` / `text` / `bytes`)
- The WebAssembly JS API on a choice of two backends, wasmtime or wasmi, selected at compile time
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

Or choose the `min-size-release` profile to get a size-favored build:

```bash
$ cargo build --profile min-size-release
```

Meanwhile, Den will not attempt to make code smaller intentionally, we will and should let the compiler do the job.
If you want to optimize even further, the easiest way is to run `build-std`. You can headout to
the [min-sized-rust guide](https://github.com/johnthagen/min-sized-rust#optimize-libstd-with-build-std) for more.

**(WIP)** In addition, you can also install it as a binary:

```bash
$ cargo install den
```

## Testing

The two WebAssembly backends are mutually exclusive and share the JS-API layer without sharing its
capabilities, so a green run on one says nothing about the other. Both have to pass:

```bash
# wasmtime backend (the default feature set)
$ cargo test --workspace --all-targets

# wasmi backend
$ cargo test --workspace --all-targets --no-default-features \
    --features stdlib,typescript,react,wasm-wasmi
```

`--no-default-features` is not optional for the second one: cargo features are additive, so leaving
the defaults on keeps `wasm-wasmtime` enabled and the build stops on a `compile_error!`.

## Build matrix

State of the build:

| Architecture➡️<br/>Platform⬇️ | i386 | amd64 | arm32 | arm64 | ppc64le | s390x |
|:-----------------------------:|:----:|:-----:|:-----:|:-----:|:-------:|:-----:|
|            Windows            |  ❓   |  ✔️   |  🤷   |   ❓   |   🤷    |  🤷   |
|             Linux             |  ❓   |   ❓   |   ❓   |   ❓   |    ❓    |   ❓   |
|             MacOS             |  🤷  |   ❓   |  🤷   |   ❓   |   🤷    |  🤷   |
|            FreeBSD            |  ❓   |   ❓   |   ❓   |   ❓   |    ❓    |   ❓   |
|            Android            |  🤷  |   ❓   |  🤷   |   ❓   |   🤷    |  🤷   |
|              iOS              |  🤷  |  🤷   |  🤷   |   ❓   |   🤷    |  🤷   | 

_in the format of [\<state\> - \<emoji\> (\<emoji text\>): \<description\>]_

- Perfect - ✅ (greenlit tick)
    - Unit/Integration/E2E tests AC (all cleared)
    - Can be chosen as RC builds
    - Official Docker builds are available for end user
- Choked - ❎ (greenlit cross)
    - Unit/Integration tests failure
    - Implies Passed
    - Should be somewhat usable at this point
- Passed - ✔️ (non-greenlit tick)
    - Build passed
    - Build artifacts maybe provided
- Failed - ❌ (non-greenlit cross)
    - Build failure
- Unknown - ❓ (question mark)
    - Possible but not investigated
- Undefined - 🤷 (shrug)
    - Don't care condition/Impossible/Not applicable

TODO: write a bot that automatically updates the status from CI/CD pipeline, change the format to a markdown table.

TODO: add a triplet table to complete the build matrix/cube a format of _architecture-platform_ **[cartesian product]**
_compiler_

TODO: put the explanations to the design document. Only put the build cube in the frontpage but keep a link to the
design document.

# TODO LIST

Note that this project is still in its pre-alpha and subjects to major re-architect. Den *can* run for now but it is not
yet functional and reliable. I expect this to be at least yearlong to come and I hope I have enough free time to spend
on it.

There are still a lot of bugs that needs to be addressed before it can be deemed functional:

- [ ] MAKE SOME UNIT TESTS AND INTEGRATION TESTS
    - [x] Unit tests for the transpiler, the engine and the standard library
    - [x] Integration tests for the WebAssembly JS API, driven as JS through a real QuickJS context
      on both backends
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
    - [x] Remove the need for the global state. There is only one so far and that is the "global cancellation token"
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
    - [x] Testing: `cargo test` as a matrix over both WebAssembly backends
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

