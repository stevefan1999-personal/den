use std::fmt::Write;

use derivative::Derivative;
use derive_more::{From, Into};
use rquickjs::{class::Trace, Ctx, TypedArray};

#[derive(Trace, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "TextEncoder")]
pub struct TextEncoder {}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextEncoder {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {}
    }

    #[qjs(get, enumerable)]
    pub fn encoding(&self) -> &'static str {
        "utf-8".into()
    }

    pub fn encode<'js>(&self, src: String, ctx: Ctx<'js>) -> rquickjs::Result<TypedArray<'js, u8>> {
        TypedArray::new_copy(ctx, src)
    }

    pub fn encode_into<'js>(
        &self,
        src: String,
        dest: TypedArray<'js, u8>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<()> {
        todo!()
    }
}

#[rquickjs::module]
pub mod text {
    use rquickjs::{module::Exports, Ctx};

    pub use super::TextEncoder;

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &mut Exports<'js>) -> rquickjs::Result<()> {
        for (k, v) in exports.iter() {
            ctx.globals().set(k.to_str()?, v)?;
        }

        Ok(())
    }
}
