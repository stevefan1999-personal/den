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

/// Install the selected built-in TLS provider unless the host installed one.
///
/// With neither provider feature this deliberately does nothing, leaving a
/// custom process provider in the embedding application's hands.
#[cfg(feature = "ring")]
pub fn install_default_crypto_provider() {
    let provider = rustls::crypto::ring::default_provider();
    let _provider_installed = provider.install_default();
}

/// Leave TLS provider selection to the embedding application.
#[cfg(not(feature = "ring"))]
pub const fn install_default_crypto_provider() {}

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "TlsStream")]
pub struct TlsStreamWrapper {
    #[qjs(skip_trace)]
    stream: Arc<RwLock<TlsStream<TcpStream>>>,
}

impl TlsStreamWrapper {
    /// Trust for a client connection.
    ///
    /// A custom CA replaces the platform store. Without one the platform
    /// verifier is used, so WebSocket and future HTTP clients share this rule.
    pub(crate) fn client_config(ca_pem: Option<&str>) -> std::io::Result<ClientConfig> {
        install_default_crypto_provider();
        let Some(pem) = ca_pem else {
            return ClientConfig::builder()
                .with_platform_verifier()
                .map(rustls::ConfigBuilder::<
                    ClientConfig,
                    rustls::client::WantsClientCert,
                >::with_no_client_auth)
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
}

impl_stream_wrapper! {
    TlsStreamWrapper,

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> {
        let this = self
            .stream
            .try_read()
            .map_err(|_error| rquickjs::Error::Unknown)?;
        Ok(this.get_ref().0.local_addr()?.into())
    }

    #[qjs(static)]
    pub async fn connect(addr: String, domain: String, Opt(ca_pem): Opt<String>) -> Result<Self> {
        let connector = Self::connector(ca_pem.as_deref())?;
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

impl TlsListenerWrapper {
    /// A PEM certificate chain and private key, with no client certificate.
    fn acceptor(cert_pem: &str, key_pem: &str) -> Result<TlsAcceptor> {
        install_default_crypto_provider();
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

#[rquickjs::methods]
impl TlsListenerWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new TlsListener()`
    // throw: instances only ever come from `TlsListener.listen`.
    #[expect(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub const fn new() {}

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
        let acceptor = Self::acceptor(&cert_pem, &key_pem)?;
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener: Arc::new(listener),
            acceptor,
        })
    }
}

#[cfg(all(test, feature = "ring"))]
#[path = "../tests/unit/tls.rs"]
mod tests;
