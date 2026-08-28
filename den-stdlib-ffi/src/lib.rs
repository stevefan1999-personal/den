//! `den:ffi` — calling C from JS.
//!
//! Off by default at compile time (`--features stdlib-ffi`), import-only (it
//! installs no global), and denied at run time without an [`FfiGrant`]. The
//! design, the safety envelope and the phase plan are in
//! `docs/research/19-den-ffi.md`; §5.1 is the list of things this module
//! deliberately cannot protect you from.
//!
//! Phase 1 marshals `i32`, `f64` and `void`, synchronously. Everything else in
//! the published schema — 64-bit integers, buffers, pointers, structs,
//! callbacks, `nonblocking` — throws `FfiError { kind: "Schema" }` at
//! `open()`, naming what is missing.

mod error;
mod grant;
mod library;
mod schema;

pub use crate::{error::FfiError, grant::FfiGrant};

#[rquickjs::module]
pub mod ffi {
    use rquickjs::{Class, Ctx, Object, Result, Value, function::Opt, module::Exports};

    #[expect(
        clippy::module_name_repetitions,
        reason = "the published names in `types/den-ffi.d.ts`"
    )]
    pub use super::{FfiError, FfiGrant};

    /// Load a shared library and bind every symbol the schema names.
    ///
    /// `grant` is the capability, not a flag: a realm that was never granted
    /// FFI has nothing to pass here, and a module that is not handed a grant
    /// cannot bind a symbol.
    #[rquickjs::function]
    pub fn open<'js>(
        ctx: Ctx<'js>, path: String, schema: Object<'js>, grant: Opt<Value<'js>>,
    ) -> Result<Object<'js>> {
        super::library::open(ctx, path, schema, grant.0)
    }

    /// This realm's FFI grant, or `null` when it has none.
    ///
    /// The composition root — `den --allow-ffi[=PATH]` — is what puts one in
    /// userdata. There is no way to mint one from JS.
    #[rquickjs::function]
    pub fn grant(ctx: Ctx<'_>) -> Result<Value<'_>> {
        // `null`, not `undefined`: "this realm has no grant" is an answer, not
        // a missing one.
        let Some(granted) = ctx.userdata::<FfiGrant>() else {
            return Ok(Value::new_null(ctx.clone()));
        };
        Ok(Class::instance(ctx.clone(), granted.clone())?.into_value())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        super::error::install(ctx)
    }
}
