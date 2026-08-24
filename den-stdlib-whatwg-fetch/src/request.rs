use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Object,
    Promise, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

use crate::{
    body::{
        apply_body_types, bytes_to_stream, is_readable_stream, is_valid_method, stream_is_locked,
        tee_stream, value_to_bytes,
    },
    headers::{Guard, Headers, is_forbidden_method},
};

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Request<'js> {
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) url:                  String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) method:               String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) credentials:          String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) redirect:             String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) mode:                 String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) cache:                String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) integrity:            String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) destination:          String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) referrer:             String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) referrer_policy:      String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) duplex:               String,
    #[qjs(get, enumerable)]
    pub(crate) keepalive:            bool,
    #[qjs(get, enumerable)]
    pub(crate) is_reload_navigation: bool,
    #[qjs(get, enumerable)]
    pub(crate) is_history_navigation: bool,
    #[qjs(get, enumerable)]
    pub(crate) signal:               Value<'js>,
    follow_source:                    Value<'js>,
    #[qjs(get, enumerable)]
    pub(crate) headers:              Class<'js, Headers>,
    pub(crate) body:                 Option<Value<'js>>,
    pub(crate) body_stream:          Option<Value<'js>>,
    pub(crate) body_used:            bool,
}

impl<'js> Request<'js> {
    pub(crate) fn wrap_input(
        ctx: Ctx<'js>, input: Value<'js>, init: Option<Object<'js>>,
    ) -> Result<Class<'js, Self>> {
        if init.is_none()
            && let Some(object) = input.as_object()
            && let Some(existing) = Class::<Self>::from_object(object)
        {
            return Ok(existing);
        }
        Class::instance(
            ctx.clone(),
            Self::new(ctx, input, Opt(init.map(Object::into_value)))?,
        )
    }

    fn following_signal(ctx: &Ctx<'js>, source: Value<'js>) -> Result<Value<'js>> {
        // The arrow *compiles* even in realms without `den:worker`'s
        // `AbortSignal`; only the call throws, so both phases need the
        // fallback. A source that is already an `AbortSignal` is used as-is:
        // wrapping it in `AbortSignal.any` would pin listener<->source cycles
        // that QuickJS's refcount GC cannot collect.
        let followed = ctx
            .eval::<Function, _>(
                "(source) => source == null ? new AbortSignal() : (source instanceof AbortSignal ? source : AbortSignal.any([source]))",
            )
            .and_then(|follow| follow.call((source.clone(),)));
        match followed {
            Ok(signal) => Ok(signal),
            Err(_) if source.is_null() || source.is_undefined() => {
                Ok(Value::new_null(ctx.clone()))
            }
            Err(_) => Ok(source),
        }
    }

    pub(crate) fn take_body(&mut self, ctx: &Ctx<'_>) -> Result<Option<Value<'js>>> {
        if self.body_used {
            return Err(Exception::throw_type(ctx, "Already read"));
        }
        if self.body.is_none() && self.body_stream.is_none() {
            return Ok(None);
        }
        self.body_used = true;
        Ok(self.body.clone().or_else(|| self.body_stream.clone()))
    }

    fn consume_promise<T, F>(
        request: Class<'js, Request<'js>>, ctx: Ctx<'js>, map: F,
    ) -> Result<Promise<'js>>
    where
        T: IntoJs<'js> + 'js,
        F: FnOnce(Vec<u8>, Ctx<'js>) -> Result<T> + 'js,
    {
        let taken = match request.borrow_mut().take_body(&ctx) {
            Ok(body) => body,
            Err(_) => {
                let thrown = ctx.catch();
                let (promise, _resolve, reject) = ctx.promise()?;
                let _ = reject.call::<_, ()>((thrown,));
                return Ok(promise);
            }
        };
        let (promise, resolve, reject) = ctx.promise()?;
        let ctx_err = ctx.clone();
        ctx.spawn(async move {
            match value_to_bytes(&ctx_err, taken).await {
                Ok(bytes) => match map(bytes, ctx_err.clone()) {
                    Ok(value) => {
                        let _ = resolve.call::<_, ()>((value,));
                    }
                    Err(_) => {
                        let thrown = ctx_err.catch();
                        let _ = reject.call::<_, ()>((thrown,));
                    }
                },
                Err(_) => {
                    let thrown = ctx_err.catch();
                    let _ = reject.call::<_, ()>((thrown,));
                }
            }
        });
        Ok(promise)
    }

    fn normalize_method(method: &str) -> String {
        let upcased = method.to_ascii_uppercase();
        if matches!(
            upcased.as_str(),
            "DELETE" | "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT"
        ) {
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
        Ok(Some(Coerced::<String>::from_js(ctx, value)?.0))
    }

    fn location_href(ctx: &Ctx<'js>) -> String {
        let Ok(location) = ctx.globals().get::<_, Object>("location") else {
            return "http://127.0.0.1/".to_string();
        };
        location
            .get::<_, String>("href")
            .ok()
            .filter(|href| !href.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1/".to_string())
    }

    pub(crate) fn resolve_url(ctx: &Ctx<'js>, input: &str) -> Result<reqwest::Url> {
        let parsed = if let Ok(url) = reqwest::Url::parse(input) {
            url
        } else {
            let base = reqwest::Url::parse(&Self::location_href(ctx)).map_err(|error| {
                Exception::throw_type(ctx, &format!("Invalid base URL: {error}"))
            })?;
            base.join(input)
                .map_err(|error| Exception::throw_type(ctx, &format!("Invalid URL: {error}")))?
        };
        if !parsed.username().is_empty() || !parsed.password().unwrap_or("").is_empty() {
            return Err(Exception::throw_type(
                ctx,
                "Request cannot be constructed from a URL that includes credentials",
            ));
        }
        Ok(parsed)
    }

    fn coerce_url(ctx: &Ctx<'js>, input: Value<'js>) -> Result<String> {
        let text = Coerced::<String>::from_js(ctx, input)?.0;
        let mut url = Self::resolve_url(ctx, &text)?;
        url.set_fragment(None);
        Ok(url.to_string())
    }

    fn parse_enum(
        ctx: &Ctx<'js>, value: &str, allowed: &[&str], name: &str,
    ) -> Result<String> {
        if allowed.contains(&value) {
            Ok(value.to_string())
        } else {
            Err(Exception::throw_type(
                ctx,
                &format!("Invalid {name} value: {value}"),
            ))
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Request<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, input: Value<'js>, options: Opt<Value<'js>>) -> Result<Self> {
        let options = crate::body::optional_object(&ctx, options)?;
        if let Some(object) = options.as_ref() {
            let window: Value = object.get("window")?;
            if !window.is_undefined() && !window.is_null() {
                return Err(Exception::throw_type(&ctx, "RequestInit.window is not null"));
            }
        }

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
        let mut mode = "cors".to_string();
        let mut cache = "default".to_string();
        let mut integrity = String::new();
        let mut referrer = "about:client".to_string();
        let mut referrer_policy = String::new();
        let mut duplex = "half".to_string();
        let mut keepalive = false;
        let mut signal = Value::new_null(ctx.clone());
        let mut copied_headers = None;
        let mut copied_from_request = false;
        let mut copied_stream = None;

        if let Some(object) = input.as_object()
            && let Some(existing) = Class::<Request>::from_object(object)
        {
            let existing = existing.borrow();
            if existing.body_used && body.is_none() {
                return Err(Exception::throw_type(&ctx, "Already read"));
            }
            url = existing.url.clone();
            credentials = existing.credentials.clone();
            redirect = existing.redirect.clone();
            method = existing.method.clone();
            mode = existing.mode.clone();
            cache = existing.cache.clone();
            integrity = existing.integrity.clone();
            referrer = existing.referrer.clone();
            referrer_policy = existing.referrer_policy.clone();
            duplex = existing.duplex.clone();
            keepalive = existing.keepalive;
            signal = existing.signal.clone();
            copied_headers = Some(existing.headers.clone().into_value());
            copied_from_request = true;
        }
        let existing_has_body = copied_from_request
            && input.as_object().is_some_and(|object| {
                Class::<Request>::from_object(object).is_some_and(|existing| {
                    let existing = existing.borrow();
                    existing.body.is_some() || existing.body_stream.is_some()
                })
            });
        if !copied_from_request {
            url = Self::coerce_url(&ctx, input.clone())?;
        }

        if let Some(object) = options.as_ref() {
            if let Some(value) = Self::optional_string(&ctx, object, "credentials")? {
                credentials = Self::parse_enum(
                    &ctx,
                    &value,
                    &["omit", "same-origin", "include"],
                    "credentials",
                )?;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "redirect")? {
                redirect =
                    Self::parse_enum(&ctx, &value, &["follow", "error", "manual"], "redirect")?;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "method")? {
                if !is_valid_method(&value) {
                    return Err(Exception::throw_type(&ctx, "Invalid request method"));
                }
                method = value;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "mode")? {
                if value == "navigate" {
                    return Err(Exception::throw_type(&ctx, "Request mode navigate is invalid"));
                }
                mode = Self::parse_enum(&ctx, &value, &["same-origin", "no-cors", "cors"], "mode")?;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "cache")? {
                cache = Self::parse_enum(
                    &ctx,
                    &value,
                    &[
                        "default",
                        "no-store",
                        "reload",
                        "no-cache",
                        "force-cache",
                        "only-if-cached",
                    ],
                    "cache",
                )?;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "integrity")? {
                integrity = value;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "referrer")? {
                if value != "about:client" && value != "" {
                    let parsed = Self::resolve_url(&ctx, &value).map_err(|_| {
                        Exception::throw_type(&ctx, "RequestInit.referrer is invalid")
                    })?;
                    referrer = parsed.to_string();
                } else {
                    referrer = if value.is_empty() {
                        "no-referrer".to_string()
                    } else {
                        value
                    };
                }
            }
            if let Some(value) = Self::optional_string(&ctx, object, "referrerPolicy")? {
                referrer_policy = Self::parse_enum(
                    &ctx,
                    &value,
                    &[
                        "",
                        "no-referrer",
                        "no-referrer-when-downgrade",
                        "same-origin",
                        "origin",
                        "strict-origin",
                        "origin-when-cross-origin",
                        "strict-origin-when-cross-origin",
                        "unsafe-url",
                    ],
                    "referrerPolicy",
                )?;
            }
            if let Some(value) = Self::optional_string(&ctx, object, "duplex")? {
                duplex = Self::parse_enum(&ctx, &value, &["half"], "duplex")?;
            }
            let keepalive_value: Value = object.get("keepalive")?;
            if !keepalive_value.is_undefined() {
                keepalive = Self::is_truthy(&keepalive_value);
            }
            let signal_value: Value = object.get("signal")?;
            if !signal_value.is_undefined() {
                signal = signal_value;
            }
        }

        method = Self::normalize_method(&method);
        if is_forbidden_method(&method) {
            return Err(Exception::throw_type(&ctx, "Forbidden method"));
        }
        if mode == "no-cors" && !matches!(method.as_str(), "GET" | "HEAD" | "POST") {
            return Err(Exception::throw_type(
                &ctx,
                "no-cors mode only allows GET, HEAD, POST",
            ));
        }
        if cache == "only-if-cached" && mode != "same-origin" {
            return Err(Exception::throw_type(
                &ctx,
                "only-if-cached requires same-origin mode",
            ));
        }

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
        let headers = Class::instance(
            ctx.clone(),
            Headers::from_init(
                ctx.clone(),
                headers_init,
                if mode == "no-cors" {
                    Guard::RequestNoCors
                } else {
                    Guard::Request
                },
            )?,
        )?;

        if (method == "GET" || method == "HEAD")
            && (body.as_ref().is_some_and(Self::is_truthy) || existing_has_body && body.is_none())
        {
            return Err(Exception::throw_type(
                &ctx,
                "Body not allowed for GET or HEAD requests",
            ));
        }

        if copied_from_request
            && body.is_none()
            && let Some(object) = input.as_object()
            && let Some(existing) = Class::<Request>::from_object(object)
        {
            let stream = existing.borrow().body_stream.clone();
            if let Some(stream) = stream {
                let (left, right) = tee_stream(&ctx, stream)?;
                let mut existing = existing.borrow_mut();
                existing.body_used = true;
                existing.body_stream = Some(left);
                copied_stream = Some(right);
                body = None;
            } else if existing_has_body {
                let mut existing = existing.borrow_mut();
                existing.body_used = true;
                body = existing.body.clone();
            }
        }

        let mut body_stream = copied_stream;
        if let Some(value) = body.take()
            && !value.is_null()
            && !value.is_undefined()
        {
            if is_readable_stream(&ctx, &value)? {
                if stream_is_locked(&value) {
                    return Err(Exception::throw_type(&ctx, "ReadableStream is locked"));
                }
                if let Some(object) = options.as_ref() {
                    let duplex_value: Value = object.get("duplex")?;
                    if !duplex_value.is_undefined() && !duplex_value.is_null() {
                        let text = Coerced::<String>::from_js(&ctx, duplex_value)?.0;
                        duplex = Self::parse_enum(&ctx, &text, &["half"], "duplex")?;
                    }
                }
                body_stream = Some(value.clone());
                body = Some(value);
            } else {
                body = Some(apply_body_types(&ctx, &headers, value)?);
            }
        }

        Ok(Self {
            url,
            method,
            credentials,
            redirect,
            mode,
            cache,
            integrity,
            destination: String::new(),
            referrer,
            referrer_policy,
            duplex,
            keepalive,
            is_reload_navigation: false,
            is_history_navigation: false,
            follow_source: signal.clone(),
            signal: Self::following_signal(&ctx, signal)?,
            headers,
            body,
            body_stream,
            body_used: false,
        })
    }

    #[qjs(get, enumerable)]
    pub fn body_used(&self) -> bool { self.body_used }

    #[qjs(get, enumerable)]
    pub fn body(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut request = this.0.borrow_mut();
        if let Some(stream) = &request.body_stream {
            return Ok(stream.clone());
        }
        let Some(body) = request.body.clone() else {
            return Ok(Value::new_null(ctx));
        };
        if body.is_null() || body.is_undefined() {
            return Ok(Value::new_null(ctx));
        }
        if is_readable_stream(&ctx, &body)? {
            request.body_stream = Some(body.clone());
            return Ok(body);
        }
        if let Some(string) = body.as_string() {
            let stream = bytes_to_stream(&ctx, string.to_string()?.as_bytes())?;
            request.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        if let Ok(buffer) = ArrayBuffer::from_js(&ctx, body.clone()) {
            let bytes = buffer.as_bytes().unwrap_or(&[]).to_vec();
            let stream = bytes_to_stream(&ctx, &bytes)?;
            request.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        if let Some(object) = body.as_object()
            && let Some(blob) = Class::<den_stdlib_whatwg::blob::Blob>::from_object(object)
        {
            let stream = bytes_to_stream(&ctx, blob.borrow().bytes())?;
            request.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        Ok(Value::new_null(ctx))
    }

    pub fn array_buffer(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| ArrayBuffer::new_copy(ctx, bytes))
    }

    pub fn bytes(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| {
            rquickjs::TypedArray::new_copy(ctx, bytes)
        })
    }

    pub fn text(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, _ctx| Ok(crate::body::utf8_text(&bytes)))
    }

    pub fn json(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| {
            crate::body::parse_json_js(&ctx, &bytes)
        })
    }

    pub fn text_stream(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        let mut request = this.0.borrow_mut();
        if request.body_used {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        request.body_used = true;
        let taken = request.body.take().or_else(|| request.body_stream.take());
        drop(request);
        if let Some(value) = taken {
            if let Some(string) = value.as_string() {
                return crate::body::text_to_stream(&ctx, &string.to_string()?);
            }
        }
        crate::body::text_to_stream(&ctx, "")
    }

    pub fn blob(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let mime = this
            .0
            .borrow()
            .headers
            .borrow()
            .map
            .get("content-type")
            .cloned()
            .unwrap_or_default();
        Self::consume_promise(this.0, ctx, move |bytes, ctx| {
            crate::body::blob_from_bytes(&ctx, bytes, &mime.to_ascii_lowercase())
        })
    }

    pub fn form_data(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        let (no_body, mime) = {
            let request = this.0.borrow();
            (
                request.body.is_none() && request.body_stream.is_none(),
                request
                    .headers
                    .borrow()
                    .map
                    .get("content-type")
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        if no_body
            && !mime
                .get(..33)
                .is_some_and(|head| head.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        {
            let error = Exception::throw_type(&ctx, "Failed to parse body as FormData");
            let _ = error;
            let thrown = ctx.catch();
            let (promise, _resolve, reject) = ctx.promise()?;
            let _ = reject.call::<_, ()>((thrown,));
            return Ok(promise);
        }
        Self::consume_promise(this.0, ctx, move |bytes, ctx| {
            let ctor: rquickjs::function::Constructor = ctx
                .globals()
                .get("FormData")
                .map_err(|_| Exception::throw_type(&ctx, "FormData is not defined"))?;
            let form: Object = ctor.construct(())?;
            crate::FormBody.parse_into(&ctx, &form, &bytes, &mime)?;
            Ok(form.into_value())
        })
    }

    #[qjs(rename = "clone")]
    pub fn clone_request(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let mut request = this.0.borrow_mut();
        if request.body_used {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        let (body, body_stream) = if let Some(stream) = &request.body_stream {
            let (left, right) = tee_stream(&ctx, stream.clone())?;
            request.body_stream = Some(left);
            (request.body.clone(), Some(right))
        } else {
            (request.body.clone(), None)
        };
        let follow_source = if request.follow_source.is_null()
            || request.follow_source.is_undefined()
        {
            request.signal.clone()
        } else {
            request.follow_source.clone()
        };
        Ok(Self {
            url:                   request.url.clone(),
            method:                request.method.clone(),
            credentials:           request.credentials.clone(),
            redirect:              request.redirect.clone(),
            mode:                  request.mode.clone(),
            cache:                 request.cache.clone(),
            integrity:             request.integrity.clone(),
            destination:           request.destination.clone(),
            referrer:              request.referrer.clone(),
            referrer_policy:       request.referrer_policy.clone(),
            duplex:                request.duplex.clone(),
            keepalive:             request.keepalive,
            is_reload_navigation:  request.is_reload_navigation,
            is_history_navigation: request.is_history_navigation,
            follow_source:         follow_source.clone(),
            signal:                Self::following_signal(&ctx, follow_source)?,
            headers:               Class::instance(
                ctx.clone(),
                Headers::new(ctx.clone(), Opt(Some(request.headers.clone().into_value())))?,
            )?,
            body,
            body_stream,
            body_used:             false,
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Request" }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        self.body = None;
        self.body_stream = None;
    }
}
