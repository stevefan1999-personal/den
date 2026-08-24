//! WHATWG File API `Blob` and `File`.

use den_util::coerce_string;
use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, FromJs, JsIterator, JsLifetime, Object, Result, TypedArray,
    Value, atom::PredefinedAtom, class::Trace, function::Opt,
};

use crate::{host::Host, streams::ReadableStream};

const POOL_SIZE: usize = 65536;

#[derive(Clone)]
pub struct BlobInner {
    bytes: Vec<u8>,
    type_: String,
}

impl BlobInner {
    pub fn from_bytes(bytes: Vec<u8>, type_: String) -> Self {
        Self {
            bytes,
            type_: Host::ascii_type(&type_),
        }
    }

    pub fn from_parts<'js>(
        ctx: &Ctx<'js>, parts: Value<'js>, type_: String, native: bool,
    ) -> Result<Self> {
        Ok(Self::from_collected(
            collect_parts(ctx, parts)?,
            type_,
            native,
        ))
    }

    fn from_collected(chunks: Vec<Part>, type_: String, native: bool) -> Self {
        let mut bytes = Vec::new();
        for chunk in chunks {
            match chunk {
                Part::Bytes(chunk) => bytes.extend_from_slice(&chunk),
                Part::Text(text) if native => bytes.extend_from_slice(&native_line_endings(&text)),
                Part::Text(text) => bytes.extend_from_slice(text.as_bytes()),
            }
        }
        Self::from_bytes(bytes, type_)
    }

    pub fn bytes(&self) -> &[u8] { &self.bytes }

    pub fn mime_type(&self) -> &str { &self.type_ }

    pub fn size(&self) -> usize { self.bytes.len() }

    pub fn slice(&self, start: f64, end: f64, type_: String) -> Self {
        let size = self.bytes.len() as f64;
        let relative_start = if start < 0.0 {
            (size + start).max(0.0)
        } else {
            start.min(size)
        };
        let relative_end = if end < 0.0 {
            (size + end).max(0.0)
        } else {
            end.min(size)
        };
        let from = relative_start as usize;
        let to = relative_end.max(relative_start) as usize;
        let to = to.min(self.bytes.len());
        let from = from.min(to);
        Self::from_bytes(self.bytes[from..to].to_vec(), type_)
    }

    pub fn stream<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, ReadableStream<'js>>> {
        let source = Object::new(ctx.clone())?;
        let chunks: Vec<Vec<u8>> = self
            .bytes
            .chunks(POOL_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect();
        source.set(
            "start",
            rquickjs::Function::new(ctx.clone(), {
                let chunks = chunks.clone();
                move |ctx: Ctx<'js>, controller: Object<'js>| -> Result<()> {
                    let enqueue: rquickjs::Function = controller.get("enqueue")?;
                    for chunk in &chunks {
                        enqueue.call::<_, ()>((
                            rquickjs::function::This(controller.clone()),
                            TypedArray::<u8>::new_copy(ctx.clone(), chunk)?,
                        ))?;
                    }
                    let close: rquickjs::Function = controller.get("close")?;
                    close.call::<_, ()>((rquickjs::function::This(controller),))?;
                    Ok(())
                }
            })?,
        )?;
        Class::instance(
            ctx.clone(),
            ReadableStream::new(ctx, Opt(Some(source.into_value())))?,
        )
    }
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct Blob {
    #[qjs(skip_trace)]
    inner: BlobInner,
}

impl Blob {
    pub fn from_inner(inner: BlobInner) -> Self { Self { inner } }

    pub fn bytes(&self) -> &[u8] { self.inner.bytes() }

    pub fn mime_type(&self) -> &str { self.inner.mime_type() }

    pub fn inner(&self) -> &BlobInner { &self.inner }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Blob {
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>, blob_parts: Opt<Value<'js>>, options: Opt<Value<'js>>,
    ) -> Result<Self> {
        let parts = match blob_parts.0 {
            None => empty_sequence(&ctx)?,
            Some(value) if value.is_undefined() => empty_sequence(&ctx)?,
            Some(value) => value,
        };
        let collected = collect_parts(&ctx, parts)?;
        let bag = parse_blob_bag(&ctx, options.0)?;
        Ok(Self {
            inner: BlobInner::from_collected(collected, bag.type_, bag.native),
        })
    }

    #[qjs(get, enumerable)]
    pub fn size(&self) -> usize { self.inner.size() }

    #[qjs(get, enumerable, rename = "type")]
    pub fn mime_type_js(&self) -> String { self.inner.mime_type().to_string() }

    pub async fn text(&self) -> String { String::from_utf8_lossy(self.inner.bytes()).into_owned() }

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        ArrayBuffer::new_copy(ctx, self.inner.bytes())
    }

    #[qjs(rename = "bytes")]
    pub async fn bytes_js<'js>(&self, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        TypedArray::<u8>::new_copy(ctx, self.inner.bytes())
    }

    pub fn stream<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, ReadableStream<'js>>> {
        self.inner.stream(ctx)
    }

    pub fn slice<'js>(
        &self, ctx: Ctx<'js>, start: Opt<Value<'js>>, end: Opt<Value<'js>>, type_: Opt<Value<'js>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: slice_inner(&ctx, &self.inner, start.0, end.0, type_.0)?,
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Blob" }
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct File {
    #[qjs(skip_trace)]
    inner:         BlobInner,
    #[qjs(skip_trace)]
    name:          String,
    last_modified: f64,
}

impl File {
    pub fn from_parts(inner: BlobInner, name: String, last_modified: f64) -> Self {
        Self {
            inner,
            name,
            last_modified,
        }
    }

    pub fn bytes(&self) -> &[u8] { self.inner.bytes() }

    pub fn mime_type(&self) -> &str { self.inner.mime_type() }

    pub fn file_name(&self) -> &str { &self.name }

    pub fn inner(&self) -> &BlobInner { &self.inner }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl File {
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>, file_bits: Opt<Value<'js>>, file_name: Opt<Value<'js>>,
        options: Opt<Value<'js>>,
    ) -> Result<Self> {
        let Some(file_bits) = file_bits.0 else {
            return Err(Host::throw_type(
                &ctx,
                "Failed to construct 'File': 2 arguments required, but only 0 present.",
            ));
        };
        let Some(file_name) = file_name.0 else {
            return Err(Host::throw_type(
                &ctx,
                "Failed to construct 'File': 2 arguments required, but only 1 present.",
            ));
        };
        let collected = collect_parts(&ctx, file_bits)?;
        let name = coerce_string(&ctx, file_name)?;
        let (bag, last_modified) = parse_file_bag(&ctx, options.0)?;
        Ok(Self {
            inner: BlobInner::from_collected(collected, bag.type_, bag.native),
            name,
            last_modified: last_modified.unwrap_or_else(Self::now_millis),
        })
    }

    #[qjs(skip)]
    pub fn now_millis() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as f64)
            .unwrap_or(0.0)
    }

    #[qjs(get, enumerable)]
    pub fn name(&self) -> String { self.name.clone() }

    #[qjs(get, enumerable)]
    pub fn last_modified(&self) -> f64 { self.last_modified }

    #[qjs(get, enumerable)]
    pub fn size(&self) -> usize { self.inner.size() }

    #[qjs(get, enumerable, rename = "type")]
    pub fn mime_type_js(&self) -> String { self.inner.mime_type().to_string() }

    pub async fn text(&self) -> String { String::from_utf8_lossy(self.inner.bytes()).into_owned() }

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        ArrayBuffer::new_copy(ctx, self.inner.bytes())
    }

    #[qjs(rename = "bytes")]
    pub async fn bytes_js<'js>(&self, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        TypedArray::<u8>::new_copy(ctx, self.inner.bytes())
    }

    pub fn stream<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, ReadableStream<'js>>> {
        self.inner.stream(ctx)
    }

    pub fn slice<'js>(
        &self, ctx: Ctx<'js>, start: Opt<Value<'js>>, end: Opt<Value<'js>>, type_: Opt<Value<'js>>,
    ) -> Result<Blob> {
        Ok(Blob::from_inner(slice_inner(
            &ctx,
            &self.inner,
            start.0,
            end.0,
            type_.0,
        )?))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "File" }
}

enum Part {
    Bytes(Vec<u8>),
    Text(String),
}

struct BlobBag {
    type_:  String,
    native: bool,
}

fn collect_parts<'js>(ctx: &Ctx<'js>, parts: Value<'js>) -> Result<Vec<Part>> {
    if !parts.is_object() {
        return Err(Host::throw_type(
            ctx,
            "Failed to construct 'Blob': The provided value cannot be converted to a sequence.",
        ));
    }
    let iterator = JsIterator::<Value<'js>>::from_js(ctx, parts).map_err(|_| {
        Host::throw_type(
            ctx,
            "Failed to construct 'Blob': The object must have a callable @@iterator property.",
        )
    })?;
    let mut chunks = Vec::new();
    for part in iterator {
        let part = part?;
        if let Some(chunk) = Host::blob_like_bytes(ctx, &part) {
            chunks.push(Part::Bytes(chunk));
        } else if let Some(chunk) = Host::buffer_source_bytes(ctx, part.clone())? {
            chunks.push(Part::Bytes(chunk));
        } else {
            chunks.push(Part::Text(Host::coerce_usv_string(ctx, part)?));
        }
    }
    Ok(chunks)
}

fn parse_blob_bag<'js>(ctx: &Ctx<'js>, options: Option<Value<'js>>) -> Result<BlobBag> {
    let object = dictionary_object(ctx, options)?;
    let Some(object) = object else {
        return Ok(BlobBag {
            type_:  String::new(),
            native: false,
        });
    };
    let native = read_endings(ctx, &object)?;
    let type_ = read_type(ctx, &object)?;
    Ok(BlobBag { type_, native })
}

fn parse_file_bag<'js>(
    ctx: &Ctx<'js>, options: Option<Value<'js>>,
) -> Result<(BlobBag, Option<f64>)> {
    let object = dictionary_object(ctx, options)?;
    let Some(object) = object else {
        return Ok((
            BlobBag {
                type_:  String::new(),
                native: false,
            },
            None,
        ));
    };
    let native = read_endings(ctx, &object)?;
    let last_modified = read_last_modified(ctx, &object)?;
    let type_ = read_type(ctx, &object)?;
    Ok((BlobBag { type_, native }, last_modified))
}

fn dictionary_object<'js>(
    ctx: &Ctx<'js>, options: Option<Value<'js>>,
) -> Result<Option<Object<'js>>> {
    let Some(value) = options else {
        return Ok(None);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    if value.is_object() || value.as_function().is_some() {
        return Ok(value.as_object().map(|object| object.clone()));
    }
    Err(Host::throw_type(
        ctx,
        "Failed to construct 'Blob': parameter 2 cannot convert to dictionary.",
    ))
}

fn read_endings<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<bool> {
    let value: Value = object.get("endings")?;
    if value.is_undefined() {
        return Ok(false);
    }
    match coerce_string(ctx, value)?.as_str() {
        "transparent" => Ok(false),
        "native" => Ok(true),
        _ => {
            Err(Host::throw_type(
                ctx,
                "Failed to construct 'Blob': The provided value is not a valid enum value of type \
                 EndingType.",
            ))
        }
    }
}

fn read_type<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<String> {
    let value: Value = object.get("type")?;
    if value.is_undefined() {
        return Ok(String::new());
    }
    coerce_string(ctx, value)
}

fn read_last_modified<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<Option<f64>> {
    let value: Value = object.get("lastModified")?;
    if value.is_undefined() {
        return Ok(None);
    }
    let number = Coerced::<f64>::from_js(ctx, value)?.0;
    Ok(Some(if number.is_finite() { number } else { 0.0 }))
}

fn native_line_endings(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                out.push(b'\n');
                index += 2;
            }
            b'\r' | b'\n' => {
                out.push(b'\n');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

fn empty_sequence<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    Ok(rquickjs::Array::new(ctx.clone())?.into_value())
}

fn slice_inner<'js>(
    ctx: &Ctx<'js>, inner: &BlobInner, start: Option<Value<'js>>, end: Option<Value<'js>>,
    type_: Option<Value<'js>>,
) -> Result<BlobInner> {
    let size = inner.size() as f64;
    Ok(inner.slice(
        optional_clamp(ctx, start)?.unwrap_or(0.0),
        optional_clamp(ctx, end)?.unwrap_or(size),
        optional_type(ctx, type_)?,
    ))
}

fn optional_clamp<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<Option<f64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_undefined() {
        return Ok(None);
    }
    Ok(Some(clamp_long_long(
        Coerced::<f64>::from_js(ctx, value)?.0,
    )))
}

fn optional_type<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    if value.is_undefined() {
        return Ok(String::new());
    }
    coerce_string(ctx, value)
}

fn clamp_long_long(number: f64) -> f64 {
    if number.is_nan() {
        0.0
    } else if number >= i64::MAX as f64 {
        i64::MAX as f64
    } else if number <= i64::MIN as f64 {
        i64::MIN as f64
    } else {
        number.round_ties_even()
    }
}
