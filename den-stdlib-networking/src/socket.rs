use std::{cell::Cell, ops::Deref, sync::Arc};

use den_stdlib_core::WORLD_END;
use den_utils::FutureExt;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, Error};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::RwLock,
};

use crate::socket_addr::SocketAddrWrapper;

pub async fn listen(addr: String) -> rquickjs::Result<TcpListenerWrapper> {
    let listener = TcpListener::bind(addr)
        .with_cancellation(&WORLD_END.child_token())
        .await??;
    Ok(Arc::new(listener).into())
}

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
        let this = self.try_read().map_err(|_| Error::Unknown)?;
        let addr = this.local_addr()?;
        Ok(addr.into())
    }

    pub async fn write_all(self, buf: Vec<u8>) -> rquickjs::Result<()> {
        let mut writer = self.write().await;
        writer
            .write_all(&buf)
            .with_cancellation(&WORLD_END.child_token())
            .await??;
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
    // instance property getter
    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
        let addr = self.deref().local_addr()?;
        Ok(addr.into())
    }

    pub async fn accept(self) -> rquickjs::Result<TcpStreamWrapper> {
        let (stream, addr) = self
            .deref()
            .accept()
            .with_cancellation(&WORLD_END.child_token())
            .await??;
        let stream = Arc::new(RwLock::new(stream));
        let addr = addr;
        Ok((stream.into()))
    }
}
