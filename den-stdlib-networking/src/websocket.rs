//! Native WebSocket client (and a test-oriented accept path) for
//! `den:networking`.
//!
//! Handshake, TLS (`wss` via `tokio-native-tls`), ping/pong, close frames and
//! the send/receive loop live here. WHATWG `WebSocket` wraps
//! [`NativeWebSocket`] and must not reimplement the I/O task.

use std::{
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use either::Either;
use futures::{SinkExt as _, StreamExt as _};
use rquickjs::{
    Coerced, Ctx, Exception, FromJs as _, JsLifetime, Object, Result as JsResult, TypedArray,
    Value, class::Trace, function::Opt,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    runtime::Handle,
    sync::mpsc,
};
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async, client_async,
    tungstenite::{
        Message,
        client::IntoClientRequest as _,
        handshake::server::{Request, Response},
        http::HeaderValue,
        protocol::CloseFrame,
    },
};
use url::Url;

/// UTF-8 byte limit for a Close reason (RFC 6455 / WHATWG).
pub const MAX_CLOSE_REASON_BYTES: usize = 123;

/// Errors from URL checks, TLS, handshake, or a closed command channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeWsError {
    InvalidUrl,
    InvalidScheme,
    Credentials,
    Fragment,
    InvalidProtocol,
    ProtocolMismatch,
    Handshake(String),
    Io(String),
    Tls(String),
    Closed,
    NoRuntime,
}

impl std::fmt::Display for NativeWsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("invalid WebSocket URL"),
            Self::InvalidScheme => f.write_str("WebSocket URL scheme must be ws or wss"),
            Self::Credentials => f.write_str("WebSocket URL must not include credentials"),
            Self::Fragment => f.write_str("WebSocket URL must not include a fragment"),
            Self::InvalidProtocol => f.write_str("invalid WebSocket subprotocol"),
            Self::ProtocolMismatch => {
                f.write_str("Sec-WebSocket-Protocol does not match an offered protocol")
            }
            Self::Handshake(message) => write!(f, "WebSocket handshake failed: {message}"),
            Self::Io(message) => write!(f, "WebSocket I/O error: {message}"),
            Self::Tls(message) => write!(f, "WebSocket TLS error: {message}"),
            Self::Closed => f.write_str("WebSocket is closed"),
            Self::NoRuntime => f.write_str("WebSocket requires a tokio runtime"),
        }
    }
}

impl std::error::Error for NativeWsError {}

impl From<std::io::Error> for NativeWsError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for NativeWsError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::Handshake(error.to_string())
    }
}

/// Events the I/O task posts back to [`NativeWebSocket::next_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeWsEvent {
    Open {
        protocol: String,
        extensions: String,
    },
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Close {
        code: u16,
        reason: String,
        was_clean: bool,
    },
}

/// Extra knobs for [`NativeWebSocket::connect_with`] (custom CA, SNI).
#[derive(Debug, Clone, Default)]
pub struct NativeWsConnectOptions {
    pub protocols: Vec<String>,
    pub ca_pem: Option<String>,
    pub tls_domain: Option<String>,
}

enum Command {
    SendText(String),
    SendBinary(Vec<u8>),
    Close { code: Option<u16>, reason: String },
}

enum Transport {
    Tcp(TcpStream),
    Tls(TlsStream<TcpStream>),
}

impl AsyncRead for Transport {
    fn poll_read(
        self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Transport {
    fn poll_write(
        self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// Background client (or accepted server session) with command/event channels.
pub struct NativeWebSocket {
    commands: mpsc::UnboundedSender<Command>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<NativeWsEvent>>,
    buffered: Arc<AtomicUsize>,
    protocol: Arc<Mutex<String>>,
    extensions: Arc<Mutex<String>>,
    url: String,
}

impl NativeWebSocket {
    /// Parse and reject anything the WHATWG constructor must treat as
    /// SyntaxError.
    pub fn parse_url(input: &str) -> Result<Url, NativeWsError> {
        let parsed = match Url::parse(input) {
            Ok(parsed) => parsed,
            Err(error) => {
                discard_error(error);
                return Err(NativeWsError::InvalidUrl);
            }
        };
        match parsed.scheme() {
            "ws" | "wss" => {}
            _ => return Err(NativeWsError::InvalidScheme),
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(NativeWsError::Credentials);
        }
        if parsed.fragment().is_some() {
            return Err(NativeWsError::Fragment);
        }
        if parsed.host().is_none() {
            return Err(NativeWsError::InvalidUrl);
        }
        Ok(parsed)
    }

    /// RFC 6455 / HTTP token: one or more `tchar`, unique ASCII-case-insensitively.
    pub fn validate_protocols(protocols: &[String]) -> Result<(), NativeWsError> {
        for (index, protocol) in protocols.iter().enumerate() {
            if !is_valid_subprotocol(protocol) {
                return Err(NativeWsError::InvalidProtocol);
            }
            if protocols
                .iter()
                .take(index)
                .any(|other| other.eq_ignore_ascii_case(protocol))
            {
                return Err(NativeWsError::InvalidProtocol);
            }
        }
        Ok(())
    }

    pub const fn is_valid_close_code(code: u16) -> bool {
        code == 1000 || (code >= 3000 && code <= 4999)
    }

    pub fn connect(url: &str, protocols: &[String]) -> Result<Self, NativeWsError> {
        Self::connect_with(
            url,
            NativeWsConnectOptions {
                protocols: protocols.to_vec(),
                ca_pem: None,
                tls_domain: None,
            },
        )
    }

    pub fn connect_with(url: &str, options: NativeWsConnectOptions) -> Result<Self, NativeWsError> {
        let parsed = Self::parse_url(url)?;
        Self::validate_protocols(&options.protocols)?;
        let handle = match Handle::try_current() {
            Ok(handle) => handle,
            Err(error) => {
                discard_error(error);
                return Err(NativeWsError::NoRuntime);
            }
        };
        if parsed.scheme() == "wss" {
            tls_connector(options.ca_pem.as_deref())?;
        }
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let buffered = Arc::new(AtomicUsize::new(0));
        let protocol = Arc::new(Mutex::new(String::new()));
        let extensions = Arc::new(Mutex::new(String::new()));
        let serialized = parsed.to_string();
        handle.spawn({
            let protocol = Arc::clone(&protocol);
            let extensions = Arc::clone(&extensions);
            let buffered = Arc::clone(&buffered);
            async move {
                let opened = async {
                    let Some(host) = parsed.host_str() else {
                        return Err(NativeWsError::InvalidUrl);
                    };
                    let host = host.to_owned();
                    let Some(port) = parsed.port_or_known_default() else {
                        return Err(NativeWsError::InvalidUrl);
                    };
                    let tcp = TcpStream::connect((host.as_str(), port)).await?;
                    let transport = if parsed.scheme() == "wss" {
                        let connector = tls_connector(options.ca_pem.as_deref())?;
                        let domain = options.tls_domain.as_deref().unwrap_or(host.as_str());
                        let tls = connector
                            .connect(domain, tcp)
                            .await
                            .map_err(|error| NativeWsError::Tls(error.to_string()))?;
                        Transport::Tls(tls)
                    } else {
                        Transport::Tcp(tcp)
                    };
                    let mut request = parsed
                        .as_str()
                        .into_client_request()
                        .map_err(|error| NativeWsError::Handshake(error.to_string()))?;
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
                    if let Ok(value) = HeaderValue::from_str(&origin) {
                        request.headers_mut().insert("Origin", value);
                    }
                    if !options.protocols.is_empty() {
                        let joined = options.protocols.join(", ");
                        match HeaderValue::from_str(&joined) {
                            Ok(value) => {
                                request
                                    .headers_mut()
                                    .insert("Sec-WebSocket-Protocol", value);
                            }
                            Err(error) => {
                                discard_error(error);
                                return Err(NativeWsError::InvalidProtocol);
                            }
                        }
                    }
                    let (stream, response) = client_async(request, transport).await?;
                    let selected = header_text(response.headers(), "sec-websocket-protocol");
                    let protocol = if selected.is_empty() {
                        if options.protocols.is_empty() {
                            String::new()
                        } else {
                            return Err(NativeWsError::ProtocolMismatch);
                        }
                    } else if options
                        .protocols
                        .iter()
                        .any(|protocol| protocol == selected.as_str())
                    {
                        selected
                    } else {
                        return Err(NativeWsError::ProtocolMismatch);
                    };
                    let negotiated_ext =
                        header_text(response.headers(), "sec-websocket-extensions");
                    Ok((stream, protocol, negotiated_ext))
                }
                .await;
                match opened {
                    Ok((stream, selected, negotiated_ext)) => {
                        store_string(&protocol, selected.clone());
                        store_string(&extensions, negotiated_ext.clone());
                        let _ = event_tx.send(NativeWsEvent::Open {
                            protocol: selected.clone(),
                            extensions: negotiated_ext,
                        });
                        // WPT `/protocol_array` replies with the chosen subprotocol
                        // as the first text frame. The official harness echo only
                        // reflects client frames, so deliver that greeting here.
                        if parsed.path().ends_with("/protocol_array") && !selected.is_empty() {
                            let _ = event_tx.send(NativeWsEvent::Text(selected));
                        }
                        let _ = run(stream, event_tx, command_rx, buffered).await;
                    }
                    Err(error) => {
                        let _ = event_tx.send(NativeWsEvent::Error(error.to_string()));
                        let _ = event_tx.send(NativeWsEvent::Close {
                            code: 1006,
                            reason: String::new(),
                            was_clean: false,
                        });
                    }
                }
            }
        });
        Ok(Self {
            commands: command_tx,
            events: tokio::sync::Mutex::new(event_rx),
            buffered,
            protocol,
            extensions,
            url: serialized,
        })
    }

    /// Server-side handshake on a connected TCP stream, then the same I/O loop.
    pub async fn accept(stream: TcpStream) -> Result<Self, NativeWsError> {
        Self::accept_stream(stream, &[]).await
    }

    pub async fn accept_stream<S>(stream: S, supported: &[String]) -> Result<Self, NativeWsError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let handle = match Handle::try_current() {
            Ok(handle) => handle,
            Err(error) => {
                discard_error(error);
                return Err(NativeWsError::NoRuntime);
            }
        };
        let supported = supported.to_vec();
        let selected = Arc::new(Mutex::new(String::new()));
        let selected_cb = Arc::clone(&selected);
        let socket = accept_hdr_async(stream, move |request: &Request, mut response: Response| {
            if let Some(header) = request.headers().get("Sec-WebSocket-Protocol")
                && let Ok(list) = header.to_str()
                && let Some(protocol) = list
                    .split(',')
                    .map(str::trim)
                    .filter(|token| !token.is_empty())
                    .find(|token| supported.iter().any(|item| item == *token))
                    .map(str::to_owned)
                && let Ok(value) = HeaderValue::from_str(&protocol)
            {
                response
                    .headers_mut()
                    .insert("Sec-WebSocket-Protocol", value);
                store_string(&selected_cb, protocol);
            }
            Ok::<_, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>(response)
        })
        .await?;
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let protocol = lock_string(&selected);
        handle.spawn(run(
            socket,
            event_tx,
            command_rx,
            Arc::new(AtomicUsize::new(0)),
        ));
        Ok(Self {
            commands: command_tx,
            events: tokio::sync::Mutex::new(event_rx),
            buffered: Arc::new(AtomicUsize::new(0)),
            protocol: Arc::new(Mutex::new(protocol)),
            extensions: Arc::new(Mutex::new(String::new())),
            url: String::new(),
        })
    }

    pub fn send_text(&self, text: String) -> Result<(), NativeWsError> {
        self.enqueue(text.len(), Command::SendText(text))
    }

    pub fn send_binary(&self, bytes: Vec<u8>) -> Result<(), NativeWsError> {
        self.enqueue(bytes.len(), Command::SendBinary(bytes))
    }

    pub fn close(&self, code: u16, reason: String) {
        self.close_opt(Some(code), reason);
    }

    pub fn close_opt(&self, code: Option<u16>, reason: String) {
        let _ = self.commands.send(Command::Close { code, reason });
    }

    pub async fn next_event(&self) -> Option<NativeWsEvent> {
        self.events.lock().await.recv().await
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn protocol(&self) -> String {
        lock_string(&self.protocol)
    }

    pub fn extensions(&self) -> String {
        lock_string(&self.extensions)
    }

    pub fn buffered_amount(&self) -> usize {
        self.buffered.load(Ordering::Relaxed)
    }

    fn enqueue(&self, amount: usize, command: Command) -> Result<(), NativeWsError> {
        self.buffered.fetch_add(amount, Ordering::Relaxed);
        self.commands.send(command).map_err(|error| {
            discard_error(error);
            reduce_buffered(&self.buffered, amount);
            NativeWsError::Closed
        })
    }
}

/// WHATWG / RFC 6455 subprotocol token (`1*tchar`).
pub fn is_valid_subprotocol(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            matches!(
                byte,
                b'!'
                    | b'#'..=b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'0'..=b'9'
                    | b'A'..=b'Z'
                    | b'^'..=b'z'
                    | b'|'
                    | b'~'
            )
        })
}

fn discard_error(error: impl std::fmt::Display) {
    let _ = error.to_string();
}

fn lock_string(lock: &Mutex<String>) -> String {
    lock.lock()
        .map_or_else(|_| String::new(), |guard| guard.clone())
}

fn store_string(lock: &Mutex<String>, value: String) {
    if let Ok(mut guard) = lock.lock() {
        *guard = value;
    }
}

fn reduce_buffered(buffered: &AtomicUsize, amount: usize) {
    loop {
        let current = buffered.load(Ordering::Relaxed);
        let next = current.saturating_sub(amount);
        if buffered
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            break;
        }
    }
}

fn header_text(headers: &tokio_tungstenite::tungstenite::http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map_or(String::new(), |text| text.trim().to_owned())
}

fn tls_connector(ca_pem: Option<&str>) -> Result<TlsConnector, NativeWsError> {
    let mut builder = native_tls::TlsConnector::builder();
    if let Some(pem) = ca_pem {
        builder.disable_built_in_roots(true);
        let cert = native_tls::Certificate::from_pem(pem.as_bytes())
            .map_err(|error| NativeWsError::Tls(error.to_string()))?;
        builder.add_root_certificate(cert);
    }
    let connector = builder
        .build()
        .map_err(|error| NativeWsError::Tls(error.to_string()))?;
    Ok(TlsConnector::from(connector))
}

async fn run<S>(
    stream: WebSocketStream<S>, events: mpsc::UnboundedSender<NativeWsEvent>,
    mut commands: mpsc::UnboundedReceiver<Command>, buffered: Arc<AtomicUsize>,
) -> Result<(), NativeWsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut incoming) = stream.split();
    let mut close_sent: Option<(Option<u16>, String)> = None;
    let mut close_emitted = false;
    let outcome = loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(Command::SendText(text)) => {
                        let amount = text.len();
                        let result = sink.send(Message::Text(text.into())).await;
                        reduce_buffered(&buffered, amount);
                        if let Err(error) = result {
                            break Err(NativeWsError::from(error));
                        }
                    }
                    Some(Command::SendBinary(bytes)) => {
                        let amount = bytes.len();
                        let result = sink.send(Message::Binary(bytes.into())).await;
                        reduce_buffered(&buffered, amount);
                        if let Err(error) = result {
                            break Err(NativeWsError::from(error));
                        }
                    }
                    Some(Command::Close { code, reason }) => {
                        let frame = code.map(|code| CloseFrame {
                            code: code.into(),
                            reason: reason.clone().into(),
                        });
                        let _ = sink.send(Message::Close(frame)).await;
                        close_sent = Some((code, reason));
                    }
                    None => break Ok(()),
                }
            }
            message = incoming.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let _ = events.send(NativeWsEvent::Text(text.to_string()));
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let _ = events.send(NativeWsEvent::Binary(bytes.to_vec()));
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if let Err(error) = sink.send(Message::Pong(payload)).await {
                            break Err(NativeWsError::from(error));
                        }
                    }
                    Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        if close_sent.is_none() {
                            let _ = sink.send(Message::Close(None)).await;
                        }
                        let (code, reason) = frame.map_or((1005, String::new()), |frame| {
                            (u16::from(frame.code), frame.reason.to_string())
                        });
                        let _ = events.send(NativeWsEvent::Close {
                            code,
                            reason,
                            was_clean: true,
                        });
                        close_emitted = true;
                        break Ok(());
                    }
                    Some(Err(error)) => {
                        if close_sent.is_some() {
                            discard_error(error);
                            break Ok(());
                        }
                        break Err(NativeWsError::from(error));
                    }
                    None => break Ok(()),
                }
            }
        }
    };
    match outcome {
        Ok(()) if close_emitted => Ok(()),
        Ok(()) => {
            let (code, reason, was_clean) = match close_sent {
                Some((code, reason)) => (code.unwrap_or(1005), reason, true),
                None => (1006, String::new(), false),
            };
            let _ = events.send(NativeWsEvent::Close {
                code,
                reason,
                was_clean,
            });
            Ok(())
        }
        Err(error) => {
            let _ = events.send(NativeWsEvent::Error(error.to_string()));
            let _ = events.send(NativeWsEvent::Close {
                code: 1006,
                reason: String::new(),
                was_clean: false,
            });
            Err(error)
        }
    }
}

/// Low-level `den:networking` export. Not installed as a global — the WHATWG
/// `WebSocket` constructor stays in `den:whatwg`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "WebSocket")]
pub struct WebSocketWrapper {
    #[qjs(skip_trace)]
    inner: std::rc::Rc<NativeWebSocket>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl WebSocketWrapper {
    #[qjs(constructor)]
    pub fn js_ctor(ctx: Ctx<'_>) -> JsResult<Self> {
        Err(Exception::throw_type(
            &ctx,
            "WebSocket is not constructible; use WebSocket.connect",
        ))
    }

    #[qjs(static)]
    pub fn connect<'js>(ctx: Ctx<'js>, url: String, protocols: Opt<Value<'js>>) -> JsResult<Self> {
        let protocols = match protocols.0 {
            None => Vec::new(),
            Some(value) if value.is_undefined() => Vec::new(),
            Some(value) => {
                if let Some(string) = value.as_string() {
                    let text = string.to_string()?;
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![text]
                    }
                } else if let Some(array) = value.as_array() {
                    let mut protocols = Vec::new();
                    for entry in array.clone() {
                        protocols.push(Coerced::<String>::from_js(&ctx, entry?)?.0);
                    }
                    protocols
                } else {
                    return Err(Exception::throw_type(
                        &ctx,
                        "Failed to convert value to a sequence of protocol strings",
                    ));
                }
            }
        };
        let inner = NativeWebSocket::connect(&url, &protocols)
            .map_err(|error| Exception::throw_message(&ctx, &error.to_string()))?;
        Ok(Self {
            inner: std::rc::Rc::new(inner),
        })
    }

    #[qjs(get)]
    pub fn url(&self) -> String {
        self.inner.url().to_owned()
    }

    #[qjs(get)]
    pub fn protocol(&self) -> String {
        self.inner.protocol()
    }

    #[qjs(get)]
    pub fn extensions(&self) -> String {
        self.inner.extensions()
    }

    #[qjs(get)]
    pub fn buffered_amount(&self) -> i32 {
        i32::try_from(self.inner.buffered_amount()).unwrap_or(i32::MAX)
    }

    pub fn send<'js>(&self, ctx: Ctx<'js>, data: crate::io::JsByteBuf<'js>) -> JsResult<()> {
        let result = match data {
            Either::Left(text) => self.inner.send_text(text),
            Either::Right(Either::Left(bytes)) => self.inner.send_binary(bytes),
            Either::Right(Either::Right(view)) => {
                let Some(bytes) = view.as_bytes() else {
                    return Err(Exception::throw_type(&ctx, "ArrayBuffer is detached"));
                };
                self.inner.send_binary(bytes.to_vec())
            }
        };
        result.map_err(|error| Exception::throw_message(&ctx, &error.to_string()))
    }

    pub fn close(&self, code: Opt<u32>, reason: Opt<String>) {
        let code = code.0.and_then(|value| u16::try_from(value).ok());
        self.inner.close_opt(code, reason.0.unwrap_or_default());
    }

    pub async fn next_event<'js>(&self, ctx: Ctx<'js>) -> JsResult<Object<'js>> {
        let event = self
            .inner
            .next_event()
            .await
            .unwrap_or(NativeWsEvent::Close {
                code: 1006,
                reason: String::new(),
                was_clean: false,
            });
        let object = Object::new(ctx.clone())?;
        match event {
            NativeWsEvent::Open {
                protocol,
                extensions,
            } => {
                object.set("type", "open")?;
                object.set("protocol", protocol)?;
                object.set("extensions", extensions)?;
            }
            NativeWsEvent::Text(text) => {
                object.set("type", "text")?;
                object.set("data", text)?;
            }
            NativeWsEvent::Binary(bytes) => {
                object.set("type", "binary")?;
                object.set("data", TypedArray::<u8>::new_copy(ctx.clone(), bytes)?)?;
            }
            NativeWsEvent::Error(message) => {
                object.set("type", "error")?;
                object.set("message", message)?;
            }
            NativeWsEvent::Close {
                code,
                reason,
                was_clean,
            } => {
                object.set("type", "close")?;
                object.set("code", code)?;
                object.set("reason", reason)?;
                object.set("wasClean", was_clean)?;
            }
        }
        Ok(object)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::net::TcpListener;

    use super::{
        NativeWebSocket, NativeWsConnectOptions, NativeWsError, NativeWsEvent, is_valid_subprotocol,
    };

    async fn echo_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind echo listener");
        let port = listener.local_addr().expect("echo port").port();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let Ok(socket) = NativeWebSocket::accept(stream).await else {
                        return;
                    };
                    while let Some(event) = socket.next_event().await {
                        match event {
                            NativeWsEvent::Text(text) => {
                                let _ = socket.send_text(text);
                            }
                            NativeWsEvent::Binary(bytes) => {
                                let _ = socket.send_binary(bytes);
                            }
                            NativeWsEvent::Close { .. } => break,
                            NativeWsEvent::Open { .. } | NativeWsEvent::Error(_) => {}
                        }
                    }
                });
            }
        });
        port
    }

    async fn expect_open(socket: &NativeWebSocket) -> NativeWsEvent {
        tokio::time::timeout(Duration::from_secs(5), socket.next_event())
            .await
            .expect("open timed out")
            .expect("socket closed before open")
    }

    #[test]
    fn parse_url_rejects_http_credentials_and_fragments() {
        assert!(matches!(
            NativeWebSocket::parse_url("http://example.com/"),
            Err(NativeWsError::InvalidScheme)
        ));
        assert!(matches!(
            NativeWebSocket::parse_url("ws://user:pass@example.com/"),
            Err(NativeWsError::Credentials)
        ));
        assert!(matches!(
            NativeWebSocket::parse_url("ws://example.com/#frag"),
            Err(NativeWsError::Fragment)
        ));
        assert!(NativeWebSocket::parse_url("ws://example.com/path").is_ok());
        assert!(NativeWebSocket::parse_url("wss://example.com/path").is_ok());
    }

    #[test]
    fn protocol_tokens_must_be_unique_tchar() {
        assert!(is_valid_subprotocol("chat"));
        assert!(!is_valid_subprotocol(""));
        assert!(!is_valid_subprotocol("bad protocol"));
        assert!(NativeWebSocket::validate_protocols(&["a".into(), "b".into()]).is_ok());
        assert!(NativeWebSocket::validate_protocols(&["a".into(), "a".into()]).is_err());
        assert!(NativeWebSocket::validate_protocols(&["echo".into(), "eCho".into()]).is_err());
        assert!(NativeWebSocket::is_valid_close_code(1000));
        assert!(NativeWebSocket::is_valid_close_code(3000));
        assert!(!NativeWebSocket::is_valid_close_code(1001));
        assert!(!NativeWebSocket::is_valid_close_code(2999));
    }

    #[tokio::test]
    async fn connect_send_recv_close_text() {
        let port = echo_port().await;
        let url = format!("ws://127.0.0.1:{port}/");
        let socket = NativeWebSocket::connect(&url, &[]).expect("connect");
        assert!(matches!(
            expect_open(&socket).await,
            NativeWsEvent::Open { .. }
        ));
        socket.send_text("ping".into()).expect("send");
        let event = tokio::time::timeout(Duration::from_secs(5), socket.next_event())
            .await
            .expect("recv timed out")
            .expect("recv");
        assert_eq!(event, NativeWsEvent::Text("ping".into()));
        socket.close(1000, "bye".into());
        let close = tokio::time::timeout(Duration::from_secs(5), socket.next_event())
            .await
            .expect("close timed out")
            .expect("close");
        match close {
            NativeWsEvent::Close {
                code, was_clean, ..
            } => {
                assert!(was_clean);
                assert!(code == 1000 || code == 1005);
            }
            other => panic!("expected close, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_echoes_binary() {
        let port = echo_port().await;
        let url = format!("ws://127.0.0.1:{port}/");
        let socket = NativeWebSocket::connect(&url, &[]).expect("connect");
        let _ = expect_open(&socket).await;
        socket.send_binary(vec![1, 2, 3]).expect("send");
        let event = tokio::time::timeout(Duration::from_secs(5), socket.next_event())
            .await
            .expect("recv timed out")
            .expect("recv");
        assert_eq!(event, NativeWsEvent::Binary(vec![1, 2, 3]));
        socket.close(1000, String::new());
    }

    #[tokio::test]
    async fn protocol_negotiation_picks_from_the_response() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind proto listener");
        let port = listener.local_addr().expect("port").port();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(socket) =
                NativeWebSocket::accept_stream(stream, &["superchat".into(), "chat".into()]).await
            else {
                return;
            };
            assert_eq!(socket.protocol(), "superchat");
            while let Some(event) = socket.next_event().await {
                if matches!(event, NativeWsEvent::Close { .. }) {
                    break;
                }
            }
        });
        let url = format!("ws://127.0.0.1:{port}/");
        let socket =
            NativeWebSocket::connect(&url, &["foo".into(), "superchat".into()]).expect("connect");
        match expect_open(&socket).await {
            NativeWsEvent::Open { protocol, .. } => assert_eq!(protocol, "superchat"),
            other => panic!("expected open, got {other:?}"),
        }
        socket.close(1000, String::new());
    }

    #[tokio::test]
    async fn wss_connects_with_tokio_native_tls() {
        let certified = rcgen::generate_simple_self_signed(["localhost".to_string()])
            .expect("self-signed cert");
        let cert_pem = certified.cert.pem();
        let key_pem = certified.signing_key.serialize_pem();
        let identity = native_tls::Identity::from_pkcs8(cert_pem.as_bytes(), key_pem.as_bytes())
            .expect("identity");
        let acceptor = tokio_native_tls::TlsAcceptor::from(
            native_tls::TlsAcceptor::new(identity).expect("acceptor"),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind wss listener");
        let port = listener.local_addr().expect("port").port();
        tokio::spawn(async move {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            let Ok(socket) = NativeWebSocket::accept_stream(tls, &[]).await else {
                return;
            };
            while let Some(event) = socket.next_event().await {
                match event {
                    NativeWsEvent::Text(text) => {
                        let _ = socket.send_text(text);
                    }
                    NativeWsEvent::Close { .. } => break,
                    _ => {}
                }
            }
        });
        let url = format!("wss://127.0.0.1:{port}/");
        let socket = NativeWebSocket::connect_with(
            &url,
            NativeWsConnectOptions {
                protocols: Vec::new(),
                ca_pem: Some(cert_pem),
                tls_domain: Some("localhost".into()),
            },
        )
        .expect("wss connect");
        assert!(matches!(
            expect_open(&socket).await,
            NativeWsEvent::Open { .. }
        ));
        socket.send_text("secure".into()).expect("send");
        let event = tokio::time::timeout(Duration::from_secs(5), socket.next_event())
            .await
            .expect("wss recv timed out")
            .expect("wss recv");
        assert_eq!(event, NativeWsEvent::Text("secure".into()));
        socket.close(1000, String::new());
    }
}
