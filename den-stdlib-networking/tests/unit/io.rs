use std::{io::Cursor, sync::Arc};

use den_core::engine::Engine;
use rquickjs::CatchResultExt as _;
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
