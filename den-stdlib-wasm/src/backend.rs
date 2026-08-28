//! wasmtime 48 integration for the WebAssembly JS API.
//!
//! This module owns the den-specific pieces: the store payload
//! ([`OwnedCtx`], [`StoreData`]), the engine [`Config`] ([`new_engine`]), and
//! optional WASI linking. Wasmtime types are used directly.
//!
//! `Store`, `Linker` and `Caller` are aliases only because they are
//! parameterized on [`StoreData`]. `Engine`, `Module`, `Val`, … are not
//! aliased — they are wasmtime's types.
//!
//! `jit` is a den cfg that chooses the *target triple*, not whether Cranelift
//! is linked. Cranelift is always a wasmtime cargo feature so modules compile
//! at runtime; without `jit` (or on a host ISA Cranelift does not target)
//! [`new_engine`] sets `Config::target("pulley64")` / `"pulley32"` so
//! Cranelift emits portable Pulley bytecode instead of native RWX pages.
//!
//! # Which proposals this build accepts
//!
//! [`new_engine`] spells the proposal set out rather than inheriting wasmtime's
//! defaults, so a defaults change shows up as a diff rather than as a module
//! that quietly stops validating.
//!
//! | Proposal                        | accepted |
//! |---------------------------------|----------|
//! | MVP + mutable-global            | yes      |
//! | sign-extension, sat-float-to-int| yes      |
//! | multi-value, multi-memory       | yes      |
//! | bulk-memory, reference-types    | yes      |
//! | tail-call, extended-const       | yes      |
//! | memory64                        | yes      |
//! | threads (shared memory, atomics)| **off**  |
//! | custom-page-sizes               | off      |
//! | wide-arithmetic                 | off      |
//! | simd, relaxed-simd              | yes      |
//! | gc / `anyref`                   | yes      |
//! | exceptions / tags               | yes      |
//! | function-references             | yes      |
//!
//! WASI preview1 is not a proposal but a host API, and it is opt-in rather than
//! configured; the `wasi` feature adds preview1 host imports.
//!
//! Threads is the one proposal turned *off* where the engine offers it: den
//! cannot represent a shared memory at the JS boundary, so accepting
//! `(memory 1 1 shared)` would only buy a module that instantiates and then
//! cannot be wrapped.

use rquickjs::Ctx;
use wasmtime::{Config, Engine, Error, ExternType, Module};
#[cfg(feature = "wasi")]
use wasmtime_wasi::p1::WasiP1Ctx;

/// The one store a JS context owns, parameterized on den's payload.
pub type Store = wasmtime::Store<StoreData>;
/// Linker bound to [`StoreData`].
pub type Linker = wasmtime::Linker<StoreData>;
/// Host-callback caller bound to [`StoreData`].
pub type Caller<'a> = wasmtime::Caller<'a, StoreData>;

/// The one import namespace `link_wasi` defines, and therefore the only one
/// `den:wasm`'s `wasiImports()` may be passed as.
#[cfg(feature = "wasi")]
pub const WASI_NAMESPACE: &str = "wasi_snapshot_preview1";

/// Host ISAs Cranelift can emit native code for. Everything else — and any
/// build that leaves `jit` off — compiles wasm to Pulley bytecode via the
/// same Cranelift frontend (`Config::target`).
const HOST_HAS_CRANELIFT: bool = cfg!(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "s390x",
    target_arch = "riscv64",
));

/// True when this build compiles wasm to Pulley rather than native code.
///
/// `jit` chooses the *target triple*, not whether Cranelift is linked:
/// Cranelift is always a wasmtime cargo feature so modules can be compiled
/// at runtime. aarch64-apple-darwin *has* Cranelift; App Store / hardened
/// runtime / iOS builds leave `jit` off so they still emit Pulley.
const USES_PULLEY: bool = !cfg!(feature = "jit") || !HOST_HAS_CRANELIFT;

/// The store payload parks a [`den_util::OwnedCtx`]: wasmtime 48 bounds
/// `T: 'static` on `Linker`/`Instance`/`Func`, so it cannot borrow `'js`.
/// Re-exported because it is named throughout this crate's API; it lives in
/// `den-util` because `den-stdlib-ffi`'s libffi trampoline needs the same
/// handle.
pub use den_util::OwnedCtx;

/// Payload of the single [`Store`] den keeps per JS context.
///
/// `'static` by construction — see [`OwnedCtx`]. With the `wasi` feature, its
/// context is created lazily by `link_wasi`: building one inherits the
/// host's stdio and environment, which no context may be handed until a script
/// has asked for WASI by name.
pub struct StoreData {
    ctx:  OwnedCtx,
    #[cfg(feature = "wasi")]
    wasi: Option<WasiP1Ctx>,
}

impl StoreData {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        Self {
            ctx:                           OwnedCtx::new(ctx),
            #[cfg(feature = "wasi")]
            wasi:                          None,
        }
    }

    /// The store's WASI context, built on first use by `init`.
    #[cfg(feature = "wasi")]
    pub fn wasi_or_init(&mut self, init: impl FnOnce() -> WasiP1Ctx) -> &mut WasiP1Ctx {
        self.wasi.get_or_insert_with(init)
    }

    /// Run `f` with the JS context that owns this store.
    pub fn with_ctx<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R { self.ctx.with(f) }
}

/// Every proposal den depends on, spelled out — see the module docs for the
/// table.
///
/// wasmtime's defaults (`WasmFeatures::WASM3`, refined by its cargo features)
/// are a moving target, so inheriting them is what made a module validate on
/// one wasmtime release and not the next.
pub fn new_engine() -> Result<Engine, Error> {
    let mut config = Config::new();
    config
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_multi_value(true)
        .wasm_multi_memory(true)
        .wasm_tail_call(true)
        .wasm_extended_const(true)
        .wasm_memory64(true)
        // `custom-page-sizes` and `wide-arithmetic` are still phase-3.
        // Threads are compiled out: den has no SharedArrayBuffer alias.
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false)
        // GC pulls in typed function references, which is why they move
        // together.
        .wasm_simd(true)
        .wasm_relaxed_simd(true)
        .wasm_gc(true)
        .wasm_function_references(true)
        .wasm_exceptions(true);

    if USES_PULLEY {
        config.target(if cfg!(target_pointer_width = "64") {
            "pulley64"
        } else {
            "pulley32"
        })?;
    }

    Engine::new(&config)
}

/// Binary-only: `Module::new` would silently accept WAT text, which the JS API
/// must not.
pub fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<Module, Error> {
    Module::from_binary(engine, bytes)
}

/// Define the whole `wasi_snapshot_preview1` namespace in `linker`.
///
/// Idempotent on purpose: a module with several WASI imports reaches its
/// namespace once per import, and `add_to_linker_sync` defines all ~45
/// functions every time it is called, which an ordinary linker rejects as a
/// duplicate definition.
#[cfg(feature = "wasi")]
pub fn link_wasi(linker: &mut Linker) -> Result<(), Error> {
    linker.allow_shadowing(true);
    let result = wasmtime_wasi::p1::add_to_linker_sync(linker, |data: &mut StoreData| {
        // THE sandbox decision, and the reason the context is built here rather than
        // with the store: this inherits the host's stdio and environment, so it may
        // only happen once a caller has asked for WASI by passing `wasiImports()` as
        // an import namespace. A store that is never handed to a WASI module never
        // builds one.
        data.wasi_or_init(|| {
            wasmtime_wasi::WasiCtxBuilder::new()
                .inherit_stdio()
                .inherit_env()
                .build_p1()
        })
    });
    linker.allow_shadowing(false);
    result
}

pub const fn extern_kind_name(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "function",
        ExternType::Global(_) => "global",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
        ExternType::Tag(_) => "tag",
    }
}

/// One JS program, run against the compiled-in engine.
///
/// A capability constant that only the code it gates ever reads proves
/// nothing: the same `#[cfg]` would pick both the behaviour and the
/// expectation, so the assertion can never fail. Everything below is therefore
/// either engine-*independent* — the error class a script sees — or a claim
/// about the engine checked against what the engine actually does with a
/// module.
#[cfg(test)]
#[path = "../tests/unit/backend_parity.rs"]
mod parity;

#[cfg(test)]
#[path = "../tests/unit/backend.rs"]
mod tests;
