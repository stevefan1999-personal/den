#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use base64_simd::STANDARD;
    use rquickjs::{module::Exports, Coerced, Ctx};

    pub use crate::cancellation::CancellationTokenWrapper;

    #[rquickjs::function()]
    pub fn btoa(value: Coerced<String>) -> rquickjs::Result<String> {
        Ok(STANDARD.encode_to_string(value.as_bytes()))
    }

    #[rquickjs::function()]
    pub fn atob<'js>(ctx: Ctx<'js>, value: Coerced<String>) -> rquickjs::Result<String> {
        match STANDARD.decode_to_vec(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(e) => Err(rquickjs::Exception::throw_internal(&ctx, &format!("{e}"))),
        }
    }

    #[qjs(declare)]
    pub fn declare(declare: &rquickjs::module::Declarations) -> rquickjs::Result<()> {
        declare.declare("atob")?;
        declare.declare("btoa")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> rquickjs::Result<()> {
        ctx.globals().set("atob", js_atob)?;
        ctx.globals().set("btoa", js_btoa)?;

        Ok(())
    }
}

pub mod cancellation;
