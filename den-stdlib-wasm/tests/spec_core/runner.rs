//! Wasmtime execution of official `*.wast` scripts.
//!
//! The engine is den's [`den_stdlib_wasm::backend::new_engine`] so the proposal
//! set matches `WebAssembly.validate` / `Module`. Individual directive
//! failures become `Err` strings; the per-file nextest trial decides pass/fail.

use std::{collections::HashMap, path::Path};

use den_stdlib_wasm::backend;
use wasmtime::{
    AnyRef, Engine, ExternRef, Global, GlobalType, Instance, Linker, Memory, MemoryType, Module,
    Mutability, Ref, Store, Table, TableType, ThrownException, Val, ValType,
};
use wast::{
    QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet,
    core::{AbstractHeapType, HeapType, NanPattern, V128Pattern, WastArgCore, WastRetCore},
    lexer::Lexer,
    parser::{self, ParseBuffer},
    token::{F32, F64, Id},
};

/// Binaries of modules that compiled successfully in this file.
pub struct FileOutcome {
    pub compiled: Vec<Vec<u8>>,
}

enum Outcome {
    Values(Vec<Val>),
    Trap(wasmtime::Error),
}

struct Script {
    store:    Store<()>,
    linker:   Linker<()>,
    defined:  HashMap<String, Module>,
    bound:    HashMap<String, Instance>,
    active:   Option<Instance>,
    latest:   Option<Module>,
    compiled: Vec<Vec<u8>>,
}

/// Run every directive in `source`. `path` is used for `names.wast` lexing.
pub fn run_wast(path: &Path, source: &str) -> Result<FileOutcome, String> {
    let engine = backend::new_engine().map_err(|error| error.to_string())?;
    let mut script = Script::new(&engine)?;
    let mut lexer = Lexer::new(source);
    if file_name_is(path, "names.wast") {
        lexer.allow_confusing_unicode(true);
    }
    let buffer = ParseBuffer::new_with_lexer(lexer).map_err(|error| error.to_string())?;
    let ast = parser::parse::<Wast<'_>>(&buffer).map_err(|error| error.to_string())?;
    for directive in ast.directives {
        let line = directive.span().linecol_in(source).0.saturating_add(1);
        script
            .directive(directive)
            .map_err(|error| format!("line {line}: {error}"))?;
    }
    Ok(FileOutcome {
        compiled: script.compiled,
    })
}

fn file_name_is(path: &Path, name: &str) -> bool {
    path.file_name()
        .and_then(|file| file.to_str())
        .is_some_and(|file| file == name)
}

impl Script {
    fn new(engine: &Engine) -> Result<Self, String> {
        let mut store = Store::new(engine, ());
        let mut linker = Linker::new(engine);
        linker.allow_shadowing(true);
        define_spectest(&mut store, &mut linker)?;
        Ok(Self {
            store,
            linker,
            defined: HashMap::new(),
            bound: HashMap::new(),
            active: None,
            latest: None,
            compiled: Vec::new(),
        })
    }

    fn directive(&mut self, directive: WastDirective<'_>) -> Result<(), String> {
        match directive {
            WastDirective::Module(quoted) => {
                let (name, module) = self.compile_quoted(quoted)?;
                let instance = self.instantiate(&module)?;
                self.remember(name.as_deref(), instance)?;
                Ok(())
            }
            WastDirective::ModuleDefinition(quoted) => {
                let (name, module) = self.compile_quoted(quoted)?;
                if let Some(name) = name {
                    self.defined.insert(name, module.clone());
                }
                self.latest = Some(module);
                Ok(())
            }
            WastDirective::ModuleInstance {
                instance, module, ..
            } => {
                let compiled = self.lookup_defined(module)?;
                let created = self.instantiate(&compiled)?;
                self.remember(instance.map(|id| id.name()), created)?;
                Ok(())
            }
            WastDirective::Register { name, module, .. } => {
                self.register(name, module.map(|id| id.name()))
            }
            WastDirective::Invoke(call) => {
                match self.invoke(call)? {
                    Outcome::Values(_) => Ok(()),
                    Outcome::Trap(error) => Err(format!("invoke trapped: {error}")),
                }
            }
            WastDirective::AssertReturn { exec, results, .. } => {
                match self.execute(exec)? {
                    Outcome::Values(values) => match_vals(&mut self.store, &values, &results),
                    Outcome::Trap(error) => Err(format!("expected return, trapped: {error}")),
                }
            }
            WastDirective::AssertTrap { exec, message, .. } => {
                match self.execute(exec)? {
                    Outcome::Values(values) => {
                        Err(format!("expected trap ({message}), got {values:?}"))
                    }
                    Outcome::Trap(error) => require_trap(&error, message),
                }
            }
            WastDirective::AssertExhaustion { call, message, .. } => {
                match self.invoke(call)? {
                    Outcome::Values(values) => {
                        Err(format!("expected exhaustion ({message}), got {values:?}"))
                    }
                    Outcome::Trap(error) => require_trap(&error, message),
                }
            }
            WastDirective::AssertMalformed { mut module, .. }
            | WastDirective::AssertMalformedCustom { mut module, .. }
            | WastDirective::AssertInvalid { mut module, .. }
            | WastDirective::AssertInvalidCustom { mut module, .. } => {
                require_rejected(compile_quote(self.store.engine(), &mut module))
            }
            WastDirective::AssertUnlinkable { mut module, .. } => {
                let bytes = module.encode().map_err(|error| error.to_string())?;
                let compiled = Module::from_binary(self.store.engine(), &bytes)
                    .map_err(|error| format!("unlinkable module must compile: {error}"))?;
                match self.linker.instantiate(&mut self.store, &compiled) {
                    Ok(_) => Err("expected unlinkable instantiation to fail".to_owned()),
                    Err(_) => Ok(()),
                }
            }
            WastDirective::AssertException { exec, .. } => {
                match self.execute(exec)? {
                    Outcome::Values(values) => Err(format!("expected exception, got {values:?}")),
                    Outcome::Trap(error) => {
                        if error.is::<ThrownException>() {
                            let _ = self.store.take_pending_exception();
                            Ok(())
                        } else {
                            Err(format!("expected exception, got {error}"))
                        }
                    }
                }
            }
            WastDirective::AssertSuspension { message, .. } => {
                Err(format!("assert_suspension is not supported ({message})"))
            }
            WastDirective::Thread(_) | WastDirective::Wait { .. } => {
                Err("thread/wait directives need the threads proposal".to_owned())
            }
        }
    }

    fn compile_quoted(
        &mut self, mut quoted: QuoteWat<'_>,
    ) -> Result<(Option<String>, Module), String> {
        let name = quoted.name().map(|id| id.name().to_owned());
        let compiled = compile_quote(self.store.engine(), &mut quoted)?;
        self.compiled.push(compiled.1);
        Ok((name, compiled.0))
    }

    fn instantiate(&mut self, module: &Module) -> Result<Instance, String> {
        self.linker
            .instantiate(&mut self.store, module)
            .map_err(|error| format!("instantiation failed: {error}"))
    }

    fn remember(&mut self, name: Option<&str>, instance: Instance) -> Result<(), String> {
        if let Some(name) = name {
            self.linker
                .instance(&mut self.store, name, instance)
                .map_err(|error| error.to_string())?;
            self.bound.insert(name.to_owned(), instance);
        }
        self.active = Some(instance);
        Ok(())
    }

    fn lookup_defined(&self, module: Option<Id<'_>>) -> Result<Module, String> {
        if let Some(id) = module {
            return self
                .defined
                .get(id.name())
                .cloned()
                .ok_or_else(|| format!("no defined module {}", id.name()));
        }
        self.latest
            .clone()
            .ok_or_else(|| "no previous module definition".to_owned())
    }

    fn register(&mut self, as_name: &str, from: Option<&str>) -> Result<(), String> {
        if let Some(from) = from {
            if let Some(instance) = self.bound.get(from).copied() {
                self.linker
                    .instance(&mut self.store, as_name, instance)
                    .map_err(|error| error.to_string())?;
                self.bound.insert(as_name.to_owned(), instance);
                return Ok(());
            }
            return self
                .linker
                .alias_module(from, as_name)
                .map_err(|error| error.to_string());
        }
        let Some(instance) = self.active else {
            return Err("register without a previous instance".to_owned());
        };
        self.linker
            .instance(&mut self.store, as_name, instance)
            .map_err(|error| error.to_string())?;
        self.bound.insert(as_name.to_owned(), instance);
        Ok(())
    }

    fn execute(&mut self, exec: WastExecute<'_>) -> Result<Outcome, String> {
        match exec {
            WastExecute::Invoke(call) => self.invoke(call),
            WastExecute::Get { module, global, .. } => {
                let instance = self.resolve(module)?;
                let Some(handle) = instance.get_global(&mut self.store, global) else {
                    return Err(format!("no global export {global}"));
                };
                Ok(Outcome::Values(vec![handle.get(&mut self.store)]))
            }
            WastExecute::Wat(mut wat) => {
                let bytes = wat.encode().map_err(|error| error.to_string())?;
                let compiled = Module::from_binary(self.store.engine(), &bytes)
                    .map_err(|error| error.to_string())?;
                match self.linker.instantiate(&mut self.store, &compiled) {
                    Ok(_) => Ok(Outcome::Values(Vec::new())),
                    Err(error) => Ok(Outcome::Trap(error)),
                }
            }
        }
    }

    fn invoke(&mut self, call: WastInvoke<'_>) -> Result<Outcome, String> {
        let instance = self.resolve(call.module)?;
        let Some(func) = instance.get_func(&mut self.store, call.name) else {
            return Err(format!("no function export {}", call.name));
        };
        let mut args = Vec::new();
        for arg in call.args {
            args.push(self.convert_arg(arg)?);
        }
        let n = func.ty(&self.store).results().len();
        let mut results = vec![Val::null_func_ref(); n];
        match func.call(&mut self.store, &args, &mut results) {
            Ok(()) => Ok(Outcome::Values(results)),
            Err(error) => Ok(Outcome::Trap(error)),
        }
    }

    fn resolve(&self, module: Option<Id<'_>>) -> Result<Instance, String> {
        module.map_or_else(
            || self.active.ok_or_else(|| "no previous instance".to_owned()),
            |id| {
                self.bound
                    .get(id.name())
                    .copied()
                    .ok_or_else(|| format!("no instance {}", id.name()))
            },
        )
    }

    fn convert_arg(&mut self, arg: WastArg<'_>) -> Result<Val, String> {
        match arg {
            WastArg::Core(core) => self.core_val(core),
            WastArg::Component(_) => Err("component values are not executed".to_owned()),
            _ => Err("unsupported wast argument".to_owned()),
        }
    }

    fn core_val(&mut self, arg: WastArgCore<'_>) -> Result<Val, String> {
        match arg {
            WastArgCore::I32(value) => Ok(Val::I32(value)),
            WastArgCore::I64(value) => Ok(Val::I64(value)),
            WastArgCore::F32(F32 { bits }) => Ok(Val::F32(bits)),
            WastArgCore::F64(F64 { bits }) => Ok(Val::F64(bits)),
            WastArgCore::V128(lanes) => {
                Ok(Val::V128(u128::from_le_bytes(lanes.to_le_bytes()).into()))
            }
            WastArgCore::RefNull(heap) => Ok(null_val(heap)),
            WastArgCore::RefExtern(host) => {
                let handle =
                    ExternRef::new(&mut self.store, host).map_err(|error| error.to_string())?;
                Ok(Val::ExternRef(Some(handle)))
            }
            WastArgCore::RefHost(host) => {
                let external =
                    ExternRef::new(&mut self.store, host).map_err(|error| error.to_string())?;
                let internal = AnyRef::convert_extern(&mut self.store, external)
                    .map_err(|error| error.to_string())?;
                Ok(Val::AnyRef(Some(internal)))
            }
        }
    }
}

fn compile_quote(engine: &Engine, quoted: &mut QuoteWat<'_>) -> Result<(Module, Vec<u8>), String> {
    let bytes = quoted.encode().map_err(|error| error.to_string())?;
    let module = Module::from_binary(engine, &bytes).map_err(|error| error.to_string())?;
    Ok((module, bytes))
}

fn require_rejected(result: Result<(Module, Vec<u8>), String>) -> Result<(), String> {
    match result {
        Ok(_) => Err("module was expected to be rejected".to_owned()),
        Err(_) => Ok(()),
    }
}

fn require_trap(error: &wasmtime::Error, expected: &str) -> Result<(), String> {
    let shown = format!("{error:?}");
    if trap_matches(&shown, expected) {
        Ok(())
    } else {
        Err(format!("expected trap '{expected}', got '{shown}'"))
    }
}

fn trap_matches(actual: &str, expected: &str) -> bool {
    actual.contains(expected)
        || (expected.contains("uninitialized element") && actual.contains("uninitialized element"))
        || (expected.contains("null function")
            && (actual.contains("uninitialized element") || actual.contains("null reference")))
        || (expected.contains("null")
            && expected.contains("reference")
            && actual.contains("null reference"))
        || (expected.contains("out of bounds") && actual.contains("out of bounds"))
        || (expected.contains("integer overflow") && actual.contains("integer overflow"))
        || (expected.contains("integer divide") && actual.contains("integer"))
        || (expected.contains("unreachable") && actual.contains("unreachable"))
}

fn define_spectest(store: &mut Store<()>, linker: &mut Linker<()>) -> Result<(), String> {
    linker
        .func_wrap("spectest", "print", || {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_i32", |_: i32| {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_i64", |_: i64| {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_f32", |_: f32| {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_f64", |_: f64| {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_i32_f32", |_: i32, _: f32| {})
        .map_err(|error| error.to_string())?;
    linker
        .func_wrap("spectest", "print_f64_f64", |_: f64, _: f64| {})
        .map_err(|error| error.to_string())?;

    define_global(store, linker, "global_i32", ValType::I32, Val::I32(666))?;
    define_global(store, linker, "global_i64", ValType::I64, Val::I64(666))?;
    define_global(
        store,
        linker,
        "global_f32",
        ValType::F32,
        Val::F32(666.6_f32.to_bits()),
    )?;
    define_global(
        store,
        linker,
        "global_f64",
        ValType::F64,
        Val::F64(666.6_f64.to_bits()),
    )?;

    let table = Table::new(
        &mut *store,
        TableType::new(wasmtime::RefType::FUNCREF, 10, Some(20)),
        Ref::Func(None),
    )
    .map_err(|error| error.to_string())?;
    linker
        .define(&*store, "spectest", "table", table)
        .map_err(|error| error.to_string())?;

    let memory64_table = Table::new(
        &mut *store,
        TableType::new64(wasmtime::RefType::FUNCREF, 0, None),
        Ref::Func(None),
    )
    .map_err(|error| error.to_string())?;
    linker
        .define(&*store, "spectest", "table64", memory64_table)
        .map_err(|error| error.to_string())?;

    let memory =
        Memory::new(&mut *store, MemoryType::new(1, Some(2))).map_err(|error| error.to_string())?;
    linker
        .define(&*store, "spectest", "memory", memory)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn define_global(
    store: &mut Store<()>, linker: &mut Linker<()>, name: &str, ty: ValType, value: Val,
) -> Result<(), String> {
    let handle = Global::new(&mut *store, GlobalType::new(ty, Mutability::Const), value)
        .map_err(|error| error.to_string())?;
    linker
        .define(&*store, "spectest", name, handle)
        .map_err(|error| error.to_string())
        .map(drop)
}

const fn null_val(heap: HeapType<'_>) -> Val {
    match heap {
        HeapType::Abstract { ty, .. } => {
            match ty {
                AbstractHeapType::Func | AbstractHeapType::NoFunc => Val::FuncRef(None),
                AbstractHeapType::Extern | AbstractHeapType::NoExtern => Val::ExternRef(None),
                AbstractHeapType::Exn | AbstractHeapType::NoExn => Val::ExnRef(None),
                AbstractHeapType::Cont | AbstractHeapType::NoCont => Val::ContRef(None),
                AbstractHeapType::Any
                | AbstractHeapType::Eq
                | AbstractHeapType::Struct
                | AbstractHeapType::Array
                | AbstractHeapType::I31
                | AbstractHeapType::None => Val::AnyRef(None),
            }
        }
        HeapType::Concrete(_) | HeapType::Exact(_) => Val::AnyRef(None),
    }
}

fn match_vals(store: &mut Store<()>, got: &[Val], want: &[WastRet<'_>]) -> Result<(), String> {
    if got.len() != want.len() {
        return Err(format!(
            "expected {} results, got {}",
            want.len(),
            got.len()
        ));
    }
    for (index, (actual, expected)) in got.iter().zip(want.iter()).enumerate() {
        match_one(store, actual, expected).map_err(|error| format!("result {index}: {error}"))?;
    }
    Ok(())
}

fn match_one(store: &mut Store<()>, actual: &Val, expected: &WastRet<'_>) -> Result<(), String> {
    match expected {
        WastRet::Core(core) => match_core(store, actual, core),
        WastRet::Component(_) => Err("component results are not executed".to_owned()),
        _ => Err("unsupported wast result".to_owned()),
    }
}

fn match_core(
    store: &mut Store<()>, actual: &Val, expected: &WastRetCore<'_>,
) -> Result<(), String> {
    match expected {
        WastRetCore::I32(want) => {
            match actual {
                Val::I32(got) if got == want => Ok(()),
                _ => Err(format!("expected i32 {want}, got {actual:?}")),
            }
        }
        WastRetCore::I64(want) => {
            match actual {
                Val::I64(got) if got == want => Ok(()),
                _ => Err(format!("expected i64 {want}, got {actual:?}")),
            }
        }
        WastRetCore::F32(pattern) => {
            match actual {
                Val::F32(bits) => match_nan32(*bits, *pattern),
                _ => Err(format!("expected f32, got {actual:?}")),
            }
        }
        WastRetCore::F64(pattern) => {
            match actual {
                Val::F64(bits) => match_nan64(*bits, pattern),
                _ => Err(format!("expected f64, got {actual:?}")),
            }
        }
        WastRetCore::V128(pattern) => {
            match actual {
                Val::V128(bits) => match_v128(bits.as_u128(), pattern),
                _ => Err(format!("expected v128, got {actual:?}")),
            }
        }
        WastRetCore::RefNull(heap) => match_null(actual, *heap),
        WastRetCore::RefExtern(want) => match_extern(store, actual, *want),
        WastRetCore::RefHost(want) => match_extern(store, actual, Some(*want)),
        WastRetCore::RefFunc(_) => {
            match actual {
                Val::FuncRef(Some(_)) => Ok(()),
                _ => Err(format!("expected funcref, got {actual:?}")),
            }
        }
        WastRetCore::RefAny => {
            match actual {
                Val::AnyRef(Some(_)) => Ok(()),
                _ => Err(format!("expected anyref, got {actual:?}")),
            }
        }
        WastRetCore::RefEq => match_eq(store, actual),
        WastRetCore::RefArray => {
            match actual {
                Val::AnyRef(Some(handle)) => {
                    match handle.as_array(store) {
                        Ok(Some(_)) => Ok(()),
                        _ => Err("expected arrayref".to_owned()),
                    }
                }
                _ => Err(format!("expected arrayref, got {actual:?}")),
            }
        }
        WastRetCore::RefStruct => {
            match actual {
                Val::AnyRef(Some(handle)) => {
                    match handle.as_struct(store) {
                        Ok(Some(_)) => Ok(()),
                        _ => Err("expected structref".to_owned()),
                    }
                }
                _ => Err(format!("expected structref, got {actual:?}")),
            }
        }
        WastRetCore::RefI31 | WastRetCore::RefI31Shared => {
            match actual {
                Val::AnyRef(Some(handle)) => {
                    match handle.is_i31(store) {
                        Ok(true) => Ok(()),
                        _ => Err("expected i31ref".to_owned()),
                    }
                }
                _ => Err(format!("expected i31ref, got {actual:?}")),
            }
        }
        WastRetCore::Either(choices) => {
            if choices
                .iter()
                .any(|choice| match_core(store, actual, choice).is_ok())
            {
                Ok(())
            } else {
                Err(format!("no either-arm matched {actual:?}"))
            }
        }
    }
}

fn match_null(actual: &Val, heap: Option<HeapType<'_>>) -> Result<(), String> {
    let is_null = matches!(
        actual,
        Val::FuncRef(None)
            | Val::ExternRef(None)
            | Val::AnyRef(None)
            | Val::ExnRef(None)
            | Val::ContRef(None)
    );
    if !is_null {
        return Err(format!("expected null, got {actual:?}"));
    }
    let Some(heap) = heap else {
        return Ok(());
    };
    match (heap, actual) {
        (
            HeapType::Abstract {
                ty: AbstractHeapType::Func | AbstractHeapType::NoFunc,
                ..
            },
            Val::FuncRef(None),
        )
        | (
            HeapType::Abstract {
                ty: AbstractHeapType::Extern | AbstractHeapType::NoExtern,
                ..
            },
            Val::ExternRef(None),
        )
        | (
            HeapType::Abstract {
                ty: AbstractHeapType::Exn | AbstractHeapType::NoExn,
                ..
            },
            Val::ExnRef(None),
        )
        | (
            HeapType::Abstract {
                ty: AbstractHeapType::Cont | AbstractHeapType::NoCont,
                ..
            },
            Val::ContRef(None),
        )
        | (
            HeapType::Abstract {
                ty:
                    AbstractHeapType::Any
                    | AbstractHeapType::Eq
                    | AbstractHeapType::Struct
                    | AbstractHeapType::Array
                    | AbstractHeapType::I31
                    | AbstractHeapType::None,
                ..
            },
            Val::AnyRef(None),
        )
        | (HeapType::Concrete(_) | HeapType::Exact(_), _) => Ok(()),
        _ => Err(format!("null heap type mismatch: {actual:?}")),
    }
}

fn match_extern(store: &mut Store<()>, actual: &Val, want: Option<u32>) -> Result<(), String> {
    let handle = match actual {
        Val::ExternRef(Some(handle)) => *handle,
        Val::AnyRef(Some(handle)) => {
            ExternRef::convert_any(&mut *store, *handle).map_err(|error| error.to_string())?
        }
        _ => return Err(format!("expected externref, got {actual:?}")),
    };
    let Some(want) = want else {
        return Ok(());
    };
    let data = handle
        .data(&*store)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "externref has no host data".to_owned())?;
    let Some(got) = data.downcast_ref::<u32>() else {
        return Err("externref was not a u32 host value".to_owned());
    };
    if *got == want {
        Ok(())
    } else {
        Err(format!("expected extern {want}, got {got}"))
    }
}

fn match_eq(store: &Store<()>, actual: &Val) -> Result<(), String> {
    let Val::AnyRef(Some(handle)) = actual else {
        return Err(format!("expected eqref, got {actual:?}"));
    };
    let i31 = handle.is_i31(store).unwrap_or(false);
    let structure = handle.as_struct(store).is_ok_and(|value| value.is_some());
    let array = handle.as_array(store).is_ok_and(|value| value.is_some());
    if i31 || structure || array {
        Ok(())
    } else {
        Err("anyref is not in the eq hierarchy".to_owned())
    }
}

fn match_nan32(bits: u32, pattern: NanPattern<F32>) -> Result<(), String> {
    match &pattern {
        NanPattern::CanonicalNan if is_canon_single(bits) => Ok(()),
        NanPattern::ArithmeticNan if is_arith_single(bits) => Ok(()),
        NanPattern::Value(F32 { bits: want }) if bits == *want => Ok(()),
        NanPattern::CanonicalNan => Err(format!("expected canonical f32 nan, got {bits:#010x}")),
        NanPattern::ArithmeticNan => Err(format!("expected arithmetic f32 nan, got {bits:#010x}")),
        NanPattern::Value(F32 { bits: want }) => {
            Err(format!("expected f32 {want:#010x}, got {bits:#010x}"))
        }
    }
}

fn match_nan64(bits: u64, pattern: &NanPattern<F64>) -> Result<(), String> {
    match pattern {
        NanPattern::CanonicalNan if is_canon_double(bits) => Ok(()),
        NanPattern::ArithmeticNan if is_arith_double(bits) => Ok(()),
        NanPattern::Value(F64 { bits: want }) if bits == *want => Ok(()),
        NanPattern::CanonicalNan => Err(format!("expected canonical f64 nan, got {bits:#018x}")),
        NanPattern::ArithmeticNan => Err(format!("expected arithmetic f64 nan, got {bits:#018x}")),
        NanPattern::Value(F64 { bits: want }) => {
            Err(format!("expected f64 {want:#018x}, got {bits:#018x}"))
        }
    }
}

const fn is_canon_single(bits: u32) -> bool { bits & 0x7fff_ffff == 0x7fc0_0000 }

const fn is_arith_single(bits: u32) -> bool {
    bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0
}

const fn is_canon_double(bits: u64) -> bool {
    bits & 0x7fff_ffff_ffff_ffff == 0x7ff8_0000_0000_0000
}

const fn is_arith_double(bits: u64) -> bool {
    bits & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000 && bits & 0x000f_ffff_ffff_ffff != 0
}

fn match_v128(bits: u128, pattern: &V128Pattern) -> Result<(), String> {
    let le = bits.to_le_bytes();
    match pattern {
        V128Pattern::I8x16(want) => lanes_i8(&le, want),
        V128Pattern::I16x8(want) => lanes_i16(&le, want),
        V128Pattern::I32x4(want) => lanes_i32(&le, want),
        V128Pattern::I64x2(want) => lanes_i64(&le, want),
        V128Pattern::F32x4(want) => lanes_f32(&le, want),
        V128Pattern::F64x2(want) => lanes_f64(&le, want),
    }
}

fn lanes_i8(le: &[u8; 16], want: &[i8; 16]) -> Result<(), String> {
    for (lane, expected) in want.iter().copied().enumerate() {
        let Some(&byte) = le.get(lane) else {
            return Err(format!("missing i8x16 lane {lane}"));
        };
        let got = byte as i8;
        if got != expected {
            return Err(format!("i8x16 lane {lane}: expected {expected}, got {got}"));
        }
    }
    Ok(())
}

fn lanes_i16(le: &[u8; 16], want: &[i16; 8]) -> Result<(), String> {
    for (lane, expected) in want.iter().copied().enumerate() {
        let got = read_i16(le, lane)?;
        if got != expected {
            return Err(format!("i16x8 lane {lane}: expected {expected}, got {got}"));
        }
    }
    Ok(())
}

fn lanes_i32(le: &[u8; 16], want: &[i32; 4]) -> Result<(), String> {
    for (lane, expected) in want.iter().copied().enumerate() {
        let got = read_i32(le, lane)?;
        if got != expected {
            return Err(format!("i32x4 lane {lane}: expected {expected}, got {got}"));
        }
    }
    Ok(())
}

fn lanes_i64(le: &[u8; 16], want: &[i64; 2]) -> Result<(), String> {
    for (lane, expected) in want.iter().copied().enumerate() {
        let got = read_i64(le, lane)?;
        if got != expected {
            return Err(format!("i64x2 lane {lane}: expected {expected}, got {got}"));
        }
    }
    Ok(())
}

fn lanes_f32(le: &[u8; 16], want: &[NanPattern<F32>; 4]) -> Result<(), String> {
    for (lane, pattern) in want.iter().enumerate() {
        let bits = read_i32(le, lane)? as u32;
        match_nan32(bits, *pattern).map_err(|error| format!("f32x4 lane {lane}: {error}"))?;
    }
    Ok(())
}

fn lanes_f64(le: &[u8; 16], want: &[NanPattern<F64>; 2]) -> Result<(), String> {
    for (lane, pattern) in want.iter().enumerate() {
        let bits = read_i64(le, lane)? as u64;
        match_nan64(bits, pattern).map_err(|error| format!("f64x2 lane {lane}: {error}"))?;
    }
    Ok(())
}

fn read_i16(le: &[u8; 16], lane: usize) -> Result<i16, String> {
    let start = lane
        .checked_mul(2)
        .ok_or_else(|| "i16 lane overflow".to_owned())?;
    let end = start
        .checked_add(2)
        .ok_or_else(|| "i16 lane overflow".to_owned())?;
    let bytes = le
        .get(start..end)
        .ok_or_else(|| format!("missing i16x8 lane {lane}"))?;
    let arr: [u8; 2] = bytes
        .try_into()
        .map_err(|_error| format!("short i16x8 lane {lane}"))?;
    Ok(i16::from_le_bytes(arr))
}

fn read_i32(le: &[u8; 16], lane: usize) -> Result<i32, String> {
    let start = lane
        .checked_mul(4)
        .ok_or_else(|| "i32 lane overflow".to_owned())?;
    let end = start
        .checked_add(4)
        .ok_or_else(|| "i32 lane overflow".to_owned())?;
    let bytes = le
        .get(start..end)
        .ok_or_else(|| format!("missing i32x4 lane {lane}"))?;
    let arr: [u8; 4] = bytes
        .try_into()
        .map_err(|_error| format!("short i32x4 lane {lane}"))?;
    Ok(i32::from_le_bytes(arr))
}

fn read_i64(le: &[u8; 16], lane: usize) -> Result<i64, String> {
    let start = lane
        .checked_mul(8)
        .ok_or_else(|| "i64 lane overflow".to_owned())?;
    let end = start
        .checked_add(8)
        .ok_or_else(|| "i64 lane overflow".to_owned())?;
    let bytes = le
        .get(start..end)
        .ok_or_else(|| format!("missing i64x2 lane {lane}"))?;
    let arr: [u8; 8] = bytes
        .try_into()
        .map_err(|_error| format!("short i64x2 lane {lane}"))?;
    Ok(i64::from_le_bytes(arr))
}
