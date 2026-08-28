use den_core::engine::Engine;
use either::Either;
use rquickjs::{CatchResultExt, convert::List, function::Opt};

use super::{TlsListenerWrapper, TlsStreamWrapper};

struct TestCert;

impl TestCert {
    fn localhost() -> (String, String) {
        let certified = rcgen::generate_simple_self_signed(["localhost".to_string()])
            .expect("self-signed cert");
        (certified.cert.pem(), certified.signing_key.serialize_pem())
    }
}

#[tokio::test]
async fn connect_to_a_local_acceptor_round_trips() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let (cert_pem, key_pem) = TestCert::localhost();
                let listener =
                    TlsListenerWrapper::listen("127.0.0.1:0".into(), cert_pem.clone(), key_pem)
                        .await?;
                let dest = listener.local_addr()?.to_string();
                let connecting =
                    TlsStreamWrapper::connect(dest, "localhost".into(), Opt(Some(cert_pem)));
                let accepting = listener.accept();
                let (client, accepted) = tokio::join!(connecting, accepting);
                let client = client?;
                let List((server, _)) = accepted?;
                client
                    .clone()
                    .write_all(Either::Right(Either::Left(b"hello TLS!".to_vec())))
                    .await?;
                let received = {
                    let chunk = server.read(11, ctx.clone()).await?;
                    chunk
                        .as_bytes()
                        .expect("the chunk is still attached")
                        .to_vec()
                };
                Ok::<_, rquickjs::Error>(format!("bytes:{}", received == b"hello TLS!"))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the tls stream round-trips");
    assert_eq!(outcome, "bytes:true");
}
