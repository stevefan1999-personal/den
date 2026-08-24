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

#[cfg(test)]
mod tests {
    use den_core::engine::Engine;

    #[tokio::test(flavor = "multi_thread")]
    async fn networking_module_exports_socket_classes() {
        let names: String = Engine::new()
            .await
            .eval(
                r#"
                  const ns = await import("den:networking");
                  Object.keys(ns).sort().join(",")
                "#,
            )
            .await
            .expect("den:networking evaluates");
        assert_eq!(
            names,
            "TcpListener,TcpStream,TlsListener,TlsStream,UdpSocket,UnixListener,UnixStream,\
             WebSocket"
        );
    }
}
