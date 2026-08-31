use den_core::engine::Engine;
use either::Either;
use rquickjs::{CatchResultExt as _, convert::List};

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
                let addr = socket.local_addr()?;
                let List((chunk, from)) = socket.recv_from(64, ctx.clone()).await?;
                let received = chunk
                    .as_bytes()
                    .expect("the chunk is still attached")
                    .to_vec();
                Ok::<_, rquickjs::Error>(format!(
                    "bytes:{} from:{} ipv4:{} loopback:{}",
                    received == payload,
                    from.to_string() == dest,
                    addr.is_ipv4() && addr.ip().is_ipv4(),
                    addr.ip().is_loopback()
                        && !addr.ip().is_unspecified()
                        && !addr.ip().is_multicast()
                ))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the datagram round-trips");
    assert_eq!(outcome, "bytes:true from:true ipv4:true loopback:true");
}

#[tokio::test]
async fn connected_send_recv_round_trips_on_loopback() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let server = UdpSocketWrapper::bind("127.0.0.1:0".into()).await?;
                let dest = server.local_addr()?.to_string();
                let client = UdpSocketWrapper::bind("127.0.0.1:0".into()).await?;
                client.clone().connect(dest).await?;
                client
                    .clone()
                    .send(Either::Right(Either::Left(b"pong".to_vec())))
                    .await?;
                let chunk = server.recv(64, ctx.clone()).await?;
                let received = chunk
                    .as_bytes()
                    .expect("the chunk is still attached")
                    .to_vec();
                Ok::<_, rquickjs::Error>(format!("bytes:{}", received == b"pong"))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the connected datagram round-trips");
    assert_eq!(outcome, "bytes:true");
}
