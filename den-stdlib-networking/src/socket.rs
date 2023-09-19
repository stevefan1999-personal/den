use std::{ops::Deref, sync::Arc};

use den_stdlib_core::WorldsEndExt;
use den_utils::FutureExt;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use either::Either;
use rquickjs::{class::Trace, convert::List, Ctx, Error, TypedArray};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
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

    pub async fn write_all<'js>(
        self,
        buf: Either<Vec<u8>, TypedArray<'js, u8>>,
        ctx: Ctx<'_>,
    ) -> rquickjs::Result<()> {
        let buf = match buf {
            Either::Left(ref x) => x,
            Either::Right(ref x) => x.as_bytes().unwrap(),
        };
        let mut write = self.stream.write().await;
        write
            .write_all(&buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(())
    }

    pub async fn read_to_end<'js>(self, ctx: Ctx<'js>) -> rquickjs::Result<TypedArray<'js, u8>> {
        let mut buf = vec![];
        let mut write = self.stream.write().await;
        write
            .read_to_end(&mut buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        TypedArray::new(ctx, buf)
    }

    pub async fn read_to_string(self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        let mut str = String::new();
        let mut write = self.stream.write().await;
        write
            .read_to_string(&mut str)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(str)
    }

    pub async fn read<'js>(
        self,
        bytes: usize,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<TypedArray<'js, u8>> {
        let mut buf = vec![0; bytes];
        let mut write = self.stream.write().await;
        write
            .read(&mut buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        TypedArray::new(ctx, buf)
    }

    pub async fn flush(self) -> rquickjs::Result<()> {
        let mut write = self.stream.write().await;
        write.flush().await?;
        Ok(())
    }

    pub async fn shutdown(self) -> rquickjs::Result<()> {
        let mut write = self.stream.write().await;
        write.shutdown().await?;
        Ok(())
    }

    #[qjs(static)]
    pub async fn connect(addr: String, ctx: Ctx<'_>) -> rquickjs::Result<Self> {
        let stream = TcpStream::connect(addr)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(Arc::new(RwLock::new(stream)).into())
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

    pub async fn accept(
        self,
        ctx: Ctx<'_>,
    ) -> rquickjs::Result<List<(TcpStreamWrapper, SocketAddrWrapper)>> {
        let (stream, addr) = self
            .deref()
            .accept()
            .with_cancellation(&ctx.worlds_end())
            .await??;
        let stream = Arc::new(RwLock::new(stream));
        Ok(List((stream.into(), addr.into())))
    }

    #[qjs(static)]
    pub async fn listen(addr: String, ctx: Ctx<'_>) -> rquickjs::Result<Self> {
        let listener = TcpListener::bind(addr)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(Arc::new(listener).into())
    }
}
