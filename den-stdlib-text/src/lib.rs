use derive_more::{Debug, From, Into};
use either::Either;
use encoding_rs::{DecoderResult, Encoding};
use indexmap::{IndexMap, indexmap};
use rquickjs::{
    ArrayBuffer, Ctx, Exception, JsLifetime, Object, Result, TypedArray, class::Trace, prelude::*,
};

#[derive(Trace, JsLifetime, Clone, Debug, From, Into)]
#[rquickjs::class]
pub struct TextDecoder {
    #[qjs(skip_trace)]
    #[debug(ignore)]
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

                // `as_bytes` yields `None` once the buffer is detached, which JS can do at
                // any point before the call lands here.
                let buffer = match buffer {
                    Either::Left(ref buf) => buf.as_bytes(),
                    Either::Right(ref buf) => buf.as_bytes(),
                }
                .ok_or_else(|| Exception::throw_type(&ctx, "buffer is detached"))?;

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

#[derive(Trace, JsLifetime, Clone, Debug, From, Into)]
#[rquickjs::class]
pub struct TextEncoder {}

impl Default for TextEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEncoder {
    /// Length in bytes of the longest UTF-8 prefix of `src` that fits in
    /// `capacity`, never splitting a code point. https://encoding.spec.whatwg.org/#dom-textencoder-encodeinto
    /// requires `encodeInto` to truncate rather than overrun the destination.
    fn utf8_prefix_fitting(src: &str, capacity: usize) -> usize {
        src.char_indices()
            .map(|(offset, character)| offset + character.len_utf8())
            .take_while(|end| *end <= capacity)
            .last()
            .unwrap_or(0)
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
        ctx: Ctx<'js>,
    ) -> Result<IndexMap<&'static str, usize>> {
        // `as_raw` is the only mutable view rquickjs 0.12 offers — `as_bytes` hands
        // back a shared `&[u8]` — and it reports a detached buffer as `None`.
        let raw = dest
            .as_raw()
            .ok_or_else(|| Exception::throw_type(&ctx, "destination is detached"))?;

        let written = Self::utf8_prefix_fitting(&src, raw.len);

        // SAFETY: `raw` is QuickJS's own live allocation for this view. Nothing else
        // aliases it here — no JS runs between `as_raw` and the end of the
        // copy, so the buffer cannot be detached or resized underneath us.
        let dest = unsafe { core::slice::from_raw_parts_mut(raw.ptr.as_ptr(), raw.len) };
        dest[..written].copy_from_slice(&src.as_bytes()[..written]);

        // `read` is counted in UTF-16 code units, `written` in bytes.
        Ok(indexmap! {
            "read" => src[..written].chars().map(char::len_utf16).sum(),
            "written" => written
        })
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod text {
    use rquickjs::{Ctx, Result, class::JsClass, module::Exports};

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

#[cfg(test)]
mod tests {
    use super::TextEncoder;

    #[test]
    fn utf8_prefix_never_overruns_or_splits_a_code_point() {
        // "é" is two bytes, "€" is three: a capacity landing mid-character must not be
        // used.
        assert_eq!(TextEncoder::utf8_prefix_fitting("hello", 2), 2);
        assert_eq!(TextEncoder::utf8_prefix_fitting("hello", 99), 5);
        assert_eq!(TextEncoder::utf8_prefix_fitting("é", 1), 0);
        assert_eq!(TextEncoder::utf8_prefix_fitting("é", 2), 2);
        assert_eq!(TextEncoder::utf8_prefix_fitting("a€", 3), 1);
        assert_eq!(TextEncoder::utf8_prefix_fitting("a€", 4), 4);
        assert_eq!(TextEncoder::utf8_prefix_fitting("", 8), 0);
    }
}
