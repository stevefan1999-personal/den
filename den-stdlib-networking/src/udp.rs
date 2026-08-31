use std::{net::SocketAddr, sync::Arc};

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{Ctx, JsLifetime, Result, TypedArray, class::Trace, convert::List};
use tokio::net::UdpSocket;

use crate::{
    io::{JsByteBuf, JsByteBufExt as _},
    socket_addr::SocketAddrWrapper,
};

#[derive(Trace, JsLifetime, Clone, Debug, From, Into, Deref, DerefMut)]
#[rquickjs::class(rename = "UdpSocket")]
pub struct UdpSocketWrapper {
    #[qjs(skip_trace)]
    socket: Arc<UdpSocket>,
}

#[rquickjs::methods]
impl UdpSocketWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new UdpSocket()`
    // throw: instances only ever come from `UdpSocket.bind`.
    #[qjs(constructor)]
    pub const fn new_js() {}

    #[qjs(get, enumerable, rename = "localAddr")]
    pub fn local_addr(&self) -> Result<SocketAddrWrapper> { Ok(self.socket.local_addr()?.into()) }

    #[qjs(static)]
    pub async fn bind(addr: String) -> Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Arc::new(socket).into())
    }

    pub async fn connect(self, addr: String) -> Result<()> {
        self.socket.connect(addr).await?;
        Ok(())
    }

    pub async fn send(self, buf: JsByteBuf<'_>) -> Result<usize> {
        Ok(self.socket.send(buf.as_bytes()?).await?)
    }

    pub async fn recv(self, max: usize, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        TypedArray::new_copy(ctx, self.recv_bytes(max).await?)
    }

    #[qjs(rename = "sendTo")]
    pub async fn send_to(self, buf: JsByteBuf<'_>, addr: String) -> Result<usize> {
        Ok(self.socket.send_to(buf.as_bytes()?, addr).await?)
    }

    #[qjs(rename = "recvFrom")]
    pub async fn recv_from(
        self, max: usize, ctx: Ctx<'_>,
    ) -> Result<List<(TypedArray<'_, u8>, SocketAddrWrapper)>> {
        let (payload, addr) = self.recv_from_bytes(max).await?;
        Ok(List((TypedArray::new_copy(ctx, payload)?, addr.into())))
    }
}

impl UdpSocketWrapper {
    async fn recv_bytes(&self, max: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0; max];
        // A short datagram is the normal case. Returning the full `max`-sized
        // buffer would hand the caller trailing zeroes that never came off the
        // wire and are indistinguishable from real payload.
        let received = self.socket.recv(&mut buf).await?;
        buf.truncate(received);
        Ok(buf)
    }

    async fn recv_from_bytes(&self, max: usize) -> Result<(Vec<u8>, SocketAddr)> {
        let mut buf = vec![0; max];
        let (received, addr) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(received);
        Ok((buf, addr))
    }
}

#[cfg(test)]
#[path = "../tests/unit/udp.rs"]
mod tests;
