//! The one error den:ffi throws, and its `kind` discriminant.

use std::{fmt::Display, path::Path};

use rquickjs::{Class, Ctx, Error, JsLifetime, Object, class::Trace};

/// Which rule was broken. One sum type, one discriminant, per
/// `docs/research/19-den-ffi.md` §4.8 — no sentinel returns and no untyped
/// throws.
#[derive(Clone, Copy, Debug)]
pub enum ErrorKind {
    /// No grant, or a grant that does not cover the resolved path.
    NotCapable,
    /// `dlopen` refused the file.
    Open,
    /// `dlsym` has no such name.
    Symbol,
    /// The schema is malformed, or names something den:ffi cannot do.
    Schema,
    /// den's struct arithmetic and libffi's disagree about this target's ABI.
    Layout,
    /// A number argument does not fit the declared C type.
    Range,
    /// A well-typed call with an argument den refuses to marshal.
    BadArgument,
    /// The library was disposed, and the address is no longer mapped.
    Closed,
}

impl ErrorKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NotCapable => "NotCapable",
            Self::Open => "Open",
            Self::Symbol => "Symbol",
            Self::Schema => "Schema",
            Self::Layout => "Layout",
            Self::Range => "Range",
            Self::BadArgument => "BadArgument",
            Self::Closed => "Closed",
        }
    }

    /// Throw an [`FfiError`] carrying this kind.
    pub fn throw<M: Display>(self, ctx: &Ctx<'_>, message: M) -> Error {
        FfiError::new(self, message, None, None).raise(ctx)
    }

    /// Throw an [`FfiError`] whose `path` names the library it is about.
    pub fn throw_at<M: Display>(self, ctx: &Ctx<'_>, message: M, path: &Path) -> Error {
        let path = path.to_string_lossy().into_owned();
        FfiError::new(self, message, Some(path), None).raise(ctx)
    }

    /// Throw an [`FfiError`] whose `symbol` names the schema entry it is about.
    pub fn throw_for<M: Display>(self, ctx: &Ctx<'_>, message: M, symbol: &str) -> Error {
        FfiError::new(self, message, None, Some(symbol.into())).raise(ctx)
    }
}

/// The thrown value. A real class rather than a decorated `Error` so that
/// `instanceof` works and the `kind` is a property, not a parsed message —
/// `install` chains its prototype onto `Error.prototype`, the same way
/// `den:assert` does for `AssertionError`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "FfiError")]
pub struct FfiError {
    #[qjs(get, skip_trace)]
    kind:    &'static str,
    #[qjs(get, skip_trace)]
    message: String,
    #[qjs(get, skip_trace)]
    path:    Option<String>,
    #[qjs(get, skip_trace)]
    symbol:  Option<String>,
}

impl FfiError {
    fn new<M: Display>(
        kind: ErrorKind, message: M, path: Option<String>, symbol: Option<String>,
    ) -> Self {
        Self {
            kind: kind.as_str(),
            message: message.to_string(),
            path,
            symbol,
        }
    }

    fn raise(self, ctx: &Ctx<'_>) -> Error {
        match Class::instance(ctx.clone(), self) {
            Ok(instance) => ctx.throw(instance.into_value()),
            Err(error) => error,
        }
    }
}

#[rquickjs::methods]
impl FfiError {
    // rquickjs exports a class only if it declares a constructor, and a `()`
    // return makes `new FfiError()` throw: instances only come from den:ffi.
    #[qjs(constructor)]
    pub const fn new_js() {}

    #[qjs(get)]
    pub const fn name(&self) -> &'static str { "FfiError" }
}

/// Make `FfiError` an `Error`: QuickJS builds a native class' prototype from
/// `Object.prototype`, so the chain has to be spliced once, at module
/// evaluation.
pub fn install(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let Some(prototype) = Class::<FfiError>::prototype(ctx)? else {
        return Ok(());
    };
    let error: rquickjs::Constructor = ctx.globals().get("Error")?;
    let error_prototype: Object = error.get("prototype")?;
    prototype.set_prototype(Some(&error_prototype))?;
    Ok(())
}
