//! WHATWG XMLHttpRequest on top of `fetch()`.

use std::{cell::Cell, rc::Rc, time::Duration};

use rquickjs::{
    ArrayBuffer, Class, Constructor, Ctx, Function, JsLifetime, Object, Promise, Result,
    TypedArray, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::{
    event_target::{HostEventTarget, SharedEvents},
    host::Host,
};

const UNSENT: i32 = 0;
const OPENED: i32 = 1;
const HEADERS_RECEIVED: i32 = 2;
const LOADING: i32 = 3;
const DONE: i32 = 4;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct XMLHttpRequest<'js> {
    events:           SharedEvents<'js>,
    ready_state:      i32,
    status:           u16,
    status_text:      String,
    response_url:     String,
    response_headers: String,
    #[qjs(skip_trace)]
    response_body:    Vec<u8>,
    response_type:    String,
    override_charset: Option<String>,
    timeout:          u64,
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

impl<'js> Trace<'js> for XMLHttpRequest<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(events) = self.events.try_borrow() {
            events.trace(tracer);
        }
    }
}

impl<'js> XMLHttpRequest<'js> {
    fn dispatch(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        HostEventTarget::dispatch_shared(&this.borrow().events, ctx, this.as_inner(), event)?;
        Ok(())
    }

    fn set_ready_state(this: &Class<'js, Self>, ctx: &Ctx<'js>, state: i32) -> Result<()> {
        if this.borrow().ready_state != state {
            this.borrow_mut().ready_state = state;
            Self::dispatch(this, ctx, Host::event(ctx, "readystatechange")?)?;
        }
        Ok(())
    }

    fn handler(this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str) -> Value<'js> {
        this.0.borrow().events.borrow().handler_or_null(&ctx, type_)
    }

    fn set_handler(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str, value: Value<'js>,
    ) -> Result<()> {
        this.0.borrow().events.borrow_mut().set_handler(
            &ctx,
            this.0.as_inner().clone(),
            type_,
            value,
        )
    }

    fn charset(mime: &str) -> Option<String> {
        let lower = mime.to_ascii_lowercase();
        let marker = "charset=";
        let pos = lower.find(marker)?;
        let rest = mime[pos + marker.len()..].trim();
        let rest = rest
            .strip_prefix('"')
            .map(|value| value.split('"').next().unwrap_or(value))
            .unwrap_or(rest);
        let value = rest.split(';').next().unwrap_or(rest).trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }

    fn decode(this: &Class<'js, Self>, ctx: &Ctx<'js>) -> String {
        let (override_charset, body, header_charset) = {
            let xhr = this.borrow();
            (
                xhr.override_charset.clone(),
                xhr.response_body.clone(),
                xhr.response_header("content-type")
                    .and_then(|mime| Self::charset(&mime)),
            )
        };
        let label = override_charset.or(header_charset);
        if let Some(label) = label {
            if let Ok(ctor) = ctx
                .globals()
                .get::<_, rquickjs::function::Constructor>("TextDecoder")
            {
                if let Ok(decoder) = ctor.construct::<_, Object>((label,)) {
                    if let Ok(decode) = decoder.get::<_, Function>("decode") {
                        if let Ok(view) = TypedArray::<u8>::new_copy(ctx.clone(), &body) {
                            if let Ok(text) = decode.call::<_, String>((This(decoder), view)) {
                                return text;
                            }
                        }
                    }
                }
            }
        }
        if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("TextDecoder")
        {
            if let Ok(decoder) = ctor.construct::<_, Object>(()) {
                if let Ok(decode) = decoder.get::<_, Function>("decode") {
                    if let Ok(view) = TypedArray::<u8>::new_copy(ctx.clone(), &body) {
                        if let Ok(text) = decode.call::<_, String>((This(decoder), view)) {
                            return text;
                        }
                    }
                }
            }
        }
        String::from_utf8_lossy(&body).into_owned()
    }
}

impl<'js> XMLHttpRequest<'js> {
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

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> XMLHttpRequest<'js> {
    #[qjs(constructor)]
    pub fn new(_ctx: Ctx<'js>) -> Self {
        Self {
            events:           HostEventTarget::share(),
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

    #[qjs(static, get, rename = "UNSENT")]
    pub fn unsent_const() -> i32 { UNSENT }

    #[qjs(static, get, rename = "OPENED")]
    pub fn opened_const() -> i32 { OPENED }

    #[qjs(static, get, rename = "HEADERS_RECEIVED")]
    pub fn headers_received_const() -> i32 { HEADERS_RECEIVED }

    #[qjs(static, get, rename = "LOADING")]
    pub fn loading_const() -> i32 { LOADING }

    #[qjs(static, get, rename = "DONE")]
    pub fn done_const() -> i32 { DONE }

    #[qjs(get)]
    pub fn ready_state(&self) -> i32 { self.ready_state }

    #[qjs(get)]
    pub fn status(&self) -> u16 { self.status }

    #[qjs(get)]
    pub fn status_text(&self) -> String { self.status_text.clone() }

    #[qjs(get)]
    pub fn response_url(&self) -> String { self.response_url.clone() }

    #[qjs(get)]
    pub fn response_type(&self) -> String { self.response_type.clone() }

    #[qjs(set, rename = "responseType")]
    pub fn set_response_type(&mut self, value: String) { self.response_type = value; }

    #[qjs(get)]
    pub fn timeout(&self) -> u64 { self.timeout }

    #[qjs(set, rename = "timeout")]
    pub fn set_timeout(&mut self, value: f64) {
        self.timeout = if value.is_finite() && value > 0.0 {
            value as u64
        } else {
            0
        };
    }

    #[qjs(get)]
    pub fn with_credentials(&self) -> bool { self.with_credentials }

    #[qjs(set, rename = "withCredentials")]
    pub fn set_with_credentials(&mut self, value: bool) { self.with_credentials = value; }

    #[qjs(get)]
    pub fn upload(&self, ctx: Ctx<'js>) -> Value<'js> { Value::new_undefined(ctx) }

    #[qjs(get, rename = "responseXML")]
    pub fn response_xml(&self, ctx: Ctx<'js>) -> Value<'js> { Value::new_null(ctx) }

    #[qjs(get)]
    pub fn response(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let ready = this.0.borrow().ready_state;
        let response_type = this.0.borrow().response_type.clone();
        if ready != DONE {
            return Ok(if response_type.is_empty() || response_type == "text" {
                String::new().into_js_str(&ctx)?
            } else {
                Value::new_null(ctx)
            });
        }
        match response_type.as_str() {
            "" | "text" => Self::decode(&this.0, &ctx).into_js_str(&ctx),
            "arraybuffer" => {
                let body = this.0.borrow().response_body.clone();
                ArrayBuffer::new_copy(ctx, body).map(|buffer| buffer.into_value())
            }
            "json" => {
                let text = Self::decode(&this.0, &ctx);
                match Host::json_parse(&ctx, &text) {
                    Ok(value) => Ok(value),
                    Err(_) => Ok(Value::new_null(ctx)),
                }
            }
            _ => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get)]
    pub fn response_text(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<String> {
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

    pub fn get_response_header(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>> {
        match self.response_header(&name) {
            Some(value) => rquickjs::IntoJs::into_js(value, &ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn open(
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

    pub fn override_mime_type(
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

    pub fn set_request_header(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, name: String, value: String,
    ) -> Result<()> {
        if this.0.borrow().ready_state != OPENED {
            return Err(Host::throw_dom(
                &ctx,
                "Failed to execute setRequestHeader: the object's state must be OPENED",
                "InvalidStateError",
            ));
        }
        this.0.borrow_mut().request_headers.push((name, value));
        Ok(())
    }

    pub fn add_event_listener(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: String, callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .add(&ctx, type_, callback, options.0)
    }

    pub fn remove_event_listener(
        this: This<Class<'js, Self>>, type_: String, callback: Value<'js>, options: Opt<Value<'js>>,
    ) {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .remove(&type_, &callback, options.0);
    }

    pub fn dispatch_event(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, event: Value<'js>,
    ) -> Result<bool> {
        HostEventTarget::dispatch_shared(&this.0.borrow().events, &ctx, this.0.as_inner(), event)
    }

    pub fn abort(this: This<Class<'js, Self>>) { this.0.borrow().aborted.set(true); }

    pub fn send(this: This<Class<'js, Self>>, ctx: Ctx<'js>, body: Opt<Value<'js>>) -> Result<()> {
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
            .map_err(|_| Host::throw_type(&ctx, "fetch is not defined"))?;
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
        if let Some(payload) = payload {
            if method != "GET" && method != "HEAD" {
                init.set("body", payload)?;
            }
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
                        let bytes = match XMLHttpRequest::response_bytes(&ctx, &response).await {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                XMLHttpRequest::fail(&this, &ctx, &aborted, &timed_out);
                                return;
                            }
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
                        this.borrow().events.borrow_mut().clear();
                    }
                    Err(_) => XMLHttpRequest::fail(&this, &ctx, &aborted, &timed_out),
                }
            }
        });
        let _ = abort;
        Ok(())
    }

    #[qjs(get, rename = "onreadystatechange")]
    pub fn onreadystatechange(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "readystatechange")
    }

    #[qjs(set, rename = "onreadystatechange")]
    pub fn set_onreadystatechange(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "readystatechange", value)
    }

    #[qjs(get, rename = "onload")]
    pub fn onload(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "load")
    }

    #[qjs(set, rename = "onload")]
    pub fn set_onload(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "load", value)
    }

    #[qjs(get, rename = "onerror")]
    pub fn onerror(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "error")
    }

    #[qjs(set, rename = "onerror")]
    pub fn set_onerror(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "error", value)
    }

    #[qjs(get, rename = "onloadend")]
    pub fn onloadend(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "loadend")
    }

    #[qjs(set, rename = "onloadend")]
    pub fn set_onloadend(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "loadend", value)
    }

    #[qjs(get, rename = "onloadstart")]
    pub fn onloadstart(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "loadstart")
    }

    #[qjs(set, rename = "onloadstart")]
    pub fn set_onloadstart(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "loadstart", value)
    }

    #[qjs(get, rename = "onprogress")]
    pub fn onprogress(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "progress")
    }

    #[qjs(set, rename = "onprogress")]
    pub fn set_onprogress(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "progress", value)
    }

    #[qjs(get, rename = "onabort")]
    pub fn onabort(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "abort")
    }

    #[qjs(set, rename = "onabort")]
    pub fn set_onabort(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "abort", value)
    }

    #[qjs(get, rename = "ontimeout")]
    pub fn ontimeout(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "timeout")
    }

    #[qjs(set, rename = "ontimeout")]
    pub fn set_ontimeout(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "timeout", value)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "XMLHttpRequest" }
}

impl<'js> XMLHttpRequest<'js> {
    fn headers_text(ctx: &Ctx<'js>, response: &Object<'js>) -> String {
        let Ok(headers) = response.get::<_, Object>("headers") else {
            return String::new();
        };
        if let Ok(for_each) = headers.get::<_, Function>("forEach") {
            let collected = Rc::new(std::cell::RefCell::new(String::new()));
            let sink = Rc::clone(&collected);
            if let Ok(callback) = Function::new(
                ctx.clone(),
                move |value: String, name: String| -> Result<()> {
                    sink.borrow_mut().push_str(&format!("{name}: {value}\r\n"));
                    Ok(())
                },
            ) {
                let _ = for_each.call::<_, ()>((This(headers), callback));
            }
            return collected.take();
        }
        String::new()
    }

    async fn response_bytes(_ctx: &Ctx<'js>, response: &Object<'js>) -> Result<Vec<u8>> {
        let array_buffer: Function = response.get("arrayBuffer")?;
        let promise: Promise = array_buffer.call((This(response.clone()),))?;
        let buffer: ArrayBuffer = promise.into_future().await?;
        Ok(buffer
            .as_bytes()
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default())
    }

    fn fail(
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
        this.borrow().events.borrow_mut().clear();
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

trait IntoJsStr<'js> {
    fn into_js_str(self, ctx: &Ctx<'js>) -> Result<Value<'js>>;
}

impl<'js> IntoJsStr<'js> for String {
    fn into_js_str(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        rquickjs::IntoJs::into_js(self, ctx)
    }
}
