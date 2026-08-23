//! WHATWG `FormData`. Multipart is `Symbol.for("den.toMultipartBlob")`.

use rquickjs::{
    Class, Ctx, FromJs, Function, IntoJs, Iterable, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, Rest, This},
};

use crate::{
    blob::{Blob, BlobInner, File},
    host::Host,
};

#[derive(Clone)]
enum FormValue {
    Text(String),
    File(File),
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct FormData {
    #[qjs(skip_trace)]
    entries: Vec<(String, FormValue)>,
}

impl FormData {
    fn normalize<'js>(
        ctx: &Ctx<'js>, name: String, value: Value<'js>, filename: Option<String>,
    ) -> Result<(String, FormValue)> {
        if Host::is_blob_like(&value) {
            let filename = filename
                .or_else(|| Host::file_name(&value))
                .unwrap_or_else(|| "blob".to_string());
            let same_file = Host::is_file_like(&value)
                && Host::file_name(&value).as_deref() == Some(filename.as_str());
            if same_file {
                let file = Class::<File>::from_value(&value)?;
                return Ok((name, FormValue::File(file.borrow().clone())));
            }
            let inner = if let Ok(file) = Class::<File>::from_value(&value) {
                file.borrow().inner().clone()
            } else if let Ok(blob) = Class::<Blob>::from_value(&value) {
                blob.borrow().inner().clone()
            } else {
                BlobInner::from_bytes(Vec::new(), String::new())
            };
            let mime = inner.mime_type().to_string();
            let file = File::from_parts(
                BlobInner::from_bytes(inner.bytes().to_vec(), mime),
                filename,
                File::now_millis(),
            );
            return Ok((name, FormValue::File(file)));
        }
        Ok((name, FormValue::Text(Host::coerce_string(ctx, value)?)))
    }

    fn value_js<'js>(&self, ctx: &Ctx<'js>, value: &FormValue) -> Result<Value<'js>> {
        match value {
            FormValue::Text(text) => rquickjs::IntoJs::into_js(text.clone(), ctx),
            FormValue::File(file) => {
                Class::instance(ctx.clone(), file.clone()).map(|class| class.into_value())
            }
        }
    }

    pub fn to_multipart_blob<'js>(&self, ctx: &Ctx<'js>) -> Result<Class<'js, Blob>> {
        let boundary = Self::boundary();
        let mut bytes = Vec::new();
        let prefix = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"");
        for (name, value) in &self.entries {
            match value {
                FormValue::Text(text) => {
                    bytes.extend_from_slice(prefix.as_bytes());
                    bytes.extend_from_slice(Self::escape(&Self::crlf(name)).as_bytes());
                    bytes.extend_from_slice(b"\"\r\n\r\n");
                    bytes.extend_from_slice(Self::crlf(text).as_bytes());
                    bytes.extend_from_slice(b"\r\n");
                }
                FormValue::File(file) => {
                    bytes.extend_from_slice(prefix.as_bytes());
                    bytes.extend_from_slice(Self::escape(&Self::crlf(name)).as_bytes());
                    bytes.extend_from_slice(b"\"; filename=\"");
                    bytes.extend_from_slice(Self::escape(file.file_name()).as_bytes());
                    bytes.extend_from_slice(b"\"\r\nContent-Type: ");
                    let mime = if file.mime_type().is_empty() {
                        "application/octet-stream"
                    } else {
                        file.mime_type()
                    };
                    bytes.extend_from_slice(mime.as_bytes());
                    bytes.extend_from_slice(b"\r\n\r\n");
                    bytes.extend_from_slice(file.bytes());
                    bytes.extend_from_slice(b"\r\n");
                }
            }
        }
        bytes.extend_from_slice(format!("--{boundary}--").as_bytes());
        Class::instance(
            ctx.clone(),
            Blob::from_inner(BlobInner::from_bytes(
                bytes,
                format!("multipart/form-data; boundary={boundary}"),
            )),
        )
    }

    fn boundary() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        format!("----formdata-den-{nanos:x}")
    }

    fn crlf(value: &str) -> String {
        value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r\n")
    }

    fn escape(value: &str) -> String {
        value
            .replace('\n', "%0A")
            .replace('\r', "%0D")
            .replace('"', "%22")
    }

    fn ensure(ctx: &Ctx<'_>, args: usize, expected: usize) -> Result<()> {
        if args < expected {
            return Err(Host::throw_type(
                ctx,
                &format!("{expected} argument required, but only {args} present."),
            ));
        }
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl FormData {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, form: Opt<Value<'js>>) -> Result<Self> {
        if let Some(form) = form.0 {
            if !form.is_undefined() && !form.is_null() {
                return Err(Host::throw_type(
                    &ctx,
                    "Failed to construct 'FormData': HTMLFormElement is not supported",
                ));
            }
        }
        Ok(Self {
            entries: Vec::new(),
        })
    }

    pub fn append<'js>(&mut self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        Self::ensure(&ctx, args.0.len(), 2)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        let filename = args
            .0
            .get(2)
            .cloned()
            .and_then(|value| Host::coerce_string(&ctx, value).ok());
        let entry = Self::normalize(&ctx, name, args.0[1].clone(), filename)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn delete<'js>(&mut self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        Self::ensure(&ctx, args.0.len(), 1)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        self.entries.retain(|(existing, _)| existing != &name);
        Ok(())
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Value<'js>> {
        Self::ensure(&ctx, args.0.len(), 1)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        for (existing, value) in &self.entries {
            if existing == &name {
                return self.value_js(&ctx, value);
            }
        }
        Ok(Value::new_null(ctx))
    }

    pub fn get_all<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Vec<Value<'js>>> {
        Self::ensure(&ctx, args.0.len(), 1)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        let mut result = Vec::new();
        for (existing, value) in &self.entries {
            if existing == &name {
                result.push(self.value_js(&ctx, value)?);
            }
        }
        Ok(result)
    }

    pub fn has<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<bool> {
        Self::ensure(&ctx, args.0.len(), 1)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        Ok(self.entries.iter().any(|(existing, _)| existing == &name))
    }

    pub fn set<'js>(&mut self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        Self::ensure(&ctx, args.0.len(), 2)?;
        let name = Host::coerce_string(&ctx, args.0[0].clone())?;
        let filename = args
            .0
            .get(2)
            .cloned()
            .and_then(|value| Host::coerce_string(&ctx, value).ok());
        let replacement = Self::normalize(&ctx, name.clone(), args.0[1].clone(), filename)?;
        let mut result = Vec::new();
        let mut replace = true;
        for entry in self.entries.drain(..) {
            if entry.0 == name {
                if replace {
                    result.push(replacement.clone());
                    replace = false;
                }
            } else {
                result.push(entry);
            }
        }
        if replace {
            result.push(replacement);
        }
        self.entries = result;
        Ok(())
    }

    pub fn keys<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let keys: Vec<String> = self.entries.iter().map(|(name, _)| name.clone()).collect();
        Iterable::from(keys).into_js(&ctx)
    }

    pub fn values<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut values = Vec::new();
        for (_, value) in &self.entries {
            values.push(self.value_js(&ctx, value)?);
        }
        Iterable::from(values).into_js(&ctx)
    }

    pub fn entries<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut pairs = Vec::new();
        for (name, value) in &self.entries {
            let pair = rquickjs::Array::new(ctx.clone())?;
            pair.set(0, name.clone())?;
            pair.set(1, self.value_js(&ctx, value)?)?;
            pairs.push(pair);
        }
        Iterable::from(pairs).into_js(&ctx)
    }

    pub fn for_each<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, args: Rest<Value<'js>>,
    ) -> Result<()> {
        Self::ensure(&ctx, args.0.len(), 1)?;
        let callback = Function::from_js(&ctx, args.0[0].clone())?;
        let this_arg = args
            .0
            .get(1)
            .cloned()
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let form = this.0.borrow();
        for (name, value) in &form.entries {
            let js_value = form.value_js(&ctx, value)?;
            callback.call::<_, ()>((
                This(this_arg.clone()),
                js_value,
                name.clone(),
                this.0.clone(),
            ))?;
        }
        Ok(())
    }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn js_iterator<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> { self.entries(ctx) }

    #[qjs(rename = "toMultipartBlob")]
    pub fn to_multipart_blob_js<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, Blob>> {
        self.to_multipart_blob(&ctx)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "FormData" }
}
