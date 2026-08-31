use rquickjs::{Class, Ctx, Exception, JsLifetime, Result, class::Trace};

#[derive(Clone, Copy)]
pub enum HttpErrorKind {
    Aborted,
    AddrInUse,
    Bind,
}

impl HttpErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aborted => "Aborted",
            Self::AddrInUse => "AddrInUse",
            Self::Bind => "Bind",
        }
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "HttpError")]
pub struct HttpError {
    #[qjs(skip_trace)]
    kind:    &'static str,
    #[qjs(skip_trace)]
    message: String,
}

impl HttpError {
    pub fn from_kind<M: Into<String>>(kind: HttpErrorKind, message: M) -> Self {
        Self {
            kind:    kind.as_str(),
            message: message.into(),
        }
    }

    pub fn throw<M: Into<String>>(
        ctx: &Ctx<'_>, kind: HttpErrorKind, message: M,
    ) -> rquickjs::Error {
        Class::instance(ctx.clone(), Self::from_kind(kind, message)).map_or_else(
            |_error| Exception::throw_internal(ctx, "could not create HttpError"),
            |error| ctx.throw(error.into_value()),
        )
    }

    pub fn instance<'js, M: Into<String>>(
        ctx: &Ctx<'js>, kind: HttpErrorKind, message: M,
    ) -> Result<Class<'js, Self>> {
        Class::instance(ctx.clone(), Self::from_kind(kind, message))
    }
}

#[rquickjs::methods]
impl HttpError {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get, enumerable)]
    pub const fn name(&self) -> &'static str { "HttpError" }

    #[qjs(get, enumerable)]
    pub const fn kind(&self) -> &'static str { self.kind }

    #[qjs(get, enumerable)]
    pub fn message(&self) -> String { self.message.clone() }
}
