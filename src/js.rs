use crate::loader::http::HttpLoader;
use crate::loader::mmap_script::MmapScriptLoader;
use crate::resolver::http::HttpResolver;
use rquickjs::{bind, BuiltinLoader, BuiltinResolver, Context, FileResolver, Runtime, Tokio};
use tokio::sync::broadcast::Receiver;

#[bind(object)]
#[quickjs(bare)]
pub mod timer {
    use rquickjs::Function;

    #[quickjs(rename = "setInterval")]
    pub fn set_interval(_func: Function, _delay: Option<usize>) -> usize {
        todo!()
    }

    #[quickjs(rename = "clearInterval")]
    pub fn clear_interval(_handle: usize) {
        todo!()
    }

    #[quickjs(rename = "setTimeout")]
    pub fn set_timeout(_func: Function, _delay: Option<usize>) -> usize {
        todo!()
    }

    #[quickjs(rename = "clearTimeout")]
    pub fn clear_timeout(_handle: usize) {
        todo!()
    }
}

#[bind(object)]
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

pub fn create_qjs_context(mut interrupt: Receiver<()>) -> (Runtime, Context) {
    let resolver = (
        BuiltinResolver::default(),
        HttpResolver::default(),
        FileResolver::default().with_path("./"),
    );
    let loader = (
        BuiltinLoader::default(),
        HttpLoader::default(),
        MmapScriptLoader::default(),
    );
    let rt = Runtime::new().unwrap();
    rt.set_loader(resolver, loader);
    rt.set_interrupt_handler(Some(Box::new(move || interrupt.try_recv().is_ok())));

    let _ = rt.spawn_executor(Tokio);
    let ctx = Context::full(&rt).unwrap();
    ctx.enable_big_num_ext(true);
    ctx.with(|ctx| {
        let global = ctx.globals();
        global.init_def::<Console>().unwrap();
        global.init_def::<Timer>().unwrap();
    });
    (rt, ctx)
}
