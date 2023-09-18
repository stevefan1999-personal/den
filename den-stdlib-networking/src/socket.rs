use std::{ops::Deref, sync::Arc};

use den_stdlib_core::WORLD_END;
use den_utils::FutureExt;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, convert::List, Error};
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
    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
        let this = self.stream.try_read().map_err(|_| Error::Unknown)?;
        let addr = this.local_addr()?;
        Ok(addr.into())
    }

    pub async fn write_all(self, buf: Vec<u8>) -> rquickjs::Result<()> {
        let mut write = self.stream.write().await;
        write
            .write_all(&buf)
            .with_cancellation(&WORLD_END.child_token())
            .await??;
        Ok(())
    }

    pub async fn read_to_end(self) -> rquickjs::Result<Vec<u8>> {
        let mut buf = vec![];
        let mut write = self.stream.write().await;
        write
            .read_to_end(&mut buf)
            .with_cancellation(&WORLD_END.child_token())
            .await??;
        Ok(buf)
    }

    pub async fn shutdown(self) -> rquickjs::Result<()> {
        let mut write = self.stream.write().await;
        write.shutdown().await?;
        Ok(())
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
    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
        Ok(self.deref().local_addr()?.into())
    }

    pub async fn accept(self) -> rquickjs::Result<List<(TcpStreamWrapper, SocketAddrWrapper)>> {
        let (stream, addr) = self
            .deref()
            .accept()
            .with_cancellation(&WORLD_END.child_token())
            .await??;
        let stream = Arc::new(RwLock::new(stream));
        Ok(List((stream.into(), addr.into())))
    }
}
