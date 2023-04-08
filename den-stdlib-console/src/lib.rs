use rquickjs::bind;

#[bind(object, public)]
pub mod console {
    use colored::Colorize;
    use rquickjs::Coerced;

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
