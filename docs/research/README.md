# Research notes

Four notes were written for the Web Workers API:
[08](08-web-workers-spec.md) is the conformance checklist for dedicated `Worker`,
`EventTarget`/`Event`/`MessageEvent`/`ErrorEvent`, `MessageChannel`/`MessagePort`, `BroadcastChannel`
and `structuredClone`, with every N/A (origins, CSP, SharedWorker, ServiceWorker) named rather than
forgotten; [09](09-rquickjs-threads-and-event-loop.md) is what rquickjs 0.12 actually guarantees for
one runtime per OS thread — Send/Sync under `parallel`, the `idle()`/`drive()`/`ctx.spawn` semantics
that make a live worker keep the process alive, interrupt-based termination, the tokio flavour den's
loaders force, and two rquickjs bugs found by its compile-and-run probe; [10](10-structured-clone-strategy.md)
is the case for riding quickjs-ng's own `JS_WriteObject2` serializer (the same one its `os.Worker`
uses) plus a small JS pre-pass, including a quickjs-ng `Map` serialisation bug the pre-pass has to
sidestep; and [11](11-workers-den-integration-and-tests.md) places it all in den — the `WorkerHost`
seam that keeps `den-stdlib-worker` from depending on `den-core`, shutdown and joining, error
propagation, and the test plan.

[12](12-wintertc-txiki-gap.md) maps the WinterTC / txiki.js web-platform surface onto
den: what already exists, the JS-prelude vs Rust-native split (copy txiki's split, not
its C stack), and the file ownership for the parallel worktrees that implement the gap.

[13](13-rquickjs-macros-shutdown-temporal.md) is the rquickjs 0.12 module-macro /
`idle()`+interrupt shutdown / IndexMap `IntoJs` note, plus wrapping
`temporal_rs` and running test262 Temporal.

[14](14-runtime-feature-roadmap.md) compares Deno 2.9.5, Node 26.8.0 and Bun
1.4.0 as product inputs, then defines a Den-first native-Rust standard-library
roadmap without compatibility aliases or evaluated bootstrap code.

[15](15-stdlib-parity-gap.md) is the per-API evidence behind that roadmap: 2112
rows across 26 domains comparing den's working tree against locally installed
Node 26.5.0, Deno 2.9.4 and Bun 1.3.9 binaries, gathered by reflecting each
runtime's globals and built-in modules, reading every den crate at source
level, and re-verifying each table with a second independent pass. It scores
80% of rows missing or partial, names the five defects (exit 0, no argv, empty
`import.meta`, `number[]` bytes, console formatting) that recur as P0 rows in
18 domains, and marks the rows den should deliberately never build.

[16](16-cancellation-without-tokens.md) asks why den needs a `stop_token` at all
when Node, Deno and Bun have no equivalent, and answers it with strace: all
three die of the signal rather than unwinding, abandoning pending fetch and
accept, so den's cooperative-cancellation plumbing buys behaviour nobody else
has. It proves by probe that dropping an rquickjs runtime with pending
`ctx.spawn` futures — and dropping `idle()` mid-await — is sound, then picks the
design that follows: the process is the scope, SIGINT is owned by
`den-stdlib-process`, and the root loop becomes `select!(signal inbox, idle())`,
with a migration plan that starts by deleting code.

[17](17-graceful-shutdown-and-external-stop.md) answers the follow-up: how an
embedder stops a running den, and what "gracefully shut down all resources"
buys. It splits the one word "stop" into the three operations it conflates —
kill (the kernel's), interrupt of a tight bytecode loop (the QuickJS flag's) and
drain (the owner of `idle()`'s) — and finds an owner for each that already
exists, so the embedder recipe is a host-owned flag plus a `select!` plus
`drop(engine)`, with no new type on `Engine`. It then kills the file-buffer
worry by probe: den holds no user-space write buffer, so abrupt death equals
graceful stop on every path except three that tear (whole-file `write`, `copy`,
`assertSnapshot`), of which only `write` gets a fix — an opt-in
`{ atomic: true }` that renames a tempfile over the target. Graceful Ctrl-C
stays a JS signal listener that removes itself and calls `exit()`, the same
idiom Node, Deno and Bun document, measured here against all three.

[18](18-den-http.md) answers 17's open q7 — den has no `serve()` and no listener
`close()` — with a whole HTTP server that is one `ctx.spawn`. It rests on the
fact that `Ctx::spawn` carries no `Send` bound and neither does hyper's h1
service path, so a hyper connection can be a spawned future whose service holds
a JS `Function<'js>` directly; probed for h1 and, with a four-line `LocalExec`,
for real h2 as well. Exactly one hop is forced (`S::ResBody: 'static`), and the
`mpsc::channel(1)` behind it is shown by probe not to be the backpressure people
assume — about 3.4 MiB sits in hyper and socket buffers first. Liveness and
drain fall out of existing rules rather than new ones, though hyper-util's
graceful helper turns out to be incompatible with h1 upgrades and is replaced by
twelve hand-rolled lines. It also finds three blockers in code den already
ships: a rooted `ReadableStream` aborts the runtime at teardown (`rc=134`, so
every shutdown under traffic aborts), `set-cookie` is a silent no-op on
`Response`, and request headers passed as a plain object silently lose `Host`
and `Cookie`. Routing is `matchit` for its conflict errors at bind time, TLS is
rustls because native-tls cannot do ALPN on macOS, and the WebSocket engine den
already has needs one new constructor and zero new crates.

[19](19-den-ffi.md) designs `den:ffi` as a plain-data symbol table and then
argues against building it this cycle: every dlopen/pointer/callback row in
[15](15-stdlib-parity-gap.md) §3.18 is P3, gated behind a den:permissions
surface that is P0 and does not exist, while the P0/P1 rows in that section are
the Rust embedding API and wasm. What it establishes for when the turn comes:
only libffi's `middle` layer can express a runtime signature, `call_return_into`
writes exactly `type.size()` bytes (canary-probed) while `call<R>` widens to a
register and is the real out-of-bounds trap, `Cif` is `!Send` so it must be
rebuilt on the far side of any thread hop, and a `!Send` closure buys nothing
because C calls the trampoline from whatever thread it likes. The two teeth: a
sync call whose C function fires a callback from a spawned thread deadlocks den
outright (`exit=124`), refused at marshal time but not closable in general; and
a panic or JS throw inside an `extern "C"` trampoline aborts the process, so
every body needs `catch_unwind`. `BigInt::to_i64` corrupts out-of-range values
silently in two distinct ways. The standing recommendation is wasm as den's
sanctioned portable-native path.

**These are snapshots, not living documents.** Treat them as context for design decisions, not as
the current source of truth. For that, read [ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.
