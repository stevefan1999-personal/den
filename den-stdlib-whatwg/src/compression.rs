//! CompressionStream / DecompressionStream wrapping flate2.

use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

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
            _ => {
                Err(Host::throw_type(
                    ctx,
                    &format!("Unsupported compression format: '{label}'"),
                ))
            }
        }
    }
}

trait Codec: Write {
    fn output(&self) -> &[u8];

    fn finish(&mut self) -> io::Result<()> { self.flush() }

    fn process(&mut self, input: &[u8], finish: bool, emitted: &mut usize) -> io::Result<Vec<u8>> {
        self.write_all(input)?;
        if finish {
            self.finish()?;
        } else {
            self.flush()?;
        }
        let output = self.output();
        let delta = output.get(*emitted..).unwrap_or_default().to_vec();
        *emitted = output.len();
        Ok(delta)
    }
}

macro_rules! codecs {
    (encoders: [$($encoder:ty),*], decoders: [$($decoder:ty),*]) => {
        $(
            impl Codec for $encoder {
                fn output(&self) -> &[u8] { self.get_ref() }
                fn finish(&mut self) -> io::Result<()> { self.try_finish() }
            }
        )*
        $(
            impl Codec for $decoder {
                fn output(&self) -> &[u8] { self.get_ref() }
            }
        )*
    };
}

codecs!(
    encoders: [GzEncoder<Vec<u8>>, ZlibEncoder<Vec<u8>>, DeflateEncoder<Vec<u8>>],
    decoders: [GzDecoder<Vec<u8>>, ZlibDecoder<Vec<u8>>, DeflateDecoder<Vec<u8>>]
);

pub struct Compressor {
    codec:   Box<dyn Codec>,
    emitted: usize,
}

pub struct Decompressor {
    codec:   Box<dyn Codec>,
    emitted: usize,
}

impl Compressor {
    pub fn new(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let level = Compression::default();
        let codec: Box<dyn Codec> = match Format::from_label(ctx, format)? {
            Format::Gzip => Box::new(GzEncoder::new(Vec::new(), level)),
            Format::Deflate => Box::new(ZlibEncoder::new(Vec::new(), level)),
            Format::DeflateRaw => Box::new(DeflateEncoder::new(Vec::new(), level)),
        };
        Ok(Self { codec, emitted: 0 })
    }

    pub fn process(&mut self, ctx: &Ctx<'_>, input: &[u8], finish: bool) -> Result<Vec<u8>> {
        self.codec
            .process(input, finish, &mut self.emitted)
            .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))
    }
}

impl Decompressor {
    pub fn new(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let codec: Box<dyn Codec> = match Format::from_label(ctx, format)? {
            Format::Gzip => Box::new(GzDecoder::new(Vec::new())),
            Format::Deflate => Box::new(ZlibDecoder::new(Vec::new())),
            Format::DeflateRaw => Box::new(DeflateDecoder::new(Vec::new())),
        };
        Ok(Self { codec, emitted: 0 })
    }

    pub fn process(&mut self, ctx: &Ctx<'_>, input: &[u8]) -> Result<Vec<u8>> {
        self.codec
            .process(input, false, &mut self.emitted)
            .map_err(|err| rquickjs::Exception::throw_internal(ctx, &format!("{err}")))
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
    #[qjs(get)]
    readable: Class<'js, ReadableStream<'js>>,
    #[qjs(get)]
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
        let transform = TransformStream::new(
            ctx.clone(),
            Opt(Some(transformer.into_value())),
            Opt(None),
            Opt(None),
        )?;
        Ok(Self {
            readable: transform.readable.clone(),
            writable: transform.writable.clone(),
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "CompressionStream" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct DecompressionStream<'js> {
    #[qjs(get)]
    readable: Class<'js, ReadableStream<'js>>,
    #[qjs(get)]
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
        let transform = TransformStream::new(
            ctx.clone(),
            Opt(Some(transformer.into_value())),
            Opt(None),
            Opt(None),
        )?;
        Ok(Self {
            readable: transform.readable.clone(),
            writable: transform.writable.clone(),
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "DecompressionStream" }
}
