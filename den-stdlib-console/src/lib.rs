use colored::Colorize;
use rquickjs::convert::Coerced;

#[derive(rquickjs::class::Trace)]
#[rquickjs::class]
pub struct Console {}

#[rquickjs::methods]
impl Console {
    #[qjs(constructor)]
    pub fn new() {}

    pub fn log(msg: Coerced<String>) {
        println!("{}", msg.0);
    }

    pub fn info(msg: Coerced<String>) {
        println!("{}", msg.0.bright_cyan());
    }

    pub fn warn(msg: Coerced<String>) {
        println!("{}", msg.0.bright_yellow());
    }

    pub fn error(msg: Coerced<String>) {
        println!("{}", msg.0.bright_red());
    }
}
