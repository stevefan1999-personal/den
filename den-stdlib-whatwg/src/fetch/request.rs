use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Exception, Function, IntoJs, JsLifetime, Object, Promise,
    Result, TypedArray, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

pub struct ServerRequest<'js> {
    pub url:     String,
    pub method:  String,
    pub headers: Vec<(String, String)>,
    pub body:    Vec<u8>,
    pub signal:  Value<'js>,
}

use super::{
    body::{
        apply_body_types, is_readable_stream, is_valid_method, stream_is_locked, tee_stream,
        value_as_body_stream, value_to_bytes,
    },
    headers::{Guard, Headers, is_forbidden_method},
};

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Request<'js> {
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) url:                   String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) method:                String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) credentials:           String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) redirect:              String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) mode:                  String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) cache:                 String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) integrity:             String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) destination:           String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) referrer:              String,
    #[qjs(get, enumerable, rename = "referrerPolicy", skip_trace)]
    pub(crate) referrer_policy:       String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) duplex:                String,
    #[qjs(get, enumerable)]
    pub(crate) keepalive:             bool,
    #[qjs(get, enumerable, rename = "isReloadNavigation")]
    pub(crate) is_reload_navigation:  bool,
    #[qjs(get, enumerable, rename = "isHistoryNavigation")]
    pub(crate) is_history_navigation: bool,
    #[qjs(get, enumerable)]
    pub(crate) signal:                Value<'js>,
    follow_source:                    Value<'js>,
    #[qjs(get, enumerable)]
    pub(crate) headers:               Class<'js, Headers>,
    pub(crate) body:                  Option<Value<'js>>,
    pub(crate) body_stream:           Option<Value<'js>>,
    pub(crate) body_used:             bool,
}

impl<'js> Request<'js> {
    pub fn from_server(ctx: &Ctx<'js>, request: ServerRequest<'js>) -> Result<Self> {
        let mut headers = Headers::from_pairs(request.headers);
        headers.set_guard(Guard::Request);
        let body = if request.body.is_empty() {
            None
        } else {
            Some(TypedArray::<u8>::new_copy(ctx.clone(), request.body)?.into_value())
        };
        let signal = request.signal;
        Ok(Self {
            url: request.url,
            method: request.method,
            credentials: "same-origin".to_string(),
            redirect: "manual".to_string(),
            mode: "same-origin".to_string(),
            cache: "no-store".to_string(),
            integrity: String::new(),
            destination: String::new(),
            referrer: "no-referrer".to_string(),
            referrer_policy: String::new(),
            duplex: "half".to_string(),
            keepalive: false,
            is_reload_navigation: false,
            is_history_navigation: false,
            signal: signal.clone(),
            follow_source: signal,
            headers: Class::instance(ctx.clone(), headers)?,
            body,
            body_stream: None,
            body_used: false,
        })
    }

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
        let fallback = || {
            if source.is_null() || source.is_undefined() {
                Value::new_null(ctx.clone())
            } else {
                source.clone()
            }
        };
        let ctor: Function = if let Ok(ctor) = ctx.globals().get("AbortSignal") {
            ctor
        } else {
            drop(ctx.catch());
            return Ok(fallback());
        };
        let any: Function = if let Ok(any) = ctor.get("any") {
            any
        } else {
            drop(ctx.catch());
            return Ok(fallback());
        };
        let sources = Array::new(ctx.clone())?;
        if !source.is_null() && !source.is_undefined() {
            sources.set(0, source.clone())?;
        }
        any.call((This(ctor.into_value()), sources))
    }

    fn is_body_used(&self) -> bool {
        self.body_used
            || self
                .body_stream
                .as_ref()
                .is_some_and(super::body::stream_is_disturbed)
    }

    pub(crate) fn take_body(&mut self, ctx: &Ctx<'_>) -> Result<Option<Value<'js>>> {
        if self.is_body_used() {
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
        let Ok(taken) = request.borrow_mut().take_body(&ctx) else {
            let thrown = ctx.catch();
            let (promise, _resolve, reject) = ctx.promise()?;
            let _ = reject.call::<_, ()>((thrown,));
            return Ok(promise);
        };
        let (promise, resolve, reject) = ctx.promise()?;
        let ctx_err = ctx.clone();
        ctx.spawn(async move {
            if let Ok(bytes) = value_to_bytes(&ctx_err, taken).await {
                if let Ok(value) = map(bytes, ctx_err.clone()) {
                    let _ = resolve.call::<_, ()>((value,));
                } else {
                    let thrown = ctx_err.catch();
                    let _ = reject.call::<_, ()>((thrown,));
                }
            } else {
                let thrown = ctx_err.catch();
                let _ = reject.call::<_, ()>((thrown,));
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
            return string.to_string().map_or(true, |text| !text.is_empty());
        }
        true
    }

    fn optional_string(ctx: &Ctx<'js>, object: &Object<'js>, key: &str) -> Result<Option<String>> {
        let value: Value = object.get(key)?;
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }
        Ok(Some(den_util::coerce_string(ctx, value)?))
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
        let text = den_util::coerce_string(ctx, input)?;
        let mut url = Self::resolve_url(ctx, &text)?;
        url.set_fragment(None);
        Ok(url.to_string())
    }

    fn parse_enum(ctx: &Ctx<'js>, value: &str, allowed: &[&str], name: &str) -> Result<String> {
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
        let options = super::body::optional_object(&ctx, options)?;
        if let Some(object) = options.as_ref() {
            let window: Value = object.get("window")?;
            if !window.is_undefined() && !window.is_null() {
                return Err(Exception::throw_type(
                    &ctx,
                    "RequestInit.window is not null",
                ));
            }
        }

        let mut body = match options.as_ref() {
            Some(object) => {
                let value: Value = object.get("body")?;
                if value.is_undefined() || value.is_null() {
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
        let source_request = if let Some(object) = input.as_object()
            && let Some(existing) = Class::<Request>::from_object(object)
        {
            let source = existing.clone();
            let existing = existing.borrow();
            if existing.is_body_used() && body.is_none() {
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
            signal = if existing.follow_source.is_null() || existing.follow_source.is_undefined() {
                existing.signal.clone()
            } else {
                existing.follow_source.clone()
            };
            copied_headers = Some(existing.headers.clone().into_value());
            drop(existing);
            Some(source)
        } else {
            None
        };
        let source_has_body = source_request.as_ref().is_some_and(|existing| {
            let existing = existing.borrow();
            existing.body.is_some() || existing.body_stream.is_some()
        });
        if source_request.is_none() {
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
                    return Err(Exception::throw_type(
                        &ctx,
                        "Request mode navigate is invalid",
                    ));
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
                if value != "about:client" && !value.is_empty() {
                    let parsed = Self::resolve_url(&ctx, &value).map_err(|_error| {
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
            if let Some(value) = Self::optional_string(&ctx, object, "priority")? {
                Self::parse_enum(&ctx, &value, &["high", "low", "auto"], "priority")?;
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
                if value.is_undefined() {
                    copied_headers
                } else {
                    Some(value)
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

        let body_override = body.is_some();
        if (method == "GET" || method == "HEAD") && (body_override || source_has_body) {
            return Err(Exception::throw_type(
                &ctx,
                "Body not allowed for GET or HEAD requests",
            ));
        }

        let mut body_stream = None;
        if let Some(value) = body.take() {
            if is_readable_stream(&ctx, &value) {
                if stream_is_locked(&value) || super::body::stream_is_disturbed(&value) {
                    return Err(Exception::throw_type(
                        &ctx,
                        "ReadableStream is locked or disturbed",
                    ));
                }
                // A stream body is sent as it is produced, which is what
                // `duplex: "half"` opts into. Without that opt-in the request
                // is not the one the caller asked for, so refuse it rather
                // than quietly buffering.
                let declared = options
                    .as_ref()
                    .map(|object| object.get::<_, Value>("duplex"))
                    .transpose()?
                    .filter(|value| !value.is_undefined() && !value.is_null());
                let Some(declared) = declared else {
                    return Err(Exception::throw_type(
                        &ctx,
                        "a ReadableStream body requires duplex: 'half'",
                    ));
                };
                let text = den_util::coerce_string(&ctx, declared)?;
                duplex = Self::parse_enum(&ctx, &text, &["half"], "duplex")?;
                body_stream = Some(value.clone());
                body = Some(value);
            } else {
                body = Some(apply_body_types(&ctx, &headers, value)?);
            }
        }
        let follow_source = signal;
        let signal = Self::following_signal(&ctx, follow_source.clone())?;

        if source_has_body && let Some(existing) = source_request {
            if !body_override {
                let (source_body, source_stream) = {
                    let existing = existing.borrow();
                    (existing.body.clone(), existing.body_stream.clone())
                };
                if let Some(stream) = source_stream {
                    if stream_is_locked(&stream) || super::body::stream_is_disturbed(&stream) {
                        return Err(Exception::throw_type(
                            &ctx,
                            "ReadableStream is locked or disturbed",
                        ));
                    }
                    let (_unused, copied) = tee_stream(&ctx, stream)?;
                    body_stream = Some(copied);
                    body = None;
                } else {
                    body = source_body;
                }
            }
            existing.borrow_mut().body_used = true;
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
            signal,
            follow_source,
            headers,
            body,
            body_stream,
            body_used: false,
        })
    }

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
        let stream = value_as_body_stream(&ctx, body)?;
        request.body_stream = Some(stream.clone());
        Ok(stream)
    }

    #[qjs(get, enumerable, rename = "bodyUsed")]
    pub fn body_used(&self) -> bool { self.is_body_used() }

    pub fn array_buffer(
        this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| ArrayBuffer::new_copy(ctx, bytes))
    }

    pub fn bytes(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| {
            rquickjs::TypedArray::new_copy(ctx, bytes)
        })
    }

    pub fn text(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, _ctx| {
            Ok(super::body::utf8_text(&bytes))
        })
    }

    pub fn json(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| {
            super::body::parse_json_js(&ctx, &bytes)
        })
    }

    pub fn text_stream(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut request = this.0.borrow_mut();
        if request.body.is_none() && request.body_stream.is_none() {
            return super::body::text_to_stream(&ctx, "");
        }
        if request.is_body_used() || request.body_stream.as_ref().is_some_and(stream_is_locked) {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        let stream = match request.body_stream.clone() {
            Some(stream) => stream,
            None => {
                value_as_body_stream(
                    &ctx,
                    request
                        .body
                        .clone()
                        .ok_or_else(|| Exception::throw_type(&ctx, "Body is unavailable"))?,
                )?
            }
        };
        if stream_is_locked(&stream) || super::body::stream_is_disturbed(&stream) {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        if let Some(object) = stream.as_object()
            && let Some(readable) = Class::<crate::streams::ReadableStream>::from_object(object)
        {
            crate::streams::ReadableStream::lock_for_consume(&readable, &ctx)?;
        }
        request.body_used = true;
        request.body_stream = Some(stream.clone());
        drop(request);
        super::body::text_stream_from_byte_stream(&ctx, stream)
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
            super::body::blob_from_bytes(&ctx, bytes, &mime.to_ascii_lowercase())
        })
    }

    pub fn form_data(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
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
                .map_err(|_error| Exception::throw_type(&ctx, "FormData is not defined"))?;
            let form: Object = ctor.construct(())?;
            super::FormBody::parse_into(&ctx, &form, &bytes, &mime)?;
            Ok(form.into_value())
        })
    }

    #[qjs(rename = "clone")]
    pub fn clone_request(this: This<Class<'js, Request<'js>>>, ctx: Ctx<'js>) -> Result<Self> {
        let mut request = this.0.borrow_mut();
        if request.is_body_used() {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        let (body, body_stream) = if let Some(stream) = &request.body_stream {
            let (left, right) = tee_stream(&ctx, stream.clone())?;
            request.body_stream = Some(left);
            (request.body.clone(), Some(right))
        } else {
            (request.body.clone(), None)
        };
        let follow_source =
            if request.follow_source.is_null() || request.follow_source.is_undefined() {
                request.signal.clone()
            } else {
                request.follow_source.clone()
            };
        let signal = Self::following_signal(&ctx, follow_source.clone())?;
        let guard = request.headers.borrow().guard();
        Ok(Self {
            url: request.url.clone(),
            method: request.method.clone(),
            credentials: request.credentials.clone(),
            redirect: request.redirect.clone(),
            mode: request.mode.clone(),
            cache: request.cache.clone(),
            integrity: request.integrity.clone(),
            destination: request.destination.clone(),
            referrer: request.referrer.clone(),
            referrer_policy: request.referrer_policy.clone(),
            duplex: request.duplex.clone(),
            keepalive: request.keepalive,
            is_reload_navigation: request.is_reload_navigation,
            is_history_navigation: request.is_history_navigation,
            signal,
            follow_source,
            headers: Class::instance(
                ctx.clone(),
                Headers::from_init(
                    ctx.clone(),
                    Some(request.headers.clone().into_value()),
                    guard,
                )?,
            )?,
            body,
            body_stream,
            body_used: false,
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Request" }
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        self.body = None;
        self.body_stream = None;
    }
}
