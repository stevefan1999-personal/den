//! wasmi 1.1 flavour of the backend contract. See [`super`] for the contract
//! itself.

pub use ::wasmi::{AsContext, AsContextMut};

use super::{StoreData, ValKind, ValView};

pub type Engine = ::wasmi::Engine;
pub type Config = ::wasmi::Config;
pub type Store = ::wasmi::Store<StoreData>;
pub type Linker = ::wasmi::Linker<StoreData>;
pub type Caller<'a> = ::wasmi::Caller<'a, StoreData>;
pub type Module = ::wasmi::Module;
pub type Instance = ::wasmi::Instance;
pub type Func = ::wasmi::Func;
pub type FuncType = ::wasmi::FuncType;
pub type Memory = ::wasmi::Memory;
pub type MemoryType = ::wasmi::MemoryType;
pub type Table = ::wasmi::Table;
pub type TableType = ::wasmi::TableType;
pub type Global = ::wasmi::Global;
pub type GlobalType = ::wasmi::GlobalType;
pub type Mutability = ::wasmi::Mutability;
pub type Extern = ::wasmi::Extern;
pub type ExternType = ::wasmi::ExternType;
pub type Val = ::wasmi::Val;
pub type ValType = ::wasmi::ValType;
pub type V128 = ::wasmi::V128;
pub type Error = ::wasmi::Error;

/// wasmi has no WASI implementation available here — `wasmi_wasi` is not a
/// dependency — so the slot in [`StoreData`] can never be filled. Keeping the
/// field typed as an uninhabited `Option` lets the store payload stay
/// backend-neutral at zero cost.
pub type WasiCtx = core::convert::Infallible;

pub const NAME: &str = "wasmi";
/// wasmi 1.1 implements neither the exception-handling nor the GC proposal at
/// all — there is no `Config` knob to turn either on — so a module using tags
/// or `anyref` is rejected here and accepted on wasmtime.
pub const SUPPORTS_TAGS: bool = false;
pub const SUPPORTS_ANYREF: bool = false;
/// `v128` *values* are never representable at the JS boundary on either
/// backend, but modules containing `v128` instructions are, and LLVM emits them
/// routinely. wasmi gates SIMD on its `simd` cargo feature (config.rs:75,
/// `WasmFeatures::SIMD` is `cfg!(feature = "simd")`), which den's manifest
/// enables so that `WebAssembly.validate` cannot answer differently per build
/// for the same bytes.
pub const SUPPORTS_V128: bool = true;
/// `wasmi_wasi` is not a dependency of den, so this backend has no preview1
/// implementation to hand out — see [`link_wasi`].
pub const SUPPORTS_WASI: bool = false;

/// Every proposal den depends on, spelled out — see [`super`] for how the two
/// backends line up. Everything here matches wasmi 1.1's own defaults; it is
/// written out anyway so that a defaults change in either engine shows up as a
/// diff rather than as a module that stops validating on one build.
pub fn new_engine() -> Result<Engine, Error> {
    let mut config = Config::default();
    config
        .wasm_mutable_global(true)
        .wasm_sign_extension(true)
        .wasm_saturating_float_to_int(true)
        .wasm_multi_value(true)
        .wasm_multi_memory(true)
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_tail_call(true)
        .wasm_extended_const(true)
        .wasm_memory64(true)
        .floats(true)
        .wasm_simd(SUPPORTS_V128)
        .wasm_relaxed_simd(SUPPORTS_V128)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false);
    // The JS API must reject a malformed module at compile time; wasmi's default
    // `LazyTranslation` mode would surface translation errors at first call
    // instead.
    config.compilation_mode(::wasmi::CompilationMode::Eager);
    // `Module::custom_sections()` needs them retained (default, spelled out because
    // we expose it).
    config.ignore_custom_sections(false);
    Ok(Engine::new(&config))
}

/// `Module::new` alone would silently accept WAT text; `Module::validate` is
/// binary-only, which is the behaviour the JS API requires.
pub fn compile_module(engine: &Engine, bytes: &[u8]) -> Result<Module, Error> {
    Module::validate(engine, bytes)?;
    Module::new(engine, bytes)
}

/// wasmi's linker holds no store reference; the argument exists to match
/// wasmtime's shape.
pub fn linker_define(
    linker: &mut Linker,
    _store: &Store,
    module: &str,
    name: &str,
    item: Extern,
) -> Result<(), Error> {
    linker.define(module, name, item)?;
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
    linker.instantiate_and_start(store, module)
}

/// The negative half of the backend contract: [`SUPPORTS_WASI`] is `false`, so
/// `wasiImports()` refuses before any instantiation can reach this. It stays as
/// the second line of defence — the day a `wasmi_wasi` dependency appears, this
/// is the one function that has to change.
pub fn link_wasi(_linker: &mut Linker) -> Result<(), Error> {
    Err(host_error(
        "WASI is not supported by the wasmi backend of this build",
    ))
}

/// wasmi derives a global's type from its initial value, so `ty` is only
/// validated here.
pub fn new_global(
    store: &mut Store,
    ty: &ValType,
    mutable: bool,
    value: Val,
) -> Result<Global, Error> {
    if value.ty() != *ty {
        return Err(host_error(
            "type mismatch: initial value provided does not match the type of this global",
        ));
    }
    let mutability = if mutable {
        Mutability::Var
    } else {
        Mutability::Const
    };
    Ok(Global::new(store, value, mutability))
}

pub fn new_table(
    store: &mut Store,
    element: &ValType,
    minimum: u32,
    maximum: Option<u32>,
    init: Option<Val>,
) -> Result<Table, Error> {
    if !element.is_ref() {
        return Err(host_error("table element type must be a reference type"));
    }
    let init = init.unwrap_or_else(|| Val::default(*element));
    Table::new(store, TableType::new(*element, minimum, maximum), init)
}

pub fn new_memory_type(
    minimum: u64,
    maximum: Option<u64>,
    shared: bool,
) -> Result<MemoryType, Error> {
    if shared {
        return Err(host_error(
            "shared memory requires the threads proposal, unsupported by the wasmi backend",
        ));
    }
    let mut builder = MemoryType::builder();
    builder.min(minimum);
    builder.max(maximum);
    Ok(builder.build()?)
}

pub fn host_error(message: &str) -> Error {
    Error::new(message.to_owned())
}

pub fn extern_kind_name(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "function",
        ExternType::Global(_) => "global",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
    }
}

pub fn global_content(ty: &GlobalType) -> ValType {
    ty.content()
}

pub fn val_type_kind(ty: &ValType) -> Option<ValKind> {
    Some(match ty {
        ValType::I32 => ValKind::I32,
        ValType::I64 => ValKind::I64,
        ValType::F32 => ValKind::F32,
        ValType::F64 => ValKind::F64,
        ValType::V128 => ValKind::V128,
        ValType::FuncRef => ValKind::FuncRef,
        ValType::ExternRef => ValKind::ExternRef,
    })
}

pub fn val_type_from_kind(kind: ValKind) -> Option<ValType> {
    Some(match kind {
        ValKind::I32 => ValType::I32,
        ValKind::I64 => ValType::I64,
        ValKind::F32 => ValType::F32,
        ValKind::F64 => ValType::F64,
        ValKind::V128 => ValType::V128,
        ValKind::FuncRef => ValType::FuncRef,
        ValKind::ExternRef => ValType::ExternRef,
        // wasmi implements neither the GC nor the function-references proposal.
        ValKind::AnyRef => return None,
    })
}

pub fn val_view(value: &Val) -> ValView {
    use ::wasmi::Ref;

    match value {
        Val::I32(x) => ValView::I32(*x),
        Val::I64(x) => ValView::I64(*x),
        Val::F32(x) => ValView::F32(f32::from(*x)),
        Val::F64(x) => ValView::F64(f64::from(*x)),
        Val::V128(_) => ValView::V128,
        Val::FuncRef(Ref::Null) | Val::ExternRef(Ref::Null) => ValView::NullRef,
        _ => ValView::Ref,
    }
}

/// `None` for `v128`, which the JS API rejects outright.
pub fn val_default(ty: &ValType) -> Option<Val> {
    match ty {
        ValType::V128 => None,
        _ => Some(Val::default(*ty)),
    }
}
