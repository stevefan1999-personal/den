//! `WebAssembly.CompileError` / `.LinkError` / `.RuntimeError`.
//!
//! The JS API (§5.10) requires these to implement the *NativeError Object
//! Structure*: callable with and without `new`, `length === 1`, `name === "…"`,
//! `C.__proto__ === Error`, `C.prototype.__proto__ === Error.prototype`,
//! instances that are `instanceof Error` and carry a `.stack`. rquickjs classes
//! cannot extend a JS builtin, and `class X extends Error` would break
//! the call-without-`new` requirement, so the constructors are ordinary
//! Functions built once at module evaluation and kept in the context userdata
//! for Rust to throw with.

use core::fmt::Display;

use rquickjs::{
    Constructor, Ctx, Exception, Function, JsLifetime, Object, Result, Value, atom::PredefinedAtom,
    function::Opt, object::Property,
};

/// NativeError `(message, options)` with `length` 1: `options` is
/// `arguments[1]`.
fn construct_error<'js>(
    ctx: Ctx<'js>, message: Opt<Value<'js>>, options: Opt<Value<'js>>,
) -> Result<Object<'js>> {
    let message = message
        .0
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let options = options
        .0
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    den_util::construct(&ctx, "Error", (message, options))
}

/// One NativeError constructor on `namespace`, callable with and without `new`.
fn native_error<'js>(
    ctx: &Ctx<'js>, namespace: &Object<'js>, name: &'static str,
) -> Result<Constructor<'js>> {
    let error: Function<'js> = ctx.globals().get("Error")?;
    let error_proto: Object<'js> = error.get(PredefinedAtom::Prototype)?;
    let proto = Object::new_proto(ctx.clone(), Some(&error_proto))?;
    let constructor = Constructor::new_prototype(ctx, proto.clone(), construct_error)?;
    constructor.set_name(name)?;
    constructor.set_length(1)?;
    constructor.set_prototype(Some(&*error))?;
    proto.prop(
        PredefinedAtom::Constructor,
        Property::from(constructor.clone())
            .writable()
            .configurable(),
    )?;
    proto.prop("name", Property::from(name).writable().configurable())?;
    proto.prop("message", Property::from("").writable().configurable())?;
    namespace.prop(
        name,
        Property::from(constructor.clone())
            .writable()
            .configurable(),
    )?;
    Ok(constructor)
}

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
    pub fn throw<M: Display>(self, ctx: &Ctx<'_>, message: M) -> rquickjs::Error {
        let message = message.to_string();
        WebAssemblyErrors::constructor(ctx, self)
            .and_then(|constructor| constructor.construct::<_, Value>((message.as_str(),)))
            .map_or_else(
                |_error| {
                    // The classes are installed by `js_wasm`'s evaluate hook, so this is only
                    // reachable if JS deleted them again; a plain `Error` beats a panic.
                    den_util::stack::throw_error(ctx, &format!("{}: {message}", self.name()))
                },
                |error| ctx.throw(error),
            )
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
        let compile = native_error(ctx, namespace, "CompileError")?;
        let link = native_error(ctx, namespace, "LinkError")?;
        let runtime = native_error(ctx, namespace, "RuntimeError")?;

        ctx.store_userdata(Self {
            compile,
            link,
            runtime,
        })
        .map_err(|_error| {
            Exception::throw_internal(ctx, "WebAssembly error classes already in use")
        })?;
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
pub fn throw_compile<M: Display>(ctx: &Ctx<'_>, message: M) -> rquickjs::Error {
    WebAssemblyErrorKind::Compile.throw(ctx, message)
}

/// Throw `WebAssembly.LinkError(message)`.
pub fn throw_link<M: Display>(ctx: &Ctx<'_>, message: M) -> rquickjs::Error {
    WebAssemblyErrorKind::Link.throw(ctx, message)
}

/// Throw `WebAssembly.RuntimeError(message)`.
pub fn throw_runtime<M: Display>(ctx: &Ctx<'_>, message: M) -> rquickjs::Error {
    WebAssemblyErrorKind::Runtime.throw(ctx, message)
}

#[cfg(test)]
#[path = "../tests/unit/error.rs"]
mod tests;
