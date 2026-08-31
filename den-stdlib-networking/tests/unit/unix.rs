#[cfg(unix)] use super::UnixListenerWrapper;
use super::UnixStreamWrapper;

#[cfg(unix)]
#[tokio::test]
async fn listen_connect_write_read_round_trips() {
    use den_core::engine::Engine;
    use either::Either;
    use rquickjs::{CatchResultExt as _, convert::List};

    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let (path, _unlink) = sock_path();
                let listener = UnixListenerWrapper::listen(path.clone()).await?;
                let connecting = UnixStreamWrapper::connect(path);
                let accepting = listener.accept();
                let (client, accepted) = tokio::join!(connecting, accepting);
                let client = client?;
                let List((server, _)) = accepted?;
                client
                    .clone()
                    .write_all(Either::Right(Either::Left(b"ping".to_vec())))
                    .await?;
                let received = {
                    let chunk = server.read(4, ctx.clone()).await?;
                    chunk
                        .as_bytes()
                        .expect("the chunk is still attached")
                        .to_vec()
                };
                Ok::<_, rquickjs::Error>(format!("bytes:{}", received == b"ping"))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the unix stream round-trips");
    assert_eq!(outcome, "bytes:true");
}

#[cfg(not(unix))]
#[tokio::test]
async fn unix_stream_connect_is_unsupported() {
    let error = UnixStreamWrapper::connect("/tmp/den.sock".into())
        .await
        .expect_err("windows has no unix-domain sockets");
    let message = error.to_string();
    assert!(
        message.contains("not supported"),
        "unexpected error: {message}"
    );
}

#[cfg(unix)]
fn sock_path() -> (String, UnlinkOnDrop) {
    static UNIQUE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "den-unix-{}-{}.sock",
        std::process::id(),
        UNIQUE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    let displayed = path.to_string_lossy().into_owned();
    (displayed, UnlinkOnDrop(path))
}

#[cfg(unix)]
struct UnlinkOnDrop(std::path::PathBuf);

#[cfg(unix)]
impl Drop for UnlinkOnDrop {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}
