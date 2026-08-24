#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use rquickjs::{Coerced, Ctx, Exception, Result, module::Exports};

    #[rquickjs::function]
    pub fn btoa(Coerced(value): Coerced<String>) -> String {
        base64_simd::STANDARD.encode_to_string(value.as_bytes())
    }

    // The macro injects `Ctx` by value, and the body only borrows it; a
    // reference parameter is not an option at this boundary.
    #[rquickjs::function]
    pub fn atob(ctx: Ctx<'_>, Coerced(value): Coerced<String>) -> Result<String> {
        match base64_simd::STANDARD.decode_to_vec(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(error) => Err(Exception::throw_internal(&ctx, &format!("{error}"))),
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

pub mod exceptions;
