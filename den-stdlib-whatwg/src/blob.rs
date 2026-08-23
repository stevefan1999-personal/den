//! WHATWG File API `Blob` and `File`.

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

    pub fn from_parts<'js>(ctx: &Ctx<'js>, parts: Value<'js>, type_: String) -> Result<Self> {
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
        let mut bytes = Vec::new();
        for part in iterator {
            let part = part?;
            if let Some(chunk) = Host::blob_like_bytes(ctx, &part) {
                bytes.extend_from_slice(&chunk);
            } else if let Some(chunk) = Host::buffer_source_bytes(ctx, part.clone())? {
                bytes.extend_from_slice(&chunk);
            } else {
                bytes.extend_from_slice(Coerced::<String>::from_js(ctx, part)?.0.as_bytes());
            }
        }
        Ok(Self::from_bytes(bytes, type_))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn mime_type(&self) -> &str {
        &self.type_
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

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
    pub fn from_inner(inner: BlobInner) -> Self {
        Self { inner }
    }

    pub fn bytes(&self) -> &[u8] {
        self.inner.bytes()
    }

    pub fn mime_type(&self) -> &str {
        self.inner.mime_type()
    }

    pub fn inner(&self) -> &BlobInner {
        &self.inner
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Blob {
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>,
        blob_parts: Opt<Value<'js>>,
        options: Opt<Value<'js>>,
    ) -> Result<Self> {
        let options = match options.0 {
            None => None,
            Some(value) if value.is_null() => None,
            Some(value) if value.is_object() => Some(value),
            Some(value) if value.as_function().is_some() => Some(value),
            Some(_) => {
                return Err(Host::throw_type(
                    &ctx,
                    "Failed to construct 'Blob': parameter 2 cannot convert to dictionary.",
                ));
            }
        };
        let type_ = options
            .as_ref()
            .and_then(|value| value.as_object())
            .map(|obj| {
                obj.get::<_, Value>("type")
                    .ok()
                    .filter(|value| !value.is_undefined() && !value.is_null())
                    .and_then(|value| Host::coerce_string(&ctx, value).ok())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let parts = blob_parts.0.unwrap_or_else(|| {
            rquickjs::Array::new(ctx.clone())
                .expect("array")
                .into_value()
        });
        Ok(Self {
            inner: BlobInner::from_parts(&ctx, parts, type_)?,
        })
    }

    #[qjs(get, enumerable)]
    pub fn size(&self) -> usize {
        self.inner.size()
    }

    #[qjs(get, enumerable, rename = "type")]
    pub fn mime_type_js(&self) -> String {
        self.inner.mime_type().to_string()
    }

    pub async fn text(&self) -> String {
        String::from_utf8_lossy(self.inner.bytes()).into_owned()
    }

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        ArrayBuffer::new_copy(ctx, self.inner.bytes())
    }

    pub fn stream<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, ReadableStream<'js>>> {
        self.inner.stream(ctx)
    }

    pub fn slice(&self, start: Opt<f64>, end: Opt<f64>, type_: Opt<String>) -> Self {
        let size = self.inner.size() as f64;
        Self {
            inner: self.inner.slice(
                start.0.unwrap_or(0.0),
                end.0.unwrap_or(size),
                type_.0.unwrap_or_default(),
            ),
        }
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Blob"
    }
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct File {
    #[qjs(skip_trace)]
    inner: BlobInner,
    #[qjs(skip_trace)]
    name: String,
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

    pub fn bytes(&self) -> &[u8] {
        self.inner.bytes()
    }

    pub fn mime_type(&self) -> &str {
        self.inner.mime_type()
    }

    pub fn file_name(&self) -> &str {
        &self.name
    }

    pub fn inner(&self) -> &BlobInner {
        &self.inner
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl File {
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>,
        file_bits: Opt<Value<'js>>,
        file_name: Opt<Value<'js>>,
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
        let name = Host::coerce_string(&ctx, file_name)?;
        let type_ = options
            .0
            .as_ref()
            .and_then(|value| value.as_object())
            .and_then(|obj| obj.get::<_, Coerced<String>>("type").ok())
            .map(|value| value.0)
            .unwrap_or_default();
        let blob = Blob::new(ctx.clone(), Opt(Some(file_bits)), Opt(options.0.clone()))?;
        let inner = if blob.inner.mime_type().is_empty() && !type_.is_empty() {
            BlobInner::from_bytes(blob.inner.bytes().to_vec(), type_)
        } else {
            blob.inner
        };
        let last_modified = match options.0.as_ref().and_then(|value| value.as_object()) {
            Some(obj) => match obj.get::<_, Value>("lastModified") {
                Ok(value) if !value.is_undefined() => value
                    .as_number()
                    .filter(|number| number.is_finite())
                    .unwrap_or(0.0),
                _ => Self::now_millis(),
            },
            None => Self::now_millis(),
        };
        Ok(Self {
            inner,
            name,
            last_modified,
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
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[qjs(get, enumerable)]
    pub fn last_modified(&self) -> f64 {
        self.last_modified
    }

    #[qjs(get, enumerable)]
    pub fn size(&self) -> usize {
        self.inner.size()
    }

    #[qjs(get, enumerable, rename = "type")]
    pub fn mime_type_js(&self) -> String {
        self.inner.mime_type().to_string()
    }

    pub async fn text(&self) -> String {
        String::from_utf8_lossy(self.inner.bytes()).into_owned()
    }

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        ArrayBuffer::new_copy(ctx, self.inner.bytes())
    }

    pub fn stream<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, ReadableStream<'js>>> {
        self.inner.stream(ctx)
    }

    pub fn slice(&self, start: Opt<f64>, end: Opt<f64>, type_: Opt<String>) -> Blob {
        let size = self.inner.size() as f64;
        Blob::from_inner(self.inner.slice(
            start.0.unwrap_or(0.0),
            end.0.unwrap_or(size),
            type_.0.unwrap_or_default(),
        ))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "File"
    }
}
