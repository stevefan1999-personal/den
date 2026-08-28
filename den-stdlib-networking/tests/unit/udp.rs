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
