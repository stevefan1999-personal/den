use den_util::BufferSource;
use encoding_rs::{DecoderResult, Encoding};
use rquickjs::{Ctx, Exception, JsLifetime, Object, Result, TypedArray, class::Trace, prelude::*};

pub use crate::js_text_module as js_text;

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct TextDecoder {
    #[qjs(skip_trace)]
    encoding: &'static Encoding,

    #[qjs(get, enumerable)]
    fatal:      bool,
    #[qjs(get, enumerable, rename = "ignoreBOM")]
    ignore_bom: bool,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextDecoder {
    #[qjs(constructor)]
    pub fn new<'js>(label: Opt<String>, opts: Opt<Object<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let label = label.0.unwrap_or_else(|| "utf-8".to_owned());

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
    pub fn encoding(&self) -> String { self.encoding.name().to_ascii_lowercase() }

    pub fn decode(&self, buffer: Option<BufferSource>, ctx: Ctx<'_>) -> Result<String> {
        match buffer {
            Some(buffer) => {
                let mut decoder = if self.ignore_bom {
                    self.encoding.new_decoder_without_bom_handling()
                } else {
                    self.encoding.new_decoder()
                };

                let buffer = buffer.bytes();

                let len = if self.fatal {
                    decoder.max_utf8_buffer_length_without_replacement(buffer.len())
                } else {
                    decoder.max_utf8_buffer_length(buffer.len())
                };

                let mut output = len.map_or_else(String::new, String::with_capacity);
                if self.fatal {
                    let (res, _) =
                        decoder.decode_to_string_without_replacement(buffer, &mut output, true);
                    if let DecoderResult::Malformed(_, _) = res {
                        Err(Exception::throw_type(
                            &ctx,
                            "invalid decoding encountered and no replacements allowed",
                        ))
                    } else {
                        Ok(output)
                    }
                } else {
                    let _ = decoder.decode_to_string(buffer, &mut output, true);
                    Ok(output)
                }
            }
            None => Ok(String::new()),
        }
    }
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct TextEncoder {}

impl Default for TextEncoder {
    fn default() -> Self { Self::new() }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl TextEncoder {
    #[qjs(constructor)]
    pub const fn new() -> Self { Self {} }

    #[qjs(get, enumerable)]
    pub const fn encoding(&self) -> &'static str { "utf-8" }

    pub fn encode<'js>(&self, src: String, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        TypedArray::new_copy(ctx, src)
    }

    pub fn encode_into<'js>(
        &self, src: String, dest: TypedArray<'js, u8>, ctx: Ctx<'js>,
    ) -> Result<Object<'js>> {
        // `as_raw` is the only mutable view rquickjs 0.12 offers — `as_bytes` hands
        // back a shared `&[u8]` — and it reports a detached buffer as `None`.
        let raw = dest
            .as_raw()
            .ok_or_else(|| Exception::throw_type(&ctx, "destination is detached"))?;

        let written = src.floor_char_boundary(raw.len);

        // SAFETY: `raw` is QuickJS's own live allocation for this view. Nothing else
        // aliases it here — no JS runs between `as_raw` and the end of the
        // copy, so the buffer cannot be detached or resized underneath us.
        let dest = unsafe { core::slice::from_raw_parts_mut(raw.ptr.as_ptr(), raw.len) };
        let encoded = src
            .get(..written)
            .ok_or_else(|| Exception::throw_internal(&ctx, "invalid UTF-8 boundary"))?;
        dest.get_mut(..written)
            .ok_or_else(|| Exception::throw_internal(&ctx, "destination is too short"))?
            .copy_from_slice(encoded.as_bytes());

        // `read` is counted in UTF-16 code units, `written` in bytes.
        let result = Object::new(ctx)?;
        result.set("read", encoded.chars().map(char::len_utf16).sum::<usize>())?;
        result.set("written", written)?;
        Ok(result)
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod text_module {
    use rquickjs::{Ctx, Result, class::JsClass as _, module::Exports};

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
