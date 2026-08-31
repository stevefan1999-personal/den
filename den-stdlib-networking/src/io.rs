use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use either::Either;
use rquickjs::{Ctx, Error, Result, TypedArray};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::RwLock,
};

/// JS `write`/`send` payloads: UTF-8 text, a copied `Vec<u8>`, or a live
/// `Uint8Array`. Detached buffers are refused rather than treated as empty.
pub type JsByteBuf<'js> = Either<String, Either<Vec<u8>, TypedArray<'js, u8>>>;

pub trait JsByteBufExt {
    fn as_bytes(&self) -> Result<&[u8]>;
}

impl JsByteBufExt for JsByteBuf<'_> {
    fn as_bytes(&self) -> Result<&[u8]> {
        match self {
            Either::Left(text) => Ok(text.as_bytes()),
            Either::Right(Either::Left(bytes)) => Ok(bytes.as_slice()),
            Either::Right(Either::Right(array)) => {
                array
                    .as_bytes()
                    .ok_or_else(|| Error::new_from_js_message("typed array", "bytes", "detached"))
            }
        }
    }
}

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncReadWrapper<T: ?Sized>(pub Arc<RwLock<T>>);

impl<T: AsyncRead + Unpin + ?Sized> AsyncReadWrapper<T> {
    pub async fn read_to_end(self) -> Result<Vec<u8>> {
        let mut buf = vec![];
        let mut write = self.write().await;
        write.read_to_end(&mut buf).await?;
        drop(write);
        Ok(buf)
    }

    pub async fn read_to_string(self) -> Result<String> {
        let mut str = String::new();
        let mut write = self.write().await;
        write.read_to_string(&mut str).await?;
        drop(write);
        Ok(str)
    }

    pub async fn read(self, bytes: usize, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        let mut buf = vec![0; bytes];
        let mut write = self.write().await;
        // A short read is the normal case on a socket. Returning the full
        // `bytes`-sized buffer would hand the caller trailing zeroes that never
        // came off the wire and are indistinguishable from real payload, so the
        // buffer is cut down to what actually arrived.
        let received = write.read(&mut buf).await?;
        drop(write);
        buf.truncate(received);
        // `new_copy`, never `new`: `new` gives QuickJS the `Vec`'s store plus a
        // free hook it runs twice on detach (quickjs.c:58037 and :57935), and
        // `transfer` reallocs a pointer its allocator never made — plain script
        // could abort the process with `chunk.buffer.transfer()`. The price is
        // one memcpy of a single read's worth of bytes; correctness first.
        TypedArray::new_copy(ctx, buf)
    }
}

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncWriteWrapper<T: ?Sized>(pub Arc<RwLock<T>>);

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWriteWrapper<T> {
    pub async fn write_all(self, buf: JsByteBuf<'_>) -> Result<()> {
        let bytes = buf.as_bytes()?;
        let mut write = self.write().await;
        write.write_all(bytes).await?;
        drop(write);
        Ok(())
    }

    pub async fn flush(self) -> Result<()> {
        let mut write = self.write().await;
        write.flush().await?;
        drop(write);
        Ok(())
    }

    pub async fn shutdown(self) -> Result<()> {
        let mut write = self.write().await;
        write.shutdown().await?;
        drop(write);
        Ok(())
    }
}

/// The six byte-stream methods every socket class in this crate exposes to JS
/// (`read_to_string`, `read_to_end`, `read`, `write_all`, `flush`,
/// `shutdown`), delegating over `self.stream` to [`AsyncReadWrapper`] and
/// [`AsyncWriteWrapper`]. TCP, TLS and Unix sockets all mean the same thing
/// by them, so they are written once here.
///
/// `#[rquickjs::methods]` rebuilds the impl it is given from bare `fn` items
/// only — a macro invocation inside it would be dropped — so the macro
/// generates the whole impl, taking the class's other methods in as
/// `$extra`. The delegation bodies name `AsyncReadWrapper`,
/// `AsyncWriteWrapper`, `JsByteBuf`, `Ctx`, `Result` and `TypedArray` bare,
/// so every call site must have those in scope.
macro_rules! impl_stream_wrapper {
    // Unix domain sockets: the very same signatures, but on platforms
    // without them every call fails instead of delegating.
    ($wrapper:ident, unsupported: $unsupported:path, $($extra:item)*) => {
        #[rquickjs::methods]
        impl $wrapper {
            // rquickjs only attaches `#[qjs(static)]` members to a class
            // that declares a constructor, and a `()` return makes `new`
            // throw: instances only ever come from the class's own
            // constructors.
            #[qjs(constructor)]
            pub const fn new_js() {}

            $($extra)*

            pub async fn read_to_string(self) -> Result<String> {
                #[cfg(unix)]
                {
                    AsyncReadWrapper(self.stream).read_to_string().await
                }
                #[cfg(not(unix))]
                {
                    let _ = self;
                    Err($unsupported())
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
                    Err($unsupported())
                }
            }

            pub async fn read(
                self, bytes: usize, ctx: Ctx<'_>,
            ) -> Result<TypedArray<'_, u8>> {
                #[cfg(unix)]
                {
                    AsyncReadWrapper(self.stream).read(bytes, ctx).await
                }
                #[cfg(not(unix))]
                {
                    let _ = (self, bytes, ctx);
                    Err($unsupported())
                }
            }

            pub async fn write_all(self, buf: JsByteBuf<'_>) -> Result<()> {
                #[cfg(unix)]
                {
                    AsyncWriteWrapper(self.stream).write_all(buf).await
                }
                #[cfg(not(unix))]
                {
                    let _ = (self, buf);
                    Err($unsupported())
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
                    Err($unsupported())
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
                    Err($unsupported())
                }
            }
        }
    };

    // Direct delegation: TCP and TLS.
    ($wrapper:ident, $($extra:item)*) => {
        #[rquickjs::methods]
        impl $wrapper {
            // rquickjs only attaches `#[qjs(static)]` members to a class
            // that declares a constructor, and a `()` return makes `new`
            // throw: instances only ever come from the class's own
            // constructors.
            #[qjs(constructor)]
            pub const fn new_js() {}

            $($extra)*

            pub async fn read_to_string(self) -> Result<String> {
                AsyncReadWrapper(self.stream).read_to_string().await
            }

            pub async fn read_to_end(self) -> Result<Vec<u8>> {
                AsyncReadWrapper(self.stream).read_to_end().await
            }

            pub async fn read(
                self, bytes: usize, ctx: Ctx<'_>,
            ) -> Result<TypedArray<'_, u8>> {
                AsyncReadWrapper(self.stream).read(bytes, ctx).await
            }

            pub async fn write_all(self, buf: JsByteBuf<'_>) -> Result<()> {
                AsyncWriteWrapper(self.stream).write_all(buf).await
            }

            pub async fn flush(self) -> Result<()> {
                AsyncWriteWrapper(self.stream).flush().await
            }

            pub async fn shutdown(self) -> Result<()> {
                AsyncWriteWrapper(self.stream).shutdown().await
            }
        }
    };
}
pub(crate) use impl_stream_wrapper;

#[cfg(test)]
#[path = "../tests/unit/io.rs"]
mod tests;
