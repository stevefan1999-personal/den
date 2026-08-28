//! `den:ffi` — calling C from JS.
//!
//! Off by default at compile time (`--features stdlib-ffi`), import-only (it
//! installs no global), and denied at run time without an [`FfiGrant`]. The
//! design, the safety envelope and the phase plan are in
//! `docs/research/19-den-ffi.md`; §5.1 is the list of things this module
//! deliberately cannot protect you from.
//!
//! The scalar vocabulary is complete — every integer width, `bool`, `f32`,
//! `f64`, `pointer` and `void`, as arguments, as results and as static
//! symbols — a `Uint8Array` reaches C as a borrowed `buffer` argument, and a
//! JS function reaches C as a `{ callback }` function pointer, callable from
//! den's own thread or from one of C's. A `nonblocking: true` symbol runs on a
//! worker and returns a Promise, which is also the only kind of symbol a
//! callback may be handed to (§4.7). Structs are still refused by name at
//! `open()`, with `FfiError { kind: "Schema" }`.

mod callback;
mod error;
mod grant;
mod library;
mod marshal;
mod pointer;
mod schema;

pub use crate::{error::FfiError, grant::FfiGrant};

#[rquickjs::module]
pub mod ffi {
    use rquickjs::{
        Class, Ctx, Function, Object, Result, Value,
        function::Opt,
        module::{Declarations, Exports},
    };

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

    /// Mint a C function pointer for `function`.
    ///
    /// The handle owns the trampoline: `def` is checked here, and the
    /// signature it declares is checked again against every slot the handle is
    /// passed to. An armed callback keeps den running until it is disposed,
    /// because C may call it at any time and from any thread.
    #[rquickjs::function]
    pub fn callback<'js>(
        ctx: Ctx<'js>, def: Value<'js>, function: Function<'js>,
    ) -> Result<Value<'js>> {
        super::callback::create(ctx, def, function)
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

    /// `ptr` and `suffix` are values, not functions, so the macro cannot
    /// derive their declarations from an item signature.
    #[qjs(declare)]
    pub fn declare(declare: &Declarations<'_>) -> Result<()> {
        declare.declare("ptr")?;
        declare.declare("suffix")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        super::error::install(ctx)?;
        super::callback::FfiRealm::install(ctx)?;
        exports.export("ptr", super::pointer::namespace(ctx)?)?;
        // The platform's shared-library extension, so a script can name one
        // library across three operating systems without a `switch`.
        exports.export("suffix", std::env::consts::DLL_SUFFIX)?;
        Ok(())
    }
}
