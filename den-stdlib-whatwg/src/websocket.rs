//! Native WebSocket transport. The JS class in `prelude/websocket.js` owns
//! EventTarget; this type is send/close plus an event pump.

use std::cell::RefCell;

use futures::{SinkExt, StreamExt};
use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, TypedArray, Value, class::Trace, function::Opt,
};
use tokio::{runtime::Handle, sync::mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

enum Command {
    SendText(String),
    SendBinary(Vec<u8>),
    Close { code: u16, reason: String },
}

enum Event {
    Open { protocol: String },
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Close { code: u16, reason: String },
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct NativeWebSocket {
    #[qjs(skip_trace)]
    commands: mpsc::UnboundedSender<Command>,
    #[qjs(skip_trace)]
    events: RefCell<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl NativeWebSocket {
    fn spawn_connection(
        url: String,
        protocols: Option<String>,
        events: mpsc::UnboundedSender<Event>,
        commands: mpsc::UnboundedReceiver<Command>,
    ) {
        Handle::current().spawn(async move {
            if let Err(error) = Self::run(url, protocols, events.clone(), commands).await {
                let _ = events.send(Event::Error(error));
                let _ = events.send(Event::Close {
                    code: 1006,
                    reason: String::new(),
                });
            }
        });
    }

    async fn run(
        url: String,
        protocols: Option<String>,
        events: mpsc::UnboundedSender<Event>,
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
        let _ = events.send(Event::Open { protocol });
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
                            let _ = events.send(Event::Text(text.to_string()));
                        }
                        Some(Ok(Message::Binary(bytes))) => {
                            let _ = events.send(Event::Binary(bytes.to_vec()));
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            sink.send(Message::Pong(payload)).await.map_err(|err| err.to_string())?;
                        }
                        Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                        Some(Ok(Message::Close(frame))) => {
                            let (code, reason) = frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())).unwrap_or((1005, String::new()));
                            let _ = events.send(Event::Close { code, reason });
                            break;
                        }
                        Some(Err(err)) => {
                            let _ = events.send(Event::Error(err.to_string()));
                            let _ = events.send(Event::Close { code: 1006, reason: String::new() });
                            break;
                        }
                        None => {
                            let _ = events.send(Event::Close { code: 1006, reason: String::new() });
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn event_object<'js>(ctx: &Ctx<'js>, event: Event) -> Result<Object<'js>> {
        let object = Object::new(ctx.clone())?;
        match event {
            Event::Open { protocol } => {
                object.set("type", "open")?;
                object.set("protocol", protocol)?;
            }
            Event::Text(text) => {
                object.set("type", "message")?;
                object.set("data", text)?;
                object.set("binary", false)?;
            }
            Event::Binary(bytes) => {
                object.set("type", "message")?;
                object.set("data", TypedArray::<u8>::new_copy(ctx.clone(), bytes)?)?;
                object.set("binary", true)?;
            }
            Event::Error(message) => {
                object.set("type", "error")?;
                object.set("message", message)?;
            }
            Event::Close { code, reason } => {
                object.set("type", "close")?;
                object.set("code", code)?;
                object.set("reason", reason)?;
            }
        }
        Ok(object)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl NativeWebSocket {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>, url: String, protocols: Opt<String>) -> Result<Self> {
        Handle::try_current()
            .map_err(|_| Exception::throw_internal(&ctx, "WebSocket requires a tokio runtime"))?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self::spawn_connection(url, protocols.0, event_tx, command_rx);
        Ok(Self {
            commands: command_tx,
            events: RefCell::new(Some(event_rx)),
        })
    }

    pub async fn next_event<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut events = self
            .events
            .borrow_mut()
            .take()
            .ok_or_else(|| Exception::throw_internal(&ctx, "WebSocket event pump is busy"))?;
        let event = events.recv().await;
        *self.events.borrow_mut() = Some(events);
        match event {
            Some(event) => Ok(Self::event_object(&ctx, event)?.into_value()),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn send_text(&self, ctx: Ctx<'_>, text: String) -> Result<()> {
        self.commands
            .send(Command::SendText(text))
            .map_err(|_| Exception::throw_internal(&ctx, "WebSocket is closed"))
    }

    pub fn send_binary<'js>(&self, ctx: Ctx<'js>, bytes: TypedArray<'js, u8>) -> Result<()> {
        let data = bytes
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(&ctx, "buffer is detached"))?
            .to_vec();
        self.commands
            .send(Command::SendBinary(data))
            .map_err(|_| Exception::throw_internal(&ctx, "WebSocket is closed"))
    }

    pub fn close(&self, code: Opt<u16>, reason: Opt<String>) {
        let _ = self.commands.send(Command::Close {
            code: code.0.unwrap_or(1000),
            reason: reason.0.unwrap_or_default(),
        });
    }
}
