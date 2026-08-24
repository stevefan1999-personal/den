pub mod io;
pub mod ip_addr;
pub mod socket;
pub mod socket_addr;
pub mod tls;
pub mod udp;
pub mod unix;
pub mod websocket;

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod networking {
    pub use crate::{
        socket::{TcpListenerWrapper as TcpListener, TcpStreamWrapper as TcpStream},
        tls::{TlsListenerWrapper as TlsListener, TlsStreamWrapper as TlsStream},
        udp::UdpSocketWrapper as UdpSocket,
        unix::{UnixListenerWrapper as UnixListener, UnixStreamWrapper as UnixStream},
        websocket::WebSocketWrapper as WebSocket,
    };
}
