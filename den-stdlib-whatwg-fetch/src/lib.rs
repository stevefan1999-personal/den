use std::{cell::RefCell, rc::Rc};

use den_utils::serde_json::SerdeJsonValue;
use derive_more::derive::{From, Into};
use rquickjs::{ArrayBuffer, Ctx, Exception, IntoJs, JsLifetime, Result, TypedArray, class::Trace};

#[derive(Trace, JsLifetime, Clone, Debug, From, Into)]
#[rquickjs::class(rename = "Response")]
pub struct Response {
    #[qjs(skip_trace)]
    inner: Rc<RefCell<Option<reqwest::Response>>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Response {
    // `Response::constructor` is what gets bound as the `Response` global,
    // and it only exists when the class declares a constructor. Returning
    // `()` makes `new Response()` throw, as only `fetch` produces one.
    #[allow(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub fn new() {}

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        if let Some(inner) = self.inner.take() {
            let bytes = inner
                .bytes()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))?;

            // `new_copy`, never `new`: `new` lends QuickJS the Rust allocation
            // plus a free hook it runs twice on detach (quickjs.c:58037 and
            // :57935), and `transfer` reallocs a pointer its allocator never
            // produced, so `(await r.arrayBuffer()).transfer()` aborted the
            // process. The cost is one extra copy of the body — paid to make it
            // an ordinary JS buffer that can be detached and transferred.
            ArrayBuffer::new_copy(ctx, bytes)
        } else {
            Err(Exception::throw_type(&ctx, "Already distributed"))
        }
    }

    pub async fn blob<'js>(&self, ctx: Ctx<'js>) -> Result<()> {
        Err(ctx.throw("TODO".into_js(&ctx)?))
    }

    pub async fn bytes<'js>(&self, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
        if let Some(inner) = self.inner.take() {
            let bytes = inner
                .bytes()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))?;

            // Same as `array_buffer` above: an owned QuickJS allocation, so
            // detach and transfer are legitimate. One extra copy of the body.
            TypedArray::new_copy(ctx, bytes)
        } else {
            Err(Exception::throw_type(&ctx, "Already distributed"))
        }
    }

    pub async fn form_data<'js>(ctx: Ctx<'js>) -> Result<()> {
        Err(ctx.throw("TODO".into_js(&ctx)?))
    }

    pub async fn json<'js>(&self, ctx: Ctx<'js>) -> Result<SerdeJsonValue> {
        if let Some(inner) = self.inner.take() {
            Ok(inner
                .json::<serde_json::Value>()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))?
                .into())
        } else {
            Err(Exception::throw_type(&ctx, "Already distributed"))
        }
    }

    pub async fn text<'js>(&self, ctx: Ctx<'js>) -> Result<String> {
        if let Some(inner) = self.inner.take() {
            inner
                .text()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))
        } else {
            Err(Exception::throw_type(&ctx, "Already distributed"))
        }
    }

    #[qjs(enumerable, get)]
    pub fn body_used(&self) -> bool {
        self.inner.borrow().is_none()
    }

    #[qjs(enumerable, get)]
    pub fn ok(&self) -> bool {
        self.inner
            .borrow()
            .as_ref()
            .map(|inner| inner.status().is_success())
            .unwrap_or(false)
    }

    #[qjs(enumerable, get)]
    pub fn redirected(&self) -> bool {
        self.inner
            .borrow()
            .as_ref()
            .map(|inner| inner.status().is_redirection())
            .unwrap_or(false)
    }

    #[qjs(enumerable, get)]
    pub fn status<'js>(&self, ctx: Ctx<'js>) -> Result<u16> {
        match self
            .inner
            .borrow()
            .as_ref()
            .map(|inner| inner.status().into())
        {
            Some(x) => Ok(x),
            None => Err(Exception::throw_internal(&ctx, "Already consumed")),
        }
    }

    #[qjs(enumerable, get)]
    pub fn status_text<'js>(&self, ctx: Ctx<'js>) -> Result<&str> {
        match self
            .inner
            .borrow()
            .as_ref()
            .map(|inner| inner.status().canonical_reason())
        {
            Some(Some(x)) => Ok(x),
            Some(None) => Ok(""),
            None => Err(Exception::throw_internal(&ctx, "Already consumed")),
        }
    }

    #[qjs(enumerable, get)]
    pub fn url<'js>(&self, ctx: Ctx<'js>) -> Result<String> {
        match self
            .inner
            .borrow()
            .as_ref()
            .map(|inner| inner.url().to_string())
        {
            Some(x) => Ok(x),
            None => Err(Exception::throw_internal(&ctx, "Already consumed")),
        }
    }

    #[qjs(enumerable, get, rename = "type")]
    pub fn type_<'js>(&self, ctx: Ctx<'js>) -> Result<&str> {
        Err(ctx.throw("TODO".into_js(&ctx)?))
    }
}

#[rquickjs::function()]
pub async fn fetch<'js>(ctx: Ctx<'js>, url: String) -> Result<Response> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| Exception::throw_internal(&ctx, &format!("{e:?}")))?;

    Ok(Response {
        inner: Rc::new(RefCell::new(Some(response))),
    })
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod whatwg {
    use rquickjs::{
        Ctx, Result,
        class::JsClass,
        module::{Declarations, Exports},
    };

    pub use super::Response;

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("fetch")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
        e.export("fetch", super::js_fetch)?;
        ctx.globals().set("fetch", super::js_fetch)?;
        ctx.globals().set("Response", Response::constructor(ctx))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt};

    use super::Response;

    /// A body handed to script has to be a buffer QuickJS itself allocated.
    /// Lending it a Rust allocation registers a free hook that quickjs-ng runs
    /// twice on detach (quickjs.c:58037 and :57935), and `transfer` reallocs
    /// that foreign pointer, so `(await response.arrayBuffer()).transfer(2)`
    /// aborted the process — an abort that takes this test binary with it, so
    /// the snippet returning at all is the assertion.
    #[tokio::test]
    async fn a_response_body_survives_transfer_and_detach() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        let outcome: String = context
            .async_with(async |ctx| {
                // Built from an `http::Response`, so the body is real but no
                // socket is involved. `Response` holds an `Rc`, so it cannot be
                // captured by the `Send` closure and is made here.
                let respond = || {
                    Response {
                        inner: Rc::new(RefCell::new(Some(http::Response::new("body").into()))),
                    }
                };
                let run = async {
                    let buffer = respond().array_buffer(ctx.clone()).await?;
                    let view = respond().bytes(ctx.clone()).await?;
                    ctx.globals().set("body", buffer)?;
                    ctx.globals().set("view", view)?;
                    ctx.eval::<String, _>(
                        r#"
                          const moved = body.transfer(2);
                          const movedView = view.buffer.transfer();
                          [new Uint8Array(moved).join("-"),
                           String.fromCharCode(...new Uint8Array(movedView)),
                           body.detached, view.byteLength].join(",")
                        "#,
                    )
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the snippet evaluates");
        assert_eq!(outcome, "98-111,body,true,0");
    }
}
