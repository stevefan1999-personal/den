use rquickjs::{
    Array, ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime,
    Object, Result, Symbol, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
    promise::MaybePromise,
};

use crate::{SerdeJsonValue, headers::Headers};

const METHODS: [&str; 9] = [
    "CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE",
];

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Request<'js> {
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) url: String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) method: String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) credentials: String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) redirect: String,
    #[qjs(get, enumerable)]
    pub(crate) mode: Value<'js>,
    #[qjs(get, enumerable)]
    pub(crate) signal: Value<'js>,
    #[qjs(get, enumerable)]
    pub(crate) referrer: Value<'js>,
    #[qjs(get, enumerable)]
    pub(crate) headers: Class<'js, Headers>,
    pub(crate) body: Option<Value<'js>>,
    pub(crate) body_used: bool,
}

impl<'js> Request<'js> {
    pub(crate) fn wrap_input(
        ctx: Ctx<'js>,
        input: Value<'js>,
        init: Option<Object<'js>>,
    ) -> Result<Class<'js, Self>> {
        if init.is_none()
            && let Some(object) = input.as_object()
            && let Some(existing) = Class::<Self>::from_object(object)
        {
            return Ok(existing);
        }
        Class::instance(ctx.clone(), Self::new(ctx, input, Opt(init))?)
    }

    pub(crate) fn take_body(&mut self, ctx: &Ctx<'_>) -> Result<Option<Value<'js>>> {
        if self.body_used {
            return Err(Exception::throw_type(ctx, "Already read"));
        }
        self.body_used = true;
        Ok(self.body.take())
    }

    pub(crate) async fn body_to_bytes(ctx: &Ctx<'js>, body: Option<Value<'js>>) -> Result<Vec<u8>> {
        let Some(body) = body.filter(|value| !value.is_null() && !value.is_undefined()) else {
            return Ok(Vec::new());
        };
        if let Some(string) = body.as_string() {
            return Ok(string.to_string()?.into_bytes());
        }
        if let Ok(buffer) = ArrayBuffer::from_js(ctx, body.clone()) {
            return copy_buffer(ctx, buffer.as_bytes());
        }
        if is_array_buffer_view(ctx, &body)? {
            return copy_view(ctx, &body);
        }
        if let Some(object) = body.as_object() {
            let method: Value = object.get("arrayBuffer")?;
            if method.is_function() {
                let produced: Value =
                    Function::from_js(ctx, method)?.call((This(object.clone()),))?;
                let resolved = MaybePromise::from_js(ctx, produced)?
                    .into_future::<Value>()
                    .await?;
                return Box::pin(Self::body_to_bytes(ctx, Some(resolved))).await;
            }
        }
        Ok(Coerced::<String>::from_js(ctx, body)?.0.into_bytes())
    }

    fn normalize_method(method: &str) -> String {
        let upcased = method.to_ascii_uppercase();
        if METHODS.contains(&upcased.as_str()) {
            upcased
        } else {
            method.to_string()
        }
    }

    fn is_truthy(value: &Value<'js>) -> bool {
        if value.is_null() || value.is_undefined() {
            return false;
        }
        if let Some(flag) = value.as_bool() {
            return flag;
        }
        if let Some(number) = value.as_number() {
            return number != 0.0 && !number.is_nan();
        }
        if let Some(string) = value.as_string() {
            return string
                .to_string()
                .map(|text| !text.is_empty())
                .unwrap_or(true);
        }
        true
    }

    fn optional_string(ctx: &Ctx<'js>, object: &Object<'js>, key: &str) -> Result<Option<String>> {
        let value: Value = object.get(key)?;
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }
        let text = Coerced::<String>::from_js(ctx, value)?.0;
        Ok(if text.is_empty() { None } else { Some(text) })
    }

    fn coerce_url(ctx: &Ctx<'js>, input: Value<'js>) -> Result<String> {
        Ok(Coerced::<String>::from_js(ctx, input)?.0)
    }

    fn apply_body_types(
        ctx: &Ctx<'js>,
        headers: &Class<'js, Headers>,
        mut body: Value<'js>,
    ) -> Result<Value<'js>> {
        if let Some(object) = body.as_object()
            && is_instance_of_global(ctx, object, "URLSearchParams")?
        {
            let text: String = object
                .get::<_, Function>("toString")?
                .call((This(object.clone()),))?;
            body = text.into_js(ctx)?;
            set_content_type_if_missing(
                headers,
                "application/x-www-form-urlencoded;charset=UTF-8",
            )?;
        }
        if let Some(object) = body.as_object() {
            let key = Symbol::new_global(ctx.clone(), "den.toMultipartBlob")?;
            let converter: Value = object.get(key)?;
            if converter.is_function() {
                body = Function::from_js(ctx, converter)?.call((This(object.clone()),))?;
            }
        }
        if body.as_string().is_some() {
            set_content_type_if_missing(headers, "text/plain;charset=UTF-8")?;
        } else if let Some(object) = body.as_object()
            && is_instance_of_global(ctx, object, "Blob")?
        {
            let mime: Value = object.get("type")?;
            if let Some(mime) = mime.as_string() {
                let mime = mime.to_string()?;
                if !mime.is_empty() {
                    set_content_type_if_missing(headers, &mime)?;
                }
            }
        }
        Ok(body)
    }
}

fn is_instance_of_global<'js>(ctx: &Ctx<'js>, object: &Object<'js>, name: &str) -> Result<bool> {
    let Ok(ctor) = ctx.globals().get::<_, Value>(name) else {
        return Ok(false);
    };
    Ok(ctor.is_function() && object.is_instance_of(&ctor))
}

fn set_content_type_if_missing(headers: &Class<'_, Headers>, value: &str) -> Result<()> {
    let mut headers = headers.borrow_mut();
    if !headers.map.contains_key("content-type") {
        headers.map.insert("content-type".into(), value.to_string());
    }
    Ok(())
}

fn is_array_buffer_view<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    let Ok(array_buffer) = ctx.globals().get::<_, Object>("ArrayBuffer") else {
        return Ok(false);
    };
    let Ok(is_view) = array_buffer.get::<_, Function>("isView") else {
        return Ok(false);
    };
    is_view.call((value.clone(),))
}

fn copy_buffer(ctx: &Ctx<'_>, bytes: Option<&[u8]>) -> Result<Vec<u8>> {
    bytes
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Exception::throw_type(ctx, "buffer is detached"))
}

fn copy_view<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
    let Some(object) = value.as_object() else {
        return Ok(Vec::new());
    };
    let buffer: ArrayBuffer = object.get("buffer")?;
    let offset: usize = object.get("byteOffset").unwrap_or(0);
    let length: usize = object.get("byteLength").unwrap_or(0);
    let bytes = buffer
        .as_bytes()
        .ok_or_else(|| Exception::throw_type(ctx, "buffer is detached"))?;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| Exception::throw_range(ctx, "view is out of bounds"))?;
    Ok(bytes[offset..end].to_vec())
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Request<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, input: Value<'js>, options: Opt<Object<'js>>) -> Result<Self> {
        let options = options.0;
        let mut body = match options.as_ref() {
            Some(object) => {
                let value: Value = object.get("body")?;
                if value.is_undefined() {
                    None
                } else {
                    Some(value)
                }
            }
            None => None,
        };

        let mut url = String::new();
        let mut credentials = "same-origin".to_string();
        let mut redirect = "follow".to_string();
        let mut method = "GET".to_string();
        let mut mode = Value::new_null(ctx.clone());
        let mut signal = Value::new_null(ctx.clone());
        let mut copied_headers = None;
        let mut copied_from_request = false;

        if let Some(object) = input.as_object()
            && let Some(existing) = Class::<Request>::from_object(object)
        {
            let existing = existing.borrow();
            if existing.body_used {
                return Err(Exception::throw_type(&ctx, "Already read"));
            }
            url = existing.url.clone();
            credentials = existing.credentials.clone();
            redirect = existing.redirect.clone();
            method = existing.method.clone();
            mode = existing.mode.clone();
            signal = existing.signal.clone();
            copied_headers = Some(existing.headers.clone().into_value());
            if body.is_none() {
                body = existing.body.clone();
            }
            copied_from_request = true;
        }
        if !copied_from_request {
            url = Self::coerce_url(&ctx, input)?;
        }

        if let Some(object) = options.as_ref() {
            if let Some(value) = Self::optional_string(&ctx, object, "credentials")? {
                credentials = value;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "redirect")? {
                redirect = value;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "method")? {
                method = value;
            }
            let mode_value: Value = object.get("mode")?;
            if Self::is_truthy(&mode_value) {
                mode = mode_value;
            }
            let signal_value: Value = object.get("signal")?;
            if Self::is_truthy(&signal_value) {
                signal = signal_value;
            }
        }

        method = Self::normalize_method(&method);

        let headers_init = match options.as_ref() {
            Some(object) => {
                let value: Value = object.get("headers")?;
                if Self::is_truthy(&value) {
                    Some(value)
                } else {
                    copied_headers
                }
            }
            None => copied_headers,
        };
        let headers = Class::instance(ctx.clone(), Headers::new(ctx.clone(), Opt(headers_init))?)?;

        if (method == "GET" || method == "HEAD") && body.as_ref().is_some_and(Self::is_truthy) {
            return Err(Exception::throw_type(
                &ctx,
                "Body not allowed for GET or HEAD requests",
            ));
        }

        if let Some(value) = body.take()
            && !value.is_null()
            && !value.is_undefined()
        {
            body = Some(Self::apply_body_types(&ctx, &headers, value)?);
        }

        Ok(Self {
            url,
            method,
            credentials,
            redirect,
            mode,
            signal,
            referrer: Value::new_null(ctx),
            headers,
            body,
            body_used: false,
        })
    }

    #[qjs(get, enumerable)]
    pub fn body_used(&self) -> bool {
        self.body_used
    }

    #[qjs(get, enumerable)]
    pub fn body(&self, ctx: Ctx<'js>) -> Value<'js> {
        Value::new_null(ctx)
    }

    pub async fn array_buffer(
        this: This<Class<'js, Request<'js>>>,
        ctx: Ctx<'js>,
    ) -> Result<ArrayBuffer<'js>> {
        let body = this.0.borrow_mut().take_body(&ctx)?;
        let bytes = Self::body_to_bytes(&ctx, body).await?;
        ArrayBuffer::new_copy(ctx, bytes)
    }

    pub async fn text(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<String> {
        let buffer = Self::array_buffer(This(this.0), ctx.clone()).await?;
        Ok(String::from_utf8_lossy(buffer.as_bytes().unwrap_or(&[])).into_owned())
    }

    pub async fn json(
        this: This<Class<'js, Request<'js>>>,
        ctx: Ctx<'js>,
    ) -> Result<SerdeJsonValue> {
        let text = Self::text(This(this.0), ctx.clone()).await?;
        serde_json::from_str::<serde_json::Value>(&text)
            .map(Into::into)
            .map_err(|err| Exception::throw_syntax(&ctx, &format!("{err:?}")))
    }

    pub async fn blob(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mime = this
            .0
            .borrow()
            .headers
            .borrow()
            .map
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        let buffer = Self::array_buffer(This(this.0), ctx.clone()).await?;
        let ctor: rquickjs::function::Constructor = ctx
            .globals()
            .get("Blob")
            .map_err(|_| Exception::throw_type(&ctx, "Blob is not defined"))?;
        let parts = Array::new(ctx.clone())?;
        parts.set(0, buffer)?;
        let opts = Object::new(ctx.clone())?;
        opts.set("type", mime)?;
        ctor.construct((parts, opts))
    }

    #[qjs(rename = "clone")]
    pub fn clone_request(&self, ctx: Ctx<'js>) -> Result<Self> {
        if self.body_used {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        Ok(Self {
            url: self.url.clone(),
            method: self.method.clone(),
            credentials: self.credentials.clone(),
            redirect: self.redirect.clone(),
            mode: self.mode.clone(),
            signal: self.signal.clone(),
            referrer: Value::new_null(ctx.clone()),
            headers: Class::instance(
                ctx.clone(),
                Headers::new(ctx.clone(), Opt(Some(self.headers.clone().into_value())))?,
            )?,
            body: self.body.clone(),
            body_used: false,
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Request"
    }
}
