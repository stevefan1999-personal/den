//! WHATWG EventSource (SSE) on top of `fetch()`.

use std::{cell::Cell, rc::Rc};

use rquickjs::{
    Class, Ctx, FromJs, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};
use url::Url;

use crate::{
    event_target::{HostEventTarget, SharedEvents},
    host::Host,
};

const CONNECTING: i32 = 0;
const OPEN: i32 = 1;
const CLOSED: i32 = 2;
const DEFAULT_RECONNECT_MS: u64 = 3000;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct EventSource<'js> {
    events:            SharedEvents<'js>,
    url:               String,
    with_credentials:  bool,
    ready_state:       i32,
    origin:            String,
    buffer:            String,
    data_buffer:       String,
    event_type_buffer: String,
    last_event_id:     String,
    last_event_id_buf: String,
    reconnect_ms:      u64,
    #[qjs(skip_trace)]
    closed:            Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    aborted:           Rc<Cell<bool>>,
}

impl<'js> Trace<'js> for EventSource<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(events) = self.events.try_borrow() {
            events.trace(tracer);
        }
    }
}

impl<'js> EventSource<'js> {
    fn dispatch(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        let this = this.clone();
        let fire = Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> Result<()> {
            HostEventTarget::dispatch_shared(
                &this.borrow().events,
                &ctx,
                this.as_inner(),
                event.clone(),
            )?;
            Ok(())
        })?;
        fire.defer(())?;
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

    fn resolve_url(ctx: &Ctx<'_>, url: &str) -> Result<String> {
        match Url::parse(url) {
            Ok(parsed) if parsed.scheme() == "http" || parsed.scheme() == "https" => {
                Ok(parsed.to_string())
            }
            _ => {
                Err(Host::throw_dom(
                    ctx,
                    &format!("Cannot open an EventSource to '{url}'."),
                    "SyntaxError",
                ))
            }
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> EventSource<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, url: String, options: Opt<Object<'js>>) -> Result<Class<'js, Self>> {
        let url = Self::resolve_url(&ctx, &url)?;
        let with_credentials = options
            .0
            .as_ref()
            .and_then(|obj| obj.get::<_, bool>("withCredentials").ok())
            .unwrap_or(false);
        let closed = Rc::new(Cell::new(false));
        let class = Class::instance(ctx.clone(), Self {
            events: HostEventTarget::share(),
            url,
            with_credentials,
            ready_state: CONNECTING,
            origin: String::new(),
            buffer: String::new(),
            data_buffer: String::new(),
            event_type_buffer: String::new(),
            last_event_id: String::new(),
            last_event_id_buf: String::new(),
            reconnect_ms: DEFAULT_RECONNECT_MS,
            closed: Rc::clone(&closed),
            aborted: Rc::new(Cell::new(false)),
        })?;
        let start = Function::new(ctx.clone(), {
            let this = class.clone();
            move |ctx: Ctx<'js>| -> Result<()> {
                EventSource::start(this.clone(), ctx);
                Ok(())
            }
        })?;
        start.defer(())?;
        Ok(class)
    }

    #[qjs(static, get, rename = "CONNECTING")]
    pub fn connecting_const() -> i32 { CONNECTING }

    #[qjs(static, get, rename = "OPEN")]
    pub fn open_const() -> i32 { OPEN }

    #[qjs(static, get, rename = "CLOSED")]
    pub fn closed_const() -> i32 { CLOSED }

    #[qjs(get)]
    pub fn url(&self) -> String { self.url.clone() }

    #[qjs(get)]
    pub fn with_credentials(&self) -> bool { self.with_credentials }

    #[qjs(get)]
    pub fn ready_state(&self) -> i32 {
        if self.closed.get() {
            CLOSED
        } else {
            self.ready_state
        }
    }

    pub fn close(&self) {
        self.closed.set(true);
        self.aborted.set(true);
        self.ready_state_slot_closed();
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

    #[qjs(get, rename = "onopen")]
    pub fn onopen(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "open")
    }

    #[qjs(set, rename = "onopen")]
    pub fn set_onopen(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "open", value)
    }

    #[qjs(get, rename = "onmessage")]
    pub fn onmessage(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "message")
    }

    #[qjs(set, rename = "onmessage")]
    pub fn set_onmessage(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "message", value)
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

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "EventSource" }
}

impl<'js> EventSource<'js> {
    fn ready_state_slot_closed(&self) { let _ = self; }

    fn start(this: Class<'js, Self>, ctx: Ctx<'js>) {
        if this.borrow().closed.get() {
            this.borrow_mut().ready_state = CLOSED;
            return;
        }
        {
            let mut es = this.borrow_mut();
            es.buffer.clear();
            es.data_buffer.clear();
            es.event_type_buffer.clear();
            es.last_event_id_buf = es.last_event_id.clone();
            es.aborted = Rc::new(Cell::new(false));
        }
        let (url, last_event_id) = {
            let es = this.borrow();
            (es.url.clone(), es.last_event_id.clone())
        };
        let Ok(fetch) = ctx.globals().get::<_, Function>("fetch") else {
            return;
        };
        let headers = Object::new(ctx.clone()).ok();
        let Some(headers) = headers else { return };
        let _ = headers.set("Accept", "text/event-stream");
        let _ = headers.set("Cache-Control", "no-cache");
        if !last_event_id.is_empty() {
            let _ = headers.set("Last-Event-ID", last_event_id);
        }
        let init = Object::new(ctx.clone()).ok();
        let Some(init) = init else { return };
        let _ = init.set("headers", headers);
        let Ok(promise) = fetch.call::<_, Promise>((url, init)) else {
            EventSource::reconnect(this, ctx);
            return;
        };
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                let response = match promise.into_future::<Object>().await {
                    Ok(response) => response,
                    Err(_) => {
                        EventSource::reconnect(this, ctx);
                        return;
                    }
                };
                if this.borrow().closed.get() {
                    if let Ok(cancel) = response.get::<_, Function>("_cancelBody") {
                        let _ = cancel.call::<_, ()>((This(response),));
                    }
                    return;
                }
                let status: u16 = response.get("status").unwrap_or(0);
                let mime = EventSource::content_type(&response);
                if status != 200 || mime != "text/event-stream" {
                    if let Ok(cancel) = response.get::<_, Function>("_cancelBody") {
                        let _ = cancel.call::<_, ()>((This(response),));
                    }
                    EventSource::fail(&this, &ctx);
                    return;
                }
                {
                    let mut es = this.borrow_mut();
                    es.ready_state = OPEN;
                    let response_url: String =
                        response.get("url").unwrap_or_else(|_| es.url.clone());
                    es.origin = Url::parse(&response_url)
                        .map(|url| url.origin().ascii_serialization())
                        .unwrap_or_else(|_| es.url.clone());
                }
                let _ = EventSource::dispatch(
                    &this,
                    &ctx,
                    Host::event(&ctx, "open").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                );
                let text = match EventSource::response_text(&ctx, &response).await {
                    Ok(text) => text,
                    Err(_) => {
                        EventSource::reconnect(this, ctx);
                        return;
                    }
                };
                if this.borrow().closed.get() || text.is_empty() {
                    return;
                }
                EventSource::feed(&this, &ctx, &text);
                EventSource::reconnect(this, ctx);
            }
        });
    }

    fn content_type(response: &Object<'_>) -> String {
        let Ok(headers) = response.get::<_, Object>("headers") else {
            return String::new();
        };
        let Ok(get) = headers.get::<_, Function>("get") else {
            return String::new();
        };
        let Ok(value) = get.call::<_, Value>((This(headers.clone()), "content-type")) else {
            return String::new();
        };
        if value.is_null() || value.is_undefined() {
            return String::new();
        }
        let Ok(text) = String::from_js(headers.ctx(), value) else {
            return String::new();
        };
        text.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    }

    async fn response_text(_ctx: &Ctx<'js>, response: &Object<'js>) -> Result<String> {
        let text: Function = response.get("text")?;
        let promise: Promise = text.call((This(response.clone()),))?;
        promise.into_future().await
    }

    fn fail(this: &Class<'js, Self>, ctx: &Ctx<'js>) {
        if this.borrow().closed.get() {
            return;
        }
        this.borrow_mut().ready_state = CLOSED;
        this.borrow().closed.set(true);
        let _ = Self::dispatch(
            this,
            ctx,
            Host::event(ctx, "error").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
        );
    }

    fn reconnect(this: Class<'js, Self>, ctx: Ctx<'js>) {
        if this.borrow().closed.get() {
            this.borrow_mut().ready_state = CLOSED;
            return;
        }
        this.borrow_mut().ready_state = CONNECTING;
        let _ = Self::dispatch(
            &this,
            &ctx,
            Host::event(&ctx, "error").unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
        );
        if this.borrow().closed.get() {
            this.borrow_mut().ready_state = CLOSED;
            return;
        }
        let delay = this.borrow().reconnect_ms;
        let Ok(set_timeout) = ctx.globals().get::<_, Function>("setTimeout") else {
            return;
        };
        let callback = Function::new(ctx.clone(), {
            let this = this.clone();
            move |ctx: Ctx<'js>| -> Result<()> {
                if this.borrow().closed.get() {
                    return Ok(());
                }
                EventSource::start(this.clone(), ctx);
                Ok(())
            }
        });
        let Ok(callback) = callback else {
            return;
        };
        let _ = set_timeout.call::<_, Value>((callback, delay));
    }

    fn feed(this: &Class<'js, Self>, ctx: &Ctx<'js>, chunk: &str) {
        this.borrow_mut().buffer.push_str(chunk);
        let buffer = this.borrow().buffer.clone();
        let len = buffer.len();
        let bytes = buffer.as_bytes();
        let mut pos = 0;
        let mut line_start = 0;
        while pos < len {
            let c = bytes[pos];
            if c == 0x0a {
                let line = &buffer[line_start..pos];
                Self::process_line(this, ctx, line);
                pos += 1;
                line_start = pos;
            } else if c == 0x0d {
                if pos == len - 1 {
                    break;
                }
                let line = &buffer[line_start..pos];
                Self::process_line(this, ctx, line);
                pos += if bytes[pos + 1] == 0x0a { 2 } else { 1 };
                line_start = pos;
            } else {
                pos += 1;
            }
        }
        this.borrow_mut().buffer = buffer[line_start..].to_string();
    }

    fn process_line(this: &Class<'js, Self>, ctx: &Ctx<'js>, line: &str) {
        if line.is_empty() {
            Self::dispatch_message(this, ctx);
            return;
        }
        if line.as_bytes().first() == Some(&0x3a) {
            return;
        }
        let (field, value) = match line.split_once(':') {
            None => (line, ""),
            Some((field, value)) => {
                let value = value.strip_prefix(' ').unwrap_or(value);
                (field, value)
            }
        };
        Self::process_field(this, field, value);
    }

    fn process_field(this: &Class<'js, Self>, field: &str, value: &str) {
        let mut es = this.borrow_mut();
        match field {
            "event" => es.event_type_buffer = value.to_string(),
            "data" => {
                es.data_buffer.push_str(value);
                es.data_buffer.push('\n');
            }
            "id" => {
                if !value.contains('\0') {
                    es.last_event_id_buf = value.to_string();
                }
            }
            "retry" => {
                if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                    if let Ok(ms) = value.parse() {
                        es.reconnect_ms = ms;
                    }
                }
            }
            _ => {}
        }
    }

    fn dispatch_message(this: &Class<'js, Self>, ctx: &Ctx<'js>) {
        let (data, type_, origin, last_event_id, closed) = {
            let mut es = this.borrow_mut();
            es.last_event_id = es.last_event_id_buf.clone();
            if es.data_buffer.is_empty() {
                es.event_type_buffer.clear();
                return;
            }
            let mut data = std::mem::take(&mut es.data_buffer);
            if data.ends_with('\n') {
                data.pop();
            }
            let type_ = if es.event_type_buffer.is_empty() {
                "message".to_string()
            } else {
                std::mem::take(&mut es.event_type_buffer)
            };
            es.data_buffer.clear();
            es.event_type_buffer.clear();
            (
                data,
                type_,
                es.origin.clone(),
                es.last_event_id.clone(),
                es.closed.get() || es.ready_state == CLOSED,
            )
        };
        if closed {
            return;
        }
        let data_value =
            rquickjs::IntoJs::into_js(data, ctx).unwrap_or_else(|_| Value::new_null(ctx.clone()));
        let _ = Self::dispatch(
            this,
            ctx,
            Host::message_event(ctx, &type_, data_value, &origin, &last_event_id)
                .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
        );
    }
}
