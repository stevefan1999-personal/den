//! WHATWG `FormData`. Multipart is `Symbol.for("den.toMultipartBlob")`.

use den_util::coerce_string;
use rquickjs::{
    Class, Coerced, Ctx, Function, IntoJs as _, Iterable, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

use crate::{
    blob::{Blob, File, Inner},
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
            let inner = Class::<File>::from_value(&value).map_or_else(
                |_error| {
                    Class::<Blob>::from_value(&value).map_or_else(
                        |_error| Inner::from_bytes(Vec::new(), String::new()),
                        |blob| blob.borrow().inner().clone(),
                    )
                },
                |file| file.borrow().inner().clone(),
            );
            let mime = inner.mime_type().to_string();
            let file = File::from_parts(
                Inner::from_bytes(inner.bytes().to_vec(), mime),
                filename,
                File::now_millis(),
            );
            return Ok((name, FormValue::File(file)));
        }
        Ok((name, FormValue::Text(coerce_string(ctx, value)?)))
    }

    fn value_js<'js>(ctx: &Ctx<'js>, value: &FormValue) -> Result<Value<'js>> {
        match value {
            FormValue::Text(text) => rquickjs::IntoJs::into_js(text.clone(), ctx),
            FormValue::File(file) => {
                Class::instance(ctx.clone(), file.clone()).map(rquickjs::Class::into_value)
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
            Blob::from_inner(Inner::from_bytes(
                bytes,
                format!("multipart/form-data; boundary={boundary}"),
            )),
        )
    }

    fn boundary() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
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
}

#[rquickjs::methods(rename_all = "camelCase")]
impl FormData {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, form: Opt<Value<'js>>) -> Result<Self> {
        if let Some(form) = form.0
            && !form.is_undefined()
            && !form.is_null()
        {
            return Err(Host::throw_type(
                &ctx,
                "Failed to construct 'FormData': HTMLFormElement is not supported",
            ));
        }
        Ok(Self {
            entries: Vec::new(),
        })
    }

    pub fn append<'js>(
        &mut self, ctx: Ctx<'js>, name: Coerced<String>, value: Value<'js>,
        filename: Opt<Value<'js>>,
    ) -> Result<()> {
        let filename = filename.0.and_then(|value| coerce_string(&ctx, value).ok());
        let entry = Self::normalize(&ctx, name.0, value, filename)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn delete(&mut self, name: Coerced<String>) {
        self.entries.retain(|(existing, _)| existing != &name.0);
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, name: Coerced<String>) -> Result<Value<'js>> {
        for (existing, value) in &self.entries {
            if existing == &name.0 {
                return Self::value_js(&ctx, value);
            }
        }
        Ok(Value::new_null(ctx))
    }

    pub fn get_all<'js>(&self, ctx: Ctx<'js>, name: Coerced<String>) -> Result<Vec<Value<'js>>> {
        let mut result = Vec::new();
        for (existing, value) in &self.entries {
            if existing == &name.0 {
                result.push(Self::value_js(&ctx, value)?);
            }
        }
        Ok(result)
    }

    pub fn has(&self, name: Coerced<String>) -> bool {
        self.entries.iter().any(|(existing, _)| existing == &name.0)
    }

    pub fn set<'js>(
        &mut self, ctx: Ctx<'js>, name: Coerced<String>, value: Value<'js>,
        filename: Opt<Value<'js>>,
    ) -> Result<()> {
        let name = name.0;
        let filename = filename.0.and_then(|value| coerce_string(&ctx, value).ok());
        let replacement = Self::normalize(&ctx, name.clone(), value, filename)?;
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
            values.push(Self::value_js(&ctx, value)?);
        }
        Iterable::from(values).into_js(&ctx)
    }

    pub fn entries<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut pairs = Vec::new();
        for (name, value) in &self.entries {
            let pair = rquickjs::Array::new(ctx.clone())?;
            pair.set(0, name.clone())?;
            pair.set(1, Self::value_js(&ctx, value)?)?;
            pairs.push(pair);
        }
        Iterable::from(pairs).into_js(&ctx)
    }

    pub fn for_each<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, callback: Function<'js>,
        this_arg: Opt<Value<'js>>,
    ) -> Result<()> {
        let this_arg = this_arg
            .0
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let form = this.0.borrow();
        for (name, value) in &form.entries {
            let js_value = Self::value_js(&ctx, value)?;
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
    pub const fn to_string_tag() -> &'static str { "FormData" }
}
