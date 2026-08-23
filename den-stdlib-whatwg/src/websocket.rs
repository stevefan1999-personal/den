//! WebSocket EventTarget wrapping tokio-tungstenite.

use std::{cell::RefCell, rc::Rc};

use futures::{SinkExt, StreamExt};
use rquickjs::{
    Class, Ctx, Exception, JsLifetime, Result, TypedArray, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, Rest, This},
};
use tokio::{runtime::Handle, sync::mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

use crate::{
    blob::Blob,
    event_target::{HostEventTarget, SharedEvents},
    host::Host,
};

const CONNECTING: i32 = 0;
const OPEN: i32 = 1;
const CLOSING: i32 = 2;
const CLOSED: i32 = 3;

enum Command {
    SendText(String),
    SendBinary(Vec<u8>),
    Close { code: u16, reason: String },
}

enum NativeEvent {
    Open { protocol: String },
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Close { code: u16, reason: String },
}

pub struct NativeWebSocket {
    commands: mpsc::UnboundedSender<Command>,
    events: RefCell<Option<mpsc::UnboundedReceiver<NativeEvent>>>,
}

impl NativeWebSocket {
    fn spawn_connection(
        url: String,
        protocols: Option<String>,
        events: mpsc::UnboundedSender<NativeEvent>,
        commands: mpsc::UnboundedReceiver<Command>,
    ) {
        Handle::current().spawn(async move {
            if let Err(error) = Self::run(url, protocols, events.clone(), commands).await {
                let _ = events.send(NativeEvent::Error(error));
                let _ = events.send(NativeEvent::Close {
                    code: 1006,
                    reason: String::new(),
                });
            }
        });
    }

    async fn run(
        url: String,
        protocols: Option<String>,
        events: mpsc::UnboundedSender<NativeEvent>,
        mut commands: mpsc::UnboundedReceiver<Command>,
    ) -> std::result::Result<(), String> {
        let mut request = url.into_client_request().map_err(|err| err.to_string())?;
        if let Some(protocols) = protocols {
            request.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                protocols.parse().map_err(
                    |err: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| {
                        err.to_string()
                    },
                )?,
            );
        }
        let (stream, response) = connect_async(request)
            .await
            .map_err(|err| err.to_string())?;
        let protocol = response
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let _ = events.send(NativeEvent::Open { protocol });
        let (mut sink, mut incoming) = stream.split();
        loop {
            tokio::select! {
              command = commands.recv() => {
                match command {
                  Some(Command::SendText(text)) => {
                    sink.send(Message::Text(text.into())).await.map_err(|err| err.to_string())?;
                  }
                  Some(Command::SendBinary(bytes)) => {
                    sink.send(Message::Binary(bytes.into())).await.map_err(|err| err.to_string())?;
                  }
                  Some(Command::Close { code, reason }) => {
                    let _ = sink.send(Message::Close(Some(
                      tokio_tungstenite::tungstenite::protocol::CloseFrame {
                        code: code.into(),
                        reason: reason.into(),
                      },
                    ))).await;
                    break;
                  }
                  None => break,
                }
              }
              message = incoming.next() => {
                match message {
                  Some(Ok(Message::Text(text))) => {
                    let _ = events.send(NativeEvent::Text(text.to_string()));
                  }
                  Some(Ok(Message::Binary(bytes))) => {
                    let _ = events.send(NativeEvent::Binary(bytes.to_vec()));
                  }
                  Some(Ok(Message::Ping(payload))) => {
                    sink.send(Message::Pong(payload)).await.map_err(|err| err.to_string())?;
                  }
                  Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                  Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())).unwrap_or((1005, String::new()));
                    let _ = events.send(NativeEvent::Close { code, reason });
                    break;
                  }
                  Some(Err(err)) => {
                    let _ = events.send(NativeEvent::Error(err.to_string()));
                    let _ = events.send(NativeEvent::Close { code: 1006, reason: String::new() });
                    break;
                  }
                  None => {
                    let _ = events.send(NativeEvent::Close { code: 1006, reason: String::new() });
                    break;
                  }
                }
              }
            }
        }
        Ok(())
    }

    fn connect(ctx: &Ctx<'_>, url: String, protocols: Option<String>) -> Result<Self> {
        Handle::try_current()
            .map_err(|_| Exception::throw_internal(ctx, "WebSocket requires a tokio runtime"))?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self::spawn_connection(url, protocols, event_tx, command_rx);
        Ok(Self {
            commands: command_tx,
            events: RefCell::new(Some(event_rx)),
        })
    }

    async fn next_event(&self) -> Option<NativeEvent> {
        let mut events = self.events.borrow_mut().take()?;
        let event = events.recv().await;
        *self.events.borrow_mut() = Some(events);
        event
    }

    fn send_text(&self, ctx: &Ctx<'_>, text: String) -> Result<()> {
        self.commands
            .send(Command::SendText(text))
            .map_err(|_| Exception::throw_internal(ctx, "WebSocket is closed"))
    }

    fn send_binary(&self, ctx: &Ctx<'_>, bytes: Vec<u8>) -> Result<()> {
        self.commands
            .send(Command::SendBinary(bytes))
            .map_err(|_| Exception::throw_internal(ctx, "WebSocket is closed"))
    }

    fn close(&self, code: u16, reason: String) {
        let _ = self.commands.send(Command::Close { code, reason });
    }
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct WebSocket<'js> {
    events: SharedEvents<'js>,
    #[qjs(skip_trace)]
    native: Rc<NativeWebSocket>,
    binary_type: String,
    protocol: String,
    url: String,
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
        HostEventTarget::dispatch_shared(&this.borrow().events, ctx, this.as_inner(), event)?;
        Ok(())
    }

    fn handler(this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str) -> Value<'js> {
        this.0.borrow().events.borrow().handler_or_null(&ctx, type_)
    }

    fn set_handler(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        type_: &'static str,
        value: Value<'js>,
    ) -> Result<()> {
        this.0.borrow().events.borrow_mut().set_handler(
            &ctx,
            this.0.as_inner().clone(),
            type_,
            value,
        )
    }

    fn parse_url(ctx: &Ctx<'_>, url: &str) -> Result<String> {
        let parsed = Url::parse(url).map_err(|_| Host::throw_message(ctx, "Invalid URL"))?;
        if parsed.scheme() != "ws" && parsed.scheme() != "wss" {
            return Err(Host::throw_message(ctx, "Invalid URL"));
        }
        Ok(parsed.to_string())
    }

    fn protocols(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<Option<String>> {
        let Some(value) = value else {
            return Ok(None);
        };
        if let Some(string) = value.as_string() {
            let text = string.to_string()?;
            return Ok(if text.is_empty() { None } else { Some(text) });
        }
        if let Some(array) = value.as_array() {
            let mut protocols = Vec::new();
            for entry in array.clone().into_iter() {
                protocols.push(String::from_js_value(ctx, entry?)?);
            }
            let joined = protocols.join(",");
            return Ok(if joined.is_empty() {
                None
            } else {
                Some(joined)
            });
        }
        if let Some(obj) = value.as_object() {
            if let Ok(protocols) = obj.get::<_, Value>("protocols") {
                return Self::protocols(ctx, Some(protocols));
            }
        }
        Ok(None)
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
                        NativeEvent::Open { protocol } => {
                            this.borrow_mut().protocol = protocol;
                            this.borrow_mut().ready_state = OPEN;
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Host::event(&ctx, "open")
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                            );
                        }
                        NativeEvent::Text(text) => {
                            let data = rquickjs::IntoJs::into_js(text, &ctx)
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Host::message_event(&ctx, "message", data, "", "")
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                            );
                        }
                        NativeEvent::Binary(bytes) => {
                            let binary_type = this.borrow().binary_type.clone();
                            let data = if binary_type == "blob" {
                                Class::instance(
                                    ctx.clone(),
                                    Blob::from_inner(crate::blob::BlobInner::from_bytes(
                                        bytes,
                                        String::new(),
                                    )),
                                )
                                .map(|class| class.into_value())
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
                            } else {
                                TypedArray::<u8>::new_copy(ctx.clone(), bytes)
                                    .map(|view| view.into_value())
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
                            };
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Host::message_event(&ctx, "message", data, "", "")
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                            );
                        }
                        NativeEvent::Error(message) => {
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Host::error_event(&ctx, &message)
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                            );
                        }
                        NativeEvent::Close { code, reason } => {
                            this.borrow_mut().ready_state = CLOSED;
                            let _ = WebSocket::dispatch(
                                &this,
                                &ctx,
                                Host::close_event(&ctx, code, &reason, code == 1000)
                                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                            );
                            break;
                        }
                    }
                }
            }
        });
    }
}

trait FromJsString {
    fn from_js_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String>;
}

impl FromJsString for String {
    fn from_js_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
        rquickjs::FromJs::from_js(ctx, value)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WebSocket<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, url: String, protocols: Opt<Value<'js>>) -> Result<Class<'js, Self>> {
        let url = Self::parse_url(&ctx, &url)?;
        let protocols = Self::protocols(&ctx, protocols.0)?;
        let native = NativeWebSocket::connect(&ctx, url.clone(), protocols)?;
        let class = Class::instance(
            ctx.clone(),
            Self {
                events: HostEventTarget::share(),
                native: Rc::new(native),
                binary_type: "blob".to_string(),
                protocol: String::new(),
                url,
                ready_state: CONNECTING,
            },
        )?;
        Self::pump(class.clone(), ctx);
        Ok(class)
    }

    #[qjs(static, get, rename = "CONNECTING")]
    pub fn connecting_const() -> i32 {
        CONNECTING
    }
    #[qjs(static, get, rename = "OPEN")]
    pub fn open_const() -> i32 {
        OPEN
    }
    #[qjs(static, get, rename = "CLOSING")]
    pub fn closing_const() -> i32 {
        CLOSING
    }
    #[qjs(static, get, rename = "CLOSED")]
    pub fn closed_const() -> i32 {
        CLOSED
    }

    #[qjs(get)]
    pub fn binary_type(&self) -> String {
        self.binary_type.clone()
    }
    #[qjs(set, rename = "binaryType")]
    pub fn set_binary_type(&mut self, ctx: Ctx<'_>, value: String) -> Result<()> {
        if value != "arraybuffer" && value != "blob" {
            return Err(Host::throw_message(
                &ctx,
                &format!("Unsupported binaryType: {value}"),
            ));
        }
        self.binary_type = value;
        Ok(())
    }
    #[qjs(get)]
    pub fn protocol(&self) -> String {
        self.protocol.clone()
    }
    #[qjs(get)]
    pub fn ready_state(&self) -> i32 {
        self.ready_state
    }
    #[qjs(get)]
    pub fn url(&self) -> String {
        self.url.clone()
    }
    #[qjs(get)]
    pub fn buffered_amount(&self) -> i32 {
        0
    }
    #[qjs(get)]
    pub fn extensions(&self) -> &'static str {
        ""
    }

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
        if let Some(string) = data.as_string() {
            return this.0.borrow().native.send_text(&ctx, string.to_string()?);
        }
        if let Some(bytes) = Host::blob_like_bytes(&ctx, &data) {
            return this.0.borrow().native.send_binary(&ctx, bytes);
        }
        if let Some(bytes) = Host::buffer_source_bytes(&ctx, data)? {
            return this.0.borrow().native.send_binary(&ctx, bytes);
        }
        Ok(())
    }

    pub fn close(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        args: Rest<Value<'js>>,
    ) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == CLOSING || ready == CLOSED {
            return Ok(());
        }
        let code = args
            .0
            .first()
            .and_then(|value| value.as_number())
            .map(|number| number as u16)
            .unwrap_or(1000);
        let reason = args
            .0
            .get(1)
            .cloned()
            .map(|value| Host::coerce_string(&ctx, value))
            .transpose()?
            .unwrap_or_default();
        if code != 1000 && !(3000..=4999).contains(&code) {
            return Err(Host::throw_range(&ctx, "Invalid code value"));
        }
        if reason.encode_utf16().count() > 123 {
            return Err(Host::throw_syntax(&ctx, "Invalid reason value"));
        }
        this.0.borrow_mut().ready_state = CLOSING;
        this.0.borrow().native.close(code, reason);
        Ok(())
    }

    pub fn add_event_listener(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        type_: String,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .add(&ctx, type_, callback, options.0)
    }

    pub fn remove_event_listener(
        this: This<Class<'js, Self>>,
        type_: String,
        callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .remove(&type_, &callback, options.0);
    }

    pub fn dispatch_event(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        event: Value<'js>,
    ) -> Result<bool> {
        HostEventTarget::dispatch_shared(&this.0.borrow().events, &ctx, this.0.as_inner(), event)
    }

    #[qjs(get, rename = "onopen")]
    pub fn onopen(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "open")
    }
    #[qjs(set, rename = "onopen")]
    pub fn set_onopen(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "open", value)
    }
    #[qjs(get, rename = "onmessage")]
    pub fn onmessage(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "message")
    }
    #[qjs(set, rename = "onmessage")]
    pub fn set_onmessage(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "message", value)
    }
    #[qjs(get, rename = "onerror")]
    pub fn onerror(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "error")
    }
    #[qjs(set, rename = "onerror")]
    pub fn set_onerror(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "error", value)
    }
    #[qjs(get, rename = "onclose")]
    pub fn onclose(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "close")
    }
    #[qjs(set, rename = "onclose")]
    pub fn set_onclose(
        this: This<Class<'js, Self>>,
        ctx: Ctx<'js>,
        value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "close", value)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "WebSocket"
    }
}
