use std::sync::{Arc, Mutex};

use derive_more::derive::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, Result, Ctx};

#[derive(Trace, Clone, From, Into, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store<'js> {
    #[qjs(skip_trace)]
    inner: Arc<Mutex<wasmtime::Store<Ctx<'js>>>>,
}

#[rquickjs::methods]
impl<'js> Store<'js> {
    #[qjs(constructor)]
    pub fn new(engine: crate::engine::Engine, ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(wasmtime::Store::new(&engine, ctx))),
        })
    }
}
