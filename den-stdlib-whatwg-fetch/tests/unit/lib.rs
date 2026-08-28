use den_core::engine::Engine;
use rquickjs::{CatchResultExt, Class, Promise, Value, prelude::This};

use super::Response;

/// A body handed to script has to be a buffer QuickJS itself allocated.
/// Lending it a Rust allocation registers a free hook that quickjs-ng runs
/// twice on detach (quickjs.c:58037 and :57935), and `transfer` reallocs
/// that foreign pointer, so `(await response.arrayBuffer()).transfer(2)`
/// aborted the process — an abort that takes this test binary with it, so
/// the snippet returning at all is the assertion.
#[tokio::test]
async fn a_response_body_survives_transfer_and_detach() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            // Built from an `http::Response`, so the body is real but no
            // socket is involved. `Response` holds an `Rc`, so it cannot be
            // captured by the `Send` closure and is made here.
            let respond = || {
                Response::from_reqwest(&ctx, http::Response::new("body").into(), "basic")
                    .expect("response")
            };
            let run = async {
                let buffer = Response::array_buffer(
                    This(Class::instance(ctx.clone(), respond())?),
                    ctx.clone(),
                )?
                .into_future::<Value>()
                .await?;
                let view =
                    Response::bytes(This(Class::instance(ctx.clone(), respond())?), ctx.clone())?
                        .into_future::<Value>()
                        .await?;
                ctx.globals().set("body", buffer)?;
                ctx.globals().set("view", view)?;
                ctx.eval::<String, _>(include_str!(
                    "../fixtures/unit/lib/a_response_body_survives_transfer_and_detach.js"
                ))
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .expect("the snippet evaluates");
    assert_eq!(outcome, "98-111,body,true,0");
}

#[tokio::test]
async fn response_blob_wraps_the_body_when_blob_exists() {
    let engine = Engine::new().await;
    let outcome: String = engine
        .context
        .async_with(async |ctx| {
            let run = async {
                let response = Response::from_reqwest(
                    &ctx,
                    http::Response::builder()
                        .header("content-type", "text/plain")
                        .body("hello")
                        .expect("response")
                        .into(),
                    "basic",
                )
                .expect("from_reqwest");
                let blob =
                    Response::blob(This(Class::instance(ctx.clone(), response)?), ctx.clone())?
                        .into_future::<Value>()
                        .await?;
                ctx.globals().set("blob", blob)?;
                ctx.eval::<Promise, _>(include_str!(
                    "../fixtures/unit/lib/response_blob_wraps_the_body_when_blob_exists.js"
                ))?
                .into_future::<String>()
                .await
            };
            run.await.catch(&ctx).map_err(|err| err.to_string())
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(outcome, "true|text/plain|hello");
}
