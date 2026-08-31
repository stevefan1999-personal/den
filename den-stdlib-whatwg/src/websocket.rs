//! WHATWG WebSocket: EventTarget + IDL around `den-stdlib-networking`.

use std::rc::Rc;

use den_stdlib_networking::websocket::{
    MAX_CLOSE_REASON_BYTES, NativeWebSocket, NativeWsError, NativeWsEvent,
};
use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, FromJs as _, Function, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

use crate::{blob::Blob, host::Host};

const CONNECTING: i32 = 0;
const OPEN: i32 = 1;
const CLOSING: i32 = 2;
const CLOSED: i32 = 3;

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename_all = "camelCase")]
pub struct WebSocket {
    #[qjs(skip_trace)]
    native:      Rc<NativeWebSocket>,
    binary_type: String,
    #[qjs(get)]
    protocol:    String,
    #[qjs(get)]
    extensions:  String,
    origin:      String,
    #[qjs(get)]
    url:         String,
    #[qjs(get)]
    ready_state: i32,
}

impl WebSocket {
    fn dispatch<'js>(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        den_stdlib_worker::events::dispatch_trusted(
            ctx.clone(),
            this.as_inner().clone().into_value(),
            event,
        )?;
        Ok(())
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

    fn event_or_undefined<'js>(ctx: &Ctx<'js>, event: Result<Value<'js>>) -> Value<'js> {
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

    fn protocols_from_list<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<String>> {
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

    fn protocols_from_constructor_arg<'js>(
        ctx: &Ctx<'js>, value: Value<'js>,
    ) -> Result<Vec<String>> {
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

    fn pump<'js>(this: Class<'js, Self>, ctx: Ctx<'js>) {
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
                                    Blob::from_inner(crate::blob::Inner::from_bytes(
                                        bytes,
                                        String::new(),
                                    )),
                                )
                                .map_or_else(
                                    |_| Value::new_undefined(ctx.clone()),
                                    Class::into_value,
                                )
                            } else {
                                ArrayBuffer::new_copy(ctx.clone(), bytes).map_or_else(
                                    |_| Value::new_undefined(ctx.clone()),
                                    ArrayBuffer::into_value,
                                )
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
    pub fn install_idl_constants(ctx: &Ctx<'_>) -> Result<()> {
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
impl WebSocket {
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>, url: String, protocols: Opt<Value<'js>>,
    ) -> Result<Class<'js, Self>> {
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
    pub const fn connecting_const() -> i32 { CONNECTING }

    #[qjs(static, get, rename = "OPEN")]
    pub const fn open_const() -> i32 { OPEN }

    #[qjs(static, get, rename = "CLOSING")]
    pub const fn closing_const() -> i32 { CLOSING }

    #[qjs(static, get, rename = "CLOSED")]
    pub const fn closed_const() -> i32 { CLOSED }

    #[qjs(get)]
    pub fn binary_type(&self) -> String { self.binary_type.clone() }

    #[qjs(set, rename = "binaryType")]
    pub fn set_binary_type(&mut self, value: String) {
        if value == "arraybuffer" || value == "blob" {
            self.binary_type = value;
        }
    }

    #[qjs(get)]
    pub fn buffered_amount(&self) -> i32 {
        i32::try_from(self.native.buffered_amount()).unwrap_or(i32::MAX)
    }

    pub fn send<'js>(this: This<Class<'js, Self>>, ctx: Ctx<'js>, data: Value<'js>) -> Result<()> {
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

    pub fn close<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, code: Opt<Value<'js>>, reason: Opt<Value<'js>>,
    ) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == CLOSING || ready == CLOSED {
            return Ok(());
        }
        let code = match code.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => {
                let number = Coerced::<f64>::from_js(&ctx, value)?.0;
                Some(Self::clamp_unsigned_short(number))
            }
        };
        let reason = match reason.0 {
            Some(value) if !value.is_undefined() => Host::coerce_usv_string(&ctx, value)?,
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

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "WebSocket" }
}
