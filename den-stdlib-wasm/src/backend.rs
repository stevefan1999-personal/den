//! wasmtime 48 integration for the WebAssembly JS API.
//!
//! This module owns the den-specific pieces: the store payload
//! ([`OwnedCtx`], [`StoreData`]), the engine [`Config`] ([`new_engine`]),
//! WASI linking ([`link_wasi`]), and the value-type helpers ([`ValKind`],
//! [`ValView`]). wasmtime types are used directly everywhere else.
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
//! that quietly stops validating. The right-hand column names the constant
//! that reports the difference to Rust and to the JS-program test.
//!
//! | Proposal                        | accepted | reported by |
//! |---------------------------------|----------|-------------|
//! | MVP + mutable-global            | yes      | —           |
//! | sign-extension, sat-float-to-int| yes      | —           |
//! | multi-value, multi-memory       | yes      | —           |
//! | bulk-memory, reference-types    | yes      | —           |
//! | tail-call, extended-const       | yes      | —           |
//! | memory64                        | yes      | —           |
//! | threads (shared memory, atomics)| **off**  | [`SUPPORTS_SHARED_MEMORY`] |
//! | custom-page-sizes               | off      | —           |
//! | wide-arithmetic                 | off      | —           |
//! | simd, relaxed-simd              | yes      | [`SUPPORTS_V128`] |
//! | gc / `anyref`                   | yes      | [`SUPPORTS_ANYREF`] |
//! | exceptions / tags               | yes      | [`SUPPORTS_TAGS`] |
//! | function-references             | yes      | [`SUPPORTS_ANYREF`] |
//!
//! WASI preview1 is not a proposal but a host API, and it is opt-in rather than
//! configured — see [`link_wasi`] and [`SUPPORTS_WASI`].
//!
//! `function-references` has no JS-API spelling of its own — it rides along
//! with gc, which wasmtime requires it for — so it shares gc's constant.
//!
//! Threads is the one proposal turned *off* where the engine offers it: den
//! cannot represent a shared memory at the JS boundary (see
//! [`SUPPORTS_SHARED_MEMORY`]), so accepting `(memory 1 1 shared)` would only
//! buy a module that instantiates and then cannot be wrapped.

use rquickjs::Ctx;
use wasmtime::{
    Config, Engine, Error, ExternType, Global, GlobalType, MemoryType, Module, Mutability, Ref,
    Table, TableType, Val, ValType,
};
use wasmtime_wasi::p1::WasiP1Ctx;

/// The one store a JS context owns, parameterized on den's payload.
pub type Store = wasmtime::Store<StoreData>;
/// Linker bound to [`StoreData`].
pub type Linker = wasmtime::Linker<StoreData>;
/// Host-callback caller bound to [`StoreData`].
pub type Caller<'a> = wasmtime::Caller<'a, StoreData>;

/// The one import namespace [`link_wasi`] defines, and therefore the only one
/// `den:wasm`'s `wasiImports()` may be passed as.
pub const WASI_NAMESPACE: &str = "wasi_snapshot_preview1";

/// Whether `new WebAssembly.Memory({ shared: true })` can succeed.
///
/// `false` here is den's limit, not wasmtime's. §5.6 requires a shared memory's
/// `[[BufferObject]]` to be a `SharedArrayBuffer`, and den has no way to build
/// one that aliases the linear memory: `JS_NewArrayBuffer` with `is_shared`
/// set produces a buffer `JS_DetachArrayBuffer` silently refuses to detach
/// (quickjs.c:57837), which would turn the growth protocol in `memory.rs` into
/// a use-after-free. wasmtime cannot help either — it allocates shared
/// memories only through `SharedMemory` and `Memory::new` bails on a shared
/// type (wasmtime-48.0.0 src/runtime/memory.rs:303).
///
/// So the constant is the honest answer, it makes the JS API throw a
/// `TypeError`, and `new_engine` derives `wasm_threads` from it so that no
/// module can smuggle a shared memory in past the JS API either.
pub const SUPPORTS_SHARED_MEMORY: bool = false;

/// Tags, `anyref` and `v128` are all reachable here: wasmtime's `gc` cargo
/// feature — which `EXCEPTIONS` and `GC_TYPES` hang off (config.rs:2550) — is
/// on by default and den's manifest does not turn defaults off, and cargo
/// features are additive so no dependent can take it away. `new_engine` below
/// asks for each proposal explicitly, so a build that somehow lost the backing
/// feature fails loudly in `Engine::new` instead of quietly validating fewer
/// modules than these constants promise.
pub const SUPPORTS_TAGS: bool = true;
pub const SUPPORTS_ANYREF: bool = true;
pub const SUPPORTS_V128: bool = true;
/// `wasmtime-wasi` is a dependency, so `wasiImports()` has a preview1
/// implementation to hand out — see [`link_wasi`].
pub const SUPPORTS_WASI: bool = true;

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
pub const USES_PULLEY: bool = !cfg!(feature = "jit") || !HOST_HAS_CRANELIFT;

/// Owning, lifetime-erased handle to the QuickJS context that created the
/// [`Store`].
///
/// wasmtime 48 bounds `T: 'static` on `Linker`/`Instance`/`Func`, so the store
/// payload cannot borrow `'js`. `Ctx` is refcounted (`Clone` is
/// `JS_DupContext`), so parking one keeps the `JSContext` alive and
/// [`OwnedCtx::with`] mints a callback-scoped `'js` on demand — `Ctx` is
/// invariant in `'js`, so it has to be minted rather than reborrowed.
///
/// This is the only `unsafe` in the crate's foundation; keep it that way.
///
/// Deliberately *not* `Sync`: nothing here needs it (the store payload is
/// only ever reached through `&mut Store`, and the host closures that must
/// be `Send + Sync` capture no `OwnedCtx`), and asserting it would make
/// `StoreData` — hence `wasmtime::Store<StoreData>` — look shareable
/// between threads, which a `JSContext` is not.
pub struct OwnedCtx(Ctx<'static>);

impl OwnedCtx {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        // SAFETY: `from_raw` takes a reference of its own via `JS_DupContext`, and the
        // caller is inside `ctx`, so the runtime lock is held right now.
        Self(unsafe { Ctx::from_raw(ctx.as_raw()) })
    }

    /// Re-narrow the erased context to a callback-scoped `'js`.
    ///
    /// This is the only way to reach the context: a `fn ctx(&self) -> Ctx<'_>`
    /// would hand out a lifetime the caller could outlive.
    pub fn with<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        // SAFETY: `self.0` holds a live reference to this context — `Ctx::from_raw`
        // performs `JS_DupContext`, and the runtime drops its userdata (hence this
        // value) before `JS_FreeRuntime` — and the runtime lock is held by whoever
        // called into wasm: the `Store` this lives in is userdata of that very
        // context, and a host callback is only entered from a JS call that holds the
        // lock for the whole closure. The minted `Ctx` never escapes.
        let ctx = unsafe { Ctx::from_raw(self.0.as_raw()) };
        f(&ctx)
    }
}

/// Payload of the single [`Store`] den keeps per JS context.
///
/// `'static` by construction — see [`OwnedCtx`]. The WASI context is created
/// lazily by [`link_wasi`], and only ever from there: building one inherits the
/// host's stdio and environment, which no context may be handed until a script
/// has asked for WASI by name.
pub struct StoreData {
    ctx:  OwnedCtx,
    wasi: Option<WasiP1Ctx>,
}

impl StoreData {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        Self {
            ctx:  OwnedCtx::new(ctx),
            wasi: None,
        }
    }

    /// The store's WASI context, built on first use by `init`.
    pub fn wasi_or_init(&mut self, init: impl FnOnce() -> WasiP1Ctx) -> &mut WasiP1Ctx {
        self.wasi.get_or_insert_with(init)
    }

    /// Run `f` with the JS context that owns this store.
    pub fn with_ctx<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R { self.ctx.with(f) }
}

/// Discriminant of a [`ValType`].
///
/// wasmtime nests reference types in `ValType::Ref(RefType)` and derives
/// neither `Copy` nor `PartialEq`, so shared code matches on this instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValKind {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
    AnyRef,
}

impl ValKind {
    /// The `WebAssembly.Global`/`Table` descriptor spelling of this type.
    pub const fn name(self) -> &'static str {
        match self {
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::V128 => "v128",
            Self::FuncRef => "anyfunc",
            Self::ExternRef => "externref",
            Self::AnyRef => "anyref",
        }
    }

    /// Inverse of [`ValKind::name`], accepting `"funcref"` as an alias of
    /// `"anyfunc"`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "i32" => Self::I32,
            "i64" => Self::I64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "v128" => Self::V128,
            "anyfunc" | "funcref" => Self::FuncRef,
            "externref" => Self::ExternRef,
            "anyref" => Self::AnyRef,
            _ => return None,
        })
    }
}

/// View of a [`Val`], so that JS conversion can be written once.
///
/// Floats are real values here: wasmtime stores raw IEEE bits in
/// `Val::F32`/`F64`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValView {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    V128,
    /// A null reference of any reference type.
    NullRef,
    /// A non-null reference; not representable in JS without the host value
    /// cache.
    Ref,
}

/// Every proposal den depends on, spelled out — see the module docs for the
/// table.
///
/// wasmtime's defaults (`WasmFeatures::WASM3`, refined by its cargo features)
/// are a moving target, so inheriting them is what made a module validate on
/// one wasmtime release and not the next. Each knob below is driven by the
/// capability constant that reports it.
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
        // `custom-page-sizes` and `wide-arithmetic` are still phase-3;
        // threads is den's own refusal (no SharedArrayBuffer alias).
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false)
        .wasm_threads(SUPPORTS_SHARED_MEMORY)
        // GC pulls in typed function references, which is why they move
        // together.
        .wasm_simd(SUPPORTS_V128)
        .wasm_relaxed_simd(SUPPORTS_V128)
        .wasm_gc(SUPPORTS_ANYREF)
        .wasm_function_references(SUPPORTS_ANYREF)
        .wasm_exceptions(SUPPORTS_TAGS);

    #[cfg(feature = "jit")]
    {
        // Native Cranelift when the host ISA is a Cranelift target
        // (x86_64, aarch64, s390x, riscv64). If the host is not, fall
        // through to Pulley.
        if !HOST_HAS_CRANELIFT {
            config.target(if cfg!(target_pointer_width = "64") {
                "pulley64"
            } else {
                "pulley32"
            })?;
        }
    }
    #[cfg(not(feature = "jit"))]
    {
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
pub fn link_wasi(linker: &mut Linker) -> Result<(), Error> {
    linker.allow_shadowing(true);
    let linked = wasmtime_wasi::p1::add_to_linker_sync(linker, |data: &mut StoreData| {
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
    linked
}

pub fn new_global(
    store: &mut Store, ty: &ValType, mutable: bool, value: Val,
) -> Result<Global, Error> {
    let mutability = if mutable {
        Mutability::Var
    } else {
        Mutability::Const
    };
    Global::new(store, GlobalType::new(ty.clone(), mutability), value)
}

pub fn new_table(
    store: &mut Store, element: &ValType, minimum: u32, maximum: Option<u32>, init: Option<Val>,
) -> Result<Table, Error> {
    let ValType::Ref(element) = element else {
        return Err(Error::msg("table element type must be a reference type"));
    };
    let init = match init {
        Some(value) => {
            value
                .ref_()
                .ok_or_else(|| Error::msg("table initialiser must be a reference value"))?
        }
        None => Ref::null(element.heap_type()),
    };
    Table::new(
        store,
        TableType::new(element.clone(), minimum, maximum),
        init,
    )
}

pub fn new_memory_type(
    minimum: u64, maximum: Option<u64>, shared: bool,
) -> Result<MemoryType, Error> {
    let mut builder = wasmtime::MemoryTypeBuilder::new();
    builder.min(minimum).max(maximum).shared(shared);
    builder.build()
}

pub fn extern_kind_name(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "function",
        ExternType::Global(_) => "global",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
        ExternType::Tag(_) => "tag",
    }
}

/// `"i32"`, `"anyfunc"`, … to wasmtime's type, or `None` when the name is
/// unknown.
pub fn val_type_from_str(name: &str) -> Option<ValType> {
    ValKind::parse(name).and_then(val_type_from_kind)
}

/// Descriptor spelling of a wasmtime type, or `None` for types with no JS name
/// (GC reference types such as `structref`).
pub fn val_type_name(ty: &ValType) -> Option<&'static str> { val_type_kind(ty).map(ValKind::name) }

pub fn val_type_kind(ty: &ValType) -> Option<ValKind> {
    use wasmtime::RefType;

    Some(match ty {
        ValType::I32 => ValKind::I32,
        ValType::I64 => ValKind::I64,
        ValType::F32 => ValKind::F32,
        ValType::F64 => ValKind::F64,
        ValType::V128 => ValKind::V128,
        ValType::Ref(reference) if reference.matches(&RefType::FUNCREF) => ValKind::FuncRef,
        ValType::Ref(reference) if reference.matches(&RefType::EXTERNREF) => ValKind::ExternRef,
        ValType::Ref(reference) if reference.matches(&RefType::ANYREF) => ValKind::AnyRef,
        // The remaining GC reference types (structref, arrayref, exnref, …) have no JS-API name.
        ValType::Ref(_) => return None,
    })
}

pub fn val_type_from_kind(kind: ValKind) -> Option<ValType> {
    Some(match kind {
        ValKind::I32 => ValType::I32,
        ValKind::I64 => ValType::I64,
        ValKind::F32 => ValType::F32,
        ValKind::F64 => ValType::F64,
        ValKind::V128 => ValType::V128,
        ValKind::FuncRef => ValType::FUNCREF,
        ValKind::ExternRef => ValType::EXTERNREF,
        ValKind::AnyRef => ValType::ANYREF,
    })
}

pub fn val_view(value: &Val) -> ValView {
    match value {
        Val::I32(x) => ValView::I32(*x),
        Val::I64(x) => ValView::I64(*x),
        Val::F32(bits) => ValView::F32(f32::from_bits(*bits)),
        Val::F64(bits) => ValView::F64(f64::from_bits(*bits)),
        Val::V128(_) => ValView::V128,
        Val::FuncRef(None)
        | Val::ExternRef(None)
        | Val::AnyRef(None)
        | Val::ExnRef(None)
        | Val::ContRef(None) => ValView::NullRef,
        _ => ValView::Ref,
    }
}

/// `None` when the type has no JS-representable default: non-nullable
/// references, and `v128`, which the JS API rejects outright.
pub fn val_default(ty: &ValType) -> Option<Val> {
    match val_type_kind(ty) {
        Some(ValKind::V128) => None,
        _ => Val::default_for_ty(ty),
    }
}

/// One JS program, run against the compiled-in engine.
///
/// A capability constant that only the code it gates ever reads proves
/// nothing: the same `#[cfg]` would pick both the behaviour and the
/// expectation, so the assertion can never fail. Everything below is therefore
/// either engine-*independent* — the error class a script sees — or a claim
/// about the engine checked against what the engine actually does with a
/// module. A constant that stops matching its engine fails here instead of in
/// somebody's script.
#[cfg(test)]
mod parity {
    use super::{SUPPORTS_ANYREF, SUPPORTS_SHARED_MEMORY, SUPPORTS_TAGS, SUPPORTS_V128};

    /// Each entry is `"<what>: <outcome>"`, where the outcome is the value the
    /// operation produced or the `name` of the error it threw.
    const OBSERVE: &str = r#"
      const observations = [];
      const attempt = (f) => {
        try {
          return String(f());
        } catch (error) {
          return error.name;
        }
      };
      const observe = (what, f) => observations.push(`${what}: ${attempt(f)}`);
      const validates = (wat) => WebAssembly.validate(denWasm.wat2wasm(wat));
      const run = (wat, name, ...args) =>
        new WebAssembly.Instance(new WebAssembly.Module(denWasm.wat2wasm(wat)))
          .exports[name](...args);

      // Shared memory: refused at both the JS API and the module level.
      observe("a shared memory", () => new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true }));
      observe("a shared memory without a maximum", () => new WebAssembly.Memory({ initial: 1, shared: true }));
      observe("a module with a shared memory", () => validates(`(module (memory 1 1 shared))`));
      observe("a module using atomics", () =>
        validates(`(module (memory 1 1 shared) (func (result i32) i32.const 0 i32.atomic.load))`));

      // Value types with no JS representation, rejected by the shared
      // descriptor plumbing rather than by the engine.
      observe("an anyref global", () => new WebAssembly.Global({ value: "anyref" }, null));
      observe("a v128 global", () => new WebAssembly.Global({ value: "v128" }, 0));

      observe("a module using anyref", () => validates(`(module (func (result anyref) ref.null any))`));
      observe("a module using v128", () => validates(`(module (func (result v128) v128.const i32x4 0 0 0 0))`));
      observe("a module with a tag", () => validates(`(module (tag (param i32)))`));
      observe("a tag", () => new WebAssembly.Tag({ parameters: ["i32"] }) instanceof WebAssembly.Tag);

      observe("a module using tail calls", () => validates(`(module (func $f) (func (return_call $f)))`));
      observe("a module using extended const", () =>
        validates(`(module (global i32 (i32.add (i32.const 1) (i32.const 2))))`));
      observe("a module with two memories", () => validates(`(module (memory 1) (memory 1))`));
      observe("a module using memory64", () => validates(`(module (memory i64 1))`));
      observe("a module using custom page sizes", () => validates(`(module (memory 1 (pagesize 1)))`));
      observe("a module using bulk memory", () =>
        validates(`(module (memory 1) (func (memory.fill (i32.const 0) (i32.const 0) (i32.const 1))))`));

      observe("an exported function", () =>
        run(`(module (func (export "add") (param i32 i32) (result i32)
               local.get 0 local.get 1 i32.add))`, "add", 1, 2));
      observe("a trapping export", () => run(`(module (func (export "boom") unreachable))`, "boom"));

      observations
    "#;

    #[test]
    fn the_engine_answers_the_js_program_the_constants_promise() {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let engine = den_core::engine::Engine::new().await;
                let observed: Vec<String> = engine
                    .eval(&format!(
                        "const denWasm = await import('den:wasm');\n{OBSERVE}"
                    ))
                    .await
                    .expect("the program runs");

                let expected = [
                    ("a shared memory", "TypeError".to_owned()),
                    ("a shared memory without a maximum", "TypeError".to_owned()),
                    (
                        "a module with a shared memory",
                        SUPPORTS_SHARED_MEMORY.to_string(),
                    ),
                    ("a module using atomics", SUPPORTS_SHARED_MEMORY.to_string()),
                    ("an anyref global", "TypeError".to_owned()),
                    ("a v128 global", "TypeError".to_owned()),
                    ("a module using anyref", SUPPORTS_ANYREF.to_string()),
                    ("a module using v128", SUPPORTS_V128.to_string()),
                    ("a module with a tag", SUPPORTS_TAGS.to_string()),
                    // The constructor reports the same lack as a `TypeError`, per §5.9.
                    (
                        "a tag",
                        if SUPPORTS_TAGS { "true" } else { "TypeError" }.to_owned(),
                    ),
                    ("a module using tail calls", "true".to_owned()),
                    ("a module using extended const", "true".to_owned()),
                    ("a module with two memories", "true".to_owned()),
                    ("a module using memory64", "true".to_owned()),
                    ("a module using custom page sizes", "false".to_owned()),
                    ("a module using bulk memory", "true".to_owned()),
                    ("an exported function", "3".to_owned()),
                    ("a trapping export", "RuntimeError".to_owned()),
                ]
                .map(|(what, outcome)| format!("{what}: {outcome}"));

                assert_eq!(observed, expected.to_vec());
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{USES_PULLEY, new_engine};

    #[test]
    fn the_engine_is_pulley_exactly_when_the_build_selects_it() {
        let engine = new_engine().expect("engine");
        assert_eq!(
            engine.is_pulley(),
            USES_PULLEY,
            "Engine::is_pulley() must match the jit/host-ISA selection"
        );
    }
}
