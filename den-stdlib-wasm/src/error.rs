//! `WebAssembly.CompileError` / `.LinkError` / `.RuntimeError`.
//!
//! The JS API (§5.10) requires these to implement the *NativeError Object
//! Structure*: callable with and without `new`, `length === 1`, `name === "…"`,
//! `C.__proto__ === Error`, `C.prototype.__proto__ === Error.prototype`,
//! instances that are `instanceof Error` and carry a `.stack`. rquickjs classes
//! cannot extend a JS builtin, and `class X extends Error` would break
//! the call-without-`new` requirement, so the constructors are built in JS-land
//! once at module evaluation and kept in the context userdata for Rust to throw
//! with.

use core::fmt::Display;

use rquickjs::{Constructor, Ctx, Exception, JsLifetime, Object, Result, Value};

/// Builds the three constructors on the namespace object and returns them.
///
/// Each is an ordinary function (not a class) so that
/// `WebAssembly.CompileError("boom")` works without `new`, and `arguments[1]`
/// is forwarded so the standard `(message, options)` signature is honoured
/// while `length` stays `1`.
const DEFINE_ERRORS: &str = r#"
(namespace) => ["CompileError", "LinkError", "RuntimeError"].map((name) => {
  const constructor = function (message) {
    const error = new Error(message, arguments[1]);
    Object.setPrototypeOf(error, new.target ? new.target.prototype : constructor.prototype);
    return error;
  };
  Object.setPrototypeOf(constructor, Error);
  constructor.prototype = Object.create(Error.prototype, {
    constructor: { value: constructor, writable: true, configurable: true },
    name: { value: name, writable: true, configurable: true },
    message: { value: "", writable: true, configurable: true },
  });
  Object.defineProperty(constructor, "name", { value: name, configurable: true });
  Object.defineProperty(namespace, name, {
    value: constructor,
    writable: true,
    configurable: true,
  });
  return constructor;
})
"#;

/// Which of the three error classes to raise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebAssemblyErrorKind {
    Compile,
    Link,
    Runtime,
}

impl WebAssemblyErrorKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Compile => "CompileError",
            Self::Link => "LinkError",
            Self::Runtime => "RuntimeError",
        }
    }

    /// Throw this error with `message` as its message, always returning the
    /// `Err` payload the caller should propagate.
    pub fn throw(self, ctx: &Ctx<'_>, message: impl Display) -> rquickjs::Error {
        let message = message.to_string();
        match WebAssemblyErrors::constructor(ctx, self)
            .and_then(|constructor| constructor.construct::<_, Value>((message.as_str(),)))
        {
            Ok(error) => ctx.throw(error),
            // The classes are installed by `js_wasm`'s evaluate hook, so this is only reachable if
            // JS deleted them again; a plain `Error` is a better outcome than a panic.
            Err(_) => Exception::throw_message(ctx, &format!("{}: {message}", self.name())),
        }
    }
}

/// The three constructors, kept in the context userdata so throwing does not
/// have to walk the globals (and still works if JS replaces
/// `globalThis.WebAssembly`).
#[derive(JsLifetime)]
pub struct WebAssemblyErrors<'js> {
    compile: Constructor<'js>,
    link:    Constructor<'js>,
    runtime: Constructor<'js>,
}

impl<'js> WebAssemblyErrors<'js> {
    /// Define `CompileError`, `LinkError` and `RuntimeError` on the
    /// `WebAssembly` namespace object and remember them. Call once, from
    /// the module's `evaluate` hook.
    pub fn install(ctx: &Ctx<'js>, namespace: &Object<'js>) -> Result<()> {
        let define: rquickjs::Function<'js> = ctx.eval(DEFINE_ERRORS)?;
        let constructors: Vec<Constructor<'js>> = define.call((namespace.clone(),))?;
        let [compile, link, runtime] = <[Constructor<'js>; 3]>::try_from(constructors)
            .map_err(|_| Exception::throw_internal(ctx, "WebAssembly error class setup failed"))?;

        ctx.store_userdata(Self {
            compile,
            link,
            runtime,
        })
        .map_err(|_| Exception::throw_internal(ctx, "WebAssembly error classes already in use"))?;
        Ok(())
    }

    fn constructor(ctx: &Ctx<'js>, kind: WebAssemblyErrorKind) -> Result<Constructor<'js>> {
        let errors = ctx
            .userdata::<Self>()
            .ok_or_else(|| Exception::throw_internal(ctx, "WebAssembly error classes missing"))?;
        Ok(match kind {
            WebAssemblyErrorKind::Compile => errors.compile.clone(),
            WebAssemblyErrorKind::Link => errors.link.clone(),
            WebAssemblyErrorKind::Runtime => errors.runtime.clone(),
        })
    }
}

/// Throw `WebAssembly.CompileError(message)`.
pub fn throw_compile_error(ctx: &Ctx<'_>, message: impl Display) -> rquickjs::Error {
    WebAssemblyErrorKind::Compile.throw(ctx, message)
}

/// Throw `WebAssembly.LinkError(message)`.
pub fn throw_link_error(ctx: &Ctx<'_>, message: impl Display) -> rquickjs::Error {
    WebAssemblyErrorKind::Link.throw(ctx, message)
}

/// Throw `WebAssembly.RuntimeError(message)`.
pub fn throw_runtime_error(ctx: &Ctx<'_>, message: impl Display) -> rquickjs::Error {
    WebAssemblyErrorKind::Runtime.throw(ctx, message)
}

#[cfg(test)]
mod tests {
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
                    .eval(format!(
                        r#"
                          (() => {{
                            const C = WebAssembly.{name};
                            const constructed = new C("boom");
                            const called = C("boom");
                            return C.length === 1
                              && C.name === "{name}"
                              && Object.getPrototypeOf(C) === Error
                              && Object.getPrototypeOf(C.prototype) === Error.prototype
                              && C.prototype.name === "{name}"
                              && C.prototype.message === ""
                              && C.prototype.constructor === C
                              && constructed instanceof C
                              && constructed instanceof Error
                              && constructed.name === "{name}"
                              && constructed.message === "boom"
                              && typeof constructed.stack === "string"
                              && called instanceof C
                              && new C().message === ""
                              && new C("x", {{ cause: 1 }}) instanceof C;
                          }})()
                        "#
                    ))
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
}
