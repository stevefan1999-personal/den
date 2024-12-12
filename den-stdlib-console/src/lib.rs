#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod console {
    use colored::Colorize;
    use indexmap::indexmap;
    use rquickjs::{module::Exports, Coerced, Ctx, IntoJs, Result};

    #[rquickjs::function]
    pub fn log(msg: Coerced<String>) {
        println!("{}", msg.0);
    }

    #[rquickjs::function]
    pub fn info(msg: Coerced<String>) {
        println!("{}", msg.0.bright_cyan());
    }

    #[rquickjs::function]
    pub fn warn(msg: Coerced<String>) {
        println!("{}", msg.0.bright_yellow());
    }

    #[rquickjs::function]
    pub fn error(msg: Coerced<String>) {
        println!("{}", msg.0.bright_red());
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        ctx.globals().set(
            "console",
            indexmap! {
                "log" => js_log.into_js(ctx)?,
                "info" => js_info.into_js(ctx)?,
                "warn" => js_warn.into_js(ctx)?,
                "error" => js_error.into_js(ctx)?,
            },
        )?;
        Ok(())
    }
}
