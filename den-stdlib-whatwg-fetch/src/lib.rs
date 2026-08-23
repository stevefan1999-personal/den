use std::{
    cell::RefCell,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use derive_more::derive::{From, Into};
use futures::{Stream, StreamExt, future::Either};
use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Error, Exception, FromJs, Function, IntoJs, JsLifetime, Object,
    Result, TypedArray, Value as JsValue,
    class::Trace,
    function::{Constructor, This},
};
use serde_json::Value;
use tokio::sync::Notify;

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
    Live(reqwest::Response),
    Stream(BodyStream),
    Taken,
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "Response")]
pub struct Response {
    status: u16,
    redirected: bool,
    #[qjs(skip_trace)]
    status_text: String,
    #[qjs(skip_trace)]
    url: String,
    #[qjs(skip_trace)]
    headers: Vec<(String, String)>,
    #[qjs(skip_trace)]
    inner: Rc<RefCell<ResponseBody>>,
}

impl Response {
    fn from_reqwest(response: reqwest::Response) -> Self {
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        Self {
            status: status.as_u16(),
            redirected: false,
            status_text: status.canonical_reason().unwrap_or("").to_string(),
            url: response.url().to_string(),
            headers,
            inner: Rc::new(RefCell::new(ResponseBody::Live(response))),
        }
    }

    fn content_type(&self) -> String {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }

    async fn take_bytes(&self, ctx: &Ctx<'_>) -> Result<Vec<u8>> {
        let taken = {
            let mut inner = self.inner.borrow_mut();
            core::mem::replace(&mut *inner, ResponseBody::Taken)
        };
        match taken {
            ResponseBody::Live(response) => response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|err| Exception::throw_syntax(ctx, &format!("{err:?}"))),
            ResponseBody::Stream(mut stream) => {
                let mut out = Vec::new();
                while let Some(chunk) = stream.next().await {
                    out.extend(chunk.map_err(|err| Exception::throw_syntax(ctx, &err))?);
                }
                Ok(out)
            }
            ResponseBody::Taken => Err(Exception::throw_type(ctx, "Already distributed")),
        }
    }

    async fn next_chunk(&self, ctx: &Ctx<'_>) -> Result<Option<Vec<u8>>> {
        let taken = {
            let mut inner = self.inner.borrow_mut();
            core::mem::replace(&mut *inner, ResponseBody::Taken)
        };
        let mut stream = match taken {
            ResponseBody::Live(response) => Box::pin(response.bytes_stream().map(|item| {
                item.map(|bytes| bytes.to_vec())
                    .map_err(|err| err.to_string())
            })) as BodyStream,
            ResponseBody::Stream(stream) => stream,
            ResponseBody::Taken => return Ok(None),
        };
        match stream.next().await {
            Some(Ok(chunk)) => {
                *self.inner.borrow_mut() = ResponseBody::Stream(stream);
                Ok(Some(chunk))
            }
            Some(Err(err)) => Err(Exception::throw_syntax(ctx, &err)),
            None => Ok(None),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Response {
    // `Response::constructor` is what gets bound as the `Response` global,
    // and it only exists when the class declares a constructor. Returning
    // `()` makes `new Response()` throw, as only `fetch` produces one.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        let bytes = self.take_bytes(&ctx).await?;
        // `new_copy`, never `new`: `new` lends QuickJS the Rust allocation
        // plus a free hook it runs twice on detach (quickjs.c:58037 and
        // :57935), and `transfer` reallocs a pointer its allocator never
        // produced, so `(await r.arrayBuffer()).transfer()` aborted the
        // process. The cost is one extra copy of the body — paid to make it
        // an ordinary JS buffer that can be detached and transferred.
        ArrayBuffer::new_copy(ctx, bytes)
    }

    pub async fn blob<'js>(&self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        let bytes = self.take_bytes(&ctx).await?;
        let mime = self.content_type();
        let ctor: Constructor<'js> = ctx
            .globals()
            .get("Blob")
            .map_err(|_| Exception::throw_type(&ctx, "Blob is not defined"))?;
        let parts = Array::new(ctx.clone())?;
        parts.set(0, TypedArray::<u8>::new_copy(ctx.clone(), bytes)?)?;
        let opts = Object::new(ctx.clone())?;
        opts.set("type", mime)?;
        ctor.construct((parts, opts))
    }

    pub async fn bytes<'js>(&self, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        let bytes = self.take_bytes(&ctx).await?;
        TypedArray::new_copy(ctx, bytes)
    }

    pub async fn form_data<'js>(&self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        let bytes = self.take_bytes(&ctx).await?;
        let ctor: Constructor<'js> = ctx
            .globals()
            .get("FormData")
            .map_err(|_| Exception::throw_type(&ctx, "FormData is not defined"))?;
        let form: Object<'js> = ctor.construct(())?;
        FormBody.parse_into(&ctx, &form, &bytes, &self.content_type())?;
        Ok(form.into_value())
    }

    pub async fn json<'js>(&self, ctx: Ctx<'js>) -> Result<SerdeJsonValue> {
        let bytes = self.take_bytes(&ctx).await?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map(Into::into)
            .map_err(|err| Exception::throw_syntax(&ctx, &format!("{err:?}")))
    }

    pub async fn text<'js>(&self, ctx: Ctx<'js>) -> Result<String> {
        let bytes = self.take_bytes(&ctx).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Pull the next body chunk. EventSource uses this in place of
    /// `response.body.getReader()` because ReadableStream is installed later
    /// by `den:whatwg`.
    #[qjs(rename = "_readChunk")]
    pub async fn read_chunk<'js>(&self, ctx: Ctx<'js>) -> Result<JsValue<'js>> {
        match self.next_chunk(&ctx).await? {
            Some(bytes) => TypedArray::<u8>::new_copy(ctx, bytes).map(|view| view.into_value()),
            None => Ok(JsValue::new_null(ctx)),
        }
    }

    #[qjs(rename = "_cancelBody")]
    pub fn cancel_body(&self) {
        *self.inner.borrow_mut() = ResponseBody::Taken;
    }

    #[qjs(enumerable, get)]
    pub fn body_used(&self) -> bool {
        !matches!(*self.inner.borrow(), ResponseBody::Live(_))
    }

    #[qjs(enumerable, get)]
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    #[qjs(enumerable, get)]
    pub fn redirected(&self) -> bool {
        self.redirected
    }

    #[qjs(enumerable, get)]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[qjs(enumerable, get)]
    pub fn status_text(&self) -> String {
        self.status_text.clone()
    }

    #[qjs(enumerable, get)]
    pub fn url(&self) -> String {
        self.url.clone()
    }

    #[qjs(enumerable, get, rename = "type")]
    pub fn type_(&self) -> &'static str {
        "basic"
    }

    #[qjs(enumerable, get)]
    pub fn headers<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, Headers>> {
        Class::instance(ctx, Headers::from_pairs(self.headers.iter().cloned()))
    }
}

struct AbortWatch {
    aborted: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl AbortWatch {
    fn from_js<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> Result<Option<Self>> {
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        let Some(obj) = value.as_object() else {
            return Ok(None);
        };
        let already = obj.get::<_, JsValue>("aborted")?.as_bool().unwrap_or(false);
        let watch = Self {
            aborted: Arc::new(AtomicBool::new(already)),
            notify: Arc::new(Notify::new()),
        };
        if already {
            watch.notify.notify_one();
            return Ok(Some(watch));
        }
        if let Ok(add) = obj.get::<_, Function>("addEventListener") {
            let aborted = Arc::clone(&watch.aborted);
            let notify = Arc::clone(&watch.notify);
            let callback = Function::new(ctx.clone(), move || {
                aborted.store(true, Ordering::SeqCst);
                notify.notify_one();
            })?;
            add.call::<_, ()>((This(obj.clone()), "abort", callback))?;
        }
        Ok(Some(watch))
    }
}

struct FetchInit {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    signal: Option<AbortWatch>,
}

impl FetchInit {
    fn abort_error(ctx: &Ctx<'_>) -> Error {
        if let Ok(ctor) = ctx.globals().get::<_, Constructor>("DOMException")
            && let Ok(exc) =
                ctor.construct::<_, JsValue>(("The operation was aborted.", "AbortError"))
        {
            return ctx.throw(exc);
        }
        Exception::throw_type(ctx, "The operation was aborted.")
    }

    fn client() -> &'static reqwest::Client {
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        CLIENT.get_or_init(reqwest::Client::new)
    }

    async fn send(self, ctx: &Ctx<'_>) -> Result<Response> {
        if let Some(signal) = &self.signal
            && signal.aborted.load(Ordering::SeqCst)
        {
            return Err(Self::abort_error(ctx));
        }
        let method = reqwest::Method::from_bytes(self.method.as_bytes())
            .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))?;
        let mut builder = Self::client().request(method, &self.url);
        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = self.body {
            builder = builder.body(body);
        }
        let request = builder.send();
        let original_url = self.url.clone();
        let response = if let Some(signal) = self.signal {
            let abort = signal.notify.notified();
            futures::pin_mut!(request);
            futures::pin_mut!(abort);
            match futures::future::select(request, abort).await {
                Either::Left((result, _)) => {
                    result.map_err(|err| Exception::throw_internal(ctx, &format!("{err:?}")))?
                }
                Either::Right(_) => return Err(Self::abort_error(ctx)),
            }
        } else {
            request
                .await
                .map_err(|err| Exception::throw_internal(ctx, &format!("{err:?}")))?
        };
        let redirected = response.url().as_str() != original_url;
        let mut produced = Response::from_reqwest(response);
        produced.redirected = redirected;
        Ok(produced)
    }
}

struct FormBody;

impl FormBody {
    fn parse_into<'js>(
        &self,
        ctx: &Ctx<'js>,
        form: &Object<'js>,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<()> {
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
        self.parse_urlencoded(ctx, form, &String::from_utf8_lossy(bytes))
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
        &self,
        ctx: &Ctx<'js>,
        form: &Object<'js>,
        bytes: &[u8],
        boundary: &str,
    ) -> Result<()> {
        let append: Function = form.get("append")?;
        let delimiter = format!("--{boundary}").into_bytes();
        let Some(mut pos) = self.index_of(bytes, &delimiter, 0) else {
            return Ok(());
        };
        pos += delimiter.len();
        loop {
            if bytes.get(pos) == Some(&b'-') && bytes.get(pos + 1) == Some(&b'-') {
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
        Ok(())
    }

    fn append_part<'js>(
        &self,
        ctx: &Ctx<'js>,
        form: &Object<'js>,
        append: &Function<'js>,
        header_text: &str,
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
    ctx: Ctx<'js>,
    input: JsValue<'js>,
    init: Option<Object<'js>>,
) -> Result<Response> {
    let request = Request::wrap_input(ctx.clone(), input, init)?;
    let abort = AbortWatch::from_js(&ctx, request.borrow().signal.clone())?;
    if let Some(signal) = &abort
        && signal.aborted.load(Ordering::SeqCst)
    {
        return Err(FetchInit::abort_error(&ctx));
    }
    let (url, method, headers, consume_body) = {
        let request = request.borrow();
        let method = request.method.clone();
        let consume_body = method != "GET" && method != "HEAD";
        (
            request.url.clone(),
            method,
            request.headers.borrow().pairs(),
            consume_body,
        )
    };
    let body = if consume_body {
        let taken = request.borrow_mut().take_body(&ctx)?;
        let bytes = Request::body_to_bytes(&ctx, taken).await?;
        if bytes.is_empty() { None } else { Some(bytes) }
    } else {
        None
    };
    FetchInit {
        url,
        method,
        headers,
        body,
        signal: abort,
    }
    .send(&ctx)
    .await
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod whatwg {
    use rquickjs::{Ctx, Object, Result, Value, function::Opt, module::Exports};

    pub use super::{Headers, Request, Response};

    #[rquickjs::function]
    pub async fn fetch<'js>(
        ctx: Ctx<'js>,
        input: Value<'js>,
        init: Opt<Object<'js>>,
    ) -> Result<Response> {
        super::fetch(ctx, input, init.0).await
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        for name in ["fetch", "Headers", "Request", "Response"] {
            globals.set(name, exports.module().get::<_, Value>(name)?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod local_http;

#[cfg(test)]
mod tests {
    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Promise};

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
                let respond = || Response::from_reqwest(http::Response::new("body").into());
                let run = async {
                    let buffer = respond().array_buffer(ctx.clone()).await?;
                    let view = respond().bytes(ctx.clone()).await?;
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
        assert_eq!(
            report,
            vec![
                "Headers: ok".to_string(),
                "Request: ok".to_string(),
                "fetch: ok".to_string(),
                "Response: ok".to_string(),
            ]
        );
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
        let server = super::local_http::serve(|incoming| {
            if incoming.method == "POST" {
                super::local_http::Outgoing::ok(incoming.body, "text/plain")
            } else {
                super::local_http::Outgoing::ok(b"{\"ok\":true}".to_vec(), "application/json")
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
        assert_eq!(report, "200|true|200|ping|basic|text/plain");
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
        let server = super::local_http::serve(|_| super::local_http::Outgoing {
            status: 200,
            headers: vec![],
            body: Vec::new(),
            hang: false,
            silent: true,
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
                        http::Response::builder()
                            .header("content-type", "text/plain")
                            .body("hello")
                            .expect("response")
                            .into(),
                    );
                    let blob = response.blob(ctx.clone()).await?;
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
