//! WHATWG WebSocket: EventTarget + IDL around `den-stdlib-networking`.

use std::rc::Rc;

use den_stdlib_networking::websocket::{
    MAX_CLOSE_REASON_BYTES, NativeWebSocket, NativeWsError, NativeWsEvent,
};
use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, FromJs as _, Function, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, Rest, This},
};

use crate::{
    blob::Blob,
    event_target::{HostEventTarget, SharedEvents},
    host::Host,
};

const CONNECTING: i32 = 0;
const OPEN: i32 = 1;
const CLOSING: i32 = 2;
const CLOSED: i32 = 3;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct WebSocket<'js> {
    events:      SharedEvents<'js>,
    #[qjs(skip_trace)]
    native:      Rc<NativeWebSocket>,
    binary_type: String,
    protocol:    String,
    extensions:  String,
    origin:      String,
    url:         String,
    ready_state: i32,
}

impl<'js> Trace<'js> for WebSocket<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(events) = self.events.try_borrow() {
            events.trace(tracer);
        }
    }
}

impl<'js> WebSocket<'js> {
    fn dispatch(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        let events = Rc::clone(&this.borrow().events);
        HostEventTarget::dispatch_shared(&events, ctx, this.as_inner(), event)?;
        Ok(())
    }

    fn handler(this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str) -> Value<'js> {
        let events = Rc::clone(&this.0.borrow().events);
        events.borrow().handler_or_null(&ctx, type_)
    }

    fn set_handler(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str, value: Value<'js>,
    ) -> Result<()> {
        let events = Rc::clone(&this.0.borrow().events);
        let target = this.0.as_inner().clone();
        events.borrow_mut().set_handler(&ctx, target, type_, value)
    }

    fn throw_native(ctx: &Ctx<'_>, error: NativeWsError) -> rquickjs::Error {
        match error {
            NativeWsError::InvalidUrl
            | NativeWsError::InvalidScheme
            | NativeWsError::Credentials
            | NativeWsError::Fragment
            | NativeWsError::InvalidProtocol => {
                Host::throw_dom(ctx, &error.to_string(), "SyntaxError")
            }
            NativeWsError::NoRuntime => {
                rquickjs::Exception::throw_internal(ctx, &error.to_string())
            }
            NativeWsError::Closed => {
                Host::throw_dom(ctx, "WebSocket is closed", "InvalidStateError")
            }
            other => Host::throw_message(ctx, &other.to_string()),
        }
    }

    fn event_or_undefined(ctx: &Ctx<'js>, event: Result<Value<'js>>) -> Value<'js> {
        event.unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
    }

    fn clamp_unsigned_short(number: f64) -> u16 {
        if !number.is_finite() || number < 0.0 {
            0
        } else if number >= f64::from(u16::MAX) {
            u16::MAX
        } else {
            u16::try_from(number as u32).unwrap_or(u16::MAX)
        }
    }

    fn protocols_from_list(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<String>> {
        if let Some(string) = value.as_string() {
            let text = string.to_string()?;
            return Ok(if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            });
        }
        if let Some(array) = value.as_array() {
            let mut protocols = Vec::new();
            for entry in array.clone() {
                protocols.push(String::from_js(ctx, entry?)?);
            }
            return Ok(protocols);
        }
        Err(Host::throw_type(
            ctx,
            "Failed to convert value to a sequence of protocol strings",
        ))
    }

    fn protocols_from_constructor_arg(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<String>> {
        if value.is_undefined() {
            return Ok(Vec::new());
        }
        if value.is_string() || value.as_array().is_some() {
            return Self::protocols_from_list(ctx, value);
        }
        if let Some(object) = value.as_object() {
            let member = object.get::<_, Value>("protocols")?;
            if member.is_undefined() {
                return Ok(Vec::new());
            }
            return Self::protocols_from_list(ctx, member);
        }
        Err(Host::throw_type(
            ctx,
            "Failed to convert value to a sequence of protocol strings",
        ))
    }

    fn pump(this: Class<'js, Self>, ctx: Ctx<'js>) {
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                loop {
                    let native = Rc::clone(&this.borrow().native);
                    let event = native.next_event().await;
                    let Some(event) = event else {
                        this.borrow_mut().ready_state = CLOSED;
                        break;
                    };
                    match event {
                        NativeWsEvent::Open {
                            protocol,
                            extensions,
                        } => {
                            {
                                let mut socket = this.borrow_mut();
                                socket.protocol = protocol;
                                socket.extensions = extensions;
                                socket.ready_state = OPEN;
                            }
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Self::event_or_undefined(&ctx, Host::event(&ctx, "open")),
                            );
                        }
                        NativeWsEvent::Text(text) => {
                            let origin = this.borrow().origin.clone();
                            let data = rquickjs::IntoJs::into_js(text, &ctx)
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Self::event_or_undefined(
                                    &ctx,
                                    Host::message_event(&ctx, "message", data, &origin, ""),
                                ),
                            );
                        }
                        NativeWsEvent::Binary(bytes) => {
                            let (binary_type, origin) = {
                                let this = this.borrow();
                                (this.binary_type.clone(), this.origin.clone())
                            };
                            let data = if binary_type == "blob" {
                                Class::instance(
                                    ctx.clone(),
                                    Blob::from_inner(crate::blob::BlobInner::from_bytes(
                                        bytes,
                                        String::new(),
                                    )),
                                )
                                .map(Class::into_value)
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
                            } else {
                                ArrayBuffer::new_copy(ctx.clone(), bytes)
                                    .map(ArrayBuffer::into_value)
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
                            };
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Self::event_or_undefined(
                                    &ctx,
                                    Host::message_event(&ctx, "message", data, &origin, ""),
                                ),
                            );
                        }
                        NativeWsEvent::Error(message) => {
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Self::event_or_undefined(&ctx, Host::error_event(&ctx, &message)),
                            );
                        }
                        NativeWsEvent::Close {
                            code,
                            reason,
                            was_clean,
                        } => {
                            this.borrow_mut().ready_state = CLOSED;
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Self::event_or_undefined(
                                    &ctx,
                                    Host::close_event(&ctx, code, &reason, was_clean),
                                ),
                            );
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Web IDL constants also live on the prototype so `socket.CONNECTING`
    /// works.
    pub fn install_idl_constants(ctx: &Ctx<'js>) -> Result<()> {
        let Some(proto) = Class::<Self>::prototype(ctx)? else {
            return Ok(());
        };
        proto.set("CONNECTING", CONNECTING)?;
        proto.set("OPEN", OPEN)?;
        proto.set("CLOSING", CLOSING)?;
        proto.set("CLOSED", CLOSED)?;
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WebSocket<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, url: String, protocols: Opt<Value<'js>>) -> Result<Class<'js, Self>> {
        let parsed =
            NativeWebSocket::parse_url(&url).map_err(|error| Self::throw_native(&ctx, error))?;
        let scheme = if parsed.scheme() == "wss" {
            "https"
        } else {
            "http"
        };
        let mut converted = parsed.clone();
        let origin = if converted.set_scheme(scheme).is_ok() {
            converted.origin().ascii_serialization()
        } else {
            let host = parsed.host_str().unwrap_or_default();
            parsed.port().map_or_else(
                || format!("{scheme}://{host}"),
                |port| format!("{scheme}://{host}:{port}"),
            )
        };
        let url = parsed.to_string();
        let protocols = match protocols.0 {
            None => Vec::new(),
            Some(value) => Self::protocols_from_constructor_arg(&ctx, value)?,
        };
        NativeWebSocket::validate_protocols(&protocols)
            .map_err(|error| Self::throw_native(&ctx, error))?;
        let native = NativeWebSocket::connect(&url, &protocols)
            .map_err(|error| Self::throw_native(&ctx, error))?;
        let class = Class::instance(ctx.clone(), Self {
            events: HostEventTarget::share(),
            native: Rc::new(native),
            binary_type: "blob".to_owned(),
            protocol: String::new(),
            extensions: String::new(),
            origin,
            url,
            ready_state: CONNECTING,
        })?;
        Self::install_idl_constants(&ctx)?;
        let start = Function::new(ctx.clone(), {
            let this = class.clone();
            move |ctx: Ctx<'js>| -> Result<()> {
                WebSocket::pump(this.clone(), ctx);
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

    #[qjs(static, get, rename = "CLOSING")]
    pub fn closing_const() -> i32 { CLOSING }

    #[qjs(static, get, rename = "CLOSED")]
    pub fn closed_const() -> i32 { CLOSED }

    #[qjs(get)]
    pub fn binary_type(&self) -> String { self.binary_type.clone() }

    #[qjs(set, rename = "binaryType")]
    pub fn set_binary_type(&mut self, value: String) {
        if value == "arraybuffer" || value == "blob" {
            self.binary_type = value;
        }
    }

    #[qjs(get)]
    pub fn protocol(&self) -> String { self.protocol.clone() }

    #[qjs(get)]
    pub fn ready_state(&self) -> i32 { self.ready_state }

    #[qjs(get)]
    pub fn url(&self) -> String { self.url.clone() }

    #[qjs(get)]
    pub fn buffered_amount(&self) -> i32 {
        i32::try_from(self.native.buffered_amount()).unwrap_or(i32::MAX)
    }

    #[qjs(get)]
    pub fn extensions(&self) -> String { self.extensions.clone() }

    pub fn send(this: This<Class<'js, Self>>, ctx: Ctx<'js>, data: Value<'js>) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == CONNECTING {
            return Err(Host::throw_dom(
                &ctx,
                "WebSocket is not open",
                "InvalidStateError",
            ));
        }
        if ready != OPEN {
            return Ok(());
        }
        let native = Rc::clone(&this.0.borrow().native);
        let result = if let Some(bytes) = Host::blob_like_bytes(&ctx, &data) {
            native.send_binary(bytes)
        } else if let Some(bytes) = Host::buffer_source_bytes(&ctx, data.clone())? {
            native.send_binary(bytes)
        } else {
            native.send_text(Host::coerce_usv_string(&ctx, data)?)
        };
        match result {
            Ok(()) | Err(NativeWsError::Closed) => Ok(()),
            Err(error) => Err(Self::throw_native(&ctx, error)),
        }
    }

    pub fn close(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, args: Rest<Value<'js>>,
    ) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == CLOSING || ready == CLOSED {
            return Ok(());
        }
        let code = match args.0.first() {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => {
                let number = Coerced::<f64>::from_js(&ctx, value.clone())?.0;
                Some(Self::clamp_unsigned_short(number))
            }
        };
        let reason = match args.0.get(1) {
            Some(value) if !value.is_undefined() => Host::coerce_usv_string(&ctx, value.clone())?,
            _ => String::new(),
        };
        if let Some(code) = code
            && !NativeWebSocket::is_valid_close_code(code)
        {
            return Err(Host::throw_dom(
                &ctx,
                "The close code must be either 1000 or in the range 3000 to 4999.",
                "InvalidAccessError",
            ));
        }
        if reason.len() > MAX_CLOSE_REASON_BYTES {
            return Err(Host::throw_dom(&ctx, "Invalid reason value", "SyntaxError"));
        }
        let native = {
            let mut socket = this.0.borrow_mut();
            socket.ready_state = CLOSING;
            Rc::clone(&socket.native)
        };
        native.close_opt(code, reason);
        Ok(())
    }

    pub fn add_event_listener(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: String, callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        let events = Rc::clone(&this.0.borrow().events);
        events.borrow_mut().add(&ctx, type_, callback, options.0)
    }

    pub fn remove_event_listener(
        this: This<Class<'js, Self>>, type_: String, callback: Value<'js>, options: Opt<Value<'js>>,
    ) {
        let events = Rc::clone(&this.0.borrow().events);
        events.borrow_mut().remove(&type_, &callback, options.0);
    }

    pub fn dispatch_event(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, event: Value<'js>,
    ) -> Result<bool> {
        let events = Rc::clone(&this.0.borrow().events);
        HostEventTarget::dispatch_shared(&events, &ctx, this.0.as_inner(), event)
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

    #[qjs(get, rename = "onclose")]
    pub fn onclose(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "close")
    }

    #[qjs(set, rename = "onclose")]
    pub fn set_onclose(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "close", value)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "WebSocket" }
}
