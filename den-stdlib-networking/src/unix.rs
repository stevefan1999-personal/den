#[cfg(unix)] use std::sync::Arc;

use rquickjs::{Ctx, Error, JsLifetime, Result, TypedArray, class::Trace, convert::List};
#[cfg(unix)]
use tokio::{
    net::{UnixListener, UnixStream},
    sync::RwLock,
};

use crate::io::JsByteBuf;
#[cfg(unix)]
use crate::io::{AsyncReadWrapper, AsyncWriteWrapper};

struct Unix;

impl Unix {
    #[cfg(unix)]
    fn pathname(addr: &tokio::net::unix::SocketAddr) -> String {
        addr.as_pathname()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    #[cfg(not(unix))]
    fn unsupported() -> Error {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unix domain sockets are not supported on this platform",
        )
        .into()
    }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class(rename = "UnixStream")]
pub struct UnixStreamWrapper {
    #[cfg(unix)]
    #[qjs(skip_trace)]
    stream: Arc<RwLock<UnixStream>>,
}

#[rquickjs::methods]
impl UnixStreamWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new UnixStream()`
    // throw: instances only ever come from `UnixStream.connect` or
    // `UnixListener.accept`.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<String> {
        #[cfg(unix)]
        {
            let this = self.stream.try_read().map_err(|_| Error::Unknown)?;
            Ok(Unix::pathname(&this.local_addr()?))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    #[qjs(static)]
    pub async fn connect(path: String) -> Result<Self> {
        #[cfg(unix)]
        {
            let stream = UnixStream::connect(path).await?;
            Ok(Self {
                stream: Arc::new(RwLock::new(stream)),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(Unix::unsupported())
        }
    }

    pub async fn read_to_string(self) -> Result<String> {
        #[cfg(unix)]
        {
            AsyncReadWrapper(self.stream).read_to_string().await
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    pub async fn read_to_end(self) -> Result<Vec<u8>> {
        #[cfg(unix)]
        {
            AsyncReadWrapper(self.stream).read_to_end().await
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    pub async fn read<'js>(self, bytes: usize, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        #[cfg(unix)]
        {
            AsyncReadWrapper(self.stream).read(bytes, ctx).await
        }
        #[cfg(not(unix))]
        {
            let _ = (self, bytes, ctx);
            Err(Unix::unsupported())
        }
    }

    pub async fn write_all<'js>(self, buf: JsByteBuf<'js>) -> Result<()> {
        #[cfg(unix)]
        {
            AsyncWriteWrapper(self.stream).write_all(buf).await
        }
        #[cfg(not(unix))]
        {
            let _ = (self, buf);
            Err(Unix::unsupported())
        }
    }

    pub async fn flush(self) -> Result<()> {
        #[cfg(unix)]
        {
            AsyncWriteWrapper(self.stream).flush().await
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    pub async fn shutdown(self) -> Result<()> {
        #[cfg(unix)]
        {
            AsyncWriteWrapper(self.stream).shutdown().await
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class(rename = "UnixListener")]
pub struct UnixListenerWrapper {
    #[cfg(unix)]
    #[qjs(skip_trace)]
    listener: Arc<UnixListener>,
}

#[rquickjs::methods]
impl UnixListenerWrapper {
    // rquickjs only attaches `#[qjs(static)]` members to a class that
    // declares a constructor, and a `()` return makes `new UnixListener()`
    // throw: instances only ever come from `UnixListener.listen`.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<String> {
        #[cfg(unix)]
        {
            Ok(Unix::pathname(&self.listener.local_addr()?))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    pub async fn accept(self) -> Result<List<(UnixStreamWrapper, String)>> {
        #[cfg(unix)]
        {
            let (stream, addr) = self.listener.accept().await?;
            Ok(List((
                UnixStreamWrapper {
                    stream: Arc::new(RwLock::new(stream)),
                },
                Unix::pathname(&addr),
            )))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(Unix::unsupported())
        }
    }

    #[qjs(static)]
    pub async fn listen(path: String) -> Result<Self> {
        #[cfg(unix)]
        {
            let listener = UnixListener::bind(path)?;
            Ok(Self {
                listener: Arc::new(listener),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(Unix::unsupported())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UnixListenerWrapper, UnixStreamWrapper};

    #[cfg(unix)]
    #[tokio::test]
    async fn listen_connect_write_read_round_trips() {
        use either::Either;
        use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, convert::List};

        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let outcome: String = context
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
}
