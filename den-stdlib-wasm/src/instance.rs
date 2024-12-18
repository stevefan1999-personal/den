use std::sync::Mutex;

use derive_more::derive::{Deref, DerefMut, From, Into};
use indexmap::IndexMap;
use rquickjs::{
    class::Trace, function::Args, prelude::*, Array, Ctx, Exception, Function, JsLifetime,
    Persistent, Result, Value,
};
use tracing::warn;
use wasmtime::{AsContext, AsContextMut, Extern, Val};

use crate::{
    error::LinkError,
    module::Module,
    store::StoreData,
    table::TableDescriptor,
    utils::{get_default_value_for_val_type, WasmValueConverter},
};

#[derive(Trace, JsLifetime, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Instance {
    #[qjs(skip_trace)]
    instance: wasmtime::Instance,
}

impl Instance {
    fn resolve_imports<'js, S: AsContextMut + AsContext<Data = StoreData<'js>>>(
        module: &Module,
        import_object: IndexMap<String, IndexMap<String, Value<'js>>>,
        mut store: S,
        linker: &mut wasmtime::Linker<StoreData<'js>>,
        ctx: &Ctx<'js>,
    ) -> Result<()> {
        // https://webassembly.github.io/spec/js-api/#read-the-imports
        for module_import in module.imports() {
            let module = module_import.module();
            let name = module_import.name();
            if let Some(o) = import_object.get(module) {
                match o.get(name) {
                    None => {
                        return Err(Exception::throw_type(
                            ctx,
                            &format!("{} is not an object", name),
                        ));
                    }
                    Some(v) => {
                        let define_result = if let wasmtime::ExternType::Func(ty) =
                            module_import.ty()
                        {
                            if !v.is_function() {
                                None
                            } else {
                                #[derive(Clone, Copy, From, Deref, DerefMut)]
                                struct DangerouslyImplementSync<T>(T);
                                unsafe impl<T> Send for DangerouslyImplementSync<T> {}
                                unsafe impl<T> Sync for DangerouslyImplementSync<T> {}

                                let js_func: Persistent<Function> = v.get()?;
                                let js_func = Mutex::new(DangerouslyImplementSync(js_func));

                                // We know that at the time of getting this call back,
                                // the context should still have had been alive
                                // For some reason a Mutex is needed to make it Send
                                // safe

                                let wasm_func = linker.func_new(
                                    module,
                                    name,
                                    ty,
                                    move |caller, params, results| {
                                        let (_, ctx) = caller.data();
                                        let func =
                                            js_func.lock().unwrap().0.clone().restore(ctx)?;

                                        let mut args = Args::new(ctx.clone(), params.len());
                                        args.push_args(
                                            params.iter().map(|x| WasmValueConverter::from(*x)),
                                        )?;
                                        let res: Value = func.call_arg(args)?;
                                        if let Some(array) = res.as_array() {
                                            if array.len() != results.len() {
                                                Err(Exception::throw_internal(
                                                    ctx,
                                                    &format!(
                                                        "JS returned an array value, but its \
                                                         length does not match the result \
                                                         requirement, expected {} results, got {}",
                                                        results.len(),
                                                        array.len()
                                                    ),
                                                ))?
                                            }

                                            for (item, result) in array.iter().zip(results) {
                                                let item: WasmValueConverter =
                                                    WasmValueConverter::from_js(ctx, item?)?;

                                                if matches!(result, Val::F32(_))
                                                    && item.f64().is_some()
                                                {
                                                    *result = item.f64().unwrap().into();
                                                } else {
                                                    *result = *item;
                                                }
                                            }
                                        } else if let Some(ret) = results.first_mut() {
                                            *ret = WasmValueConverter::from_js(ctx, res)?.into();
                                        }
                                        Ok(())
                                    },
                                );
                                Some(wasm_func)
                            }
                        } else {
                            let external: Option<Extern> = match module_import.ty() {
                                wasmtime::ExternType::Global(ty)
                                    if ty.content().is_i64() && v.is_number() =>
                                {
                                    None
                                }
                                wasmtime::ExternType::Global(ty)
                                    if !ty.content().is_i64() && v.as_big_int().is_some() =>
                                {
                                    None
                                }
                                wasmtime::ExternType::Global(ty) if ty.content().is_v128() => None,
                                wasmtime::ExternType::Global(ty) => {
                                    let item = crate::global::Global::from_js(ctx, v.clone())
                                        .or_else(|_| {
                                            crate::global::Global::from_type(
                                                ty,
                                                v,
                                                store.as_context_mut(),
                                                ctx,
                                            )
                                        })?;
                                    Some(item.inner.into())
                                }
                                wasmtime::ExternType::Table(ty) => {
                                    if let Ok(table) = crate::table::Table::from_js(ctx, v.clone())
                                    {
                                        // let actual_ty = table.inner.ty(store.as_context());
                                        // if actual_ty != ty {
                                        //     warn!(?ty, ?actual_ty, "table is different");
                                        // }

                                        Some(table.inner.into())
                                    } else {
                                        None
                                    }
                                }
                                wasmtime::ExternType::Memory(ty) => {
                                    if let Ok(memory) =
                                        crate::memory::Memory::from_js(ctx, v.clone())
                                    {
                                        let actual_ty = memory.inner.ty(store.as_context());
                                        if actual_ty != ty {
                                            warn!(?ty, ?actual_ty, "memory is different");
                                        }
                                        Some(memory.inner.into())
                                    } else {
                                        None
                                    }
                                }
                                _ => unreachable!(),
                            };

                            external
                                .map(|value| linker.define(store.as_context(), module, name, value))
                        };

                        match define_result {
                            Some(Ok(_)) => {}
                            Some(Err(x)) => {
                                return Err(Exception::throw_internal(
                                    ctx,
                                    &format!("wasm linker define error: {}", x),
                                ))
                            }
                            None => return Err(ctx.throw(LinkError::new().into_js(ctx)?)),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn make_linker<'js, S: AsContextMut + AsContext<Data = StoreData<'js>>>(
        module: &Module,
        Opt(import_object): Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
        store: S,
        ctx: &Ctx<'js>,
    ) -> Result<wasmtime::Linker<StoreData<'js>>> {
        let mut linker = wasmtime::Linker::new(module.engine());

        if module.imports().len() > 0 {
            let import_object = import_object
                .ok_or_else(|| Exception::throw_internal(ctx, "import object is not an object"))?;
            Self::resolve_imports(module, import_object, store, &mut linker, ctx)?;
        }

        Ok(linker)
    }
}

#[rquickjs::methods]
impl Instance {
    #[qjs(constructor)]
    pub fn new<'js>(
        module: &Module,
        import_object: Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let store = ctx.userdata::<crate::store::Store>().unwrap();
        let mut linker =
            Instance::make_linker(module, import_object, &mut *store.borrow_mut(), &ctx)?;
        wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |(wasi_ctx, _)| wasi_ctx).unwrap();
        let instance = linker
            .instantiate(store.borrow_mut().as_context_mut(), module)
            .map_err(|x| {
                Exception::throw_internal(&ctx, &format!("wasm instance creation error: {}", x))
            })?;
        Ok(Instance { instance })
    }

    #[qjs(get, enumerable)]
    pub fn exports<'js>(&self, ctx: Ctx<'js>) -> Result<IndexMap<String, Value<'js>>> {
        let store_ = ctx.userdata::<crate::store::Store>().unwrap();

        let mut map = IndexMap::new();
        let store = &mut *store_.borrow_mut();
        for (name, ext) in self
            .instance
            .exports(&mut *store)
            .map(|x| (x.name().to_string(), x.into_extern()))
            .collect::<Vec<(String, Extern)>>()
        {
            let value = match ext.ty(&mut *store) {
                wasmtime::ExternType::Func(func_type) => {
                    let func = self.get_func(&mut *store, &name);
                    if let Some(func) = func {
                        let func_len = func_type.params().len();
                        let func = rquickjs::Function::new(ctx.clone(), {
                            let store = store_.clone();
                            move |ctx, Rest(args): Rest<Value<'js>>| {
                                let args: Vec<Val> = args
                                    .into_iter()
                                    .map(|x| WasmValueConverter::from_js(&ctx, x).map(|x| x.into()))
                                    .collect::<Result<Vec<_>>>()?;
                                let mut results: Vec<Val> = func_type
                                    .results()
                                    .map(|ref x| {
                                        match get_default_value_for_val_type(x) {
                                            Ok(x) => Ok(x),
                                            Err(_) => Err(ctx.throw("TODO".into_js(&ctx)?)),
                                        }
                                    })
                                    .collect::<Result<Vec<_>>>()?;
                                func.call(&mut *store.borrow_mut(), &args, &mut results)
                                    .map_err(|x| {
                                        Exception::throw_internal(
                                            &ctx,
                                            &format!("failed to lock store: {}", x),
                                        )
                                    })?;
                                match results.len() {
                                    0 => Ok(Value::new_null(ctx)),
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
                    let global = if let Some(global) = self.get_global(&mut *store, &name) {
                        Ok(global)
                    } else {
                        let val = match get_default_value_for_val_type(ty.content()) {
                            Ok(x) => Ok(x),
                            Err(_) => Err(ctx.throw("TODO".into_js(&ctx)?)),
                        }?;
                        wasmtime::Global::new(&mut *store, ty, val)
                    }
                    .map_err(|x| {
                        Exception::throw_internal(
                            &ctx,
                            &format!("wasm instance create global type error: {}", x),
                        )
                    })?;
                    let global = crate::global::Global::from(global);
                    global.into_js(&ctx)?
                }
                wasmtime::ExternType::Table(ty) => {
                    let table = if let Some(table) = self.get_table(&mut *store, &name) {
                        crate::table::Table::from(table)
                    } else {
                        crate::table::Table::new_inner(
                            TableDescriptor::builder()
                                .element(ty.element().heap_type().to_string())
                                .initial(ty.minimum() as u32)
                                .maximum(ty.maximum().map(|x| x as u32))
                                .build(),
                            &mut *store,
                            &ctx,
                        )?
                    };
                    table.into_js(&ctx)?
                }
                wasmtime::ExternType::Memory(ty) => {
                    let memory = if let Some(memory) = self.get_memory(&mut *store, &name) {
                        Ok(memory)
                    } else {
                        wasmtime::Memory::new(&mut *store, ty)
                    }
                    .map_err(|x| {
                        Exception::throw_internal(
                            &ctx,
                            &format!("wasm instance create global type error: {}", x),
                        )
                    })?;
                    crate::memory::Memory::from(memory).into_js(&ctx)?
                }
            };
            map.insert(name, value);
        }

        Ok(map)
    }
}
