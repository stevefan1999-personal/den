use std::sync::Arc;

use den_stdlib_core::WorldsEndExt;
use den_utils::FutureExt;
use derive_more::{Deref, DerefMut, From, Into};
use either::Either;
use rquickjs::{Ctx, TypedArray};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::RwLock,
};

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncReadWrapper(pub Arc<RwLock<dyn AsyncRead + Unpin>>);

impl AsyncReadWrapper {
    pub async fn read_to_end(self, ctx: Ctx<'_>) -> rquickjs::Result<Vec<u8>> {
        let mut buf = vec![];
        let mut write = self.write().await;
        write
            .read_to_end(&mut buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(buf)
    }

    pub async fn read_to_string(self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        let mut str = String::new();
        let mut write = self.write().await;
        write
            .read_to_string(&mut str)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(str)
    }

    pub async fn read<'js>(
        self,
        bytes: usize,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<TypedArray<'js, u8>> {
        let mut buf = vec![0; bytes];
        let mut write = self.write().await;
        write
            .read(&mut buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        TypedArray::new(ctx, buf)
    }
}

#[derive(Clone, From, Into, Deref, DerefMut)]
pub struct AsyncWriteWrapper(pub Arc<RwLock<dyn AsyncWrite + Unpin>>);

impl AsyncWriteWrapper {
    pub async fn write_all<'js>(
        self,
        buf: Either<Vec<u8>, TypedArray<'js, u8>>,
        ctx: Ctx<'_>,
    ) -> rquickjs::Result<()> {
        let buf = match buf {
            Either::Left(ref x) => x,
            Either::Right(ref x) => x.as_bytes().unwrap(),
        };
        let mut write = self.write().await;
        write
            .write_all(&buf)
            .with_cancellation(&ctx.worlds_end())
            .await??;
        Ok(())
    }

    pub async fn flush(self) -> rquickjs::Result<()> {
        let mut write = self.write().await;
        write.flush().await?;
        Ok(())
    }

    pub async fn shutdown(self) -> rquickjs::Result<()> {
        let mut write = self.write().await;
        write.shutdown().await?;
        Ok(())
    }
}
