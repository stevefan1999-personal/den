use rquickjs::{Context, Ctx, Object, Runtime, Value};

use crate::{backend, error::WebAssemblyErrors, memory::MemoryBuffers, store::Store};

/// A fresh runtime whose context carries everything the wrappers expect:
/// the shared wasm store and the three WebAssembly error classes.
pub fn with_wasm_context<R, F: FnOnce(&Ctx<'_>) -> R>(f: F) -> R {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let engine = backend::new_engine().expect("engine");
        ctx.store_userdata(Store::new(&engine, &ctx))
            .expect("store userdata");
        ctx.store_userdata(MemoryBuffers::default())
            .expect("memory buffer registry userdata");
        let namespace = Object::new(ctx.clone()).expect("namespace");
        WebAssemblyErrors::install(&ctx, &namespace).expect("error classes");
        f(&ctx)
    })
}

/// The `name` of the pending JS error, e.g. `"TypeError"`, so that tests
/// pin the spec's error *class* rather than its wording.
pub fn pending_error_name(ctx: &Ctx<'_>) -> String {
    ctx.catch()
        .as_exception()
        .and_then(|exception| exception.get::<_, String>("name").ok())
        .unwrap_or_else(|| "not a JS error".to_owned())
}

/// Evaluate a snippet to a JS value, for feeding arguments to the wrappers.
pub fn js<'js>(ctx: &Ctx<'js>, source: &str) -> Value<'js> {
    ctx.eval(source).expect("test snippet evaluates")
}
