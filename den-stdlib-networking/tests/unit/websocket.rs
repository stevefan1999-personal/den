use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    accept_async, accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        http::HeaderValue,
    },
};

#[cfg(feature = "ring")]
use super::NativeWsConnectOptions;
use super::{NativeWebSocket, NativeWsError, NativeWsEvent, is_valid_subprotocol};

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
            tokio::spawn(echo(stream));
        }
    });
    port
}

async fn echo<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let Ok(mut socket) = accept_async(stream).await else {
        return;
    };
    while let Some(Ok(message)) = socket.next().await {
        match message {
            Message::Text(_) | Message::Binary(_) => {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            Message::Close(frame) => {
                let _ = socket.send(Message::Close(frame)).await;
                break;
            }
            _ => {}
        }
    }
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
#[expect(
    clippy::result_large_err,
    reason = "Tungstenite fixes the handshake callback's HTTP response error type"
)]
async fn protocol_negotiation_picks_from_the_response() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proto listener");
    let port = listener.local_addr().expect("port").port();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut socket) =
            accept_hdr_async(stream, |request: &Request, mut response: Response| {
                let offered = request
                    .headers()
                    .get("Sec-WebSocket-Protocol")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                assert!(offered.split(',').any(|item| item.trim() == "superchat"));
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    HeaderValue::from_static("superchat"),
                );
                Ok(response)
            })
            .await
        else {
            return;
        };
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
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

#[cfg(feature = "ring")]
#[tokio::test]
async fn wss_connects_with_tokio_rustls() {
    crate::tls::install_default_crypto_provider();
    let certified =
        rcgen::generate_simple_self_signed(["localhost".to_string()]).expect("self-signed cert");
    let cert_pem = certified.cert.pem();
    let key_pem = certified.signing_key.serialize_pem();
    let chain = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::io::Result<Vec<_>>>()
        .expect("certificate chain");
    let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())
        .expect("key pem")
        .expect("a private key");
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .expect("acceptor"),
    ));
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
        echo(tls).await;
    });
    let url = format!("wss://127.0.0.1:{port}/");
    let socket = NativeWebSocket::connect_with(&url, NativeWsConnectOptions {
        protocols:  Vec::new(),
        ca_pem:     Some(cert_pem),
        tls_domain: Some("localhost".into()),
    })
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
