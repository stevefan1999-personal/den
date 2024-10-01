use std::sync::Mutex;

use derive_more::derive::{Deref, DerefMut, From, Into};
use indexmap::IndexMap;
use rquickjs::{
    class::Trace, function::Args, prelude::*, Array, Ctx, Exception, Function, Persistent, Result,
    Value,
};
use tracing::warn;
use wasmtime::{AsContext, AsContextMut, Extern, RefType, Val};

use crate::{
    error::LinkError, module::Module, table::TableDescriptor, utils::WasmValueConverter,
    WasmtimeRuntimeData,
};

#[derive(Trace, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Instance<'js> {
    #[qjs(skip_trace)]
    instance: wasmtime::Instance,

    #[deref(ignore)]
    #[deref_mut(ignore)]
    store: crate::store::Store<'js>,
}

#[rquickjs::methods]
impl<'js> Instance<'js> {
    #[qjs(constructor)]
    pub fn new(
        module: &Module,
        Opt(import_object): Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
        store: Opt<crate::store::Store<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let mut linker = wasmtime::Linker::new(module.engine());
        let store = store
            .clone()
            .unwrap_or(ctx.userdata::<WasmtimeRuntimeData>().unwrap().store.clone());
        let instance = {
            let mut store = store.borrow_mut();

            // https://webassembly.github.io/spec/js-api/#read-the-imports
            let module_imports = module.imports();
            if module_imports.len() > 0 {
                let import_object = import_object.ok_or_else(|| {
                    Exception::throw_internal(&ctx, "import object is not an object")
                })?;

                for module_import in module.imports() {
                    let module = module_import.module();
                    let name = module_import.name();
                    if let Some(o) = import_object.get(module) {
                        if let Some(v) = o.get(name) {
                            match module_import.ty() {
                                wasmtime::ExternType::Global(ty)
                                    if ty.content().is_i64() && v.is_number() =>
                                {
                                    return Err(ctx.throw(LinkError::new().into_js(&ctx)?))
                                }
                                wasmtime::ExternType::Global(ty)
                                    if !ty.content().is_i64() && v.as_big_int().is_some() =>
                                {
                                    return Err(ctx.throw(LinkError::new().into_js(&ctx)?))
                                }
                                wasmtime::ExternType::Global(ty) if ty.content().is_v128() => {
                                    return Err(ctx.throw(LinkError::new().into_js(&ctx)?))
                                }
                                wasmtime::ExternType::Global(ty) => {
                                    let item = crate::global::Global::from_js(&ctx, v.clone())
                                        .or_else(|_| {
                                            crate::global::Global::from_type(
                                                ty,
                                                v,
                                                store.as_context_mut(),
                                                &ctx,
                                            )
                                        })?;

                                    linker
                                        .define(store.as_context(), module, name, item.inner)
                                        .map_err(|x| {
                                            Exception::throw_internal(
                                                &ctx,
                                                &format!("wasm linker define error: {}", x),
                                            )
                                        })?;
                                }
                                wasmtime::ExternType::Table(ty) => {
                                    if let Ok(table) = crate::table::Table::from_js(&ctx, v.clone())
                                    {
                                        // let actual_ty = table.inner.ty(store.as_context());
                                        // if actual_ty != ty {
                                        //     warn!(?ty, ?actual_ty, "table is different");
                                        // }

                                        linker
                                            .define(store.as_context(), module, name, table.inner)
                                            .map_err(|x| {
                                                Exception::throw_internal(
                                                    &ctx,
                                                    &format!("wasm linker define error: {}", x),
                                                )
                                            })?;
                                    } else {
                                        return Err(ctx.throw(LinkError::new().into_js(&ctx)?));
                                    }
                                }
                                wasmtime::ExternType::Memory(ty) => {
                                    if let Ok(memory) =
                                        crate::memory::Memory::from_js(&ctx, v.clone())
                                    {
                                        let actual_ty = memory.inner.ty(store.as_context());
                                        if actual_ty != ty {
                                            warn!(?ty, ?actual_ty, "memory is different");
                                        }

                                        linker
                                            .define(store.as_context(), module, name, memory.inner)
                                            .map_err(|x| {
                                                Exception::throw_internal(
                                                    &ctx,
                                                    &format!("wasm linker define error: {}", x),
                                                )
                                            })?;
                                    } else {
                                        return Err(ctx.throw(LinkError::new().into_js(&ctx)?));
                                    }
                                }
                                wasmtime::ExternType::Func(ty) => {
                                    if !v.is_function() {
                                        return Err(ctx.throw(LinkError::new().into_js(&ctx)?));
                                    }

                                    linker
                                        .func_new(module, name, ty, {
                                            #[derive(Clone, Copy, From, Deref, DerefMut)]
                                            struct DangerouslyImplementSync<T>(T);
                                            unsafe impl<T> Send for DangerouslyImplementSync<T> {}
                                            unsafe impl<T> Sync for DangerouslyImplementSync<T> {}

                                            let func: Persistent<Function> = v.get()?;
                                            let func = Mutex::new(DangerouslyImplementSync(func));
                                            // We know that at the time of getting this call back,
                                            // the context should still have had been alive
                                            // For some reason a Mutex is needed to make it Send
                                            // safe
                                            move |caller, params, results| {
                                                let ctx = caller.data();
                                                let func =
                                                    func.lock().unwrap().0.clone().restore(ctx)?;

                                                let mut args = Args::new(ctx.clone(), params.len());
                                                args.push_args(
                                                    params
                                                        .iter()
                                                        .map(|x| WasmValueConverter::from(*x)),
                                                )?;
                                                let res: Value = func.call_arg(args)?;
                                                if let Some(array) = res.as_array() {
                                                    if array.len() != results.len() {
                                                        Err(Exception::throw_internal(
                                                            ctx,
                                                            &format!(
                                                                "JS returned an array value, but \
                                                                 its length does not match the \
                                                                 result requirement, expected {} \
                                                                 results, got {}",
                                                                results.len(),
                                                                array.len()
                                                            ),
                                                        ))?
                                                    }

                                                    for (item, result) in array.iter().zip(results)
                                                    {
                                                        let item: WasmValueConverter =
                                                            WasmValueConverter::from_js(
                                                                ctx, item?,
                                                            )?;

                                                        if matches!(result, Val::F32(_))
                                                            && item.f64().is_some()
                                                        {
                                                            *result = item.f64().unwrap().into();
                                                        } else {
                                                            *result = *item;
                                                        }
                                                    }
                                                } else if let Some(ret) = results.first_mut() {
                                                    *ret = WasmValueConverter::from_js(ctx, res)?
                                                        .into();
                                                }
                                                Ok(())
                                            }
                                        })
                                        .unwrap();
                                }
                            }
                        } else {
                            return Err(Exception::throw_type(
                                &ctx,
                                &format!("{} is not an object", name),
                            ));
                        }
                    }
                }
            }

            linker
                .instantiate(store.as_context_mut(), module)
                .map_err(|x| {
                    Exception::throw_internal(&ctx, &format!("wasm instance creation error: {}", x))
                })?
        };

        Ok(Self { instance, store })
    }

    #[qjs(get, enumerable)]
    pub fn exports(&self, ctx: Ctx<'js>) -> Result<IndexMap<String, Value<'js>>> {
        let store = self.store.clone();
        let mut store = store.borrow_mut();
        let mut map = IndexMap::new();
        for (name, ext) in self
            .instance
            .exports(store.as_context_mut())
            .map(|x| (x.name().to_string(), x.into_extern()))
            .collect::<Vec<(String, Extern)>>()
        {
            let value = match ext.ty(store.as_context()) {
                wasmtime::ExternType::Func(func_type) => {
                    let func = self.get_func(store.as_context_mut(), &name);
                    if let Some(func) = func {
                        let func_len = func_type.params().len();
                        let func = rquickjs::Function::new(ctx.clone(), {
                            let store = self.store.clone();
                            move |ctx, Rest(args): Rest<Value<'js>>| {
                                let args: Vec<Val> = args
                                    .into_iter()
                                    .map(|x| WasmValueConverter::from_js(&ctx, x).map(|x| x.into()))
                                    .collect::<Result<Vec<_>>>()?;
                                let mut results: Vec<Val> = func_type
                                    .results()
                                    .map(|x| {
                                        match x {
                                            wasmtime::ValType::I32 => Ok(Val::I32(0)),
                                            wasmtime::ValType::I64 => Ok(Val::I64(0)),
                                            wasmtime::ValType::F32 => Ok(Val::F32(0)),
                                            wasmtime::ValType::F64 => Ok(Val::F64(0)),
                                            wasmtime::ValType::V128 => Ok(Val::V128(0_u128.into())),
                                            wasmtime::ValType::Ref(ref_type)
                                                if ref_type.matches(&RefType::FUNCREF) =>
                                            {
                                                Ok(Val::null_func_ref())
                                            }
                                            wasmtime::ValType::Ref(ref_type)
                                                if ref_type.matches(&RefType::EXTERNREF) =>
                                            {
                                                Ok(Val::null_extern_ref())
                                            }
                                            wasmtime::ValType::Ref(ref_type)
                                                if ref_type.matches(&RefType::ANYREF) =>
                                            {
                                                Ok(Val::null_any_ref())
                                            }
                                            _ => Err(ctx.throw("TODO".into_js(&ctx)?)),
                                        }
                                    })
                                    .collect::<Result<Vec<_>>>()?;
                                func.call(store.borrow_mut().as_context_mut(), &args, &mut results)
                                    .map_err(|x| {
                                        Exception::throw_internal(
                                            &ctx,
                                            &format!("failed to lock store: {}", x),
                                        )
                                    })?;
                                match results.len() {
                                    0 => Ok(Value::new_null(ctx.clone())),
                                    1 => WasmValueConverter::from(results[0]).into_js(&ctx),
                                    _ => {
                                        Ok(Array::from_iter_js(
                                            &ctx,
                                            results
                                                .into_iter()
                                                .map(|x| WasmValueConverter::from(x).into_js(&ctx)),
                                        )?
                                        .into_value())
                                    }
                                }
                            }
                        })?;
                        func.set_length(func_len)?;
                        func.set_name(name.clone())?;

                        func.into_value()
                    } else {
                        return Err(Exception::throw_internal(
                            &ctx,
                            &format!(
                                "wasm instance declared an exported function named {}, but it is \
                                 not actually exported",
                                name
                            ),
                        ));
                    }
                }
                wasmtime::ExternType::Global(ty) => {
                    let global = crate::global::Global::from(
                        if let Some(global) = self.get_global(store.as_context_mut(), &name) {
                            global
                        } else {
                            let val = match ty.content() {
                                wasmtime::ValType::I32 => Val::I32(0),
                                wasmtime::ValType::I64 => Val::I64(0),
                                wasmtime::ValType::F32 => Val::F32(0),
                                wasmtime::ValType::F64 => Val::F64(0),
                                wasmtime::ValType::V128 => Val::V128(0_u128.into()),
                                wasmtime::ValType::Ref(ref_type)
                                    if ref_type.matches(&RefType::FUNCREF) =>
                                {
                                    Val::null_func_ref()
                                }
                                wasmtime::ValType::Ref(ref_type)
                                    if ref_type.matches(&RefType::EXTERNREF) =>
                                {
                                    Val::null_extern_ref()
                                }
                                wasmtime::ValType::Ref(ref_type)
                                    if ref_type.matches(&RefType::ANYREF) =>
                                {
                                    Val::null_any_ref()
                                }
                                _ => return Err(ctx.throw("TODO".into_js(&ctx)?)),
                            };
                            wasmtime::Global::new(store.as_context_mut(), ty, val).map_err(|x| {
                                Exception::throw_internal(
                                    &ctx,
                                    &format!("wasm instance create global type error: {}", x),
                                )
                            })?
                        },
                    );
                    global.into_js(&ctx)?
                }
                wasmtime::ExternType::Table(ty) => {
                    let table = if let Some(table) = self.get_table(store.as_context_mut(), &name) {
                        crate::table::Table::from(table)
                    } else {
                        crate::table::Table::new(
                            TableDescriptor::builder()
                                .element(ty.element().heap_type().to_string())
                                .initial(ty.minimum())
                                .maximum(ty.maximum())
                                .build(),
                            Some(self.store.clone()).into(),
                            ctx.clone(),
                        )?
                    };
                    table.into_js(&ctx)?
                }
                wasmtime::ExternType::Memory(ty) => {
                    let memory =
                        if let Some(memory) = self.get_memory(store.as_context_mut(), &name) {
                            memory
                        } else {
                            wasmtime::Memory::new(store.as_context_mut(), ty).map_err(|x| {
                                Exception::throw_internal(
                                    &ctx,
                                    &format!("wasm instance create global type error: {}", x),
                                )
                            })?
                        };
                    crate::memory::Memory::from((memory, self.store.clone())).into_js(&ctx)?
                }
            };
            map.insert(name, value);
        }

        Ok(map)
    }
}
