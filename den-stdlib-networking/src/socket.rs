use std::{ops::Deref, sync::Arc};

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
        let this = self.stream.try_read().map_err(|_| Error::Unknown)?;
        let addr = this.local_addr()?;
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
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> { Ok(self.deref().local_addr()?.into()) }

    pub async fn accept(self) -> Result<List<(TcpStreamWrapper, SocketAddrWrapper)>> {
        let (stream, addr) = self.deref().accept().await?;
        let stream = Arc::new(RwLock::new(stream));
        Ok(List((stream.into(), addr.into())))
    }

    #[qjs(static)]
    pub async fn listen(addr: String) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Arc::new(listener).into())
    }
}
