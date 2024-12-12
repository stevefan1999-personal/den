use std::{cell::RefCell, sync::Arc};

use den_utils::serde_json::SerdeJsonValue;
use derivative::Derivative;
use derive_more::derive::{From, Into};
use rquickjs::{class::Trace, ArrayBuffer, Ctx, Exception, IntoJs, JsLifetime, Result, TypedArray};

#[derive(Trace, JsLifetime, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "Response")]
pub struct Response {
    #[qjs(skip_trace)]
    inner: Arc<RefCell<Option<reqwest::Response>>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Response {
    #[qjs(constructor)]
    pub fn new() {}

    pub async fn array_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        if let Some(inner) = self.inner.take() {
            let bytes = inner
                .bytes()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))?;

            ArrayBuffer::new(ctx, bytes)
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

            TypedArray::new(ctx, bytes)
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
            Ok(inner
                .text()
                .await
                .map_err(|e| Exception::throw_syntax(&ctx, &format!("{e:?}")))?
                .into())
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
        inner: Arc::new(RefCell::new(Some(response))),
    })
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod whatwg {
    use rquickjs::{
        class::JsClass,
        module::{Declarations, Exports},
        Ctx, Result,
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
