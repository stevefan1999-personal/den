use std::sync::Arc;

use den_stdlib_io::{AsyncReadWrapper, AsyncWriteWrapper};
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, convert::List, Ctx, Error, TypedArray};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::RwLock,
};

use crate::socket_addr::SocketAddrWrapper;

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "TcpStream")]
pub struct TcpStreamWrapper {
    #[qjs(skip_trace)]
    stream: Arc<RwLock<TcpStream>>,
}

#[rquickjs::methods]
impl TcpStreamWrapper {
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
        let this = self.stream.try_read().map_err(|_| Error::Unknown)?;
        let addr = this.local_addr()?;
        Ok(addr.into())
    }

    #[qjs(static)]
    pub async fn connect(addr: String) -> rquickjs::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Arc::new(RwLock::new(stream)).into())
    }

    pub async fn read_to_string(self) -> rquickjs::Result<String> {
        AsyncReadWrapper(self.stream).read_to_string().await
    }

    pub async fn read_to_end(self) -> rquickjs::Result<Vec<u8>> {
        AsyncReadWrapper(self.stream).read_to_end().await
    }

    pub async fn read<'js>(
        self,
        bytes: usize,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<TypedArray<'js, u8>> {
        AsyncReadWrapper(self.stream).read(bytes, ctx).await
    }

    pub async fn write_all<'js>(
        self,
        buf: either::Either<Vec<u8>, TypedArray<'js, u8>>,
    ) -> rquickjs::Result<()> {
        AsyncWriteWrapper(self.stream).write_all(buf).await
    }

    pub async fn flush(self) -> rquickjs::Result<()> {
        AsyncWriteWrapper(self.stream).flush().await
    }

    pub async fn shutdown(self) -> rquickjs::Result<()> {
        AsyncWriteWrapper(self.stream).shutdown().await
    }
}

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "TcpListener")]
pub struct TcpListenerWrapper {
    #[qjs(skip_trace)]
    listener: Arc<TcpListener>,
}

#[rquickjs::methods]
impl TcpListenerWrapper {
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
        Ok(self.deref().local_addr()?.into())
    }

    pub async fn accept(self) -> rquickjs::Result<List<(TcpStreamWrapper, SocketAddrWrapper)>> {
        let (stream, addr) = self.deref().accept().await?;
        let stream = Arc::new(RwLock::new(stream));
        Ok(List((stream.into(), addr.into())))
    }

    #[qjs(static)]
    pub async fn listen(addr: String) -> rquickjs::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Arc::new(listener).into())
    }
}
