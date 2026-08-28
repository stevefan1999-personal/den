use rquickjs::{Context, Runtime};

use super::*;

fn with_namespace<R>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| {
        let namespace = Object::new(ctx.clone()).expect("namespace");
        WebAssemblyErrors::install(&ctx, &namespace).expect("install");
        ctx.globals().set("WebAssembly", namespace).expect("global");
        f(&ctx)
    })
}

#[test]
fn the_three_classes_implement_the_native_error_object_structure() {
    with_namespace(|ctx| {
        for name in ["CompileError", "LinkError", "RuntimeError"] {
            let holds: bool = ctx
                .eval(include_str!("../fixtures/unit/error_constructor.js").replace("$NAME", name))
                .expect("assertion snippet evaluates");
            assert!(holds, "{name} does not match the NativeError structure");
        }
    })
}

#[test]
fn rust_side_throws_are_catchable_as_the_matching_class() {
    with_namespace(|ctx| {
        for (kind, name) in [
            (WebAssemblyErrorKind::Compile, "CompileError"),
            (WebAssemblyErrorKind::Link, "LinkError"),
            (WebAssemblyErrorKind::Runtime, "RuntimeError"),
        ] {
            let _ = kind.throw(ctx, format_args!("bad {name}"));
            let thrown = ctx.catch();
            let exception = thrown.as_exception().expect("a JS error was thrown");
            assert_eq!(exception.get::<_, String>("name").unwrap(), name);
            assert_eq!(
                exception.get::<_, String>("message").unwrap(),
                format!("bad {name}")
            );
            let is_instance: bool = ctx
                .eval(format!("(e) => e instanceof WebAssembly.{name}"))
                .and_then(|check: rquickjs::Function| check.call((thrown.clone(),)))
                .expect("instanceof check");
            assert!(is_instance, "thrown value is not a WebAssembly.{name}");
        }
    })
}
