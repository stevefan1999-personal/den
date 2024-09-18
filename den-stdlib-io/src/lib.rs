use std::sync::Arc;

use derive_more::{Deref, DerefMut, From, Into};
use either::Either;
use rquickjs::{Ctx, Result, TypedArray};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::RwLock,
};

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
        write.read(&mut buf).await?;
        TypedArray::new(ctx, buf)
    }
}

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncWriteWrapper(pub Arc<RwLock<dyn AsyncWrite + Unpin>>);

impl AsyncWriteWrapper {
    pub async fn write_all<'js>(
        self,
        buf: Either<String, Either<Vec<u8>, TypedArray<'js, u8>>>,
    ) -> Result<()> {
        let buf = match buf {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(Either::Left(ref x)) => x.as_slice(),
            Either::Right(Either::Right(ref x)) => x.as_bytes().unwrap(),
        };
        let mut write = self.write().await;
        write.write_all(buf).await?;
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
