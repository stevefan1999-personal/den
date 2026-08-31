mod bridge;
mod conn;
mod dispatch;
mod error;
mod serve;

#[rquickjs::module(rename_vars = "camelCase")]
pub mod http {
    use rquickjs::{Ctx, Function, Object, Result, Value, module::Exports};

    pub use crate::{error::HttpError, serve::Server};

    #[rquickjs::function]
    pub fn serve<'js>(ctx: Ctx<'js>, options: crate::serve::ServeOptions<'js>) -> Result<Server> {
        options.serve(ctx)
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let constructor: Object = exports.module().get("HttpError")?;
        let prototype: Object = constructor.get("prototype")?;
        let error: Object = ctx.globals().get("Error")?;
        let error_prototype: Object = error.get("prototype")?;
        prototype.set_prototype(Some(&error_prototype))?;

        let server: Object = exports.module().get("Server")?;
        let server_prototype: Object = server.get("prototype")?;
        let symbols: Object = ctx.globals().get("Symbol")?;
        let async_dispose: Value = symbols.get("asyncDispose")?;
        if let Some(async_dispose) = async_dispose.into_symbol() {
            let close: Function = server_prototype.get("close")?;
            server_prototype.set(async_dispose, close)?;
        }
        Ok(())
    }
}
