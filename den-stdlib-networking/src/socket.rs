use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{Ctx, Error, JsLifetime, Result, TypedArray, class::Trace, convert::List};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
};

use crate::{
    io::{AsyncReadWrapper, AsyncWriteWrapper, JsByteBuf, impl_stream_wrapper},
    socket_addr::SocketAddrWrapper,
};

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "TcpStream")]
pub struct TcpStreamWrapper {
    #[qjs(skip_trace)]
    stream: Arc<RwLock<TcpStream>>,
}

impl_stream_wrapper! {
    TcpStreamWrapper,

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> {
        let this = self.stream.try_read().map_err(|_error| Error::Unknown)?;
        let addr = this.local_addr()?;
        drop(this);
        Ok(addr.into())
    }

    #[qjs(static)]
    pub async fn connect(addr: String) -> Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Arc::new(RwLock::new(stream)).into())
    }
}

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "TcpListener")]
pub struct TcpListenerWrapper {
    #[qjs(skip_trace)]
    listener: Arc<TcpListener>,
}

#[rquickjs::methods]
impl TcpListenerWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new TcpListener()`
    // throw: instances only ever come from `TcpListener.listen`.
    #[qjs(constructor)]
    pub const fn new_js() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> { Ok(self.listener.local_addr()?.into()) }

    pub async fn accept(self) -> Result<List<(TcpStreamWrapper, SocketAddrWrapper)>> {
        let (stream, addr) = self.listener.accept().await?;
        let stream = Arc::new(RwLock::new(stream));
        Ok(List((stream.into(), addr.into())))
    }

    #[qjs(static)]
    pub async fn listen(addr: String) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Arc::new(listener).into())
    }
}
