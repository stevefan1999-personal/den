use std::{net::SocketAddr, sync::Arc};

use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{Ctx, JsLifetime, Result, TypedArray, class::Trace, convert::List};
use tokio::net::UdpSocket;

use crate::{
    io::{JsByteBuf, JsBytes},
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
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

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

    pub async fn send<'js>(self, buf: JsByteBuf<'js>) -> Result<usize> {
        Ok(self.socket.send(JsBytes::as_slice(&buf)?).await?)
    }

    pub async fn recv<'js>(self, max: usize, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        TypedArray::new_copy(ctx, self.recv_bytes(max).await?)
    }

    #[qjs(rename = "sendTo")]
    pub async fn send_to<'js>(self, buf: JsByteBuf<'js>, addr: String) -> Result<usize> {
        Ok(self.socket.send_to(JsBytes::as_slice(&buf)?, addr).await?)
    }

    #[qjs(rename = "recvFrom")]
    pub async fn recv_from<'js>(
        self, max: usize, ctx: Ctx<'js>,
    ) -> Result<List<(TypedArray<'js, u8>, SocketAddrWrapper)>> {
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
mod tests {
    use den_core::engine::Engine;
    use either::Either;
    use rquickjs::{CatchResultExt, convert::List};

    use super::UdpSocketWrapper;

    #[tokio::test]
    async fn bind_send_to_self_recv_from_round_trips() {
        let engine = Engine::new().await;
        let outcome: String = engine
            .context
            .async_with(async |ctx| {
                let run = async {
                    let socket = UdpSocketWrapper::bind("127.0.0.1:0".into()).await?;
                    let dest = socket.local_addr()?.to_string();
                    let payload = b"ping".to_vec();
                    socket
                        .clone()
                        .send_to(Either::Right(Either::Left(payload.clone())), dest.clone())
                        .await?;
                    let List((chunk, from)) = socket.recv_from(64, ctx.clone()).await?;
                    let received = chunk
                        .as_bytes()
                        .expect("the chunk is still attached")
                        .to_vec();
                    Ok::<_, rquickjs::Error>(format!(
                        "bytes:{} from:{}",
                        received == payload,
                        from.to_string() == dest
                    ))
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the datagram round-trips");
        assert_eq!(outcome, "bytes:true from:true");
    }
}
