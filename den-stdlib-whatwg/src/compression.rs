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

trait CodecImpl: Write {
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
            impl CodecImpl for $encoder {
                fn output(&self) -> &[u8] { self.get_ref() }
                fn finish(&mut self) -> io::Result<()> { self.try_finish() }
            }
        )*
        $(
            impl CodecImpl for $decoder {
                fn output(&self) -> &[u8] { self.get_ref() }
            }
        )*
    };
}

codecs!(
    encoders: [GzEncoder<Vec<u8>>, ZlibEncoder<Vec<u8>>, DeflateEncoder<Vec<u8>>],
    decoders: [GzDecoder<Vec<u8>>, ZlibDecoder<Vec<u8>>, DeflateDecoder<Vec<u8>>]
);

pub struct Codec {
    inner:   Box<dyn CodecImpl>,
    emitted: usize,
}

impl Codec {
    pub fn encoder(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let level = Compression::default();
        let inner: Box<dyn CodecImpl> = match Format::from_label(ctx, format)? {
            Format::Gzip => Box::new(GzEncoder::new(Vec::new(), level)),
            Format::Deflate => Box::new(ZlibEncoder::new(Vec::new(), level)),
            Format::DeflateRaw => Box::new(DeflateEncoder::new(Vec::new(), level)),
        };
        Ok(Self { inner, emitted: 0 })
    }

    pub fn decoder(ctx: &Ctx<'_>, format: &str) -> Result<Self> {
        let inner: Box<dyn CodecImpl> = match Format::from_label(ctx, format)? {
            Format::Gzip => Box::new(GzDecoder::new(Vec::new())),
            Format::Deflate => Box::new(ZlibDecoder::new(Vec::new())),
            Format::DeflateRaw => Box::new(DeflateDecoder::new(Vec::new())),
        };
        Ok(Self { inner, emitted: 0 })
    }

    pub fn process(&mut self, ctx: &Ctx<'_>, input: &[u8], finish: bool) -> Result<Vec<u8>> {
        self.inner
            .process(input, finish, &mut self.emitted)
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

/// The transform half both streams share: bytes in, codec delta out.
fn transform_fn<'js>(ctx: &Ctx<'js>, codec: &Rc<RefCell<Codec>>) -> Result<Function<'js>> {
    Function::new(ctx.clone(), {
        let codec = Rc::clone(codec);
        move |ctx: Ctx<'js>, chunk: Value<'js>, controller: Object<'js>| -> Result<()> {
            let bytes = chunk_bytes(&ctx, chunk)?;
            let out = codec.borrow_mut().process(&ctx, &bytes, false)?;
            enqueue_bytes(&ctx, &controller, out)
        }
    })
}

fn transform_pair<'js>(
    ctx: &Ctx<'js>, transform: Function<'js>, flush: Option<Function<'js>>,
) -> Result<(
    Class<'js, ReadableStream<'js>>,
    Class<'js, WritableStream<'js>>,
)> {
    let transformer = Object::new(ctx.clone())?;
    transformer.set("transform", transform)?;
    if let Some(flush) = flush {
        transformer.set("flush", flush)?;
    }
    let stream = TransformStream::new(
        ctx.clone(),
        Opt(Some(transformer.into_value())),
        Opt(None),
        Opt(None),
    )?;
    Ok((stream.readable.clone(), stream.writable.clone()))
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
        let codec = Rc::new(RefCell::new(Codec::encoder(&ctx, &format)?));
        let flush = Function::new(ctx.clone(), {
            let codec = Rc::clone(&codec);
            move |ctx: Ctx<'js>, controller: Object<'js>| -> Result<()> {
                let out = codec.borrow_mut().process(&ctx, &[], true)?;
                enqueue_bytes(&ctx, &controller, out)
            }
        })?;
        let (readable, writable) = transform_pair(&ctx, transform_fn(&ctx, &codec)?, Some(flush))?;
        Ok(Self { readable, writable })
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
        // No flush transformer, deliberately: DecompressionStream must not
        // finish the codec at end of stream, or truncated input would look
        // complete.
        let codec = Rc::new(RefCell::new(Codec::decoder(&ctx, &format)?));
        let (readable, writable) = transform_pair(&ctx, transform_fn(&ctx, &codec)?, None)?;
        Ok(Self { readable, writable })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "DecompressionStream" }
}
