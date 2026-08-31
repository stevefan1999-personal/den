//! Native WebSocket client for `den:networking`.
//!
//! Handshake, TLS (`wss` via `tokio-rustls`), ping/pong, close frames and
//! the send/receive loop live here. WHATWG `WebSocket` wraps
//! [`NativeWebSocket`] and must not reimplement the I/O task.

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use derive_more::{Display, Error};
use futures::{SinkExt as _, StreamExt as _};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    runtime::Handle,
    sync::mpsc,
};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls::pki_types::ServerName};
use tokio_tungstenite::{
    WebSocketStream, client_async,
    tungstenite::{
        Message, client::IntoClientRequest as _, http::HeaderValue, protocol::CloseFrame,
    },
};
use url::Url;

/// UTF-8 byte limit for a Close reason (RFC 6455 / WHATWG).
pub const MAX_CLOSE_REASON_BYTES: usize = 123;

/// Errors from URL checks, TLS, handshake, or a closed command channel.
#[derive(Debug, Clone, PartialEq, Eq, Display, Error)]
pub enum NativeWsError {
    #[display("invalid WebSocket URL")]
    InvalidUrl,
    #[display("WebSocket URL scheme must be ws or wss")]
    InvalidScheme,
    #[display("WebSocket URL must not include credentials")]
    Credentials,
    #[display("WebSocket URL must not include a fragment")]
    Fragment,
    #[display("invalid WebSocket subprotocol")]
    InvalidProtocol,
    #[display("Sec-WebSocket-Protocol does not match an offered protocol")]
    ProtocolMismatch,
    #[display("WebSocket handshake failed: {_0}")]
    Handshake(#[error(not(source))] String),
    #[display("WebSocket I/O error: {_0}")]
    Io(#[error(not(source))] String),
    #[display("WebSocket TLS error: {_0}")]
    Tls(#[error(not(source))] String),
    #[display("WebSocket is closed")]
    Closed,
    #[display("WebSocket requires a tokio runtime")]
    NoRuntime,
}

impl From<std::io::Error> for NativeWsError {
    fn from(error: std::io::Error) -> Self { Self::Io(error.to_string()) }
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
        protocol:   String,
        extensions: String,
    },
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Close {
        code:      u16,
        reason:    String,
        was_clean: bool,
    },
}

/// Extra knobs for [`NativeWebSocket::connect_with`] (custom CA, SNI).
#[derive(Debug, Clone, Default)]
pub struct NativeWsConnectOptions {
    pub protocols:  Vec<String>,
    pub ca_pem:     Option<String>,
    pub tls_domain: Option<String>,
}

enum Command {
    SendText(String),
    SendBinary(Vec<u8>),
    Close { code: Option<u16>, reason: String },
}

enum Transport {
    Tcp(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
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

/// Background client with command/event channels.
pub struct NativeWebSocket {
    commands: mpsc::UnboundedSender<Command>,
    events:   tokio::sync::Mutex<mpsc::UnboundedReceiver<NativeWsEvent>>,
    buffered: Arc<AtomicUsize>,
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

    /// RFC 6455 / HTTP token: one or more `tchar`, unique
    /// ASCII-case-insensitively.
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
        Self::connect_with(url, NativeWsConnectOptions {
            protocols:  protocols.to_vec(),
            ca_pem:     None,
            tls_domain: None,
        })
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
        handle.spawn({
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
                        let server_name = ServerName::try_from(domain.to_owned())
                            .map_err(|error| NativeWsError::Tls(error.to_string()))?;
                        let tls = connector
                            .connect(server_name, tcp)
                            .await
                            .map_err(|error| NativeWsError::Tls(error.to_string()))?;
                        Transport::Tls(Box::new(tls))
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
                        let _ = event_tx.send(NativeWsEvent::Open {
                            protocol:   selected,
                            extensions: negotiated_ext,
                        });
                        let _ = run(stream, event_tx, command_rx, buffered).await;
                    }
                    Err(error) => {
                        let _ = event_tx.send(NativeWsEvent::Error(error.to_string()));
                        let _ = event_tx.send(NativeWsEvent::Close {
                            code:      1006,
                            reason:    String::new(),
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
        })
    }

    pub fn send_text(&self, text: String) -> Result<(), NativeWsError> {
        self.enqueue(text.len(), Command::SendText(text))
    }

    pub fn send_binary(&self, bytes: Vec<u8>) -> Result<(), NativeWsError> {
        self.enqueue(bytes.len(), Command::SendBinary(bytes))
    }

    pub fn close(&self, code: u16, reason: String) { self.close_opt(Some(code), reason); }

    pub fn close_opt(&self, code: Option<u16>, reason: String) {
        let _ = self.commands.send(Command::Close { code, reason });
    }

    pub async fn next_event(&self) -> Option<NativeWsEvent> {
        self.events.lock().await.recv().await
    }

    pub fn buffered_amount(&self) -> usize { self.buffered.load(Ordering::Relaxed) }

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

fn discard_error(error: impl std::fmt::Display) { let _ = error.to_string(); }

fn reduce_buffered(buffered: &AtomicUsize, amount: usize) {
    let _ = buffered.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(amount))
    });
}

fn header_text(headers: &tokio_tungstenite::tungstenite::http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map_or(String::new(), |text| text.trim().to_owned())
}

/// The `wss` connector, over the same trust rules as `TlsStream.connect`:
/// a custom CA replaces the platform store, otherwise the platform verifier.
fn tls_connector(ca_pem: Option<&str>) -> Result<TlsConnector, NativeWsError> {
    crate::tls::TlsStreamWrapper::client_config(ca_pem)
        .map(|config| TlsConnector::from(Arc::new(config)))
        .map_err(|error| NativeWsError::Tls(error.to_string()))
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
                code:      1006,
                reason:    String::new(),
                was_clean: false,
            });
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/websocket.rs"]
mod tests;
