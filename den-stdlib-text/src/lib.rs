use derivative::Derivative;
use derive_more::{From, Into};
use encoding_rs::Encoding;
use rquickjs::{class::Trace, Ctx, Object, TypedArray};

#[derive(Trace, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class]
pub struct TextDecoder {
    #[qjs(skip_trace)]
    #[derivative(Debug = "ignore")]
    encoding: &'static Encoding,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextDecoder {
    #[qjs(constructor)]
    pub fn new<'js>(label: Option<String>, ctx: Ctx<'js>) -> rquickjs::Result<Self> {
        let label = label.unwrap_or("utf-8".to_string());

        let encoding = Encoding::for_label(label.as_bytes()).ok_or_else(|| {
            rquickjs::Exception::throw_range(&ctx, &format!("unknown encoding {label}"))
        })?;
        Ok(Self { encoding })
    }

    #[qjs(get, enumerable)]
    pub fn encoding(&self) -> String {
        self.encoding.name().to_ascii_lowercase()
    }

    pub fn decode<'js>(&self, buffer: Option<Object<'js>>) -> rquickjs::Result<String> {
        match buffer {
            Some(buffer) => {
                let as_typed_array = buffer.as_typed_array::<u8>().and_then(|x| x.as_bytes());
                let as_array_buffer = buffer.as_array_buffer().and_then(|x| x.as_bytes());

                if let Some(buffer) = as_typed_array.or(as_array_buffer) {
                    let mut decoder = self.encoding.new_decoder();
                    let mut decoded = String::with_capacity(
                        decoder.max_utf8_buffer_length(buffer.len()).unwrap_or(0),
                    );
                    let _ = decoder.decode_to_string(buffer, &mut decoded, true);
                    Ok(decoded)
                } else {
                    todo!()
                }
            }
            None => Ok(String::new()),
        }
    }
}

#[derive(Trace, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class]
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

    pub use super::{TextDecoder, TextEncoder};

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &mut Exports<'js>) -> rquickjs::Result<()> {
        for (k, v) in exports.iter() {
            ctx.globals().set(k.to_str()?, v)?;
        }

        Ok(())
    }
}
