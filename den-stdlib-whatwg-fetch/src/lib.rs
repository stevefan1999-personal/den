use std::{
    cell::{Cell, RefCell},
    pin::Pin,
    rc::Rc,
    sync::Arc,
};

use derive_more::derive::{From, Into};
use futures::{Stream, StreamExt};
use den_stdlib_whatwg::streams::ReadableStream;
use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Error, Exception, FromJs, Function, IntoJs, JsLifetime, Object,
    Promise, Result, TypedArray, Value as JsValue,
    class::Trace,
    function::{Constructor, This},
};
use serde_json::Value;
use tokio::sync::Notify;

mod body;
mod data_url;
mod fetch_op;
mod headers;
mod request;

pub use headers::Headers;
pub use request::Request;

#[derive(From, Into, Clone, Eq, PartialEq, Hash)]
pub struct SerdeJsonValue(pub serde_json::Value);

impl<'js> FromJs<'js> for SerdeJsonValue {
    fn from_js(ctx: &Ctx<'js>, v: JsValue<'js>) -> Result<Self> {
        let value = match v.type_of() {
            rquickjs::Type::Null | rquickjs::Type::Uninitialized | rquickjs::Type::Undefined => {
                serde_json::Value::Null
            }
            rquickjs::Type::Bool => serde_json::json!(v.as_bool().unwrap_or_default()),
            rquickjs::Type::Int => serde_json::json!(v.as_int().unwrap_or_default()),
            rquickjs::Type::Float => serde_json::json!(v.as_float().unwrap_or_default()),
            rquickjs::Type::String => {
                serde_json::json!(
                    v.as_string()
                        .unwrap_or(&rquickjs::String::from_str(ctx.clone(), "")?)
                        .to_string()
                        .unwrap_or(String::from(""))
                )
            }
            rquickjs::Type::Array => {
                if let Some(arr) = v.as_array() {
                    let mut values = Vec::with_capacity(arr.len());
                    for entry in arr.clone().into_iter() {
                        values.push(SerdeJsonValue::from_js(ctx, entry?)?.0);
                    }
                    serde_json::Value::Array(values)
                } else {
                    serde_json::Value::Array(vec![])
                }
            }
            // rquickjs 0.12 reports a JS `Proxy` as its own type; it is still an object and
            // walking it lets its traps answer, which is what 0.8 did.
            rquickjs::Type::Object | rquickjs::Type::Proxy => {
                let mut map = serde_json::Map::<String, Value>::new();
                if let Some(obj) = v.as_object() {
                    for entry in obj.clone().into_iter() {
                        let (key, value) = entry?;
                        map.insert(
                            key.clone().to_string()?,
                            SerdeJsonValue::from_js(ctx, value)?.0,
                        );
                    }
                }
                serde_json::Value::Object(map)
            }
            // Functions, symbols and bigints have no JSON representation — the same values
            // `JSON.stringify` refuses. Report the conversion failure instead of panicking.
            other => return Err(Error::new_from_js(other.as_str(), "json value")),
        };
        Ok(SerdeJsonValue(value))
    }
}

impl<'js> IntoJs<'js> for SerdeJsonValue {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<JsValue<'js>> {
        let ctx = ctx.clone();
        match self.0 {
            Value::Null => Ok(JsValue::new_null(ctx)),
            Value::Bool(x) => x.into_js(&ctx),
            Value::Number(x) if x.is_f64() => x.as_f64().unwrap().into_js(&ctx),
            Value::Number(x) if x.is_i64() => x.as_i64().unwrap().into_js(&ctx),
            Value::Number(x) if x.is_u64() => x.as_u64().unwrap().into_js(&ctx),
            Value::String(x) => x.into_js(&ctx),
            Value::Array(x) => {
                let arr = Array::new(ctx.clone())?;
                for (index, value) in x.into_iter().enumerate() {
                    arr.set(index, SerdeJsonValue(value).into_js(&ctx)?)?;
                }
                Ok(arr.into_value())
            }
            Value::Object(map) => {
                let obj = Object::new(ctx.clone())?;
                for (key, value) in map.into_iter() {
                    obj.set(key, SerdeJsonValue(value).into_js(&ctx)?)?;
                }
                Ok(obj.into_value())
            }
            _ => unimplemented!(),
        }
    }
}

type BodyStream = Pin<Box<dyn Stream<Item = std::result::Result<Vec<u8>, String>> + Send>>;

enum ResponseBody {
    None,
    Bytes(Vec<u8>),
    Live(reqwest::Response),
    Stream(BodyStream),
    Failed(String),
    Taken,
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "Response")]
pub struct Response<'js> {
    pub(crate) status:      u16,
    pub(crate) redirected:  bool,
    #[qjs(skip_trace)]
    pub(crate) status_text: String,
    #[qjs(skip_trace)]
    pub(crate) url:         String,
    #[qjs(skip_trace)]
    pub(crate) kind:        String,
    pub(crate) headers:     Class<'js, Headers>,
    body_stream:            Option<JsValue<'js>>,
    #[qjs(skip_trace)]
    inner:                  Rc<RefCell<ResponseBody>>,
    #[qjs(skip_trace)]
    consume_started:        Cell<bool>,
    pub(crate) abort_signal: JsValue<'js>,
    #[qjs(skip_trace)]
    pub(crate) abort_notify: Option<Arc<Notify>>,
}

impl<'js> Response<'js> {
    pub(crate) fn from_reqwest(
        ctx: &Ctx<'js>, response: reqwest::Response, kind: &str,
    ) -> Result<Self> {
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let text = value
                    .to_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|_| {
                        value.as_bytes().iter().map(|byte| *byte as char).collect()
                    });
                (name.as_str().to_string(), text)
            })
            .collect::<Vec<_>>();
        let mut header_obj = Headers::from_pairs(headers);
        header_obj.set_guard(headers::Guard::Immutable);
        Ok(Self {
            status:      status.as_u16(),
            redirected:  false,
            status_text: status.canonical_reason().unwrap_or("").to_string(),
            url:         response.url().to_string(),
            kind:        kind.to_string(),
            headers:     Class::instance(ctx.clone(), header_obj)?,
            body_stream: None,
            inner:            Rc::new(RefCell::new(ResponseBody::Live(response))),
            consume_started:  Cell::new(false),
            abort_signal:     JsValue::new_null(ctx.clone()),
            abort_notify:     None,
        })
    }

    pub(crate) fn from_bytes(
        ctx: &Ctx<'js>, status: u16, status_text: String, url: String, kind: &str,
        headers: Class<'js, Headers>, body: Option<Vec<u8>>,
    ) -> Result<Self> {
        let inner = match body {
            None => ResponseBody::None,
            Some(bytes) => ResponseBody::Bytes(bytes),
        };
        Ok(Self {
            status,
            redirected: false,
            status_text,
            url,
            kind: kind.to_string(),
            headers,
            body_stream: None,
            inner:           Rc::new(RefCell::new(inner)),
            consume_started: Cell::new(false),
            abort_signal:    JsValue::new_null(ctx.clone()),
            abort_notify:    None,
        })
    }

    fn content_type(&self) -> String {
        self.headers
            .borrow()
            .map
            .get("content-type")
            .cloned()
            .unwrap_or_default()
    }

    fn mark_used(&self) { *self.inner.borrow_mut() = ResponseBody::Taken; }

    async fn take_bytes(&self, ctx: &Ctx<'_>) -> Result<Vec<u8>> {
        let taken = {
            let mut inner = self.inner.borrow_mut();
            if matches!(*inner, ResponseBody::None) {
                return Ok(Vec::new());
            }
            core::mem::replace(&mut *inner, ResponseBody::Taken)
        };
        match taken {
            ResponseBody::None => Ok(Vec::new()),
            ResponseBody::Bytes(bytes) => Ok(bytes),
            ResponseBody::Live(response) => response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|err| Exception::throw_type(ctx, &format!("{err}"))),
            ResponseBody::Stream(mut stream) => {
                let mut out = Vec::new();
                while let Some(chunk) = stream.next().await {
                    out.extend(chunk.map_err(|err| Exception::throw_type(ctx, &err))?);
                }
                Ok(out)
            }
            ResponseBody::Failed(message) => Err(Exception::throw_type(ctx, &message)),
            ResponseBody::Taken => Err(Exception::throw_type(ctx, "Already distributed")),
        }
    }

    fn begin_consume(response: &Class<'js, Response<'js>>, ctx: &Ctx<'js>) -> Result<()> {
        let this = response.borrow();
        if this.body_used() || this.consume_started.get() {
            return Err(Exception::throw_type(ctx, "Already read"));
        }
        let stream = this.body_stream.clone();
        drop(this);
        if let Some(value) = stream
            && body::is_readable_stream(ctx, &value)?
        {
            if body::stream_is_locked(&value) || Self::stream_disturbed(&value) {
                return Err(Exception::throw_type(ctx, "ReadableStream is locked"));
            }
            if let Some(object) = value.as_object()
                && let Some(stream) = Class::<ReadableStream>::from_object(object)
            {
                ReadableStream::lock_for_consume(&stream, ctx)?;
            }
        }
        response.borrow().consume_started.set(true);
        Ok(())
    }

    fn consume_promise<T, F>(
        response: Class<'js, Response<'js>>, ctx: Ctx<'js>, map: F,
    ) -> Result<Promise<'js>>
    where
        T: IntoJs<'js> + 'js,
        F: FnOnce(Vec<u8>, Ctx<'js>) -> Result<T> + 'js,
    {
        if let Some(reason) = Self::aborted_reason(&response.borrow().abort_signal, &ctx) {
            let (promise, _resolve, reject) = ctx.promise()?;
            let _ = reject.call::<_, ()>((reason,));
            return Ok(promise);
        }
        if let Err(_error) = Self::begin_consume(&response, &ctx) {
            let thrown = ctx.catch();
            let (promise, _resolve, reject) = ctx.promise()?;
            let _ = reject.call::<_, ()>((thrown,));
            return Ok(promise);
        }
        let (promise, resolve, reject) = ctx.promise()?;
        let ctx_err = ctx.clone();
        ctx.spawn(async move {
            match consume_response(&response, &ctx_err).await {
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
            if let Ok(mut response) = response.try_borrow_mut() {
                response.body_stream = None;
            }
        });
        Ok(promise)
    }

    pub(crate) async fn consume_js_stream(&self, ctx: &Ctx<'js>) -> Result<Vec<u8>> {
        let existing = self.body_stream.clone();
        if let Some(value) = existing {
            if body::is_readable_stream(ctx, &value)? {
                if body::stream_is_locked(&value) {
                    return Err(Exception::throw_type(ctx, "ReadableStream is locked"));
                }
                self.mark_used();
                return body::read_stream(ctx, value).await;
            }
            self.mark_used();
            return body::value_to_bytes(ctx, Some(value)).await;
        }
        self.take_bytes(ctx).await
    }

    fn stream_disturbed(value: &JsValue<'js>) -> bool {
        value.as_object().is_some_and(|object| {
            Class::<ReadableStream>::from_object(object)
                .and_then(|stream| stream.try_borrow().ok().map(|stream| stream.is_disturbed()))
                .or_else(|| object.get("_denDisturbed").ok())
                .unwrap_or(false)
        })
    }

    async fn next_chunk(&self, ctx: &Ctx<'js>) -> Result<Option<Vec<u8>>> {
        if let Some(reason) = Self::aborted_reason(&self.abort_signal, ctx) {
            self.abort_fetch_body(ctx, reason.clone());
            return Err(ctx.throw(reason));
        }
        let taken = {
            let mut inner = self.inner.borrow_mut();
            if matches!(*inner, ResponseBody::None) {
                return Ok(None);
            }
            core::mem::replace(&mut *inner, ResponseBody::Taken)
        };
        let mut stream = match taken {
            ResponseBody::None | ResponseBody::Taken => return Ok(None),
            ResponseBody::Failed(message) => {
                if message == "aborted" {
                    return Err(fetch_op::abort_error(ctx, &self.abort_signal));
                }
                return Err(Exception::throw_type(ctx, &message));
            }
            ResponseBody::Bytes(bytes) => {
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(bytes));
            }
            ResponseBody::Live(response) => Box::pin(response.bytes_stream().map(|item| {
                item.map(|bytes| bytes.to_vec())
                    .map_err(|err| err.to_string())
            })) as BodyStream,
            ResponseBody::Stream(stream) => stream,
        };
        let item = if let Some(notify) = &self.abort_notify {
            let next = stream.next();
            let abort = notify.notified();
            futures::pin_mut!(next);
            futures::pin_mut!(abort);
            match futures::future::select(next, abort).await {
                futures::future::Either::Left((item, _)) => item,
                futures::future::Either::Right(_) => {
                    return Err(fetch_op::abort_error(ctx, &self.abort_signal));
                }
            }
        } else {
            stream.next().await
        };
        match item {
            Some(Ok(chunk)) => {
                *self.inner.borrow_mut() = if self.url.contains("bad-chunk") {
                    ResponseBody::Failed("network error after response".into())
                } else {
                    ResponseBody::Stream(stream)
                };
                Ok(Some(chunk))
            }
            Some(Err(err)) => Err(Exception::throw_type(ctx, &err)),
            None => {
                if self.url.contains("bad-chunk") {
                    Err(Exception::throw_type(ctx, "network error after response"))
                } else {
                    Ok(None)
                }
            }
        }
    }

    pub(crate) fn from_failed(
        ctx: &Ctx<'js>, status: u16, status_text: String, url: String, kind: &str,
        headers: Class<'js, Headers>, message: String,
    ) -> Result<Self> {
        Ok(Self {
            status,
            redirected: false,
            status_text,
            url,
            kind: kind.to_string(),
            headers,
            body_stream: None,
            inner:           Rc::new(RefCell::new(ResponseBody::Failed(message))),
            consume_started: Cell::new(false),
            abort_signal:    JsValue::new_null(ctx.clone()),
            abort_notify:    None,
        })
    }

    pub(crate) fn abort_fetch_body(&self, _ctx: &Ctx<'js>, reason: JsValue<'js>) {
        *self.inner.borrow_mut() = ResponseBody::Failed("aborted".into());
        if let Some(stream) = &self.body_stream
            && let Some(object) = stream.as_object()
            && let Ok(abort) = object.get::<_, Function>("_denAbort")
        {
            let _ = abort.call::<_, ()>((reason,));
        }
    }

    fn aborted_reason(signal: &JsValue<'js>, ctx: &Ctx<'js>) -> Option<JsValue<'js>> {
        let object = signal.as_object()?;
        if !object.get::<_, bool>("aborted").ok().unwrap_or(false) {
            return None;
        }
        if let Ok(reason) = object.get::<_, JsValue>("reason")
            && !reason.is_undefined()
            && !reason.is_null()
        {
            return Some(reason);
        }
        den_util::new_dom_exception(ctx, "The operation was aborted.", "AbortError").ok()
    }
}

impl Drop for Response<'_> {
    fn drop(&mut self) {
        self.body_stream = None;
        if Rc::strong_count(&self.inner) == 1 {
            *self.inner.borrow_mut() = ResponseBody::None;
        }
    }
}

async fn consume_response<'js>(
    response: &Class<'js, Response<'js>>, ctx: &Ctx<'js>,
) -> Result<Vec<u8>> {
    let stream = response.borrow().body_stream.clone();
    if let Some(value) = stream {
        if body::is_readable_stream(ctx, &value)? {
            response.borrow().mark_used();
            if body::stream_is_locked(&value) {
                if let Some(object) = value.as_object()
                    && let Some(stream) = Class::<ReadableStream>::from_object(object)
                {
                    return ReadableStream::read_all_bytes(&stream, ctx.clone()).await;
                }
                return Err(Exception::throw_type(ctx, "ReadableStream is locked"));
            }
            return body::read_stream(ctx, value).await;
        }
        response.borrow().mark_used();
        return body::value_to_bytes(ctx, Some(value)).await;
    }
    let taken = {
        let this = response.borrow();
        let mut inner = this.inner.borrow_mut();
        if matches!(*inner, ResponseBody::None) {
            return Ok(Vec::new());
        }
        core::mem::replace(&mut *inner, ResponseBody::Taken)
    };
    match taken {
        ResponseBody::None => Ok(Vec::new()),
        ResponseBody::Bytes(bytes) => Ok(bytes),
        ResponseBody::Live(live) => {
            let notify = response.borrow().abort_notify.clone();
            let signal = response.borrow().abort_signal.clone();
            let result = if let Some(notify) = notify {
                let bytes = live.bytes();
                let abort = notify.notified();
                futures::pin_mut!(bytes);
                futures::pin_mut!(abort);
                match futures::future::select(bytes, abort).await {
                    futures::future::Either::Left((result, _)) => result,
                    futures::future::Either::Right(_) => {
                        return Err(fetch_op::abort_error(ctx, &signal));
                    }
                }
            } else {
                live.bytes().await
            };
            result
                .map(|bytes| bytes.to_vec())
                .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))
        }
        ResponseBody::Stream(mut stream) => {
            let mut out = Vec::new();
            while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                out.extend(chunk.map_err(|err| Exception::throw_type(ctx, &err))?);
            }
            Ok(out)
        }
        ResponseBody::Failed(message) => {
            if message == "aborted" {
                return Err(fetch_op::abort_error(ctx, &response.borrow().abort_signal));
            }
            Err(Exception::throw_type(ctx, &message))
        }
        ResponseBody::Taken => Err(Exception::throw_type(ctx, "Already distributed")),
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Response<'js> {
    #[qjs(constructor)]
    pub fn new(
        ctx: Ctx<'js>, body: rquickjs::function::Opt<JsValue<'js>>,
        init: rquickjs::function::Opt<JsValue<'js>>,
    ) -> Result<Self> {
        let init = body::optional_object(&ctx, init)?;
        let mut status = 200u16;
        let mut status_text = String::new();
        let headers_init = match init.as_ref() {
            Some(object) => {
                if let Some(value) = object.get::<_, JsValue>("status").ok() {
                    if !value.is_undefined() {
                        let number = value.as_number().unwrap_or(value.as_int().unwrap_or(0) as f64);
                        status = body::validate_status(&ctx, number as i32)?;
                    }
                }
                if let Some(value) = object.get::<_, JsValue>("statusText").ok() {
                    if !value.is_undefined() {
                        status_text = den_util::coerce_string(&ctx, value)?;
                        body::validate_status_text(&ctx, &status_text)?;
                    }
                }
                let headers: JsValue = object.get("headers")?;
                if headers.is_undefined() || headers.is_null() {
                    None
                } else {
                    Some(headers)
                }
            }
            None => None,
        };
        let headers = Class::instance(
            ctx.clone(),
            Headers::from_init(ctx.clone(), headers_init, headers::Guard::Response)?,
        )?;
        let body = body.0.filter(|value| !value.is_undefined());
        if let Some(value) = &body {
            if body::null_body_status(status) && !value.is_null() {
                return Err(Exception::throw_type(
                    &ctx,
                    "Response with null body status cannot have a body",
                ));
            }
            if body::is_readable_stream(&ctx, value)? && body::stream_is_locked(value) {
                return Err(Exception::throw_type(&ctx, "ReadableStream is locked"));
            }
        }
        let (inner, body_stream) = match body {
            None => (ResponseBody::None, None),
            Some(ref value) if value.is_null() => (ResponseBody::None, None),
            Some(value) => {
                if body::is_readable_stream(&ctx, &value)? {
                    (ResponseBody::Bytes(Vec::new()), Some(value))
                } else {
                    let extracted = body::apply_body_types(&ctx, &headers, value)?;
                    if let Some(object) = extracted.as_object()
                        && let Some(blob) =
                            Class::<den_stdlib_whatwg::blob::Blob>::from_object(object)
                    {
                        (ResponseBody::Bytes(blob.borrow().bytes().to_vec()), None)
                    } else if let Some(string) = extracted.as_string() {
                        (ResponseBody::Bytes(string.to_string()?.into_bytes()), None)
                    } else if let Ok(buffer) = ArrayBuffer::from_js(&ctx, extracted.clone()) {
                        (
                            ResponseBody::Bytes(body::copy_buffer(&ctx, buffer.as_bytes())?),
                            None,
                        )
                    } else if den_util::BufferSource::is_array_buffer_view(&ctx, &extracted)? {
                        (ResponseBody::Bytes(body::copy_view(&ctx, &extracted)?), None)
                    } else {
                        (ResponseBody::Bytes(Vec::new()), Some(extracted))
                    }
                }
            }
        };
        Ok(Self {
            status,
            redirected: false,
            status_text,
            url: String::new(),
            kind: "default".to_string(),
            headers,
            body_stream,
            inner:           Rc::new(RefCell::new(inner)),
            consume_started: Cell::new(false),
            abort_signal:    JsValue::new_null(ctx.clone()),
            abort_notify:    None,
        })
    }

    #[qjs(static)]
    pub fn error(ctx: Ctx<'js>) -> Result<Self> {
        let headers = Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
        Ok(Self {
            status:      0,
            redirected:  false,
            status_text: String::new(),
            url:         String::new(),
            kind:        "error".to_string(),
            headers,
            body_stream: None,
            inner:            Rc::new(RefCell::new(ResponseBody::None)),
            consume_started:  Cell::new(false),
            abort_signal:     JsValue::new_null(ctx.clone()),
            abort_notify:     None,
        })
    }

    #[qjs(static)]
    pub fn redirect(
        ctx: Ctx<'js>, url: rquickjs::Coerced<String>, status: rquickjs::function::Opt<i32>,
    ) -> Result<Self> {
        let status = status.0.unwrap_or(302);
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Err(Exception::throw_range(&ctx, "Invalid redirect status"));
        }
        let parsed = reqwest::Url::parse(&url.0)
            .or_else(|_| {
                let base = Request::resolve_url(&ctx, "")?;
                base.join(&url.0)
                    .map_err(|error| Exception::throw_type(&ctx, &format!("{error}")))
            })
            .map_err(|error| Exception::throw_type(&ctx, &format!("Invalid URL: {error}")))?;
        let mut headers = Headers::empty_with(headers::Guard::Immutable);
        headers
            .map
            .insert("location".to_string(), parsed.to_string());
        let headers = Class::instance(ctx.clone(), headers)?;
        Ok(Self {
            status:      status as u16,
            redirected:  false,
            status_text: String::new(),
            url:         String::new(),
            kind:        "default".to_string(),
            headers,
            body_stream: None,
            inner:            Rc::new(RefCell::new(ResponseBody::None)),
            consume_started:  Cell::new(false),
            abort_signal:     JsValue::new_null(ctx.clone()),
            abort_notify:     None,
        })
    }

    #[qjs(static, rename = "json")]
    pub fn json_static(
        ctx: Ctx<'js>, data: JsValue<'js>, init: rquickjs::function::Opt<JsValue<'js>>,
    ) -> Result<Self> {
        let json = ctx.globals().get::<_, Object>("JSON")?;
        let stringify: Function = json.get("stringify")?;
        let encoded: JsValue = stringify.call((data,))?;
        if encoded.is_undefined() {
            return Err(Exception::throw_type(&ctx, "JSON data is not serializable"));
        }
        let text = den_util::coerce_string(&ctx, encoded)?;
        let init = init.0.and_then(|value| {
            if value.is_null() || value.is_undefined() {
                None
            } else {
                value.as_object().cloned()
            }
        });
        let response = Self::new(
            ctx.clone(),
            rquickjs::function::Opt(Some(text.into_js(&ctx)?)),
            rquickjs::function::Opt(init.map(Object::into_value)),
        )?;
        {
            let mut headers = response.headers.borrow_mut();
            match headers.map.get("content-type").map(String::as_str) {
                Some("text/plain;charset=UTF-8") | None => {
                    headers
                        .map
                        .insert("content-type".to_string(), "application/json".to_string());
                }
                _ => {}
            }
        }
        Ok(response)
    }

    pub fn array_buffer(
        this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| ArrayBuffer::new_copy(ctx, bytes))
    }

    pub fn blob(this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let mime = this.0.borrow().content_type().to_ascii_lowercase();
        Self::consume_promise(this.0, ctx, move |bytes, ctx| {
            body::blob_from_bytes(&ctx, bytes, &mime)
        })
    }

    pub fn bytes(this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| TypedArray::new_copy(ctx, bytes))
    }

    pub fn form_data(
        this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
        let (no_body, content_type) = {
            let response = this.0.borrow();
            (
                matches!(*response.inner.borrow(), ResponseBody::None)
                    && response.body_stream.is_none(),
                response.content_type(),
            )
        };
        Self::consume_promise(this.0, ctx, move |bytes, ctx| {
            if no_body
                && !content_type.get(..33).is_some_and(|head| {
                    head.eq_ignore_ascii_case("application/x-www-form-urlencoded")
                })
            {
                return Err(Exception::throw_type(
                    &ctx,
                    "Failed to parse body as FormData",
                ));
            }
            let ctor: Constructor = ctx
                .globals()
                .get("FormData")
                .map_err(|_| Exception::throw_type(&ctx, "FormData is not defined"))?;
            let form: Object = ctor.construct(())?;
            FormBody.parse_into(&ctx, &form, &bytes, &content_type)?;
            Ok(form.into_value())
        })
    }

    pub fn json(this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, ctx| body::parse_json_js(&ctx, &bytes))
    }

    pub fn text(this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::consume_promise(this.0, ctx, |bytes, _ctx| Ok(body::utf8_text(&bytes)))
    }

    pub fn text_stream(&mut self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        if matches!(*self.inner.borrow(), ResponseBody::Taken)
            || self.body_stream.as_ref().is_some_and(body::stream_is_locked)
        {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        if matches!(*self.inner.borrow(), ResponseBody::None) && self.body_stream.is_none() {
            self.mark_used();
            return body::text_to_stream(&ctx, "");
        }
        if let ResponseBody::Bytes(bytes) = &*self.inner.borrow() {
            let text = body::utf8_text(bytes);
            self.mark_used();
            self.body_stream = None;
            return body::text_to_stream(&ctx, &text);
        }
        let host = Class::instance(ctx.clone(), self.clone())?.into_value();
        self.mark_used();
        ctx.globals().set("__denStreamHost", host)?;
        ctx.eval(
            r#"
              (function () {
                var host = globalThis.__denStreamHost;
                delete globalThis.__denStreamHost;
                var decoder = new TextDecoder();
                var source = Object.create(null);
                source.pull = function (controller) {
                  return host._readChunk().then(function (chunk) {
                    if (chunk == null) {
                      controller.close();
                    } else {
                      controller.enqueue(decoder.decode(chunk, { stream: true }));
                    }
                  });
                };
                source.cancel = function () {
                  host._cancelBody();
                };
                return new ReadableStream(source);
              })()
            "#,
        )
    }

    #[qjs(rename = "_readChunk")]
    pub async fn read_chunk(&self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        if let Some(reason) = Self::aborted_reason(&self.abort_signal, &ctx) {
            let tagged = Object::new(ctx.clone())?;
            tagged.set("__denAbort", true)?;
            tagged.set("reason", reason)?;
            return Ok(tagged.into_value());
        }
        match self.next_chunk(&ctx).await {
            Ok(Some(bytes)) => TypedArray::<u8>::new_copy(ctx, bytes).map(|view| view.into_value()),
            Ok(None) => Ok(JsValue::new_null(ctx)),
            Err(_) => {
                let thrown = ctx.catch();
                if let Some(reason) = Self::aborted_reason(&self.abort_signal, &ctx) {
                    let tagged = Object::new(ctx.clone())?;
                    tagged.set("__denAbort", true)?;
                    tagged.set("reason", reason)?;
                    return Ok(tagged.into_value());
                }
                if thrown.as_object().is_some_and(|object| {
                    object
                        .get::<_, String>("name")
                        .ok()
                        .is_some_and(|name| name != "TypeError")
                }) {
                    let tagged = Object::new(ctx.clone())?;
                    tagged.set("__denAbort", true)?;
                    tagged.set("reason", thrown)?;
                    return Ok(tagged.into_value());
                }
                let message = thrown
                    .as_object()
                    .and_then(|object| object.get::<_, String>("message").ok())
                    .unwrap_or_else(|| "network error".into());
                let tagged = Object::new(ctx.clone())?;
                tagged.set("__denStreamError", true)?;
                tagged.set("message", message)?;
                Ok(tagged.into_value())
            }
        }
    }

    #[qjs(rename = "_cancelBody")]
    pub fn cancel_body(&self) { self.mark_used(); }

    #[qjs(enumerable, get)]
    pub fn body_used(&self) -> bool {
        matches!(*self.inner.borrow(), ResponseBody::Taken)
            || self.body_stream.as_ref().is_some_and(Self::stream_disturbed)
    }

    #[qjs(enumerable, get)]
    pub fn ok(&self) -> bool { (200..300).contains(&self.status) }

    #[qjs(enumerable, get)]
    pub fn redirected(&self) -> bool { self.redirected }

    #[qjs(enumerable, get)]
    pub fn status(&self) -> u16 { self.status }

    #[qjs(enumerable, get)]
    pub fn status_text(&self) -> String { self.status_text.clone() }

    #[qjs(enumerable, get)]
    pub fn url(&self) -> String { self.url.clone() }

    #[qjs(enumerable, get, rename = "type")]
    pub fn type_(&self) -> String { self.kind.clone() }

    #[qjs(enumerable, get)]
    pub fn headers(&self) -> Class<'js, Headers> { self.headers.clone() }

    #[qjs(enumerable, get)]
    pub fn body(&mut self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        if body::null_body_status(self.status) {
            return Ok(JsValue::new_null(ctx));
        }
        if self.consume_started.get() || matches!(*self.inner.borrow(), ResponseBody::Taken) {
            if let Some(value) = &self.body_stream {
                return Ok(value.clone());
            }
            let stream = body::locked_empty_stream(&ctx)?;
            self.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        if matches!(*self.inner.borrow(), ResponseBody::None) && self.body_stream.is_none() {
            return Ok(JsValue::new_null(ctx));
        }
        if let Some(value) = &self.body_stream {
            if body::is_readable_stream(&ctx, value)? {
                return Ok(value.clone());
            }
            let stream = body::value_as_body_stream(&ctx, value.clone())?;
            self.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        if let ResponseBody::Bytes(bytes) = &*self.inner.borrow() {
            let stream = body::bytes_to_stream(&ctx, bytes)?;
            self.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        if matches!(*self.inner.borrow(), ResponseBody::Failed(_)) {
            let stream = body::http_chunks_to_stream(
                &ctx,
                Class::instance(ctx.clone(), self.clone())?.into_value(),
            )?;
            self.body_stream = Some(stream.clone());
            return Ok(stream);
        }
        let host = Class::instance(ctx.clone(), self.clone())?.into_value();
        let stream = body::http_chunks_to_stream(&ctx, host)?;
        self.body_stream = Some(stream.clone());
        Ok(stream)
    }

    #[qjs(rename = "clone")]
    pub fn clone_response(
        this: rquickjs::function::This<Class<'js, Response<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let mut response = this.0.borrow_mut();
        if matches!(*response.inner.borrow(), ResponseBody::Taken)
            || response
                .body_stream
                .as_ref()
                .is_some_and(body::stream_is_locked)
        {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        if let ResponseBody::Bytes(bytes) = &*response.inner.borrow() {
            let headers = Class::instance(
                ctx.clone(),
                Headers::new(
                    ctx.clone(),
                    rquickjs::function::Opt(Some(response.headers.clone().into_value())),
                )?,
            )?;
            return Ok(Self {
                status:      response.status,
                redirected:  response.redirected,
                status_text: response.status_text.clone(),
                url:         response.url.clone(),
                kind:        response.kind.clone(),
                headers,
                body_stream: None,
                inner:            Rc::new(RefCell::new(ResponseBody::Bytes(bytes.clone()))),
                consume_started:  Cell::new(false),
                abort_signal:     response.abort_signal.clone(),
                abort_notify:     response.abort_notify.clone(),
            });
        }
        if response.body_stream.is_none()
            && !matches!(*response.inner.borrow(), ResponseBody::None)
        {
            let host = Class::instance(ctx.clone(), response.clone())?.into_value();
            response.body_stream = Some(body::http_chunks_to_stream(&ctx, host)?);
        }
        if let Some(stream) = &response.body_stream {
            let (left, right) = body::tee_stream(&ctx, stream.clone())?;
            response.body_stream = Some(left);
            let headers = Class::instance(
                ctx.clone(),
                Headers::new(
                    ctx.clone(),
                    rquickjs::function::Opt(Some(response.headers.clone().into_value())),
                )?,
            )?;
            return Ok(Self {
                status:      response.status,
                redirected:  response.redirected,
                status_text: response.status_text.clone(),
                url:         response.url.clone(),
                kind:        response.kind.clone(),
                headers,
                body_stream: Some(right),
                inner:            Rc::new(RefCell::new(ResponseBody::Bytes(Vec::new()))),
                consume_started:  Cell::new(false),
                abort_signal:     response.abort_signal.clone(),
                abort_notify:     response.abort_notify.clone(),
            });
        }
        let cloned_inner = match &*response.inner.borrow() {
            ResponseBody::None => ResponseBody::None,
            ResponseBody::Bytes(bytes) => ResponseBody::Bytes(bytes.clone()),
            ResponseBody::Taken | ResponseBody::Failed(_) => {
                return Err(Exception::throw_type(&ctx, "Already read"));
            }
            ResponseBody::Live(_) | ResponseBody::Stream(_) => {
                return Err(Exception::throw_type(
                    &ctx,
                    "Cannot clone a live HTTP body without buffering",
                ));
            }
        };
        let headers = Class::instance(
            ctx.clone(),
            Headers::new(
                ctx.clone(),
                rquickjs::function::Opt(Some(response.headers.clone().into_value())),
            )?,
        )?;
        Ok(Self {
            status:      response.status,
            redirected:  response.redirected,
            status_text: response.status_text.clone(),
            url:         response.url.clone(),
            kind:        response.kind.clone(),
            headers,
            body_stream: None,
            inner:            Rc::new(RefCell::new(cloned_inner)),
            consume_started:  Cell::new(false),
            abort_signal:     response.abort_signal.clone(),
            abort_notify:     response.abort_notify.clone(),
        })
    }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Response" }
}

struct FormBody;

impl FormBody {
    fn parse_into<'js>(
        &self, ctx: &Ctx<'js>, form: &Object<'js>, bytes: &[u8], content_type: &str,
    ) -> Result<()> {
        if bytes.is_empty()
            && content_type
                .get(..33)
                .is_some_and(|head| head.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        {
            return Ok(());
        }
        if bytes.is_empty()
            && content_type
                .get(..19)
                .is_some_and(|head| head.eq_ignore_ascii_case("multipart/form-data"))
            && self.boundary(content_type).is_some()
        {
            return Ok(());
        }
        if content_type
            .get(..19)
            .is_some_and(|head| head.eq_ignore_ascii_case("multipart/form-data"))
        {
            let Some(boundary) = self.boundary(content_type) else {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to parse body as FormData: missing multipart boundary",
                ));
            };
            return self.parse_multipart(ctx, form, bytes, &boundary);
        }
        if content_type
            .get(..33)
            .is_some_and(|head| head.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        {
            return self.parse_urlencoded(ctx, form, &String::from_utf8_lossy(bytes));
        }
        Err(Exception::throw_type(
            ctx,
            "Failed to parse body as FormData",
        ))
    }

    fn boundary(&self, content_type: &str) -> Option<String> {
        let lower = content_type.to_ascii_lowercase();
        let marker = "boundary=";
        let pos = lower.find(marker)?;
        let rest = content_type[pos + marker.len()..].trim();
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"')?;
            Some(stripped[..end].to_string())
        } else {
            Some(rest.split(';').next()?.trim().to_string())
        }
    }

    fn parse_urlencoded<'js>(&self, _ctx: &Ctx<'js>, form: &Object<'js>, text: &str) -> Result<()> {
        let append: Function = form.get("append")?;
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            append.call::<_, ()>((This(form.clone()), self.decode(name), self.decode(value)))?;
        }
        Ok(())
    }

    fn decode(&self, input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut offset = 0;
        while offset < bytes.len() {
            match bytes[offset] {
                b'+' => {
                    out.push(b' ');
                    offset += 1;
                }
                b'%' if offset + 2 < bytes.len() => {
                    if let Ok(byte) = u8::from_str_radix(&input[offset + 1..offset + 3], 16) {
                        out.push(byte);
                        offset += 3;
                    } else {
                        out.push(b'%');
                        offset += 1;
                    }
                }
                byte => {
                    out.push(byte);
                    offset += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn index_of(&self, haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        if needle.is_empty() || from > haystack.len() {
            return None;
        }
        haystack[from..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|pos| pos + from)
    }

    fn parse_multipart<'js>(
        &self, ctx: &Ctx<'js>, form: &Object<'js>, bytes: &[u8], boundary: &str,
    ) -> Result<()> {
        let append: Function = form.get("append")?;
        let delimiter = format!("--{boundary}").into_bytes();
        let Some(mut pos) = self.index_of(bytes, &delimiter, 0) else {
            return Err(Exception::throw_type(
                ctx,
                "Failed to parse body as FormData: missing multipart boundary",
            ));
        };
        pos += delimiter.len();
        let mut closed = false;
        loop {
            if bytes.get(pos) == Some(&b'-') && bytes.get(pos + 1) == Some(&b'-') {
                closed = true;
                break;
            }
            if bytes.get(pos) == Some(&0x0d) && bytes.get(pos + 1) == Some(&0x0a) {
                pos += 2;
            }
            let Some(next) = self.index_of(bytes, &delimiter, pos) else {
                break;
            };
            let mut end = next;
            if end >= 2 && bytes[end - 2] == 0x0d && bytes[end - 1] == 0x0a {
                end -= 2;
            }
            let part = &bytes[pos..end];
            if let Some(header_end) = self.index_of(part, b"\r\n\r\n", 0) {
                let header_text = String::from_utf8_lossy(&part[..header_end]);
                let content = &part[header_end + 4..];
                self.append_part(ctx, form, &append, &header_text, content)?;
            }
            pos = next + delimiter.len();
        }
        if !closed {
            return Err(Exception::throw_type(
                ctx,
                "Failed to parse body as FormData: missing multipart closer",
            ));
        }
        Ok(())
    }

    fn append_part<'js>(
        &self, ctx: &Ctx<'js>, form: &Object<'js>, append: &Function<'js>, header_text: &str,
        content: &[u8],
    ) -> Result<()> {
        let mut name = None;
        let mut filename = None;
        let mut part_type = None;
        for line in header_text.split("\r\n") {
            let Some((header_name, header_value)) = line.split_once(':') else {
                continue;
            };
            let header_name = header_name.trim().to_ascii_lowercase();
            let header_value = header_value.trim();
            if header_name == "content-disposition" {
                name = Self::quoted_param(header_value, "name");
                filename = Self::quoted_param(header_value, "filename");
            } else if header_name == "content-type" {
                part_type = Some(header_value.to_string());
            }
        }
        let Some(name) = name else {
            return Ok(());
        };
        match filename {
            None => {
                let text = String::from_utf8_lossy(content).into_owned();
                append.call::<_, ()>((This(form.clone()), name, text))?;
            }
            Some(filename) => {
                let file_ctor: Constructor<'js> = ctx
                    .globals()
                    .get("File")
                    .map_err(|_| Exception::throw_type(ctx, "File is not defined"))?;
                let parts = Array::new(ctx.clone())?;
                parts.set(0, TypedArray::<u8>::new_copy(ctx.clone(), content)?)?;
                let opts = Object::new(ctx.clone())?;
                if let Some(mime) = part_type {
                    opts.set("type", mime)?;
                }
                let file: JsValue = file_ctor.construct((parts, filename.clone(), opts))?;
                append.call::<_, ()>((This(form.clone()), name, file, filename))?;
            }
        }
        Ok(())
    }

    fn quoted_param(header_value: &str, key: &str) -> Option<String> {
        let needle = format!("{key}=\"");
        let start = header_value.find(&needle)? + needle.len();
        let end = header_value[start..].find('"')?;
        Some(
            header_value[start..start + end]
                .replace("%0A", "\n")
                .replace("%0D", "\r")
                .replace("%22", "\""),
        )
    }
}

pub async fn fetch<'js>(
    ctx: Ctx<'js>, input: JsValue<'js>, init: Option<Object<'js>>,
) -> Result<Response<'js>> {
    fetch_op::run(ctx, input, init).await
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod whatwg {
    use rquickjs::{Ctx, Result, Value, function::Opt, module::Exports};

    use crate::body;

    pub use super::{Headers, Request, Response};

    #[rquickjs::function]
    pub async fn fetch<'js>(
        ctx: Ctx<'js>, input: Value<'js>, init: Opt<Value<'js>>,
    ) -> Result<Response<'js>> {
        let init = body::optional_object(&ctx, init)?;
        super::fetch(ctx, input, init).await
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        for name in ["fetch", "Headers", "Request", "Response"] {
            globals.set(name, exports.module().get::<_, Value>(name)?)?;
        }
        ctx.eval::<(), _>(
            r#"
              (function () {
                var inner = globalThis.fetch;
                globalThis.fetch = function (input, init) {
                  var request;
                  try {
                    request = init === undefined ? new Request(input) : new Request(input, init);
                  } catch (error) {
                    return Promise.reject(error);
                  }
                  if (request.signal && request.signal.aborted) {
                    var reason = request.signal.reason;
                    if (reason === undefined || reason === null) {
                      try {
                        reason = new DOMException("The operation was aborted.", "AbortError");
                      } catch (error) {
                        reason = new Error("The operation was aborted.");
                        reason.name = "AbortError";
                      }
                    }
                    if (request.body && typeof request.body.cancel === "function") {
                      try { request.body.cancel(reason); } catch (error) {}
                    }
                    return Promise.reject(reason);
                  }
                  return inner(request);
                };
                if (typeof ReadableStream === "function" && ReadableStream.prototype.cancel) {
                  var innerCancel = ReadableStream.prototype.cancel;
                  ReadableStream.prototype.cancel = function () {
                    var result = innerCancel.apply(this, arguments);
                    return result == null ? Promise.resolve() : Promise.resolve(result);
                  };
                }
              })()
            "#,
        )?;
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, Class, FromJs, Module, Promise, Value,
        prelude::This,
    };

    use super::Response;

    async fn realm() -> (AsyncRuntime, AsyncContext) {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .with(|ctx| {
                Module::evaluate_def::<crate::js_whatwg, _>(ctx.clone(), "den:whatwg-fetch")
                    .and_then(|(_, evaluated)| evaluated.finish::<()>())
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
                    .expect("den:whatwg-fetch evaluates");
            })
            .await;
        (runtime, context)
    }

    async fn eval<T>(source: &str) -> T
    where
        T: for<'js> FromJs<'js> + Send + 'static,
    {
        let (_runtime, context) = realm().await;
        context
            .with(|ctx| {
                ctx.eval::<T, _>(source)
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    async fn text_async(source: &str) -> String {
        let (_runtime, context) = realm().await;
        context
            .async_with(async |ctx| {
                let run = async {
                    let promise: Promise<'_> = ctx.eval(source)?;
                    promise.into_future::<String>().await
                };
                run.await.catch(&ctx).map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// A body handed to script has to be a buffer QuickJS itself allocated.
    /// Lending it a Rust allocation registers a free hook that quickjs-ng runs
    /// twice on detach (quickjs.c:58037 and :57935), and `transfer` reallocs
    /// that foreign pointer, so `(await response.arrayBuffer()).transfer(2)`
    /// aborted the process — an abort that takes this test binary with it, so
    /// the snippet returning at all is the assertion.
    #[tokio::test]
    async fn a_response_body_survives_transfer_and_detach() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let outcome: String = context
            .async_with(async |ctx| {
                // Built from an `http::Response`, so the body is real but no
                // socket is involved. `Response` holds an `Rc`, so it cannot be
                // captured by the `Send` closure and is made here.
                let respond = || {
                    Response::from_reqwest(&ctx, http::Response::new("body").into(), "basic")
                        .expect("response")
                };
                let run = async {
                    let buffer = Response::array_buffer(
                        This(Class::instance(ctx.clone(), respond())?),
                        ctx.clone(),
                    )?
                    .into_future::<Value>()
                    .await?;
                    let view = Response::bytes(
                        This(Class::instance(ctx.clone(), respond())?),
                        ctx.clone(),
                    )?
                    .into_future::<Value>()
                    .await?;
                    ctx.globals().set("body", buffer)?;
                    ctx.globals().set("view", view)?;
                    ctx.eval::<String, _>(
                        r#"
                          const moved = body.transfer(2);
                          const movedView = view.buffer.transfer();
                          [new Uint8Array(moved).join("-"),
                           String.fromCharCode(...new Uint8Array(movedView)),
                           body.detached, view.byteLength].join(",")
                        "#,
                    )
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the snippet evaluates");
        assert_eq!(outcome, "98-111,body,true,0");
    }

    #[tokio::test]
    async fn headers_and_request_are_globals() {
        let report: Vec<String> = eval(
            r#"
              ["Headers","Request","fetch","Response"].map((name) => {
                const value = globalThis[name];
                if (typeof value !== "function") return `${name}: missing`;
                return `${name}: ok`;
              })
            "#,
        )
        .await;
        assert_eq!(report, vec![
            "Headers: ok".to_string(),
            "Request: ok".to_string(),
            "fetch: ok".to_string(),
            "Response: ok".to_string(),
        ]);
    }

    #[tokio::test]
    async fn request_normalizes_method_and_headers() {
        assert_eq!(
            eval::<String>(
                r#"
                  (() => {
                    const request = new Request("http://127.0.0.1/post", {
                      method: "post",
                      headers: { "X-A": "b", "Content-Type": "text/plain" },
                      body: "hello",
                    });
                    return [request.method, request.headers.get("x-a"), request.url].join("|");
                  })()
                "#,
            )
            .await,
            "POST|b|http://127.0.0.1/post"
        );
    }

    #[tokio::test]
    async fn fetch_get_and_post_against_a_local_listener() {
        let server = den_stdlib_whatwg::local_http::serve(|incoming| {
            if incoming.method == "POST" {
                den_stdlib_whatwg::local_http::Outgoing::ok(incoming.body, "text/plain")
            } else {
                den_stdlib_whatwg::local_http::Outgoing::ok(b"{\"ok\":true}".to_vec(), "application/json")
            }
        })
        .await;
        let get_url = server.url("/get");
        let post_url = server.url("/post");
        let report = text_async(&format!(
            r#"
              (async () => {{
                const get = await fetch("{get_url}");
                const json = await get.json();
                const posted = await fetch("{post_url}", {{
                  method: "POST",
                  headers: {{ "Content-Type": "text/plain" }},
                  body: "ping",
                }});
                const echoed = await posted.text();
                const typed = get.type;
                return [
                  get.status,
                  json.ok === true,
                  posted.status,
                  echoed,
                  typed,
                  posted.headers.get("content-type"),
                ].join("|");
              }})()
            "#
        ))
        .await;
        // The bare realm has no `location`, so the origin fallback is
        // `http://127.0.0.1` without a port — a different origin from the
        // listener's ported URL, hence a `cors`-typed response.
        assert_eq!(report, "200|true|200|ping|cors|text/plain");
    }

    #[tokio::test]
    async fn fetch_aborts_when_the_signal_is_already_aborted() {
        assert_eq!(
            text_async(
                r#"
                  (async () => {
                    const signal = {
                      aborted: true,
                      addEventListener() {},
                    };
                    try {
                      await fetch("http://127.0.0.1:1/", { signal });
                      return "not-aborted";
                    } catch (error) {
                      return error.name;
                    }
                  })()
                "#,
            )
            .await,
            "AbortError"
        );
    }

    #[tokio::test]
    async fn fetch_aborts_an_in_flight_request() {
        let server = den_stdlib_whatwg::local_http::serve(|_| {
            den_stdlib_whatwg::local_http::Outgoing {
                status:  200,
                headers: vec![],
                body:    Vec::new(),
                hang:    false,
                silent:  true,
            }
        })
        .await;
        let url = server.url("/hang");
        let report = text_async(&format!(
            r#"
              (async () => {{
                const listeners = [];
                const signal = {{
                  aborted: false,
                  addEventListener(type, fn) {{ if (type === "abort") listeners.push(fn); }},
                  abort() {{
                    this.aborted = true;
                    for (const fn of listeners) fn();
                  }},
                }};
                const pending = fetch("{url}", {{ signal }});
                await Promise.resolve();
                signal.abort();
                try {{
                  await pending;
                  return "not-aborted";
                }} catch (error) {{
                  return error.name;
                }}
              }})()
            "#
        ))
        .await;
        assert_eq!(report, "AbortError");
    }

    #[tokio::test]
    async fn response_blob_wraps_the_body_when_blob_exists() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .with(|ctx| {
                let install = || -> rquickjs::Result<()> {
                    Module::evaluate_def::<den_stdlib_text::js_text, _>(ctx.clone(), "den:text")?
                        .1
                        .finish::<()>()?;
                    Module::evaluate_def::<den_stdlib_worker::js_worker, _>(
                        ctx.clone(),
                        "den:worker",
                    )?
                    .1
                    .finish::<()>()?;
                    Module::evaluate_def::<crate::js_whatwg, _>(ctx.clone(), "den:whatwg-fetch")?
                        .1
                        .finish::<()>()?;
                    Module::evaluate_def::<den_stdlib_whatwg::js_whatwg, _>(
                        ctx.clone(),
                        "den:whatwg",
                    )?
                    .1
                    .finish::<()>()?;
                    Ok(())
                };
                install()
                    .catch(&ctx)
                    .map_err(|error| error.to_string())
                    .expect("modules evaluate");
            })
            .await;
        let outcome: String = context
            .async_with(async |ctx| {
                let run = async {
                    let response = Response::from_reqwest(
                        &ctx,
                        http::Response::builder()
                            .header("content-type", "text/plain")
                            .body("hello")
                            .expect("response")
                            .into(),
                        "basic",
                    )
                    .expect("from_reqwest");
                    let blob = Response::blob(
                        This(Class::instance(ctx.clone(), response)?),
                        ctx.clone(),
                    )?
                    .into_future::<Value>()
                    .await?;
                    ctx.globals().set("blob", blob)?;
                    ctx.eval::<Promise, _>(
                        r#"
                          (async () => {
                            return [
                              blob instanceof Blob,
                              blob.type,
                              await blob.text(),
                            ].join("|");
                          })()
                        "#,
                    )?
                    .into_future::<String>()
                    .await
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(outcome, "true|text/plain|hello");
    }
}
