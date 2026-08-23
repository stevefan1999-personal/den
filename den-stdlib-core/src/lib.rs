// `btoa`/`atob` have no implementation without a backend, and the resulting
// E0308 on the empty function body says nothing about the real cause.
#[cfg(not(any(feature = "base64", feature = "base64-simd")))]
compile_error!("den-stdlib-core requires one of the `base64` or `base64-simd` features");

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use rquickjs::{Coerced, Ctx, Exception, Result, module::Exports};

    #[rquickjs::function]
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

    #[rquickjs::function]
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

    #[rquickjs::function]
    pub fn gc(ctx: Ctx<'_>) { ctx.run_gc(); }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        globals.set("atob", js_atob)?;
        globals.set("btoa", js_btoa)?;
        globals.set("gc", js_gc)?;
        Ok(())
    }
}

pub mod report;
