use rquickjs::bind;
pub use socket::*;

#[bind(object, public)]
#[quickjs(bare)]
mod socket {
    use std::{cell::Cell, ops::Deref, sync::Arc};

    use den_stdlib_core::WORLD_END;
    use den_utils::FutureExt;
    use derivative::Derivative;
    use derive_more::{Deref, DerefMut, From, Into};
    use rquickjs::Error;
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

    #[quickjs(cloneable, rename = "TcpStream")]
    #[derive(Derivative, From, Into, Deref, DerefMut)]
    #[derivative(Clone, Debug)]
    pub struct TcpStreamWrapper(Arc<RwLock<TcpStream>>);

    #[quickjs(rename = "TcpStream")]
    impl TcpStreamWrapper {
        #[quickjs(get, enumerable)]
        pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
            let this = self.0.try_read().map_err(|_| Error::Unknown)?;
            let addr = Cell::new(this.local_addr()?);
            Ok(addr.into())
        }

        pub async fn write_all(self, buf: Vec<u8>) -> rquickjs::Result<()> {
            let mut writer = self.deref().write().await;
            writer
                .write_all(&buf)
                .with_cancellation(&WORLD_END.child_token())
                .await??;
            Ok(())
        }
    }

    #[quickjs(cloneable, rename = "TcpListener")]
    #[derive(Derivative, From, Into, Deref, DerefMut)]
    #[derivative(Clone, Debug)]
    pub struct TcpListenerWrapper(Arc<TcpListener>);

    #[quickjs(rename = "TcpListener")]
    impl TcpListenerWrapper {
        // instance property getter
        #[quickjs(get, enumerable)]
        pub fn local_addr(&self) -> rquickjs::Result<SocketAddrWrapper> {
            let addr = self.deref().local_addr()?;
            Ok(Cell::new(addr).into())
        }

        pub async fn accept(self) -> rquickjs::Result<(TcpStreamWrapper, SocketAddrWrapper)> {
            let (stream, addr) = self
                .deref()
                .accept()
                .with_cancellation(&WORLD_END.child_token())
                .await??;
            let stream = Arc::new(RwLock::new(stream));
            let addr = Cell::new(addr);
            Ok((stream.into(), addr.into()))
        }
    }
}
