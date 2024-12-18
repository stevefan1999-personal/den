use derivative::Derivative;
use derive_more::{From, Into};
use either::Either;
use encoding_rs::{DecoderResult, Encoding};
use indexmap::{indexmap, IndexMap};
use rquickjs::{
    class::Trace, prelude::*, ArrayBuffer, Ctx, Exception, JsLifetime, Object, Result, TypedArray,
};

#[derive(Trace, JsLifetime, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class]
pub struct TextDecoder {
    #[qjs(skip_trace)]
    #[derivative(Debug = "ignore")]
    encoding: &'static Encoding,

    fatal:      bool,
    ignore_bom: bool,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextDecoder {
    #[qjs(constructor)]
    pub fn new<'js>(label: Opt<String>, opts: Opt<Object<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let label = label.0.unwrap_or("utf-8".to_string());

        let encoding = Encoding::for_label(label.as_bytes())
            .ok_or_else(|| Exception::throw_range(&ctx, &format!("unknown encoding {label}")))?;

        let (mut fatal, mut ignore_bom) = (false, false);

        if let Some(opts) = opts.0 {
            fatal = opts.get::<_, bool>("fatal").unwrap_or(false);
            ignore_bom = opts.get::<_, bool>("ignoreBOM").unwrap_or(false);
        }

        Ok(Self {
            encoding,
            fatal,
            ignore_bom,
        })
    }

    #[qjs(get, enumerable)]
    pub fn encoding(&self) -> String {
        self.encoding.name().to_ascii_lowercase()
    }

    #[qjs(get, enumerable)]
    pub fn fatal(&self) -> bool {
        self.fatal
    }

    #[qjs(get, enumerable, rename = "ignoreBOM")]
    pub fn ignore_bom(&self) -> bool {
        self.ignore_bom
    }

    pub fn decode<'js>(
        &self,
        buffer: Option<Either<TypedArray<'js, u8>, ArrayBuffer<'js>>>,
        ctx: Ctx<'js>,
    ) -> Result<String> {
        match buffer {
            Some(buffer) => {
                let mut decoder = if self.ignore_bom {
                    self.encoding.new_decoder_without_bom_handling()
                } else {
                    self.encoding.new_decoder()
                };

                let buffer = match buffer {
                    Either::Left(ref buf) => buf.as_bytes(),
                    Either::Right(ref buf) => buf.as_bytes(),
                }
                .unwrap();

                let len = if self.fatal {
                    decoder.max_utf8_buffer_length_without_replacement(buffer.len())
                } else {
                    decoder.max_utf8_buffer_length(buffer.len())
                };

                let mut decoded = len.map(String::with_capacity).unwrap_or_else(String::new);
                if self.fatal {
                    let (res, _) =
                        decoder.decode_to_string_without_replacement(buffer, &mut decoded, true);
                    if let DecoderResult::Malformed(_, _) = res {
                        Err(Exception::throw_type(
                            &ctx,
                            "invalid decoding encountered and no replacements allowed",
                        ))
                    } else {
                        Ok(decoded)
                    }
                } else {
                    let _ = decoder.decode_to_string(buffer, &mut decoded, true);
                    Ok(decoded)
                }
            }
            None => Ok(String::new()),
        }
    }
}

#[derive(Trace, JsLifetime, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class]
pub struct TextEncoder {}

impl Default for TextEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextEncoder {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {}
    }

    #[qjs(get, enumerable)]
    pub fn encoding(&self) -> &'static str {
        "utf-8"
    }

    pub fn encode<'js>(&self, src: String, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        TypedArray::new_copy(ctx, src)
    }

    pub fn encode_into<'js>(
        &self,
        src: String,
        dest: TypedArray<'js, u8>,
    ) -> IndexMap<&str, usize> {
        let dest = dest.as_bytes().unwrap();
        let dest = unsafe { core::slice::from_raw_parts_mut(dest.as_ptr() as *mut u8, dest.len()) };
        let (result, _, _) = encoding_rs::UTF_8.encode(&src);
        dest[..result.len()].copy_from_slice(&result);
        indexmap! {
            "read" => result.len(),
            "written" => result.len()
        }
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod text {
    use rquickjs::{class::JsClass, module::Exports, Ctx, Result};

    pub use super::{TextDecoder, TextEncoder};

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _exports: &Exports<'js>) -> Result<()> {
        ctx.globals()
            .set("TextDecoder", TextDecoder::constructor(ctx))?;
        ctx.globals()
            .set("TextEncoder", TextEncoder::constructor(ctx))?;

        Ok(())
    }
}
