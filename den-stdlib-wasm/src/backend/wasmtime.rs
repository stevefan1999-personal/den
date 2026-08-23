//! wasmtime 48 flavour of the backend contract. See [`super`] for the contract
//! itself.

pub use ::wasmtime::{AsContext, AsContextMut};

use super::{StoreData, ValKind, ValView};

pub type Engine = ::wasmtime::Engine;
pub type Config = ::wasmtime::Config;
pub type Store = ::wasmtime::Store<StoreData>;
pub type Linker = ::wasmtime::Linker<StoreData>;
pub type Caller<'a> = ::wasmtime::Caller<'a, StoreData>;
pub type Module = ::wasmtime::Module;
pub type Instance = ::wasmtime::Instance;
pub type Func = ::wasmtime::Func;
pub type FuncType = ::wasmtime::FuncType;
pub type Memory = ::wasmtime::Memory;
pub type MemoryType = ::wasmtime::MemoryType;
pub type Table = ::wasmtime::Table;
pub type TableType = ::wasmtime::TableType;
pub type Global = ::wasmtime::Global;
pub type GlobalType = ::wasmtime::GlobalType;
pub type Mutability = ::wasmtime::Mutability;
pub type Extern = ::wasmtime::Extern;
pub type ExternType = ::wasmtime::ExternType;
pub type Val = ::wasmtime::Val;
pub type ValType = ::wasmtime::ValType;
pub type V128 = ::wasmtime::V128;
pub type Error = ::wasmtime::Error;
pub type WasiCtx = ::wasmtime_wasi::p1::WasiP1Ctx;

pub const NAME: &str = "wasmtime";
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
/// `wasmtime-wasi` is a dependency of this backend, so `wasiImports()` has a
/// preview1 implementation to hand out — see [`link_wasi`].
pub const SUPPORTS_WASI: bool = true;

/// Every proposal den depends on, spelled out.
///
/// wasmtime's defaults (`WasmFeatures::WASM3`, refined by its own cargo
/// features) are a superset of wasmi's, so inheriting them is what made the
/// same module validate on one build and not the other. Each knob below is
/// either shared with wasmi or driven by the capability constant that reports
/// the difference — see [`super`] for the table.
pub fn new_engine() -> Result<Engine, Error> {
    let mut config = Config::new();
    config
        // Common ground with wasmi 1.1's defaults.
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_multi_value(true)
        .wasm_multi_memory(true)
        .wasm_tail_call(true)
        .wasm_extended_const(true)
        .wasm_memory64(true)
        // Off on both sides. `custom-page-sizes` and `wide-arithmetic` are still
        // phase-3 proposals wasmi refuses by default, and threads is den's own
        // refusal.
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false)
        .wasm_threads(super::SUPPORTS_SHARED_MEMORY)
        // wasmtime-only, each one reported by its constant. GC pulls in typed
        // function references, which is why they move together.
        .wasm_simd(SUPPORTS_V128)
        .wasm_relaxed_simd(SUPPORTS_V128)
        .wasm_gc(SUPPORTS_ANYREF)
        .wasm_function_references(SUPPORTS_ANYREF)
        .wasm_exceptions(SUPPORTS_TAGS);
    Engine::new(&config)
}

/// Binary-only: `Module::new` would silently accept WAT text, which the JS API
/// must not.
pub fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<Module, Error> {
    Module::from_binary(engine, bytes)
}

pub fn linker_define(
    linker: &mut Linker,
    store: &Store,
    module: &str,
    name: &str,
    item: Extern,
) -> Result<(), Error> {
    linker.define(store, module, name, item)?;
    Ok(())
}

pub fn linker_func_new<F>(
    linker: &mut Linker,
    module: &str,
    name: &str,
    ty: FuncType,
    func: F,
) -> Result<(), Error>
where
    F: Fn(Caller<'_>, &[Val], &mut [Val]) -> Result<(), Error> + Send + Sync + 'static,
{
    linker.func_new(module, name, ty, func)?;
    Ok(())
}

pub fn linker_instantiate(
    linker: &Linker,
    store: &mut Store,
    module: &Module,
) -> Result<Instance, Error> {
    linker.instantiate(store, module)
}

/// Define the whole `wasi_snapshot_preview1` namespace in `linker`.
///
/// Idempotent on purpose: a module with several WASI imports reaches its
/// namespace once per import, and `add_to_linker_sync` defines all ~45
/// functions every time it is called, which an ordinary linker rejects as a
/// duplicate definition.
pub fn link_wasi(linker: &mut Linker) -> Result<(), Error> {
    linker.allow_shadowing(true);
    let linked = ::wasmtime_wasi::p1::add_to_linker_sync(linker, |data: &mut StoreData| {
        // THE sandbox decision, and the reason the context is built here rather than
        // with the store: this inherits the host's stdio and environment, so it may
        // only happen once a caller has asked for WASI by passing `wasiImports()` as
        // an import namespace. A store that is never handed to a WASI module never
        // builds one.
        data.wasi_or_init(|| {
            ::wasmtime_wasi::WasiCtxBuilder::new()
                .inherit_stdio()
                .inherit_env()
                .build_p1()
        })
    });
    linker.allow_shadowing(false);
    linked
}

pub fn new_global(
    store: &mut Store,
    ty: &ValType,
    mutable: bool,
    value: Val,
) -> Result<Global, Error> {
    let mutability = if mutable {
        Mutability::Var
    } else {
        Mutability::Const
    };
    Global::new(store, GlobalType::new(ty.clone(), mutability), value)
}

pub fn new_table(
    store: &mut Store,
    element: &ValType,
    minimum: u32,
    maximum: Option<u32>,
    init: Option<Val>,
) -> Result<Table, Error> {
    let ValType::Ref(element) = element else {
        return Err(host_error("table element type must be a reference type"));
    };
    let init = match init {
        Some(value) => {
            value
                .ref_()
                .ok_or_else(|| host_error("table initialiser must be a reference value"))?
        }
        None => ::wasmtime::Ref::null(element.heap_type()),
    };
    Table::new(
        store,
        TableType::new(element.clone(), minimum, maximum),
        init,
    )
}

pub fn new_memory_type(
    minimum: u64,
    maximum: Option<u64>,
    shared: bool,
) -> Result<MemoryType, Error> {
    let mut builder = ::wasmtime::MemoryTypeBuilder::new();
    builder.min(minimum).max(maximum).shared(shared);
    builder.build()
}

pub fn host_error(message: &str) -> Error {
    Error::msg(message.to_owned())
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

pub fn global_content(ty: &GlobalType) -> ValType {
    ty.content().clone()
}

pub fn val_type_kind(ty: &ValType) -> Option<ValKind> {
    use ::wasmtime::RefType;

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
