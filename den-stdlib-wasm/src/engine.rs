use std::cell::RefCell;

use derive_more::derive::{Deref, DerefMut, From, Into};
use rquickjs::class::Trace;

#[derive(Trace, Clone, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Engine {
    #[qjs(skip_trace)]
    // wasmtime engine itself is ref counted, so clone is available default, no need to wrap it in
    // Arc
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
        let mut config = wasmtime::Config::new();
        // config.async_support(true);
        let inner = wasmtime::Engine::new(&config).unwrap();
        Self {
            inner: RefCell::new(inner),
        }
    }
}
