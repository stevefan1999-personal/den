use std::cell::RefCell;

use derive_more::derive::{Deref, DerefMut, From, Into};
use rquickjs::class::Trace;

#[derive(Trace, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Engine {
    #[qjs(skip_trace)]
    inner: RefCell<wasmtime::Engine>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[rquickjs::methods]
impl Engine {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(wasmtime::Engine::default()),
        }
    }
}
