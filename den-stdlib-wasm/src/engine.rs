use derive_more::derive::{Deref, DerefMut, From};
use rquickjs::{class::Trace, JsLifetime};

#[derive(Trace, JsLifetime, Clone, Deref, DerefMut, From)]
#[rquickjs::class]
pub struct Engine {
    #[qjs(skip_trace)]
    // wasmtime engine itself is ref counted, so clone is available default, no need to wrap it in
    // Arc
    pub(crate) inner: wasmtime::Engine,
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
        Self {
            inner: wasmtime::Engine::new(&config).unwrap(),
        }
    }
}
