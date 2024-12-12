use std::{cell::RefCell, sync::Arc};

use derive_more::derive::{Deref, DerefMut, From};
use rquickjs::{class::Trace, Ctx, JsLifetime};
use wasmtime_wasi::{preview1::WasiP1Ctx, WasiCtxBuilder};

pub type StoreData<'js> = (WasiP1Ctx, Ctx<'js>);

#[derive(Trace, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store<'js> {
    #[qjs(skip_trace)]
    pub(crate) inner: Arc<RefCell<wasmtime::Store<StoreData<'js>>>>,
}

unsafe impl<'js> JsLifetime<'js> for Store<'js> {
    type Changed<'to> = Store<'to>;
}

#[rquickjs::methods]
impl<'js> Store<'js> {
    #[qjs(constructor)]
    pub fn new(engine: &crate::engine::Engine, ctx: Ctx<'js>) -> Self {
        let wasi_ctx = WasiCtxBuilder::new()
            .inherit_stdio()
            .inherit_env()
            .build_p1();

        let inner = wasmtime::Store::new(&engine, (wasi_ctx, ctx));
        Self {
            inner: Arc::new(RefCell::new(inner)),
        }
    }
}
