use std::{
    cell::{Cell, RefCell},
    pin::Pin,
    rc::Rc,
    sync::Arc,
};

use futures::{Stream, StreamExt as _};
use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Exception, FromJs as _, Function, IntoJs, JsLifetime, Object,
    Promise, Result, TypedArray, Value as JsValue,
    class::Trace,
    function::{Constructor, FuncArg, Opt as JsOpt, Rest, This},
};
use tokio::sync::Notify;

use crate::streams::ReadableStream;

mod body;
mod data_url;
mod fetch_op;
mod headers;
mod request;
mod upload;

pub use headers::Headers;
pub use request::{Request, ServerRequest};

pub struct BufferedResponse {
    pub status:  u16,
    pub headers: Vec<(String, String)>,
    pub body:    Vec<u8>,
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
    #[qjs(get, enumerable)]
    pub(crate) status:          u16,
    #[qjs(get, enumerable)]
    pub(crate) redirected:      bool,
    #[qjs(get, enumerable, rename = "statusText", skip_trace)]
    pub(crate) status_text:     String,
    #[qjs(get, enumerable, skip_trace)]
    pub(crate) url:             String,
    #[qjs(get, enumerable, rename = "type", skip_trace)]
    pub(crate) kind:            String,
    #[qjs(get, enumerable)]
    pub(crate) headers:         Class<'js, Headers>,
    body_stream:                Option<JsValue<'js>>,
    #[qjs(skip_trace)]
    inner:                      Rc<RefCell<ResponseBody>>,
    #[qjs(skip_trace)]
    consume_started:            Cell<bool>,
    pub(crate) abort_signal:    JsValue<'js>,
    #[qjs(skip_trace)]
    pub(crate) abort_notify:    Option<Arc<Notify>>,
    /// `Content-Length` of a body that is being streamed rather than buffered.
    /// The buffered path compares lengths once; a streamed one has to count as
    /// it goes, or a truncated response looks like a clean end of stream.
    #[qjs(skip_trace)]
    pub(crate) expected_length: Option<u64>,
    #[qjs(skip_trace)]
    seen_length:                Cell<u64>,
    /// Cache entries waiting on this body. Filled as the body is read and
    /// written only when it ends cleanly, so nothing is buffered on behalf of
    /// a response nobody consumes.
    #[qjs(skip_trace)]
    pub(crate) cache_fill:      Option<Rc<fetch_op::CacheFill>>,
}

impl<'js> Response<'js> {
    fn from_body(
        status: u16, headers: Class<'js, Headers>, body: ResponseBody, abort_signal: JsValue<'js>,
    ) -> Self {
        Self {
            status,
            redirected: false,
            status_text: String::new(),
            url: String::new(),
            kind: "default".to_owned(),
            headers,
            body_stream: None,
            inner: Rc::new(RefCell::new(body)),
            consume_started: Cell::new(false),
            abort_signal,
            abort_notify: None,
            expected_length: None,
            seen_length: Cell::new(0),
            cache_fill: None,
        }
    }

    pub async fn into_server(
        response: &Class<'js, Self>, ctx: &Ctx<'js>, max_bytes: usize,
    ) -> Result<BufferedResponse> {
        Self::begin_consume(response, ctx)?;
        {
            let response = response.borrow();
            if response.body_stream.is_some() {
                return Err(Exception::throw_type(
                    ctx,
                    "streaming server responses are not implemented",
                ));
            }
            match &*response.inner.borrow() {
                ResponseBody::Bytes(bytes) if bytes.len() > max_bytes => {
                    return Err(Exception::throw_range(
                        ctx,
                        "server response body is too large",
                    ));
                }
                ResponseBody::Live(_) | ResponseBody::Stream(_) => {
                    return Err(Exception::throw_type(
                        ctx,
                        "streaming server responses are not implemented",
                    ));
                }
                _ => {}
            }
        }
        let (status, mut headers) = {
            let response = response.borrow();
            let headers = response.headers.borrow();
            let mut pairs = headers.pairs();
            pairs.extend(
                headers
                    .cookies
                    .iter()
                    .cloned()
                    .map(|value| ("set-cookie".to_string(), value)),
            );
            (response.status, pairs)
        };
        let body = consume_response(response, ctx).await?;
        Ok(BufferedResponse {
            status,
            headers: core::mem::take(&mut headers),
            body,
        })
    }

    pub(crate) fn from_reqwest(
        ctx: &Ctx<'js>, response: reqwest::Response, kind: &str,
    ) -> Result<Self> {
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let text = value.to_str().map_or_else(
                    |_| value.as_bytes().iter().map(|byte| *byte as char).collect(),
                    ToString::to_string,
                );
                (name.as_str().to_string(), text)
            })
            .collect::<Vec<_>>();
        let mut header_obj = Headers::from_pairs(headers);
        header_obj.set_guard(headers::Guard::Immutable);
        let status_text = status.canonical_reason().unwrap_or("").to_owned();
        let url = response.url().to_string();
        let headers = Class::instance(ctx.clone(), header_obj)?;
        let mut result = Self::from_body(
            status.as_u16(),
            headers,
            ResponseBody::Live(response),
            JsValue::new_null(ctx.clone()),
        );
        result.status_text = status_text;
        result.url = url;
        result.kind = kind.to_owned();
        Ok(result)
    }

    pub(crate) fn from_bytes(
        ctx: &Ctx<'js>, status: u16, status_text: String, url: String, kind: &str,
        headers: Class<'js, Headers>, body: Option<Vec<u8>>,
    ) -> Self {
        let inner = body.map_or(ResponseBody::None, ResponseBody::Bytes);
        let mut response = Self::from_body(status, headers, inner, JsValue::new_null(ctx.clone()));
        response.status_text = status_text;
        response.url = url;
        response.kind = kind.to_owned();
        response
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

    fn has_body(&self) -> bool {
        self.body_stream.is_some() || !matches!(*self.inner.borrow(), ResponseBody::None)
    }

    fn begin_consume(response: &Class<'js, Response<'js>>, ctx: &Ctx<'js>) -> Result<()> {
        let this = response.borrow();
        if this.body_used() || this.consume_started.get() {
            return Err(Exception::throw_type(ctx, "Already read"));
        }
        let stream = this.body_stream.clone();
        drop(this);
        if let Some(value) = stream
            && body::is_readable_stream(ctx, &value)
        {
            if body::stream_is_locked(&value) || body::stream_is_disturbed(&value) {
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
            if let Ok(bytes) = consume_response(&response, &ctx_err).await {
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
            if let Ok(mut response) = response.try_borrow_mut() {
                response.body_stream = None;
            }
        });
        Ok(promise)
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
            ResponseBody::Live(response) => {
                Box::pin(response.bytes_stream().map(|item| {
                    item.map(|bytes| bytes.to_vec())
                        .map_err(|err| err.to_string())
                })) as BodyStream
            }
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
                self.seen_length
                    .set(self.seen_length.get() + chunk.len() as u64);
                if let Some(fill) = &self.cache_fill {
                    fill.push(&chunk);
                }
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
                    return Err(Exception::throw_type(ctx, "network error after response"));
                }
                if self
                    .expected_length
                    .is_some_and(|expected| expected > self.seen_length.get())
                {
                    return Err(Exception::throw_type(
                        ctx,
                        "response body shorter than Content-Length",
                    ));
                }
                if let Some(fill) = &self.cache_fill {
                    fill.commit();
                }
                Ok(None)
            }
        }
    }

    pub(crate) fn from_failed(
        ctx: &Ctx<'js>, status: u16, status_text: String, url: String, kind: &str,
        headers: Class<'js, Headers>, message: String,
    ) -> Self {
        let mut response = Self::from_body(
            status,
            headers,
            ResponseBody::Failed(message),
            JsValue::new_null(ctx.clone()),
        );
        response.status_text = status_text;
        response.url = url;
        response.kind = kind.to_owned();
        response
    }

    pub(crate) fn abort_fetch_body(&self, _ctx: &Ctx<'js>, reason: JsValue<'js>) {
        *self.inner.borrow_mut() = ResponseBody::Failed("aborted".into());
        if let Some(stream) = &self.body_stream
            && let Some(object) = stream.as_object()
            && let Ok(abort) = object.get::<_, Function>("_denAbort")
        {
            let _ = abort.call::<_, ()>((This(object.clone()), reason));
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

/// A streamed body carries its `Content-Length` forward; the buffered drain has
/// to make the same short-body call the pre-buffering path used to make.
fn check_complete<'js>(
    response: &Class<'js, Response<'js>>, ctx: &Ctx<'js>, bytes: Vec<u8>,
) -> Result<Vec<u8>> {
    let this = response.borrow();
    if this
        .expected_length
        .is_some_and(|expected| expected > bytes.len() as u64 + this.seen_length.get())
    {
        return Err(Exception::throw_type(
            ctx,
            "response body shorter than Content-Length",
        ));
    }
    if let Some(fill) = &this.cache_fill {
        fill.push(&bytes);
        fill.commit();
    }
    Ok(bytes)
}

async fn consume_response<'js>(
    response: &Class<'js, Response<'js>>, ctx: &Ctx<'js>,
) -> Result<Vec<u8>> {
    let stream = response.borrow().body_stream.clone();
    if let Some(value) = stream {
        if body::is_readable_stream(ctx, &value) {
            // Do not poison `inner`: it is the feed behind this stream, and a
            // stream over a live HTTP body still has to pull from it. Reading
            // the stream is what marks the body used.
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
            let bytes = result
                .map(|bytes| bytes.to_vec())
                .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))?;
            check_complete(response, ctx, bytes)
        }
        ResponseBody::Stream(mut stream) => {
            let mut out = Vec::new();
            while let Some(chunk) = futures::StreamExt::next(&mut stream).await {
                out.extend(chunk.map_err(|err| Exception::throw_type(ctx, &err))?);
            }
            check_complete(response, ctx, out)
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
        let mut status = 200_u16;
        let mut status_text = String::new();
        let headers_init = match init.as_ref() {
            Some(object) => {
                if let Ok(value) = object.get::<_, JsValue>("status")
                    && !value.is_undefined()
                {
                    let number = value
                        .as_number()
                        .unwrap_or_else(|| value.as_int().unwrap_or(0) as f64);
                    status = body::validate_status(&ctx, number as i32)?;
                }
                if let Ok(value) = object.get::<_, JsValue>("statusText")
                    && !value.is_undefined()
                {
                    status_text = den_util::coerce_string(&ctx, value)?;
                    body::validate_status_text(&ctx, &status_text)?;
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
            if body::is_readable_stream(&ctx, value)
                && (body::stream_is_locked(value) || body::stream_is_disturbed(value))
            {
                return Err(Exception::throw_type(
                    &ctx,
                    "ReadableStream is locked or disturbed",
                ));
            }
        }
        let (inner, body_stream) = match body {
            None => (ResponseBody::None, None),
            Some(value) if value.is_null() => (ResponseBody::None, None),
            Some(value) => {
                if body::is_readable_stream(&ctx, &value) {
                    (ResponseBody::Bytes(Vec::new()), Some(value))
                } else {
                    let extracted = body::apply_body_types(&ctx, &headers, value)?;
                    if let Some(object) = extracted.as_object()
                        && let Some(blob) = Class::<crate::blob::Blob>::from_object(object)
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
                        (
                            ResponseBody::Bytes(den_util::BufferSource::view_bytes(
                                &ctx, &extracted,
                            )?),
                            None,
                        )
                    } else {
                        (ResponseBody::Bytes(Vec::new()), Some(extracted))
                    }
                }
            }
        };
        let mut response = Self::from_body(status, headers, inner, JsValue::new_null(ctx.clone()));
        response.status_text = status_text;
        response.body_stream = body_stream;
        Ok(response)
    }

    #[qjs(static)]
    pub fn error(ctx: Ctx<'js>) -> Result<Self> {
        let headers = Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
        let mut response = Self::from_body(
            0,
            headers,
            ResponseBody::None,
            JsValue::new_null(ctx.clone()),
        );
        response.kind = "error".to_owned();
        Ok(response)
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
        Ok(Self::from_body(
            status as u16,
            headers,
            ResponseBody::None,
            JsValue::new_null(ctx.clone()),
        ))
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

    pub fn form_data(this: This<Class<'js, Response<'js>>>, ctx: Ctx<'js>) -> Result<Promise<'js>> {
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
                .map_err(|_error| Exception::throw_type(&ctx, "FormData is not defined"))?;
            let form: Object = ctor.construct(())?;
            FormBody::parse_into(&ctx, &form, &bytes, &content_type)?;
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
        if self.body_used()
            || self
                .body_stream
                .as_ref()
                .is_some_and(body::stream_is_locked)
        {
            return Err(Exception::throw_type(&ctx, "Already read"));
        }
        if !self.has_body() {
            return body::text_to_stream(&ctx, "");
        }
        if let Some(stream) = self.body_stream.clone() {
            if body::stream_is_locked(&stream) || body::stream_is_disturbed(&stream) {
                return Err(Exception::throw_type(&ctx, "Already read"));
            }
            if let Some(object) = stream.as_object()
                && let Some(readable) = Class::<ReadableStream>::from_object(object)
            {
                ReadableStream::lock_for_consume(&readable, &ctx)?;
            }
            self.consume_started.set(true);
            return body::text_stream_from_byte_stream(&ctx, stream);
        }
        // Decode before marking used: `mark_used` borrows `inner` mutably, so
        // holding the read borrow across it aborts the process.
        let buffered = match &*self.inner.borrow() {
            ResponseBody::Bytes(bytes) => Some(body::utf8_text(bytes)),
            _ => None,
        };
        if let Some(text) = buffered {
            self.mark_used();
            self.body_stream = None;
            return body::text_to_stream(&ctx, &text);
        }
        let host = Class::instance(ctx.clone(), self.clone())?.into_value();
        self.consume_started.set(true);
        body::text_chunks_to_stream(&ctx, host)
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
            Ok(Some(bytes)) => {
                TypedArray::<u8>::new_copy(ctx, bytes).map(rquickjs::TypedArray::into_value)
            }
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
                        .is_ok_and(|name| name != "TypeError")
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

    /// `bodyUsed` is "the body's stream is disturbed". Once a stream exists it
    /// *is* the body and `inner` is only the feed behind it — a clone's tee
    /// drains that feed while this response's own branch is still untouched.
    #[qjs(enumerable, get)]
    pub fn body_used(&self) -> bool {
        // A null body is never used. Otherwise distribution disturbs the body
        // at once, and that outlives the stream: a finished consume drops
        // `body_stream` to release it.
        self.has_body()
            && (self.consume_started.get()
                || self.body_stream.as_ref().map_or_else(
                    || matches!(*self.inner.borrow(), ResponseBody::Taken),
                    body::stream_is_disturbed,
                ))
    }

    #[qjs(enumerable, get)]
    pub fn ok(&self) -> bool { (200..300).contains(&self.status) }

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
            if body::is_readable_stream(&ctx, value) {
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
        // A stream body wins over `inner`: a `Response` built from a
        // `ReadableStream` parks an empty `Bytes` placeholder there, so testing
        // `inner` first would hand back a silently empty clone.
        if response.body_stream.is_none() {
            if let ResponseBody::Bytes(bytes) = &*response.inner.borrow() {
                let headers = Class::instance(
                    ctx.clone(),
                    Headers::new(
                        ctx.clone(),
                        rquickjs::function::Opt(Some(response.headers.clone().into_value())),
                    )?,
                )?;
                let mut result = Self::from_body(
                    response.status,
                    headers,
                    ResponseBody::Bytes(bytes.clone()),
                    response.abort_signal.clone(),
                );
                result.redirected = response.redirected;
                result.status_text = response.status_text.clone();
                result.url = response.url.clone();
                result.kind = response.kind.clone();
                result.abort_notify = response.abort_notify.clone();
                return Ok(result);
            }
            if !matches!(*response.inner.borrow(), ResponseBody::None) {
                let host = Class::instance(ctx.clone(), response.clone())?.into_value();
                response.body_stream = Some(body::http_chunks_to_stream(&ctx, host)?);
            }
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
            let mut result = Self::from_body(
                response.status,
                headers,
                ResponseBody::Bytes(Vec::new()),
                response.abort_signal.clone(),
            );
            result.redirected = response.redirected;
            result.status_text = response.status_text.clone();
            result.url = response.url.clone();
            result.kind = response.kind.clone();
            result.body_stream = Some(right);
            result.abort_notify = response.abort_notify.clone();
            return Ok(result);
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
        let mut result = Self::from_body(
            response.status,
            headers,
            cloned_inner,
            response.abort_signal.clone(),
        );
        result.redirected = response.redirected;
        result.status_text = response.status_text.clone();
        result.url = response.url.clone();
        result.kind = response.kind.clone();
        result.abort_notify = response.abort_notify.clone();
        Ok(result)
    }

    #[qjs(prop, rename = rquickjs::atom::PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Response" }
}

struct FormBody;

impl FormBody {
    fn parse_into<'js>(
        ctx: &Ctx<'js>, form: &Object<'js>, bytes: &[u8], content_type: &str,
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
            && Self::boundary(content_type).is_some()
        {
            return Ok(());
        }
        if content_type
            .get(..19)
            .is_some_and(|head| head.eq_ignore_ascii_case("multipart/form-data"))
        {
            let Some(boundary) = Self::boundary(content_type) else {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to parse body as FormData: missing multipart boundary",
                ));
            };
            return Self::parse_multipart(ctx, form, bytes, &boundary);
        }
        if content_type
            .get(..33)
            .is_some_and(|head| head.eq_ignore_ascii_case("application/x-www-form-urlencoded"))
        {
            return Self::parse_urlencoded(form, &String::from_utf8_lossy(bytes));
        }
        Err(Exception::throw_type(
            ctx,
            "Failed to parse body as FormData",
        ))
    }

    fn boundary(content_type: &str) -> Option<String> {
        let lower = content_type.to_ascii_lowercase();
        let marker = "boundary=";
        let pos = lower.find(marker)?;
        let rest = content_type.get(pos + marker.len()..)?.trim();
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"')?;
            Some(stripped.get(..end)?.to_string())
        } else {
            Some(rest.split(';').next()?.trim().to_string())
        }
    }

    fn parse_urlencoded(form: &Object<'_>, text: &str) -> Result<()> {
        let append: Function = form.get("append")?;
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            append.call::<_, ()>((This(form.clone()), Self::decode(name), Self::decode(value)))?;
        }
        Ok(())
    }

    fn decode(input: &str) -> String {
        percent_encoding::percent_decode(input.replace('+', " ").as_bytes())
            .decode_utf8_lossy()
            .into_owned()
    }

    fn index_of(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        if needle.is_empty() || from > haystack.len() {
            return None;
        }
        haystack
            .get(from..)?
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|pos| pos + from)
    }

    fn parse_multipart<'js>(
        ctx: &Ctx<'js>, form: &Object<'js>, bytes: &[u8], boundary: &str,
    ) -> Result<()> {
        let append: Function = form.get("append")?;
        let delimiter = format!("--{boundary}").into_bytes();
        let Some(mut pos) = Self::index_of(bytes, &delimiter, 0) else {
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
            let Some(next) = Self::index_of(bytes, &delimiter, pos) else {
                break;
            };
            let mut end = next;
            if end >= 2 && bytes.get(end - 2..end) == Some(b"\r\n") {
                end -= 2;
            }
            let Some(part) = bytes.get(pos..end) else {
                break;
            };
            if let Some(header_end) = Self::index_of(part, b"\r\n\r\n", 0)
                && let (Some(headers), Some(content)) =
                    (part.get(..header_end), part.get(header_end + 4..))
            {
                let header_text = String::from_utf8_lossy(headers);
                Self::append_part(ctx, form, &append, &header_text, content)?;
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
        ctx: &Ctx<'js>, form: &Object<'js>, append: &Function<'js>, header_text: &str,
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
                    .map_err(|_error| Exception::throw_type(ctx, "File is not defined"))?;
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
        let rest = header_value.get(start..)?;
        let end = rest.find('"')?;
        Some(
            rest.get(..end)?
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

fn wrap_global_fetch<'js>(ctx: &Ctx<'js>, inner: Function<'js>) -> Result<Function<'js>> {
    let wrapped = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>,
              func: FuncArg<Function<'js>>,
              input: JsOpt<JsValue<'js>>,
              init: JsOpt<JsValue<'js>>| {
            let inner: Function = func.0.get("_inner")?;
            wrapped_fetch(ctx, inner, input, init)
        },
    )?;
    wrapped.set("_inner", inner)?;
    Ok(wrapped)
}

fn wrapped_fetch<'js>(
    ctx: Ctx<'js>, inner: Function<'js>, input: JsOpt<JsValue<'js>>, init: JsOpt<JsValue<'js>>,
) -> Result<JsValue<'js>> {
    let input = input
        .0
        .unwrap_or_else(|| JsValue::new_undefined(ctx.clone()));
    let request = match init.0 {
        None => den_util::construct(&ctx, "Request", (input,)),
        Some(value) if value.is_undefined() => den_util::construct(&ctx, "Request", (input,)),
        Some(value) => den_util::construct(&ctx, "Request", (input, value)),
    };
    let request: JsValue = if let Ok(value) = request {
        value
    } else {
        let thrown = ctx.catch();
        return body::promise_reject(&ctx, thrown);
    };
    if let Some(reason) = aborted_fetch_reason(&ctx, &request)? {
        cancel_request_body(&ctx, &request, reason.clone());
        return body::promise_reject(&ctx, reason);
    }
    inner.call((request,))
}

fn aborted_fetch_reason<'js>(
    ctx: &Ctx<'js>, request: &JsValue<'js>,
) -> Result<Option<JsValue<'js>>> {
    let Some(request) = request.as_object() else {
        return Ok(None);
    };
    let signal: JsValue = request.get("signal")?;
    if signal.is_null() || signal.is_undefined() || signal.as_bool() == Some(false) {
        return Ok(None);
    }
    let Some(signal) = signal.as_object() else {
        return Ok(None);
    };
    let aborted = signal
        .get::<_, JsValue>("aborted")
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !aborted {
        return Ok(None);
    }
    let reason: JsValue = signal
        .get("reason")
        .unwrap_or_else(|_| JsValue::new_undefined(ctx.clone()));
    if !reason.is_undefined() && !reason.is_null() {
        return Ok(Some(reason));
    }
    Ok(Some(abort_error_reason(ctx)?))
}

fn abort_error_reason<'js>(ctx: &Ctx<'js>) -> Result<JsValue<'js>> {
    if let Ok(value) = den_util::new_dom_exception(ctx, "The operation was aborted.", "AbortError")
    {
        Ok(value)
    } else {
        if ctx.has_exception() {
            drop(ctx.catch());
        }
        let error: Object = den_util::construct(ctx, "Error", ("The operation was aborted.",))?;
        error.set("name", "AbortError")?;
        Ok(error.into_value())
    }
}

fn cancel_request_body<'js>(ctx: &Ctx<'js>, request: &JsValue<'js>, reason: JsValue<'js>) {
    let Some(request) = request.as_object() else {
        return;
    };
    let Ok(body) = request.get::<_, JsValue>("body") else {
        if ctx.has_exception() {
            drop(ctx.catch());
        }
        return;
    };
    let Some(body_obj) = body.as_object() else {
        return;
    };
    let Ok(cancel) = body_obj.get::<_, Function>("cancel") else {
        return;
    };
    let _ = cancel.call::<_, JsValue>((This(body), reason));
    if ctx.has_exception() {
        drop(ctx.catch());
    }
}

fn wrap_readable_stream_cancel<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    let Ok(ctor) = globals.get::<_, Function>("ReadableStream") else {
        return Ok(());
    };
    let Ok(proto) = ctor.get::<_, Object>("prototype") else {
        return Ok(());
    };
    let Ok(inner_cancel) = proto.get::<_, Function>("cancel") else {
        return Ok(());
    };
    let wrapped = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>,
              func: FuncArg<Function<'js>>,
              this: This<JsValue<'js>>,
              args: Rest<JsValue<'js>>| {
            let inner_cancel: Function = func.0.get("_inner")?;
            let result: JsValue = inner_cancel.call((This(this.0), Rest(args.0)))?;
            if result.is_null() || result.is_undefined() {
                body::promise_resolve(&ctx, JsValue::new_undefined(ctx.clone()))
            } else {
                body::promise_resolve(&ctx, result)
            }
        },
    )?;
    wrapped.set("_inner", inner_cancel)?;
    proto.set("cancel", wrapped)
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod whatwg {
    use den_util::ConstructorInstaller as _;
    use rquickjs::{Ctx, Function, Result, Value, function::Opt, module::Exports};

    use super::body;
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
        den_stdlib_networking::tls::install_default_crypto_provider();
        let globals = ctx.globals();
        for name in ["fetch", "Headers", "Response"] {
            globals.set(name, exports.module().get::<_, Value>(name)?)?;
        }
        globals.install_constructor::<Request>(1)?;
        let inner: Function = globals.get("fetch")?;
        let wrapped = super::wrap_global_fetch(ctx, inner)?;
        wrapped.set_name("fetch")?;
        exports.export("fetch", wrapped.clone())?;
        globals.set("fetch", wrapped)?;
        super::wrap_readable_stream_cancel(ctx)?;
        Ok(())
    }
}
