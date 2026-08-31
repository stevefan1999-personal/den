#[cfg(unix)] use std::sync::Arc;

use rquickjs::{Ctx, Error, JsLifetime, Result, TypedArray, class::Trace, convert::List};
#[cfg(unix)]
use tokio::{
    net::{UnixListener, UnixStream},
    sync::RwLock,
};

#[cfg(unix)]
use crate::io::{AsyncReadWrapper, AsyncWriteWrapper};
use crate::io::{JsByteBuf, impl_stream_wrapper};

#[cfg(unix)]
fn unix_pathname(addr: &tokio::net::unix::SocketAddr) -> String {
    addr.as_pathname()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(not(unix))]
fn unix_unsupported() -> Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix domain sockets are not supported on this platform",
    )
    .into()
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class(rename = "UnixStream")]
pub struct UnixStreamWrapper {
    #[cfg(unix)]
    #[qjs(skip_trace)]
    stream: Arc<RwLock<UnixStream>>,
}

impl_stream_wrapper! {
    UnixStreamWrapper,
    unsupported: unix_unsupported,

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<String> {
        #[cfg(unix)]
        {
            let this = self
                .stream
                .try_read()
                .map_err(|_error| Error::Unknown)?;
            Ok(unix_pathname(&this.local_addr()?))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(unix_unsupported())
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
            Err(unix_unsupported())
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
    #[expect(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub const fn new() {}

    #[qjs(get, enumerable)]
    pub fn local_addr(&self) -> Result<String> {
        #[cfg(unix)]
        {
            Ok(unix_pathname(&self.listener.local_addr()?))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(unix_unsupported())
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
                unix_pathname(&addr),
            )))
        }
        #[cfg(not(unix))]
        {
            let _ = self;
            Err(unix_unsupported())
        }
    }

    #[qjs(static)]
    #[expect(
        clippy::unused_async,
        reason = "keeps the listener factory consistently awaitable"
    )]
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
            Err(unix_unsupported())
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/unix.rs"]
mod tests;
