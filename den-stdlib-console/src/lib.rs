#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod console {
    use colored::Colorize;
    use rquickjs::{module::Exports, Coerced, Ctx, Object, Result};

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
        ctx.globals().set("console", {
            let obj = Object::new(ctx.clone())?;
            obj.set("log", js_log)?;
            obj.set("info", js_info)?;
            obj.set("warn", js_warn)?;
            obj.set("error", js_error)?;
            obj
        })?;
        Ok(())
    }
}
