use std::{
    sync::{Arc, Mutex},
};

use rquickjs::{class::Trace, prelude::*, Ctx, Object};
use wasmtime::AsContextMut;

use crate::module::Module;
#[derive(Trace, Clone)]
#[rquickjs::class]
pub struct Instance {
    #[qjs(skip_trace)]
    instance: wasmtime::Instance,

    #[qjs(skip_trace)]
    store: Arc<Mutex<wasmtime::Store<()>>>,
}

#[rquickjs::methods]
impl Instance {
    #[qjs(constructor)]
    pub fn new<'js>(
        module: Module,
        _import_object: Opt<Object<'js>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Self> {
        let mut store = wasmtime::Store::new(module.engine(), ());
        let linker = wasmtime::Linker::new(module.engine());

        let instance = linker.instantiate(&mut store, &module).map_err(|x| {
            rquickjs::Exception::throw_message(&ctx, &format!("wasm module creation error: {}", x))
        })?;

        Ok(Self {
            instance,
            store: Arc::new(Mutex::new(store)),
        })
    }

    #[qjs(get, enumerable)]
    pub fn exports<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<()> {
        let store = self.store.clone();
        let mut store = store.try_lock().map_err(|x| {
            rquickjs::Exception::throw_message(&ctx, &format!("failed to lock store: {}", x))
        })?;
        let context_mut = store.as_context_mut();
        for (name, func) in self.instance.exports(context_mut).filter_map(|x| {
            let name = x.name();
            x.into_func().map(|func| (name, func))
        }) {
            println!("{:?} {:?}", name, func);
        }

        Ok(())
    }
}
