use den_core::engine::Engine;
use either::Either;
use rquickjs::{CatchResultExt as _, convert::List, function::Opt};

use super::{TlsListenerWrapper, TlsStreamWrapper};

struct TestCert;

impl TestCert {
    fn localhost() -> rquickjs::Result<(String, String)> {
        let certified = rcgen::generate_simple_self_signed(["localhost".to_string()])
            .map_err(|_error| rquickjs::Error::Unknown)?;
        Ok((certified.cert.pem(), certified.signing_key.serialize_pem()))
    }
}

#[tokio::test]
async fn connect_to_a_local_acceptor_round_trips() {
    let engine = Engine::new().await;
    let outcome = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let (cert_pem, key_pem) = TestCert::localhost()?;
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
                    let Some(bytes) = chunk.as_bytes() else {
                        return Err(rquickjs::Error::Unknown);
                    };
                    bytes.to_vec()
                };
                Ok::<_, rquickjs::Error>(format!("bytes:{}", received == b"hello TLS!"))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await;
    assert_eq!(outcome.as_deref(), Ok("bytes:true"));
}
