// `btoa`/`atob` have no implementation without a backend, and the resulting
// E0308 on the empty function body says nothing about the real cause.
#[cfg(not(any(feature = "base64", feature = "base64-simd")))]
compile_error!("den-stdlib-core requires one of the `base64` or `base64-simd` features");

use rquickjs::{Coerced, Ctx, Exception, Result};

pub use crate::cancellation::CancellationTokenWrapper;

#[rquickjs::function()]
pub fn btoa(value: Coerced<String>) -> Result<String> {
    #[cfg(feature = "base64-simd")]
    {
        use base64_simd::STANDARD;
        Ok(STANDARD.encode_to_string(value.as_bytes()))
    }
    #[cfg(all(feature = "base64", not(feature = "base64-simd")))]
    {
        use base64::prelude::*;

        Ok(BASE64_STANDARD.encode(value.as_bytes()))
    }
}

#[rquickjs::function()]
pub fn atob(ctx: Ctx<'_>, value: Coerced<String>) -> Result<String> {
    #[cfg(feature = "base64-simd")]
    {
        use base64_simd::STANDARD;
        match STANDARD.decode_to_vec(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(e) => Err(Exception::throw_internal(&ctx, &format!("{e}"))),
        }
    }
    #[cfg(all(feature = "base64", not(feature = "base64-simd")))]
    {
        use base64::prelude::*;
        match BASE64_STANDARD.decode(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(e) => Err(Exception::throw_internal(&ctx, &format!("{e}"))),
        }
    }
}

#[rquickjs::function()]
pub fn gc<'js>(ctx: Ctx<'js>) {
    ctx.run_gc();
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use rquickjs::{
        Ctx, Result,
        module::{Declarations, Exports},
    };

    pub use crate::cancellation::CancellationTokenWrapper;

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("atob")?.declare("btoa")?.declare("gc")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
        e.export("atob", super::js_atob)?
            .export("btoa", super::js_btoa)?
            .export("gc", super::js_gc)?;

        ctx.globals().set("atob", super::js_atob)?;
        ctx.globals().set("btoa", super::js_btoa)?;
        ctx.globals().set("gc", super::js_gc)?;
        Ok(())
    }
}

pub mod cancellation;
pub mod report;
