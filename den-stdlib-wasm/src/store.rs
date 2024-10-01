use std::{cell::RefCell, sync::Arc};

use derive_more::derive::{Deref, DerefMut, From};
use rquickjs::{class::Trace, Ctx, Result};

#[derive(Trace, Clone, From, Deref, DerefMut)]
#[rquickjs::class]
pub struct Store<'js> {
    #[qjs(skip_trace)]
    inner: Arc<RefCell<wasmtime::Store<Ctx<'js>>>>,
}

#[rquickjs::methods]
impl<'js> Store<'js> {
    #[qjs(constructor)]
    pub fn new(engine: crate::engine::Engine, ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(RefCell::new(wasmtime::Store::new(&engine.borrow(), ctx))),
        })
    }
}
