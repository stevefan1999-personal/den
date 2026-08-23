use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{Ctx, JsLifetime, Result, TypedArray, class::Trace, convert::List, function::Opt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
};
use tokio_native_tls::{TlsAcceptor, TlsConnector, TlsStream as NativeTlsStream};

use crate::{
    io::{AsyncReadWrapper, AsyncWriteWrapper, JsByteBuf},
    socket_addr::SocketAddrWrapper,
};

/// native-tls rather than rustls: `den-stdlib-whatwg-fetch` already pulls
/// reqwest's `native-tls` feature, so this crate shares that stack.
struct NativeTls;

impl NativeTls {
    fn error(err: native_tls::Error) -> rquickjs::Error { std::io::Error::other(err).into() }

    fn connector(ca_pem: Option<&str>) -> Result<TlsConnector> {
        let mut builder = native_tls::TlsConnector::builder();
        if let Some(pem) = ca_pem {
            builder.disable_built_in_roots(true);
            let cert = native_tls::Certificate::from_pem(pem.as_bytes()).map_err(Self::error)?;
            builder.add_root_certificate(cert);
        }
        let connector = builder.build().map_err(Self::error)?;
        Ok(TlsConnector::from(connector))
    }

    fn acceptor(cert_pem: &str, key_pem: &str) -> Result<TlsAcceptor> {
        let identity = native_tls::Identity::from_pkcs8(cert_pem.as_bytes(), key_pem.as_bytes())
            .map_err(Self::error)?;
        let acceptor = native_tls::TlsAcceptor::new(identity).map_err(Self::error)?;
        Ok(TlsAcceptor::from(acceptor))
    }
}

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "TlsStream")]
pub struct TlsStreamWrapper {
    #[qjs(skip_trace)]
    stream: Arc<RwLock<NativeTlsStream<TcpStream>>>,
}

#[rquickjs::methods]
impl TlsStreamWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new TlsStream()`
    // throw: instances only ever come from `TlsStream.connect` or
    // `TlsListener.accept`.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> {
        let this = self
            .stream
            .try_read()
            .map_err(|_| rquickjs::Error::Unknown)?;
        Ok(this.get_ref().get_ref().get_ref().local_addr()?.into())
    }

    #[qjs(static)]
    pub async fn connect(addr: String, domain: String, Opt(ca_pem): Opt<String>) -> Result<Self> {
        let connector = NativeTls::connector(ca_pem.as_deref())?;
        let tcp = TcpStream::connect(&addr).await?;
        let stream = connector
            .connect(&domain, tcp)
            .await
            .map_err(NativeTls::error)?;
        Ok(Arc::new(RwLock::new(stream)).into())
    }

    pub async fn read_to_string(self) -> Result<String> {
        AsyncReadWrapper(self.stream).read_to_string().await
    }

    pub async fn read_to_end(self) -> Result<Vec<u8>> {
        AsyncReadWrapper(self.stream).read_to_end().await
    }

    pub async fn read<'js>(self, bytes: usize, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        AsyncReadWrapper(self.stream).read(bytes, ctx).await
    }

    pub async fn write_all<'js>(self, buf: JsByteBuf<'js>) -> Result<()> {
        AsyncWriteWrapper(self.stream).write_all(buf).await
    }

    pub async fn flush(self) -> Result<()> { AsyncWriteWrapper(self.stream).flush().await }

    pub async fn shutdown(self) -> Result<()> { AsyncWriteWrapper(self.stream).shutdown().await }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
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
        let stream = self.acceptor.accept(tcp).await.map_err(NativeTls::error)?;
        Ok(List((Arc::new(RwLock::new(stream)).into(), addr.into())))
    }

    #[qjs(static)]
    pub async fn listen(addr: String, cert_pem: String, key_pem: String) -> Result<Self> {
        let acceptor = NativeTls::acceptor(&cert_pem, &key_pem)?;
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener: Arc::new(listener),
            acceptor,
        })
    }
}

#[cfg(test)]
mod tests {
    use either::Either;
    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, convert::List, function::Opt};

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
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let outcome: String = context
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
