# Research notes

Seven notes written while planning the big dependency upgrade, in the order they were needed:
[01](01-rquickjs-0.8-to-0.12.md) is the rquickjs 0.8.1 → 0.12.2 migration (the API breaks and every
den call site they touch); [02](02-wasmtime-27-to-48.md) is wasmtime/wasmtime-wasi 27 → 48,
including the `T: 'static` store-payload bound that forced `OwnedCtx`; [03](03-wasmi-1.1-api.md) is
the wasmi 1.1 embedder API, read as the candidate second WebAssembly backend and mapped item by
item onto wasmtime's; [04](04-swc-to-oxc-transpiler.md) is the swc → oxc 0.146 transpiler swap —
the parse/semantic/transform/codegen pipeline, the arena lifetimes and the behaviours den had to
keep; [05](05-webassembly-js-api-spec.md) is the WebAssembly JS API spec surface with a conformance
checklist that `den-stdlib-wasm` was built against; [06](06-misc-dependency-bumps.md) covers every
other bump plus the edition 2021 → 2024 move; and [07](07-den-architecture-and-test-strategy.md) is
the pre-upgrade architecture map and the test strategy that came out of it.

Four more were written for the Web Workers API, after the upgrade had landed:
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

**These are snapshots, not living documents.** Each was written against a specific set of vendored
crate versions at a specific moment, and their `file:line` references point at the tree *before*
the upgrade landed — doc 07 says outright that the workspace did not compile when it was written.
Several claims also did not survive contact with the compiler: the notes carry their own
verification logs recording corrections found on a second pass, and implementation turned up more.
Treat them as a strong prior on why a thing is the way it is, not as gospel about what the code
currently does. For that, read [ARCHITECTURE.md](../../ARCHITECTURE.md) or the code.
