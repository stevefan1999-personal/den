use std::{io::Cursor, sync::Arc};

use den_core::engine::Engine;
use either::Either;
use rquickjs::CatchResultExt as _;
use tokio::sync::RwLock;

use super::{AsyncReadWrapper, AsyncWriteWrapper, JsByteBuf, JsByteBufExt as _};

/// The chunk `read()` returns has to be a buffer QuickJS itself allocated.
/// A lent-out Rust `Vec` carries a free hook that quickjs-ng runs twice on
/// detach (quickjs.c:58037 and :57935) and `transfer` reallocs that foreign
/// pointer, so `chunk.buffer.transfer(2)` aborted the process. The
/// assertion is really "the snippet returned at all": an abort takes the
/// test binary with it.
#[tokio::test]
async fn a_read_chunk_survives_transfer_and_detach() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            // The reader is built inside: `dyn AsyncRead` is not `Send`, and
            // `async_with` wants a `Send` closure.
            let reader = AsyncReadWrapper(Arc::new(RwLock::new(Cursor::new(b"chunk".to_vec()))));
            let run = async {
                let chunk = reader.read(5, ctx.clone()).await?;
                ctx.globals().set("chunk", chunk)?;
                ctx.eval::<String, _>(include_str!(
                    "../fixtures/unit/io/a_read_chunk_survives_transfer_and_detach.js"
                ))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the snippet evaluates");
    assert_eq!(outcome, "99-104-117-110-107,99-104,true,0");
}

#[tokio::test]
async fn wrappers_round_trip_utf8_and_byte_payloads() {
    let reader = AsyncReadWrapper(Arc::new(RwLock::new(Cursor::new(b"hello".to_vec()))));
    assert_eq!(
        reader.clone().read_to_string().await.expect("utf8"),
        "hello"
    );

    let reader = AsyncReadWrapper(Arc::new(RwLock::new(Cursor::new(b"abc".to_vec()))));
    assert_eq!(reader.read_to_end().await.expect("bytes"), b"abc");

    let writer = AsyncWriteWrapper(Arc::new(RwLock::new(Vec::<u8>::new())));
    writer
        .clone()
        .write_all(Either::Left("xyz".into()))
        .await
        .expect("write");
    writer.clone().flush().await.expect("flush");
    writer.shutdown().await.expect("shutdown");
}

#[test]
fn byte_buffers_expose_text_and_owned_bytes() {
    let text: JsByteBuf<'_> = Either::Left("hi".into());
    assert_eq!(text.as_bytes().expect("text"), b"hi");
    let bytes: JsByteBuf<'_> = Either::Right(Either::Left(vec![1, 2]));
    assert_eq!(bytes.as_bytes().expect("bytes"), [1, 2]);
}
