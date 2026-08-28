# Den-first native standard-library roadmap

Snapshot date: 2026-08-27. Deno 2.9.5, Node.js 26.8.0, and Bun 1.4.0 are
competitive inputs, not compatibility targets.[5][6][7]

Per-API evidence: [15](15-stdlib-parity-gap.md).

## Product decision

Den does not expose Node compatibility aliases or attempt drop-in Deno/Bun
compatibility. It takes the strongest capabilities from each runtime, gives
them a smaller coherent API under `den:*` and native globals, and implements
the behavior in Rust.

The standard library obeys four rules:

1. No built-in is installed by evaluating JavaScript or TypeScript bootstrap
   source. Register Rust modules, classes, functions and property descriptors.
2. One backend owns each capability; globals and `den:*` modules delegate to
   it rather than duplicating behavior.
3. Heavy or platform-specific capabilities are Cargo features and stay out of
   minimal builds.
4. Unsupported behavior fails explicitly. No compatibility-shaped stubs.

User programs, workers, the REPL, and module files still enter QuickJS's native
compiler. The prohibition is on evaluated implementation shims, not on running
JavaScript.

## What the competitors prove useful

Deno separates Web globals and the `Deno` namespace.[24][31] Its Node layer
and external JSR standard packages are separate surfaces.[23][26]
Bun's main namespace combines servers, files, processes, databases, cloud
clients, parsers, compression, build tools, workers, testing and
utilities.[14][16] Node's module catalogue shows the depth expected of mature
filesystem, process, networking, stream, crypto, diagnostic and test
facilities.[35][36]

Den should copy capability coverage, not names or legacy semantics.

| Competitor strength | Den-native response |
|---|---|
| Deno permissions and Web APIs | capability permissions plus WPT-tested native Web classes |
| Deno JSR ecosystem | generic signed module registry/cache, not embedded `@std` copies |
| Bun server/file/process ergonomics | focused `den:http`, `den:fs`, `den:process` facades over shared Rust backends |
| Bun databases/cloud/parsers | optional database, object-store and format feature packs |
| Bun speed/build pipeline | Oxc transpilation, native loader/cache and ahead-of-time bundle metadata |
| Node depth and stability | behavior tests and explicit lifecycle/error contracts without legacy aliases |

## Existing native foundation

- `den:assert`, console, timers, base64, text encoding.
- Filesystem, environment/process/signals/DNS.
- TCP, UDP, Unix sockets, TLS and WebSocket.
- Fetch, URL/URLPattern, Blob/File/FileReader/FormData, XHR and EventSource.
- Events, aborts, workers, channels, structured clone, performance, navigator.
- Compression streams, WebCrypto digest/randomness, Temporal.
- WebAssembly core with optional WASI and optional SQLite.
- Oxc TypeScript/JSX transformation and import maps/attributes.
- Insta-backed native snapshots and SurrealKV REPL history.

After this change, no production `den-stdlib-*` implementation calls
`Ctx::eval`; only worker user-source execution remains in that tree.

## Feature roadmap

### P0 — make the runtime trustworthy

| Module/capability | Required behavior |
|---|---|
| `den:permissions` | scoped read/write/net/env/run/ffi/sys permissions; inherited or narrowed worker grants |
| `den:errors` | stable native error classes and machine-readable codes |
| timers | object handles, ref/unref/refresh/dispose, promise delays, deterministic fake clock |
| `den:events` | EventTarget plus ergonomic typed emitter and async iteration |
| `den:bytes` | BufferSource conversions, byte queues, endian readers/writers, constant-time comparison |
| `den:path` | explicit portable/posix/windows path values; glob/walk without host-dependent ambiguity |
| `den:streams` | one Rust backpressure core for Web streams, files, sockets and compression |
| module cache | content-addressed HTTP/JSR-style registry cache, integrity, offline and lockfile modes |

Permissions are required before expanding filesystem, network, process or FFI
surface; they are not a later hardening pass.

### P1 — application and server platform

| Module/capability | Required behavior |
|---|---|
| `den:http` | client/server, routing, cookies, proxy, static files, upgrades and graceful shutdown |
| `den:tls` | keys/certificates, ALPN, mTLS, reloadable server configuration |
| `den:process` | streaming child I/O, pipelines, cancellation, resource usage and portable signals |
| `den:fs` | complete sync/async files, watch, links, permissions, temp resources and atomic writes |
| `den:net` | DNS records, connection/listener options, datagrams, Unix/Windows pipes |
| `den:crypto` | complete WebCrypto plus hashes, HMAC, KDFs, signatures, passwords and secrets |
| `den:compression` | gzip/deflate/Brotli/Zstd streams and one-shot APIs |
| `den:cache` | HTTP cache and CacheStorage backed by content-addressed blobs + SurrealKV metadata |

### P2 — data, testing and operations

| Module/capability | Required behavior |
|---|---|
| `den:test` | native tests/steps, filtering, concurrency, fake time, mocks, coverage, reporters, snapshots |
| `den:bench` | statistically sound warmup/sampling and machine-readable output |
| `den:kv` | transactional KV, watch, TTL, queues and atomic checks over SurrealKV |
| `den:sql` | optional SQLite plus pluggable network SQL clients |
| `den:object-store` | optional S3-compatible client with streaming and multipart transfers |
| `den:formats` | optional TOML/YAML/CSV/JSONC/MessagePack/CBOR/XML/front matter |
| `den:archive` | tar/zip plus streaming compression and safe extraction boundaries |
| `den:observe` | structured logs, metrics, traces, diagnostics channels and heap/task snapshots |
| `den:cron` | persistent schedules, overlap policy, cancellation and missed-run semantics |

### P3 — advanced optional capabilities

- QUIC/WebTransport and HTTP/3.
- Web Storage, CacheStorage persistence and service-style background tasks.
- Canvas/image codecs/HTML rewriting.
- WebGPU as a separate large feature.
- FFI with explicit unsafe permissions and ABI/type validation.
- Native plugin ABI and embedding API.
- Bundler/minifier/package publisher using Oxc.
- Inspector protocol mapped honestly onto QuickJS capabilities.

## Better-than-compatibility differentiators

1. Capability handles rather than ambient authority: resources carry the
   permission that created them and workers can only receive narrowed grants.
2. Structured cancellation on every async operation, not ad-hoc timeout
   options.
3. One stream type across files, sockets, fetch, compression and child I/O.
4. Deterministic test mode spanning timers, randomness, network fixtures and
   snapshots.
5. Content-addressed dependency and HTTP caches with reproducible lockfiles.
6. Minimum-size feature packs: web-client, server, data, test, wasm, GPU, FFI.
7. Native Rust implementation with no evaluated stdlib bootstrap strings.

## First implementation slice

- Timer callbacks must be functions; source strings are rejected, so timers no
  longer evaluate code.
- Variadic timer arguments are forwarded natively.
- `setImmediate` and `clearImmediate` are native globals with cancellation.
- `den:path` provides host-default, explicit POSIX and explicit Windows lexical
  path operations plus native glob matching.
- CI rejects new `eval`/`eval_with_options` calls in standard-library source,
  except the worker user-program execution boundary.

Next: permission primitives, timer handle objects/fake time, and the shared
byte/stream foundations. Those unlock more features than copying any
competitor's module names.

## Acceptance gates

- Export snapshots and behavior tests for every `den:*` module/global.
- WPT where an API is Web-standard; differential competitor fixtures only for
  behaviors Den intentionally adopts.
- Permission-denial tests at every I/O trust boundary.
- Default/minimal/all-feature builds on Linux, macOS and Windows.
- No silent placeholders and no evaluated stdlib bootstraps.
- Benchmarks for runtime latency, throughput, memory, compile time and binary
  size before enabling a heavy feature by default.

## Sources

[5] https://github.com/denoland/deno/releases/tag/v2.9.5
[6] https://github.com/nodejs/node/releases/tag/v26.8.0
[7] https://github.com/oven-sh/bun/releases/tag/bun-v1.4.0
[14] https://bun.sh/docs/runtime/bun-apis
[16] https://github.com/oven-sh/bun/blob/bun-v1.4.0/packages/bun-types/bun.d.ts
[23] https://docs.deno.com/runtime/reference/node_apis
[24] https://docs.deno.com/runtime/reference/web_platform_apis
[26] https://jsr.io/@std
[31] https://github.com/denoland/deno/blob/v2.9.5/cli/tsc/dts/lib.deno.ns.d.ts
[35] https://nodejs.org/download/release/v26.8.0/docs/api
[36] https://nodejs.org/download/release/v26.8.0/docs/api/all.json
