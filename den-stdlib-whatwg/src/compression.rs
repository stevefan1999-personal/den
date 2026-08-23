//! Streaming gzip / zlib / raw-deflate for CompressionStream.

use std::io::Write;

use flate2::{
    Compression,
    write::{DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder},
};
use rquickjs::{Ctx, Exception, JsLifetime, Result, TypedArray, class::Trace, function::Opt};

#[derive(Clone, Copy)]
enum Format {
    Gzip,
    Deflate,
    DeflateRaw,
}

impl Format {
    fn from_label(ctx: &Ctx<'_>, label: &str) -> Result<Self> {
        match label {
            "gzip" => Ok(Self::Gzip),
            "deflate" => Ok(Self::Deflate),
            "deflate-raw" => Ok(Self::DeflateRaw),
            _ => Err(Exception::throw_type(
                ctx,
                &format!("Unsupported compression format: '{label}'"),
            )),
        }
    }
}

enum Encoder {
    Gzip(GzEncoder<Vec<u8>>),
    Zlib(ZlibEncoder<Vec<u8>>),
    Raw(DeflateEncoder<Vec<u8>>),
}

enum Decoder {
    Gzip(GzDecoder<Vec<u8>>),
    Zlib(ZlibDecoder<Vec<u8>>),
    Raw(DeflateDecoder<Vec<u8>>),
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Compressor {
    #[qjs(skip_trace)]
    encoder: Encoder,
    emitted: usize,
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Decompressor {
    #[qjs(skip_trace)]
    decoder: Decoder,
    emitted: usize,
}

impl Compressor {
    fn take_delta(buffer: &[u8], emitted: &mut usize) -> Vec<u8> {
        let out = buffer[*emitted..].to_vec();
        *emitted = buffer.len();
        out
    }
}

#[rquickjs::methods]
impl Compressor {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>, format: String) -> Result<Self> {
        let level = Compression::default();
        let encoder = match Format::from_label(&ctx, &format)? {
            Format::Gzip => Encoder::Gzip(GzEncoder::new(Vec::new(), level)),
            Format::Deflate => Encoder::Zlib(ZlibEncoder::new(Vec::new(), level)),
            Format::DeflateRaw => Encoder::Raw(DeflateEncoder::new(Vec::new(), level)),
        };
        Ok(Self {
            encoder,
            emitted: 0,
        })
    }

    pub fn process<'js>(
        &mut self,
        ctx: Ctx<'js>,
        chunk: TypedArray<'js, u8>,
        flush: Opt<i32>,
    ) -> Result<TypedArray<'js, u8>> {
        let input = chunk
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(&ctx, "buffer is detached"))?;
        let finish = flush.0.unwrap_or(0) != 0;
        let out = match &mut self.encoder {
            Encoder::Gzip(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        GzEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder
                        .finish()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(&full, &mut self.emitted)
                } else {
                    encoder
                        .flush()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(encoder.get_ref(), &mut self.emitted)
                }
            }
            Encoder::Zlib(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        ZlibEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder
                        .finish()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(&full, &mut self.emitted)
                } else {
                    encoder
                        .flush()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(encoder.get_ref(), &mut self.emitted)
                }
            }
            Encoder::Raw(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        DeflateEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder
                        .finish()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(&full, &mut self.emitted)
                } else {
                    encoder
                        .flush()
                        .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                    Self::take_delta(encoder.get_ref(), &mut self.emitted)
                }
            }
        };
        TypedArray::new_copy(ctx, out)
    }
}

impl Decompressor {
    fn take_delta(buffer: &[u8], emitted: &mut usize) -> Vec<u8> {
        let out = buffer[*emitted..].to_vec();
        *emitted = buffer.len();
        out
    }
}

#[rquickjs::methods]
impl Decompressor {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>, format: String) -> Result<Self> {
        let decoder = match Format::from_label(&ctx, &format)? {
            Format::Gzip => Decoder::Gzip(GzDecoder::new(Vec::new())),
            Format::Deflate => Decoder::Zlib(ZlibDecoder::new(Vec::new())),
            Format::DeflateRaw => Decoder::Raw(DeflateDecoder::new(Vec::new())),
        };
        Ok(Self {
            decoder,
            emitted: 0,
        })
    }

    pub fn process<'js>(
        &mut self,
        ctx: Ctx<'js>,
        chunk: TypedArray<'js, u8>,
    ) -> Result<TypedArray<'js, u8>> {
        let input = chunk
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(&ctx, "buffer is detached"))?;
        let out = match &mut self.decoder {
            Decoder::Gzip(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                Self::take_delta(decoder.get_ref(), &mut self.emitted)
            }
            Decoder::Zlib(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                Self::take_delta(decoder.get_ref(), &mut self.emitted)
            }
            Decoder::Raw(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| Exception::throw_internal(&ctx, &format!("{err}")))?;
                Self::take_delta(decoder.get_ref(), &mut self.emitted)
            }
        };
        TypedArray::new_copy(ctx, out)
    }
}
