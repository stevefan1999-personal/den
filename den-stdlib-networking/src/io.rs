use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use either::Either;
use rquickjs::{Ctx, Error, Result, TypedArray};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::RwLock,
};

/// JS `write`/`send` payloads: UTF-8 text, a copied `Vec<u8>`, or a live
/// `Uint8Array`. Detached buffers are refused rather than treated as empty.
pub type JsByteBuf<'js> = Either<String, Either<Vec<u8>, TypedArray<'js, u8>>>;

pub struct JsBytes;

impl JsBytes {
    pub fn as_slice<'a, 'js>(buf: &'a JsByteBuf<'js>) -> Result<&'a [u8]> {
        match buf {
            Either::Left(text) => Ok(text.as_bytes()),
            Either::Right(Either::Left(bytes)) => Ok(bytes.as_slice()),
            Either::Right(Either::Right(array)) => array
                .as_bytes()
                .ok_or_else(|| Error::new_from_js_message("typed array", "bytes", "detached")),
        }
    }
}

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncReadWrapper(pub Arc<RwLock<dyn AsyncRead + Unpin>>);

impl AsyncReadWrapper {
    pub async fn read_to_end(self) -> Result<Vec<u8>> {
        let mut buf = vec![];
        let mut write = self.write().await;
        write.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    pub async fn read_to_string(self) -> Result<String> {
        let mut str = String::new();
        let mut write = self.write().await;
        write.read_to_string(&mut str).await?;
        Ok(str)
    }

    pub async fn read<'js>(self, bytes: usize, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        let mut buf = vec![0; bytes];
        let mut write = self.write().await;
        // A short read is the normal case on a socket. Returning the full
        // `bytes`-sized buffer would hand the caller trailing zeroes that never
        // came off the wire and are indistinguishable from real payload, so the
        // buffer is cut down to what actually arrived.
        let received = write.read(&mut buf).await?;
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
pub struct AsyncWriteWrapper(pub Arc<RwLock<dyn AsyncWrite + Unpin>>);

impl AsyncWriteWrapper {
    pub async fn write_all<'js>(self, buf: JsByteBuf<'js>) -> Result<()> {
        let bytes = JsBytes::as_slice(&buf)?;
        let mut write = self.write().await;
        write.write_all(bytes).await?;
        Ok(())
    }

    pub async fn flush(self) -> Result<()> {
        let mut write = self.write().await;
        write.flush().await?;
        Ok(())
    }

    pub async fn shutdown(self) -> Result<()> {
        let mut write = self.write().await;
        write.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::Arc};

    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt};
    use tokio::sync::RwLock;

    use super::AsyncReadWrapper;

    /// The chunk `read()` returns has to be a buffer QuickJS itself allocated.
    /// A lent-out Rust `Vec` carries a free hook that quickjs-ng runs twice on
    /// detach (quickjs.c:58037 and :57935) and `transfer` reallocs that foreign
    /// pointer, so `chunk.buffer.transfer(2)` aborted the process. The
    /// assertion is really "the snippet returned at all": an abort takes the
    /// test binary with it.
    #[tokio::test]
    async fn a_read_chunk_survives_transfer_and_detach() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let outcome: String = context
            .async_with(async |ctx| {
                // The reader is built inside: `dyn AsyncRead` is not `Send`, and
                // `async_with` wants a `Send` closure.
                let reader =
                    AsyncReadWrapper(Arc::new(RwLock::new(Cursor::new(b"chunk".to_vec()))));
                let run = async {
                    let chunk = reader.read(5, ctx.clone()).await?;
                    ctx.globals().set("chunk", chunk)?;
                    ctx.eval::<String, _>(
                        r#"
                          const before = new Uint8Array(chunk).join("-");
                          const moved = chunk.buffer.transfer(2);
                          [before, new Uint8Array(moved).join("-"),
                           chunk.buffer.detached, chunk.byteLength].join(",")
                        "#,
                    )
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the snippet evaluates");
        assert_eq!(outcome, "99-104-117-110-107,99-104,true,0");
    }
}
