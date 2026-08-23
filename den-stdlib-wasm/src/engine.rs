//! The compilation engine of a JS context.

use rquickjs::{Ctx, Exception, JsLifetime, Result};

use crate::backend;

/// Kept in the context userdata, separately from [`crate::store::Store`].
///
/// Compiling and validating need an engine but no store, so keeping the two
/// apart means `WebAssembly.compile` cannot be refused just because wasm is
/// currently running and holding the store's borrow.
#[derive(Clone, JsLifetime)]
pub struct Engine {
    inner: backend::Engine,
}

impl Engine {
    pub fn new() -> core::result::Result<Self, backend::Error> {
        backend::new_engine().map(|inner| Self { inner })
    }

    /// The engine `den:wasm` installed in this context.
    pub fn from_ctx(ctx: &Ctx<'_>) -> Result<backend::Engine> {
        ctx.userdata::<Self>()
            .map(|engine| engine.inner.clone())
            .ok_or_else(|| {
                Exception::throw_internal(
                    ctx,
                    "the WebAssembly engine is missing from this context",
                )
            })
    }
}
