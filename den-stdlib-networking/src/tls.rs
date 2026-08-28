use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{Ctx, JsLifetime, Result, TypedArray, class::Trace, convert::List, function::Opt};
use rustls::{ClientConfig, RootCertStore, ServerConfig, pki_types::ServerName};
use rustls_platform_verifier::BuilderVerifierExt as _;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::{
    io::{AsyncReadWrapper, AsyncWriteWrapper, JsByteBuf, impl_stream_wrapper},
    socket_addr::SocketAddrWrapper,
};

pub(crate) struct Tls;

impl Tls {
    /// Trust for a client connection.
    ///
    /// A custom CA *replaces* the platform store rather than adding to it,
    /// which is what native-tls's `disable_built_in_roots(true)` did here and
    /// what pinning a single self-signed peer means. Without one the platform
    /// verifier decides, so this agrees with the OS trust decisions reqwest
    /// already makes.
    ///
    /// Shared with `websocket.rs`: the `wss` connector must trust exactly what
    /// `TlsStream.connect` does.
    pub(crate) fn client_config(ca_pem: Option<&str>) -> std::io::Result<ClientConfig> {
        let Some(pem) = ca_pem else {
            return ClientConfig::builder()
                .with_platform_verifier()
                .map(|verified| verified.with_no_client_auth())
                .map_err(std::io::Error::other);
        };
        let mut roots = RootCertStore::empty();
        for certificate in rustls_pemfile::certs(&mut pem.as_bytes()) {
            roots.add(certificate?).map_err(std::io::Error::other)?;
        }
        Ok(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth())
    }

    fn connector(ca_pem: Option<&str>) -> Result<TlsConnector> {
        Ok(TlsConnector::from(Arc::new(Self::client_config(ca_pem)?)))
    }

    /// The server side: a PEM chain plus its key, asking for no client
    /// certificate — the same shape `native_tls::Identity::from_pkcs8` gave.
    fn acceptor(cert_pem: &str, key_pem: &str) -> Result<TlsAcceptor> {
        let chain =
            rustls_pemfile::certs(&mut cert_pem.as_bytes()).collect::<std::io::Result<Vec<_>>>()?;
        let key = rustls_pemfile::private_key(&mut key_pem.as_bytes())?
            .ok_or_else(|| std::io::Error::other("the key PEM holds no private key"))?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(std::io::Error::other)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "TlsStream")]
pub struct TlsStreamWrapper {
    #[qjs(skip_trace)]
    stream: Arc<RwLock<TlsStream<TcpStream>>>,
}

impl_stream_wrapper! {
    TlsStreamWrapper,

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> {
        let this = self
            .stream
            .try_read()
            .map_err(|_| rquickjs::Error::Unknown)?;
        Ok(this.get_ref().0.local_addr()?.into())
    }

    #[qjs(static)]
    pub async fn connect(addr: String, domain: String, Opt(ca_pem): Opt<String>) -> Result<Self> {
        let connector = Tls::connector(ca_pem.as_deref())?;
        // SNI, and the name the certificate is checked against.
        let server_name = ServerName::try_from(domain).map_err(std::io::Error::other)?;
        let tcp = TcpStream::connect(&addr).await?;
        let stream = connector.connect(server_name, tcp).await?;
        Ok(Arc::new(RwLock::new(TlsStream::from(stream))).into())
    }
}

// No `Debug`: `tokio_rustls::TlsAcceptor` has none, and nothing formats a
// listener.
#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "TlsListener")]
pub struct TlsListenerWrapper {
    #[qjs(skip_trace)]
    listener: Arc<TcpListener>,
    #[qjs(skip_trace)]
    acceptor: TlsAcceptor,
}

#[rquickjs::methods]
impl TlsListenerWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new TlsListener()`
    // throw: instances only ever come from `TlsListener.listen`.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> { Ok(self.listener.local_addr()?.into()) }

    pub async fn accept(self) -> Result<List<(TlsStreamWrapper, SocketAddrWrapper)>> {
        let (tcp, addr) = self.listener.accept().await?;
        let stream = self.acceptor.accept(tcp).await?;
        Ok(List((
            Arc::new(RwLock::new(TlsStream::from(stream))).into(),
            addr.into(),
        )))
    }

    #[qjs(static)]
    pub async fn listen(addr: String, cert_pem: String, key_pem: String) -> Result<Self> {
        let acceptor = Tls::acceptor(&cert_pem, &key_pem)?;
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener: Arc::new(listener),
            acceptor,
        })
    }
}

#[cfg(test)]
mod tests {
    use den_core::engine::Engine;
    use either::Either;
    use rquickjs::{CatchResultExt, convert::List, function::Opt};

    use super::{TlsListenerWrapper, TlsStreamWrapper};

    struct TestCert;

    impl TestCert {
        fn localhost() -> (String, String) {
            let certified = rcgen::generate_simple_self_signed(["localhost".to_string()])
                .expect("self-signed cert");
            (certified.cert.pem(), certified.signing_key.serialize_pem())
        }
    }

    #[tokio::test]
    async fn connect_to_a_local_acceptor_round_trips() {
        let engine = Engine::new().await;
        let outcome: String = engine
            .context
            .async_with(async |ctx| {
                let run = async {
                    let (cert_pem, key_pem) = TestCert::localhost();
                    let listener =
                        TlsListenerWrapper::listen("127.0.0.1:0".into(), cert_pem.clone(), key_pem)
                            .await?;
                    let dest = listener.local_addr()?.to_string();
                    let connecting =
                        TlsStreamWrapper::connect(dest, "localhost".into(), Opt(Some(cert_pem)));
                    let accepting = listener.accept();
                    let (client, accepted) = tokio::join!(connecting, accepting);
                    let client = client?;
                    let List((server, _)) = accepted?;
                    client
                        .clone()
                        .write_all(Either::Right(Either::Left(b"hello TLS!".to_vec())))
                        .await?;
                    let received = {
                        let chunk = server.read(11, ctx.clone()).await?;
                        chunk
                            .as_bytes()
                            .expect("the chunk is still attached")
                            .to_vec()
                    };
                    Ok::<_, rquickjs::Error>(format!("bytes:{}", received == b"hello TLS!"))
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the tls stream round-trips");
        assert_eq!(outcome, "bytes:true");
    }
}
