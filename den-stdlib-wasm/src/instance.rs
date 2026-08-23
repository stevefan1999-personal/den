//! `WebAssembly.Instance`, import resolution and the two function boundaries.

use std::cell::RefCell;

use rquickjs::{
    Array, Class, Ctx, Exception, Function, IntoJs, JsLifetime, Object, Result, Value,
    class::Trace,
    function::{Args, Opt},
};

use crate::{
    Probe,
    backend::{self, Val},
    engine::Engine,
    error::{throw_link_error, throw_runtime_error},
    memory::MemoryBuffers,
    module::Module,
    store::{Store, WasiImports},
    utils::{HostReferences, WasmValue},
};

/// The JS functions every instance of this context imported into wasm.
///
/// A wasm host callback must be `Send + Sync + 'static`, which no JS value is.
/// The callback therefore captures an index into this registry and reaches the
/// function through the `Ctx` parked in the store payload.
///
/// ponytail: entries are never removed, so an imported function lives as long
/// as the context. The store already keeps every instance alive for just as
/// long, so this adds no leak that is not already there; per-instance
/// registries would need the instance to be reachable from the callback.
#[derive(JsLifetime)]
pub struct ImportedFunctions<'js> {
    functions: RefCell<Vec<Function<'js>>>,
}

impl<'js> ImportedFunctions<'js> {
    fn register(ctx: &Ctx<'js>, function: Function<'js>) -> Result<usize> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let mut functions = registry
            .functions
            .try_borrow_mut()
            .map_err(|_| Self::busy(ctx))?;
        functions.push(function);
        Ok(functions.len() - 1)
    }

    fn get(ctx: &Ctx<'js>, index: usize) -> Result<Function<'js>> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let function = registry
            .functions
            .try_borrow()
            .map_err(|_| Self::busy(ctx))?
            .get(index)
            .cloned();
        function.ok_or_else(|| {
            Exception::throw_internal(ctx, "an imported WebAssembly function went missing")
        })
    }

    fn missing(ctx: &Ctx<'js>) -> rquickjs::Error {
        Exception::throw_internal(
            ctx,
            "the WebAssembly import registry is missing from this context",
        )
    }

    /// Nothing here runs JS while the registry is borrowed, so this is a
    /// belt-and-braces answer rather than a reachable state — but a `RefCell`
    /// on a JS-reachable path may not be allowed to panic.
    fn busy(ctx: &Ctx<'js>) -> rquickjs::Error {
        Exception::throw_internal(ctx, "the WebAssembly import registry is already in use")
    }
}

impl Default for ImportedFunctions<'_> {
    fn default() -> Self {
        Self {
            functions: RefCell::new(Vec::new()),
        }
    }
}

/// One JS function imported into wasm.
///
/// Lifetime-free on purpose: this is what the engine's host callback closure
/// captures, and that closure has to be `'static`.
struct HostFunction {
    index:     usize,
    signature: backend::FuncType,
}

impl HostFunction {
    /// "Run a host function", with the JS exception left pending on the context
    /// so that the frame the trap unwinds to — an Exported Function call, or
    /// [`Instance::throw_instantiation_failure`] for a `start` function — can
    /// rethrow the original object rather than the engine's trap description.
    fn run(
        &self,
        caller: backend::Caller<'_>,
        params: &[Val],
        results: &mut [Val],
    ) -> core::result::Result<(), backend::Error> {
        caller.data().with_ctx(|ctx| {
            // wasm may have grown a memory before calling out, so the buffer
            // refresh has to happen on the way *in* as well as on the way out:
            // otherwise this JS frame writes through a view built before the
            // growth, which on wasmi is freed memory. The store is borrowed by
            // the call that got us here, hence the `Caller` rather than `Store`.
            MemoryBuffers::refresh_in(ctx, &caller)
                .and_then(|()| self.call(ctx, params, results))
                .map_err(|err| backend::host_error(&format!("imported function failed: {err}")))
        })
    }

    fn call(&self, ctx: &Ctx<'_>, params: &[Val], results: &mut [Val]) -> Result<()> {
        let function = ImportedFunctions::get(ctx, self.index)?;
        // The clone is load-bearing: wasmi's `Val` is not `Copy`.
        #[allow(clippy::clone_on_copy)]
        let arguments = params
            .iter()
            .map(|value| WasmValue(value.clone()).to_js(ctx))
            .collect::<Result<Vec<_>>>()?;
        let mut call_arguments = Args::new(ctx.clone(), arguments.len());
        call_arguments.push_args(arguments)?;
        let returned: Value = function.call_arg(call_arguments)?;

        // Result adaptation is by arity, not by the shape of the returned
        // value: with one result an Array *is* the value to coerce, and with
        // several the value must be iterable. (`Vec::from_iter` because the
        // two backends return an iterator and a slice respectively.)
        let types = Vec::from_iter(self.signature.results());
        match results.len() {
            0 => Ok(()),
            1 => {
                // Bound rather than borrowed inline: on the slice-returning backend
                // the element already *is* a reference (and `&types[0]` would be one
                // too many), on the iterator-returning one the binding takes it.
                let ty = &types[0];
                results[0] = WasmValue::from_js(ctx, &returned, ty)?.into_inner();
                Ok(())
            }
            arity => {
                let values = Self::iterate(ctx, returned)?;
                if values.len() != arity {
                    return Err(Exception::throw_type(
                        ctx,
                        &format!(
                            "imported function returned {} values, but its signature declares \
                             {arity}",
                            values.len()
                        ),
                    ));
                }
                for (slot, (value, ty)) in results.iter_mut().zip(values.iter().zip(&types)) {
                    *slot = WasmValue::from_js(ctx, &value?, ty)?.into_inner();
                }
                Ok(())
            }
        }
    }

    /// `IteratorToList` — `Array.from` is exactly that algorithm, including the
    /// `TypeError` for a value with no `Symbol.iterator`.
    fn iterate<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Array<'js>> {
        ctx.globals()
            .get::<_, Object>("Array")?
            .get::<_, Function>("from")?
            .call((value,))
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Instance<'js> {
    /// `[[Exports]]`: built once at instantiation time, frozen and
    /// null-prototype, so `i.exports === i.exports` and `i.exports.f ===
    /// i.exports.f` both hold.
    #[qjs(get, enumerable)]
    exports: Object<'js>,
}

impl<'js> Instance<'js> {
    pub fn instantiate(
        ctx: &Ctx<'js>,
        module: &Module,
        import_object: Option<Value<'js>>,
    ) -> Result<Self> {
        let engine = Engine::from_ctx(ctx)?;
        let store = Store::from_ctx(ctx)?;
        let mut linker = backend::Linker::new(&engine);

        let instantiated = store.with_mut(ctx, move |backend_store| {
            Self::read_imports(
                ctx,
                module,
                import_object.as_ref(),
                backend_store,
                &mut linker,
            )?;
            backend::linker_instantiate(&linker, backend_store, &module.inner)
                .map_err(|err| Self::throw_instantiation_failure(ctx, err))
        });
        // A `start` function runs arbitrary wasm, `memory.grow` included, so the
        // same buffer refresh a returning export gets applies here — and it has
        // to run for a failed instantiation too, which may have grown a memory
        // before it trapped. The instantiation failure is the more useful error
        // of the two, so it is the one reported.
        let refreshed = MemoryBuffers::refresh(ctx);
        let instance = instantiated?;
        refreshed?;
        // Outside the store borrow: each Exported Function takes its own to
        // read its signature, and `Object.freeze` runs JS.
        Ok(Self {
            exports: Self::create_exports_object(ctx, module, &instance)?,
        })
    }

    /// "Read the imports": one `Get` per declared import, in module order,
    /// because those `Get`s can run arbitrary JS.
    fn read_imports(
        ctx: &Ctx<'js>,
        module: &Module,
        import_object: Option<&Value<'js>>,
        store: &mut backend::Store,
        linker: &mut backend::Linker,
    ) -> Result<()> {
        let imports = Module::in_declaration_order(
            &module.declared_imports(),
            module.inner.imports().collect(),
            |import| (import.module(), import.name()),
        );
        // Step 1 of "read the imports": a module with imports needs an object to
        // read them from, and that check happens once, before any `Get`.
        //
        // Nothing is linked implicitly, `wasi_snapshot_preview1` included. den
        // used to satisfy that namespace out of `backend::link_wasi` whenever a
        // module asked for it, which handed the module the host's stdio and
        // environment on the strength of the module's own say-so — a capability
        // no caller of `WebAssembly.instantiate` ever asked to grant. An import
        // nobody satisfies is this `TypeError`; a caller who wants WASI passes
        // the namespace like any other.
        let import_object = match import_object.and_then(Value::as_object) {
            Some(object) => object.clone(),
            None if imports.is_empty() => return Ok(()),
            None => {
                return Err(Exception::throw_type(
                    ctx,
                    "the importObject of a module with imports must be an object",
                ));
            }
        };

        for import in imports {
            let (namespace_name, name) = (import.module(), import.name());
            let namespace: Value<'js> = import_object.get(namespace_name)?;
            let namespace = namespace.into_object().ok_or_else(|| {
                Exception::throw_type(
                    ctx,
                    &format!(
                        "importObject does not provide an object for the namespace \
                         \"{namespace_name}\""
                    ),
                )
            })?;
            // Explicit, opt-in WASI: `wasiImports()` from `den:wasm` stands in for the
            // whole preview1 namespace, because those functions are implemented by the
            // engine against the calling instance's own memory and have no JS spelling.
            if WasiImports::is_marker(ctx, &namespace) {
                WasiImports::link(ctx, linker, namespace_name)?;
                continue;
            }
            let value: Value<'js> = namespace.get(name)?;
            // Bound rather than borrowed inline: one backend hands out a reference to
            // the import type and the other an owned one, so the binding is what makes
            // `&ExternType` the type at the call either way.
            let ty = &import.ty();
            Self::define_import(ctx, linker, store, namespace_name, name, ty, &value)?;
        }
        Ok(())
    }

    fn define_import(
        ctx: &Ctx<'js>,
        linker: &mut backend::Linker,
        store: &mut backend::Store,
        namespace: &str,
        name: &str,
        ty: &backend::ExternType,
        value: &Value<'js>,
    ) -> Result<()> {
        if let Some(signature) = ty.func() {
            return Self::define_host_function(ctx, linker, namespace, name, signature, value);
        }
        let external = if let Some(ty) = ty.global() {
            Self::global_import(ctx, store, ty, value)?
        } else if let Some(ty) = ty.memory() {
            Self::memory_import(ctx, store, ty, value)?
        } else if let Some(ty) = ty.table() {
            Self::table_import(ctx, store, ty, value)?
        } else {
            return Err(throw_link_error(
                ctx,
                format_args!(
                    "cannot import {namespace}.{name}: a {} import is not supported",
                    backend::extern_kind_name(ty)
                ),
            ));
        };
        backend::linker_define(linker, store, namespace, name, external)
            .map_err(|err| throw_link_error(ctx, err))
    }

    fn define_host_function(
        ctx: &Ctx<'js>,
        linker: &mut backend::Linker,
        namespace: &str,
        name: &str,
        signature: &backend::FuncType,
        value: &Value<'js>,
    ) -> Result<()> {
        let function = value.as_function().cloned().ok_or_else(|| {
            throw_link_error(
                ctx,
                format_args!("import {namespace}.{name} is not a function"),
            )
        })?;
        let host = HostFunction {
            index:     ImportedFunctions::register(ctx, function)?,
            signature: signature.clone(),
        };
        backend::linker_func_new(
            linker,
            namespace,
            name,
            signature.clone(),
            move |caller, params, results| host.run(caller, params, results),
        )
        .map_err(|err| throw_link_error(ctx, err))
    }

    /// A global import is either an existing `WebAssembly.Global` or a plain
    /// value that becomes a fresh global of the declared type.
    fn global_import(
        ctx: &Ctx<'js>,
        store: &mut backend::Store,
        ty: &backend::GlobalType,
        value: &Value<'js>,
    ) -> Result<backend::Extern> {
        if let Some(global) = ctx.probe(|| {
            value
                .as_object()
                .and_then(Class::<crate::global::Global>::from_object)
        }) {
            let inner = Self::borrow_import(ctx, &global, "global")?.inner;
            return Ok(inner.into());
        }
        // Anything that is not a `Global`, a Number or a BigInt is a link
        // failure, not something to coerce: without this an omitted import
        // would silently become the default value of its type.
        if !(value.is_number() || value.as_big_int().is_some()) {
            return Err(throw_link_error(
                ctx,
                "a global import must be a WebAssembly.Global, a Number or a BigInt",
            ));
        }
        let content = backend::global_content(ty);
        // The read-the-imports steps single out the numeric flavours *before*
        // coercion, and report a mismatch as a `LinkError` rather than the
        // `TypeError` `ToWebAssemblyValue` would raise: "If valuetype is v128,
        // throw a LinkError"; "if valuetype is i64 and Type(v) is Number, throw
        // a LinkError"; "if valuetype is not i64 and Type(v) is BigInt, throw a
        // LinkError".
        let mismatched = match backend::val_type_kind(&content) {
            Some(backend::ValKind::V128) => true,
            Some(backend::ValKind::I64) => value.is_number(),
            _ => value.as_big_int().is_some(),
        };
        if mismatched {
            return Err(throw_link_error(
                ctx,
                format_args!(
                    "a {} global import cannot be initialised from this value",
                    backend::val_type_name(&content).unwrap_or("WebAssembly")
                ),
            ));
        }
        let initial = WasmValue::from_js(ctx, value, &content)?;
        backend::new_global(
            store,
            &content,
            matches!(ty.mutability(), backend::Mutability::Var),
            initial.into_inner(),
        )
        .map(Into::into)
        .map_err(|err| throw_link_error(ctx, err))
    }

    fn memory_import(
        ctx: &Ctx<'js>,
        store: &mut backend::Store,
        ty: &backend::MemoryType,
        value: &Value<'js>,
    ) -> Result<backend::Extern> {
        let memory = ctx
            .probe(|| {
                value
                    .as_object()
                    .and_then(Class::<crate::memory::Memory>::from_object)
            })
            .ok_or_else(|| {
                throw_link_error(ctx, "a memory import must be a WebAssembly.Memory object")
            })?;
        let inner = Self::borrow_import(ctx, &memory, "memory")?.inner;
        Self::check_limits(
            ctx,
            "memory",
            (ty.minimum(), ty.maximum()),
            (inner.size(&*store), inner.ty(&*store).maximum()),
        )?;
        Ok(inner.into())
    }

    fn table_import(
        ctx: &Ctx<'js>,
        store: &mut backend::Store,
        ty: &backend::TableType,
        value: &Value<'js>,
    ) -> Result<backend::Extern> {
        let table = ctx
            .probe(|| {
                value
                    .as_object()
                    .and_then(Class::<crate::table::Table>::from_object)
            })
            .ok_or_else(|| {
                throw_link_error(ctx, "a table import must be a WebAssembly.Table object")
            })?;
        let inner = Self::borrow_import(ctx, &table, "table")?.inner;
        // The element type is left to the engine: a mismatch fails
        // instantiation, which is a `LinkError` all the same.
        Self::check_limits(
            ctx,
            "table",
            (ty.minimum(), ty.maximum()),
            (inner.size(&*store), inner.ty(&*store).maximum()),
        )?;
        Ok(inner.into())
    }

    /// Borrow an imported wrapper without the panic `Class::borrow` would
    /// raise.
    ///
    /// Instantiation is re-entrant from pure JS — a `valueOf` hook passed to
    /// `memory.grow()` can start an instantiation that imports that same
    /// `memory` — so the cell may already be borrowed by the frame below.
    fn borrow_import<'a, C>(
        ctx: &Ctx<'js>,
        class: &'a Class<'js, C>,
        kind: &str,
    ) -> Result<rquickjs::class::Borrow<'a, 'js, C>>
    where
        C: rquickjs::class::JsClass<'js>,
    {
        class.try_borrow().map_err(|_| {
            throw_link_error(
                ctx,
                format_args!("the imported {kind} is already in use by an outer call"),
            )
        })
    }

    /// Limits matching, as the core spec defines it: the provided item must be
    /// at least as large as the import declares, and no less bounded.
    ///
    /// `provided` is the item's *current* size, not the minimum its descriptor
    /// was created with: the spec matches against `mem_type(store, memaddr)` /
    /// `table_type(store, tableaddr)`, and growing a memory or table updates
    /// the instance's own type (core spec, "Growing memories":
    /// `meminst.type = {min: n', max}`). A memory grown to two pages
    /// therefore satisfies an import declaring `(memory 2)` even though it
    /// was created with `{ initial: 1 }`.
    fn check_limits(
        ctx: &Ctx<'js>,
        kind: &str,
        declared: (u64, Option<u64>),
        provided: (u64, Option<u64>),
    ) -> Result<()> {
        let (declared_minimum, declared_maximum) = declared;
        let (provided_minimum, provided_maximum) = provided;
        let matches = provided_minimum >= declared_minimum
            && declared_maximum
                .is_none_or(|declared| provided_maximum.is_some_and(|actual| actual <= declared));
        if matches {
            Ok(())
        } else {
            Err(throw_link_error(
                ctx,
                format_args!(
                    "imported {kind} does not match the declared type: the module requires at \
                     least {declared_minimum} and at most {declared_maximum:?} pages or elements, \
                     but got {provided_minimum} and {provided_maximum:?}"
                ),
            ))
        }
    }

    /// "Create an exports object": module order, null prototype, frozen, once.
    fn create_exports_object(
        ctx: &Ctx<'js>,
        module: &Module,
        instance: &backend::Instance,
    ) -> Result<Object<'js>> {
        let names = Module::in_declaration_order(
            &module.declared_exports(),
            module.inner.exports().map(|export| export.name()).collect(),
            |name| *name,
        );
        // Every export is resolved under one short borrow and wrapped outside
        // it: an Exported Function takes its own borrow to read its signature,
        // and `Object.freeze` below runs JS.
        let externals = Store::from_ctx(ctx)?.with_mut(ctx, |backend_store| {
            names
                .iter()
                .map(|name| {
                    instance
                        .get_export(&mut *backend_store, name)
                        .ok_or_else(|| {
                            throw_link_error(
                                ctx,
                                format_args!(
                                    "the instance is missing its declared export \"{name}\""
                                ),
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()
        })?;

        let exports = Object::new(ctx.clone())?;
        exports.set_prototype(None)?;
        let function_indices = module.exported_function_indices();

        for (name, external) in names.iter().zip(externals) {
            // Each `into_*` consumes the extern, and its kind is only learnt by
            // trying, so every arm needs its own copy of it.
            #[allow(
                clippy::clone_on_copy,
                reason = "wasmi's `Extern` is `Copy` and wasmtime's is only `Clone`, so the clone \
                          is redundant on one backend and required on the other"
            )]
            let value = if let Some(func) = external.clone().into_func() {
                // The same builder a `funcref` read out of a table goes
                // through, so that one wasm function is one JS object however
                // JS reaches it — which is what lets `table.set(0,
                // instance.exports.f)` recognise this as an Exported Function.
                // The JS API names it after its index in the module; the export
                // name is only how JS reaches it.
                let index = function_indices.get(name).copied();
                HostReferences::exported_function(ctx, func, index)?.into_value()
            } else if let Some(memory) = external.clone().into_memory() {
                crate::memory::Memory::from(memory).into_js(ctx)?
            } else if let Some(table) = external.clone().into_table() {
                crate::table::Table::from(table).into_js(ctx)?
            } else if let Some(global) = external.clone().into_global() {
                crate::global::Global::from(global).into_js(ctx)?
            } else if let Some(tag) = Self::export_tag(&external) {
                tag.into_js(ctx)?
            } else {
                // wasmtime's `SharedMemory` is the only remaining kind, and den
                // has no `SharedArrayBuffer`-backed `Memory` to wrap one in
                // (see `backend::SUPPORTS_SHARED_MEMORY`). Refusing out loud
                // rather than skipping the entry keeps `instance.exports` and
                // `Module.exports()` telling the same story: the latter lists
                // every export the module declares, so an omission here would
                // be an `undefined` property with no diagnostic. Unreachable
                // today — the engine is built without the threads proposal, so
                // such a module never compiles — but it is the mismatch, not
                // the reachability, that this branch is about.
                return Err(throw_link_error(
                    ctx,
                    format_args!(
                        "the export \"{name}\" is of a kind this build cannot represent in JS"
                    ),
                ));
            };
            exports.set(*name, value)?;
        }

        ctx.globals()
            .get::<_, Object>("Object")?
            .get::<_, Function>("freeze")?
            .call::<_, ()>((exports.clone(),))?;
        Ok(exports)
    }

    /// An exported tag, on a backend that has tags at all.
    ///
    /// wasmi implements no part of the exception-handling proposal — its
    /// `Extern` has no tag variant and its `Tag` wrapper no field to build one
    /// from — so this is one of the few places the two engines cannot share a
    /// spelling. It stays here rather than in `backend/` because there is
    /// nothing for the other backend to implement.
    #[cfg(feature = "wasmtime")]
    fn export_tag(external: &backend::Extern) -> Option<crate::tag::Tag> {
        external
            .clone()
            .into_tag()
            .map(|inner| crate::tag::Tag { inner })
    }

    #[cfg(not(feature = "wasmtime"))]
    fn export_tag(_external: &backend::Extern) -> Option<crate::tag::Tag> {
        None
    }

    /// Whether an engine error is a *trap* rather than a link or host failure.
    ///
    /// Same story as [`Self::export_tag`]: neither engine spells this the same
    /// way and neither can express the other's, so the two-line difference
    /// lives at its single call site.
    #[cfg(feature = "wasmtime")]
    fn is_trap(error: &backend::Error) -> bool {
        error.downcast_ref::<::wasmtime::Trap>().is_some()
    }

    #[cfg(not(feature = "wasmtime"))]
    fn is_trap(error: &backend::Error) -> bool {
        error.as_trap_code().is_some()
    }

    /// The JS API separates the two ways instantiation can fail: an import that
    /// cannot be linked is a `LinkError`, while a trap — which at instantiation
    /// time can only come from an active segment initialiser or the module's
    /// `start` function — is a `RuntimeError`, because by then real wasm code
    /// has run.
    fn throw_instantiation_failure(ctx: &Ctx<'js>, error: backend::Error) -> rquickjs::Error {
        // A JS exception thrown by an import called from `start` reaches the
        // caller unchanged, exactly as it does from an exported call — so a
        // still-pending exception wins over the engine's own description.
        // (`Ctx::catch` is `JS_GetException`, which hands back
        // `JS_UNINITIALIZED` when nothing is pending — neither `undefined` nor
        // `null`, so the tag cannot be used to decide this, and re-throwing
        // that sentinel segfaults QuickJS the moment JS reads a property off
        // the caught value.)
        if ctx.has_exception() {
            rquickjs::Error::Exception
        } else if Self::is_trap(&error) {
            throw_runtime_error(ctx, error)
        } else {
            throw_link_error(ctx, error)
        }
    }
}

#[rquickjs::methods]
impl<'js> Instance<'js> {
    #[qjs(constructor)]
    pub fn new(module: &Module, import_object: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        Self::instantiate(&ctx, module, import_object.0)
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{Context, Module as JsModule, Runtime};

    use super::*;

    /// A context with the whole of `den:wasm` evaluated into it: these two
    /// cross wrappers — `Instance` into `Table`, and `Instance` against
    /// `Module` — so they are written the way JS reaches them.
    fn with_wasm_namespace<R>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let (_, evaluation) =
                JsModule::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")
                    .expect("den:wasm evaluates");
            evaluation.finish::<()>().expect("den:wasm finishes");
            f(&ctx)
        })
    }

    /// An export read off `instance.exports` is an Exported Function, so it has
    /// a `[[FunctionAddress]]` a funcref table accepts — and reading it back
    /// hands out that very object rather than a second callable wrapping the
    /// same wasm function. Before the exports object went through
    /// `HostReferences::exported_function`, the `set` threw a `TypeError`.
    const FUNCREF_ROUND_TRIP: &str = r#"
      const bytes = WebAssembly.wat2wasm(`(module
        (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))`);
      const { add } = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports;
      const table = new WebAssembly.Table({ element: "anyfunc", initial: 1 });
      table.set(0, add);
      [table.get(0) === add, table.get(0) === table.get(0), table.get(0)(20, 22),
       add.name, add.length].join(",")
    "#;

    #[test]
    fn an_export_round_trips_through_a_funcref_table_as_one_callable_object() {
        with_wasm_namespace(|ctx| {
            let outcome: String = ctx.eval(FUNCREF_ROUND_TRIP).expect("the snippet runs");
            assert_eq!(outcome, "true,true,42,0,2");
        })
    }

    /// `Module.exports()` promises a name for every declared export, so the
    /// exports object has to carry all of them: an export kind den could not
    /// wrap used to be dropped silently, leaving `instance.exports.x`
    /// `undefined` with no diagnostic anywhere.
    const BOTH_LISTINGS_AGREE: &str = r#"
      const bytes = WebAssembly.wat2wasm(`(module
        (func (export "f"))
        (memory (export "m") 1)
        (table (export "t") 1 funcref)
        (global (export "g") i32 (i32.const 0)))`);
      const module = new WebAssembly.Module(bytes);
      const declared = WebAssembly.Module.exports(module).map((entry) => entry.name).join(",");
      const built = Object.keys(new WebAssembly.Instance(module).exports).join(",");
      [declared, built].join("|")
    "#;

    #[test]
    fn the_exports_object_carries_every_export_the_module_declares() {
        with_wasm_namespace(|ctx| {
            let outcome: String = ctx.eval(BOTH_LISTINGS_AGREE).expect("the snippet runs");
            assert_eq!(outcome, "f,m,t,g|f,m,t,g");
        })
    }
}
