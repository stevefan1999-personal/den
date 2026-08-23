pub mod io;
pub mod ip_addr;
pub mod socket;
pub mod socket_addr;
pub mod tls;
pub mod udp;
pub mod unix;

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "PascalCase"
)]
pub mod networking {
    pub use crate::socket::{TcpListenerWrapper as TcpListener, TcpStreamWrapper as TcpStream};
    pub use crate::tls::{TlsListenerWrapper as TlsListener, TlsStreamWrapper as TlsStream};
    pub use crate::udp::UdpSocketWrapper as UdpSocket;
    pub use crate::unix::{UnixListenerWrapper as UnixListener, UnixStreamWrapper as UnixStream};
}

#[cfg(test)]
mod tests {
    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Module};

    #[tokio::test]
    async fn networking_module_exports_socket_classes() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let names: String = context
            .with(|ctx| {
                (|| {
                    let (module, evaluated) = Module::evaluate_def::<crate::js_networking, _>(
                        ctx.clone(),
                        "den:networking",
                    )?;
                    evaluated.finish::<()>()?;
                    ctx.globals().set("moduleExports", module.namespace()?)?;
                    ctx.eval::<String, _>("Object.keys(moduleExports).sort().join(',')")
                })()
                .catch(&ctx)
                .map_err(|err| err.to_string())
            })
            .await
            .expect("den:networking evaluates");
        assert_eq!(
            names,
            "TcpListener,TcpStream,TlsListener,TlsStream,UdpSocket,UnixListener,UnixStream"
        );
    }
}
