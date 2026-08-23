//! CompressionStream / DecompressionStream wrapping flate2.

use std::{cell::RefCell, io::Write, rc::Rc};

use flate2::{
    Compression,
    write::{DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder},
};
use rquickjs::{
    Class, Ctx, Function, JsLifetime, Object, Result, TypedArray, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

use crate::{
    host::Host,
    streams::{ReadableStream, TransformStream, WritableStream},
};

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
            _ => Err(Host::throw_type(
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

pub struct Compressor {
    encoder: Encoder,
    emitted: usize,
}

pub struct Decompressor {
    decoder: Decoder,
    emitted: usize,
}

impl Compressor {
    fn take_delta(buffer: &[u8], emitted: &mut usize) -> Vec<u8> {
        let out = buffer[*emitted..].to_vec();
        *emitted = buffer.len();
        out
    }

    pub fn new(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let level = Compression::default();
        let encoder = match Format::from_label(ctx, format)? {
            Format::Gzip => Encoder::Gzip(GzEncoder::new(Vec::new(), level)),
            Format::Deflate => Encoder::Zlib(ZlibEncoder::new(Vec::new(), level)),
            Format::DeflateRaw => Encoder::Raw(DeflateEncoder::new(Vec::new(), level)),
        };
        Ok(Self {
            encoder,
            emitted: 0,
        })
    }

    pub fn process(&mut self, ctx: &Ctx<'_>, input: &[u8], finish: bool) -> Result<Vec<u8>> {
        match &mut self.encoder {
            Encoder::Gzip(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        GzEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder.finish().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(&full, &mut self.emitted))
                } else {
                    encoder.flush().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(encoder.get_ref(), &mut self.emitted))
                }
            }
            Encoder::Zlib(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        ZlibEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder.finish().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(&full, &mut self.emitted))
                } else {
                    encoder.flush().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(encoder.get_ref(), &mut self.emitted))
                }
            }
            Encoder::Raw(encoder) => {
                encoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                if finish {
                    let encoder = std::mem::replace(
                        encoder,
                        DeflateEncoder::new(Vec::new(), Compression::default()),
                    );
                    let full = encoder.finish().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(&full, &mut self.emitted))
                } else {
                    encoder.flush().map_err(|err| {
                        rquickjs::Exception::throw_internal(ctx, &format!("{err}"))
                    })?;
                    Ok(Self::take_delta(encoder.get_ref(), &mut self.emitted))
                }
            }
        }
    }
}

impl Decompressor {
    fn take_delta(buffer: &[u8], emitted: &mut usize) -> Vec<u8> {
        let out = buffer[*emitted..].to_vec();
        *emitted = buffer.len();
        out
    }

    pub fn new(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let decoder = match Format::from_label(ctx, format)? {
            Format::Gzip => Decoder::Gzip(GzDecoder::new(Vec::new())),
            Format::Deflate => Decoder::Zlib(ZlibDecoder::new(Vec::new())),
            Format::DeflateRaw => Decoder::Raw(DeflateDecoder::new(Vec::new())),
        };
        Ok(Self {
            decoder,
            emitted: 0,
        })
    }

    pub fn process(&mut self, ctx: &Ctx<'_>, input: &[u8]) -> Result<Vec<u8>> {
        match &mut self.decoder {
            Decoder::Gzip(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                Ok(Self::take_delta(decoder.get_ref(), &mut self.emitted))
            }
            Decoder::Zlib(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                Ok(Self::take_delta(decoder.get_ref(), &mut self.emitted))
            }
            Decoder::Raw(decoder) => {
                decoder
                    .write_all(input)
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                decoder
                    .flush()
                    .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))?;
                Ok(Self::take_delta(decoder.get_ref(), &mut self.emitted))
            }
        }
    }
}

fn chunk_bytes<'js>(ctx: &Ctx<'js>, chunk: Value<'js>) -> Result<Vec<u8>> {
    Host::buffer_source_bytes(ctx, chunk)?
        .ok_or_else(|| Host::throw_type(ctx, "chunk must be a BufferSource"))
}

fn enqueue_bytes<'js>(ctx: &Ctx<'js>, controller: &Object<'js>, bytes: Vec<u8>) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let enqueue: Function = controller.get("enqueue")?;
    enqueue.call((
        This(controller.clone()),
        TypedArray::<u8>::new_copy(ctx.clone(), bytes)?,
    ))
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct CompressionStream<'js> {
    readable: Class<'js, ReadableStream<'js>>,
    writable: Class<'js, WritableStream<'js>>,
}

#[rquickjs::methods]
impl<'js> CompressionStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, format: String) -> Result<Self> {
        let compressor = Rc::new(RefCell::new(Compressor::new(&ctx, &format)?));
        let transformer = Object::new(ctx.clone())?;
        transformer.set(
            "transform",
            Function::new(ctx.clone(), {
                let compressor = Rc::clone(&compressor);
                move |ctx: Ctx<'js>, chunk: Value<'js>, controller: Object<'js>| -> Result<()> {
                    let bytes = chunk_bytes(&ctx, chunk)?;
                    let out = compressor.borrow_mut().process(&ctx, &bytes, false)?;
                    enqueue_bytes(&ctx, &controller, out)
                }
            })?,
        )?;
        transformer.set(
            "flush",
            Function::new(ctx.clone(), {
                let compressor = Rc::clone(&compressor);
                move |ctx: Ctx<'js>, controller: Object<'js>| -> Result<()> {
                    let out = compressor.borrow_mut().process(&ctx, &[], true)?;
                    enqueue_bytes(&ctx, &controller, out)
                }
            })?,
        )?;
        let transform = TransformStream::new(ctx.clone(), Opt(Some(transformer)))?;
        Ok(Self {
            readable: transform.readable(),
            writable: transform.writable(),
        })
    }

    #[qjs(get)]
    pub fn readable(&self) -> Class<'js, ReadableStream<'js>> {
        self.readable.clone()
    }

    #[qjs(get)]
    pub fn writable(&self) -> Class<'js, WritableStream<'js>> {
        self.writable.clone()
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "CompressionStream"
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct DecompressionStream<'js> {
    readable: Class<'js, ReadableStream<'js>>,
    writable: Class<'js, WritableStream<'js>>,
}

#[rquickjs::methods]
impl<'js> DecompressionStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, format: String) -> Result<Self> {
        let decompressor = Rc::new(RefCell::new(Decompressor::new(&ctx, &format)?));
        let transformer = Object::new(ctx.clone())?;
        transformer.set(
            "transform",
            Function::new(ctx.clone(), {
                let decompressor = Rc::clone(&decompressor);
                move |ctx: Ctx<'js>, chunk: Value<'js>, controller: Object<'js>| -> Result<()> {
                    let bytes = chunk_bytes(&ctx, chunk)?;
                    let out = decompressor.borrow_mut().process(&ctx, &bytes)?;
                    enqueue_bytes(&ctx, &controller, out)
                }
            })?,
        )?;
        let transform = TransformStream::new(ctx.clone(), Opt(Some(transformer)))?;
        Ok(Self {
            readable: transform.readable(),
            writable: transform.writable(),
        })
    }

    #[qjs(get)]
    pub fn readable(&self) -> Class<'js, ReadableStream<'js>> {
        self.readable.clone()
    }

    #[qjs(get)]
    pub fn writable(&self) -> Class<'js, WritableStream<'js>> {
        self.writable.clone()
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "DecompressionStream"
    }
}
