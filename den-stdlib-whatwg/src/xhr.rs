//! WHATWG XMLHttpRequest on top of `fetch()`.

use std::{cell::Cell, fmt::Write as _, rc::Rc, time::Duration};

use rquickjs::{
    ArrayBuffer, Class, Constructor, Ctx, Function, JsLifetime, Object, Promise, Result,
    TypedArray, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

use crate::host::Host;

const UNSENT: i32 = 0;
const OPENED: i32 = 1;
const HEADERS_RECEIVED: i32 = 2;
const LOADING: i32 = 3;
const DONE: i32 = 4;

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename_all = "camelCase")]
pub struct XMLHttpRequest {
    #[qjs(get)]
    ready_state:      i32,
    #[qjs(get)]
    status:           u16,
    #[qjs(get)]
    status_text:      String,
    #[qjs(get)]
    response_url:     String,
    response_headers: String,
    #[qjs(skip_trace)]
    response_body:    Vec<u8>,
    #[qjs(get, set)]
    response_type:    String,
    override_charset: Option<String>,
    timeout:          u64,
    #[qjs(get, set)]
    with_credentials: bool,
    method:           String,
    url:              String,
    #[qjs(skip_trace)]
    request_headers:  Vec<(String, String)>,
    #[qjs(skip_trace)]
    aborted:          Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    timed_out:        Rc<Cell<bool>>,
}

impl XMLHttpRequest {
    fn dispatch<'js>(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        den_stdlib_worker::events::dispatch_trusted(
            ctx.clone(),
            this.as_inner().clone().into_value(),
            event,
        )?;
        Ok(())
    }

    fn set_ready_state<'js>(this: &Class<'js, Self>, ctx: &Ctx<'js>, state: i32) -> Result<()> {
        if this.borrow().ready_state != state {
            this.borrow_mut().ready_state = state;
            Self::dispatch(this, ctx, Host::event(ctx, "readystatechange")?)?;
        }
        Ok(())
    }

    fn charset(mime: &str) -> Option<String> {
        let lower = mime.to_ascii_lowercase();
        let marker = "charset=";
        let pos = lower.find(marker)?;
        let rest = mime.get(pos + marker.len()..)?.trim();
        let rest = rest
            .strip_prefix('"')
            .map_or(rest, |value| value.split('"').next().unwrap_or(value));
        let value = rest.split(';').next().unwrap_or(rest).trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }

    fn decode<'js>(this: &Class<'js, Self>, ctx: &Ctx<'js>) -> String {
        let (override_charset, body, header_charset) = {
            let xhr = this.borrow();
            (
                xhr.override_charset.clone(),
                xhr.response_body.clone(),
                xhr.response_header("content-type")
                    .and_then(|mime| Self::charset(&mime)),
            )
        };
        if let Some(text) = override_charset
            .or(header_charset)
            .and_then(|label| Self::decode_with_label(ctx, &body, &label))
        {
            return text;
        }
        Self::decode_with_label(ctx, &body, "utf-8")
            .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned())
    }

    fn decode_with_label(ctx: &Ctx<'_>, body: &[u8], label: &str) -> Option<String> {
        let constructor = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("TextDecoder")
            .ok()?;
        let decoder = constructor.construct::<_, Object>((label,)).ok()?;
        let decode = decoder.get::<_, Function>("decode").ok()?;
        let view = TypedArray::<u8>::new_copy(ctx.clone(), body).ok()?;
        decode.call::<_, String>((This(decoder), view)).ok()
    }
}

impl XMLHttpRequest {
    fn response_header(&self, name: &str) -> Option<String> {
        let lower_name = name.to_ascii_lowercase();
        let mut values = Vec::new();
        for line in self.response_headers.split("\r\n") {
            let Some((header, value)) = line.split_once(':') else {
                continue;
            };
            if header.trim() == lower_name {
                let value = value.trim();
                if !value.is_empty() {
                    values.push(value.to_string());
                }
            }
        }
        if values.is_empty() {
            None
        } else {
            Some(values.join(", "))
        }
    }
}

impl Default for XMLHttpRequest {
    fn default() -> Self {
        Self {
            ready_state:      UNSENT,
            status:           0,
            status_text:      String::new(),
            response_url:     String::new(),
            response_headers: String::new(),
            response_body:    Vec::new(),
            response_type:    String::new(),
            override_charset: None,
            timeout:          0,
            with_credentials: false,
            method:           String::new(),
            url:              String::new(),
            request_headers:  Vec::new(),
            aborted:          Rc::new(Cell::new(false)),
            timed_out:        Rc::new(Cell::new(false)),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl XMLHttpRequest {
    #[qjs(constructor)]
    pub fn new() -> Self { Self::default() }

    #[qjs(static, get, rename = "UNSENT")]
    pub const fn unsent_const() -> i32 { UNSENT }

    #[qjs(static, get, rename = "OPENED")]
    pub const fn opened_const() -> i32 { OPENED }

    #[qjs(static, get, rename = "HEADERS_RECEIVED")]
    pub const fn headers_received_const() -> i32 { HEADERS_RECEIVED }

    #[qjs(static, get, rename = "LOADING")]
    pub const fn loading_const() -> i32 { LOADING }

    #[qjs(static, get, rename = "DONE")]
    pub const fn done_const() -> i32 { DONE }

    #[qjs(get)]
    pub const fn timeout(&self) -> u64 { self.timeout }

    #[qjs(set, rename = "timeout")]
    pub fn set_timeout(&mut self, value: f64) {
        self.timeout = if value.is_finite() && value > 0.0 {
            value as u64
        } else {
            0
        };
    }

    #[qjs(get)]
    pub fn upload<'js>(&self, ctx: Ctx<'js>) -> Value<'js> { Value::new_undefined(ctx) }

    #[qjs(get, rename = "responseXML")]
    pub fn response_xml<'js>(&self, ctx: Ctx<'js>) -> Value<'js> { Value::new_null(ctx) }

    #[qjs(get)]
    pub fn response<'js>(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let ready = this.0.borrow().ready_state;
        let response_type = this.0.borrow().response_type.clone();
        if ready != DONE {
            return Ok(if response_type.is_empty() || response_type == "text" {
                rquickjs::IntoJs::into_js(String::new(), &ctx)?
            } else {
                Value::new_null(ctx)
            });
        }
        match response_type.as_str() {
            "" | "text" => rquickjs::IntoJs::into_js(Self::decode(&this.0, &ctx), &ctx),
            "arraybuffer" => {
                let body = this.0.borrow().response_body.clone();
                ArrayBuffer::new_copy(ctx, body).map(rquickjs::ArrayBuffer::into_value)
            }
            "json" => {
                let text = Self::decode(&this.0, &ctx);
                ctx.json_parse(text)
                    .map_or_else(|_error| Ok(Value::new_null(ctx)), Ok)
            }
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get)]
    pub fn response_text<'js>(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<String> {
        let (response_type, ready) = {
            let xhr = this.0.borrow();
            (xhr.response_type.clone(), xhr.ready_state)
        };
        if !response_type.is_empty() && response_type != "text" {
            return Err(Host::throw_dom(
                &ctx,
                "Failed to read responseText: responseType is not text",
                "InvalidStateError",
            ));
        }
        if ready != LOADING && ready != DONE {
            return Ok(String::new());
        }
        Ok(Self::decode(&this.0, &ctx))
    }

    pub fn get_all_response_headers(&self) -> String { self.response_headers.clone() }

    pub fn get_response_header<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
        match self.response_header(&name) {
            Some(value) => rquickjs::IntoJs::into_js(value, &ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn open<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, method: String, url: String,
        async_flag: Opt<Value<'js>>,
    ) -> Result<()> {
        if async_flag.0.as_ref().and_then(Value::as_bool) == Some(false) {
            return Err(Host::throw_type(&ctx, "Synchronous XHR is not supported"));
        }
        {
            let mut xhr = this.0.borrow_mut();
            xhr.ready_state = UNSENT;
            xhr.status = 0;
            xhr.status_text.clear();
            xhr.response_url.clear();
            xhr.response_headers.clear();
            xhr.response_body.clear();
            xhr.method = method;
            xhr.url = url;
            xhr.request_headers.clear();
        }
        Self::set_ready_state(&this.0, &ctx, OPENED)
    }

    pub fn override_mime_type<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, mime: String,
    ) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == LOADING || ready == DONE {
            return Err(Host::throw_dom(
                &ctx,
                "overrideMimeType cannot be called in the LOADING or DONE state",
                "InvalidStateError",
            ));
        }
        this.0.borrow_mut().override_charset = Self::charset(&mime);
        Ok(())
    }

    pub fn set_request_header<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, name: String, value: String,
    ) -> Result<()> {
        if this.0.borrow().ready_state != OPENED {
            return Err(Host::throw_dom(
                &ctx,
                "Failed to execute setRequestHeader: the object's state must be OPENED",
                "InvalidStateError",
            ));
        }
        if name.contains('\0') || value.contains('\0') {
            return Err(Host::throw_dom(
                &ctx,
                "The header contains an invalid NUL character",
                "SyntaxError",
            ));
        }
        this.0.borrow_mut().request_headers.push((name, value));
        Ok(())
    }

    pub fn abort(this: This<Class<'_, Self>>) { this.0.borrow().aborted.set(true); }

    pub fn send<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, body: Opt<Value<'js>>,
    ) -> Result<()> {
        if this.0.borrow().ready_state != OPENED {
            return Err(Host::throw_dom(
                &ctx,
                "Failed to execute send: the object's state must be OPENED",
                "InvalidStateError",
            ));
        }
        Self::dispatch(
            &this.0,
            &ctx,
            Host::progress_event(&ctx, "loadstart", false, 0.0, 0.0)?,
        )?;
        let aborted = Rc::new(Cell::new(false));
        let timed_out = Rc::new(Cell::new(false));
        this.0.borrow_mut().aborted = Rc::clone(&aborted);
        this.0.borrow_mut().timed_out = Rc::clone(&timed_out);
        let (method, url, headers, timeout) = {
            let xhr = this.0.borrow();
            (
                xhr.method.clone(),
                xhr.url.clone(),
                xhr.request_headers.clone(),
                xhr.timeout,
            )
        };
        let fetch: Function = ctx
            .globals()
            .get("fetch")
            .map_err(|_error| Host::throw_type(&ctx, "fetch is not defined"))?;
        let init = Object::new(ctx.clone())?;
        init.set("method", method.clone())?;
        let header_pairs = rquickjs::Array::new(ctx.clone())?;
        for (index, (name, value)) in headers.iter().enumerate() {
            let pair = rquickjs::Array::new(ctx.clone())?;
            pair.set(0, name.clone())?;
            pair.set(1, value.clone())?;
            header_pairs.set(index, pair)?;
        }
        init.set("headers", header_pairs)?;
        let abort = match AbortSignal::create(&ctx)? {
            Some((signal, abort)) => {
                init.set("signal", signal)?;
                Some(abort)
            }
            None => None,
        };
        let payload = body
            .0
            .filter(|value| !value.is_null() && !value.is_undefined());
        if let Some(payload) = payload
            && method != "GET"
            && method != "HEAD"
        {
            init.set("body", payload)?;
        }
        if timeout > 0
            && let Some(abort) = abort.as_ref()
        {
            let abort = abort.clone();
            let timed_out = Rc::clone(&timed_out);
            let aborted = Rc::clone(&aborted);
            ctx.spawn(async move {
                tokio::time::sleep(Duration::from_millis(timeout)).await;
                if !aborted.get() {
                    timed_out.set(true);
                    let _ = abort.call::<_, ()>(());
                }
            });
        }
        let promise: Promise = fetch.call((url, init))?;
        let this = this.0.clone();
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                match promise.into_future::<Object>().await {
                    Ok(response) => {
                        if aborted.get() {
                            return;
                        }
                        {
                            let mut xhr = this.borrow_mut();
                            xhr.status = response.get("status").unwrap_or(0);
                            xhr.status_text = response.get("statusText").unwrap_or_default();
                            xhr.response_url = response.get("url").unwrap_or_default();
                            xhr.response_headers = XMLHttpRequest::headers_text(&ctx, &response);
                        }
                        let _ = XMLHttpRequest::set_ready_state(&this, &ctx, HEADERS_RECEIVED);
                        let _ = XMLHttpRequest::set_ready_state(&this, &ctx, LOADING);
                        let Ok(bytes) = XMLHttpRequest::response_bytes(&ctx, &response).await
                        else {
                            XMLHttpRequest::fail(&this, &ctx, &aborted, &timed_out);
                            return;
                        };
                        if aborted.get() {
                            return;
                        }
                        let size = bytes.len() as f64;
                        this.borrow_mut().response_body = bytes;
                        let _ = XMLHttpRequest::dispatch(
                            &this,
                            &ctx,
                            Host::progress_event(&ctx, "progress", true, size, size)
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                        );
                        let _ = XMLHttpRequest::set_ready_state(&this, &ctx, DONE);
                        let _ = XMLHttpRequest::dispatch(
                            &this,
                            &ctx,
                            Host::event(&ctx, "load")
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                        );
                        let _ = XMLHttpRequest::dispatch(
                            &this,
                            &ctx,
                            Host::event(&ctx, "loadend")
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                        );
                    }
                    Err(_) => XMLHttpRequest::fail(&this, &ctx, &aborted, &timed_out),
                }
            }
        });
        let _ = abort;
        Ok(())
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "XMLHttpRequest" }
}

impl XMLHttpRequest {
    fn headers_text<'js>(ctx: &Ctx<'js>, response: &Object<'js>) -> String {
        let Ok(headers) = response.get::<_, Object>("headers") else {
            return String::new();
        };
        if let Ok(for_each) = headers.get::<_, Function>("forEach") {
            let collected = Rc::new(std::cell::RefCell::new(String::new()));
            let sink = Rc::clone(&collected);
            if let Ok(callback) = Function::new(
                ctx.clone(),
                move |value: String, name: String| -> Result<()> {
                    let _ = write!(sink.borrow_mut(), "{name}: {value}\r\n");
                    Ok(())
                },
            ) {
                let _ = for_each.call::<_, ()>((This(headers), callback));
            }
            return collected.take();
        }
        String::new()
    }

    async fn response_bytes<'js>(_ctx: &Ctx<'js>, response: &Object<'js>) -> Result<Vec<u8>> {
        let array_buffer: Function = response.get("arrayBuffer")?;
        let promise: Promise = array_buffer.call((This(response.clone()),))?;
        let buffer: ArrayBuffer = promise.into_future().await?;
        Ok(buffer.as_bytes().map(<[u8]>::to_vec).unwrap_or_default())
    }

    fn fail<'js>(
        this: &Class<'js, Self>, ctx: &Ctx<'js>, aborted: &Rc<Cell<bool>>,
        timed_out: &Rc<Cell<bool>>,
    ) {
        if timed_out.get() {
            let _ = Self::set_ready_state(this, ctx, DONE);
            let _ = Self::dispatch(
                this,
                ctx,
                Host::event(ctx, "timeout").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
            );
        } else if aborted.get() {
            this.borrow_mut().ready_state = UNSENT;
            this.borrow_mut().status = 0;
            this.borrow_mut().status_text.clear();
            let _ = Self::dispatch(
                this,
                ctx,
                Host::event(ctx, "abort").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
            );
        } else {
            let _ = Self::set_ready_state(this, ctx, DONE);
            let _ = Self::dispatch(
                this,
                ctx,
                Host::event(ctx, "error").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
            );
        }
        let _ = Self::dispatch(
            this,
            ctx,
            Host::event(ctx, "loadend").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
        );
    }
}

struct AbortSignal;

impl AbortSignal {
    /// A real `AbortSignal` from the realm's `AbortController`. A duck-typed
    /// stand-in would take fetch's `AbortSignal.any` path, which pins
    /// listener<->source cycles QuickJS's refcount GC cannot collect; realms
    /// without the worker module simply go without abort support.
    fn create<'js>(ctx: &Ctx<'js>) -> Result<Option<(Value<'js>, Function<'js>)>> {
        let Ok(ctor) = ctx.globals().get::<_, Constructor>("AbortController") else {
            return Ok(None);
        };
        let controller: Object<'js> = ctor.construct(())?;
        let signal: Value<'js> = controller.get("signal")?;
        let abort_method: Function<'js> = controller.get("abort")?;
        let abort = Function::new(ctx.clone(), move || -> Result<()> {
            abort_method.call::<_, ()>((This(controller.clone()),))
        })?;
        Ok(Some((signal, abort)))
    }
}
