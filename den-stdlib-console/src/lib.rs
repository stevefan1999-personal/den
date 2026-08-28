use std::fmt::Write;

use rquickjs::{Error, JsLifetime, Result, Type, Value, class::Trace, function::Rest};

// Notice: this code is directly copied from here
// https://github.com/rquickjs/rquickjs-extra/blob/main/modules/console/src/lib.rs
// Except with some key changes, especially from log to tracing
// TODO: JS native stack tracing support for trace spans

/// A formatter for the [`Console`] object
///
/// This formatter is used to format values to be printed by the console object.
///
/// [`Console`]: crate::Console
#[derive(Clone, Debug, Trace, JsLifetime)]
pub struct Formatter {
    max_depth: usize,
}

impl Default for Formatter {
    fn default() -> Self { Self::new(10) }
}

impl Formatter {
    pub const fn new(max_depth: usize) -> Self { Self { max_depth } }

    pub fn format(&self, out: &mut impl Write, value: Value<'_>) -> Result<()> {
        self._format(out, value, false, 0)
    }

    /// A poor attempt at mimicking the node format
    /// See https://github.com/nodejs/node/blob/363eca1033458b8c2808207e2e5fc88e0f4df655/lib/internal/util/inspect.js#L842
    fn _format(
        &self, out: &mut impl Write, value: Value<'_>, key: bool, depth: usize,
    ) -> Result<()> {
        match value.type_of() {
            Type::String => {
                write!(
                    out,
                    "{}",
                    value
                        .into_string()
                        .ok_or(Error::new_from_js("value", "string"))?
                        .to_string()?
                )
                .map_err(|_| Error::Unknown)?;
            }
            Type::Int => {
                write!(
                    out,
                    "{}",
                    value.as_int().ok_or(Error::new_from_js("value", "int"))?
                )
                .map_err(|_| Error::Unknown)?;
            }
            Type::Bool => {
                write!(
                    out,
                    "{}",
                    value.as_bool().ok_or(Error::new_from_js("value", "bool"))?
                )
                .map_err(|_| Error::Unknown)?;
            }
            Type::Float => {
                write!(
                    out,
                    "{}",
                    value
                        .as_float()
                        .ok_or(Error::new_from_js("value", "float"))?
                )
                .map_err(|_| Error::Unknown)?;
            }
            Type::BigInt => {
                write!(
                    out,
                    "{}n",
                    value
                        .into_big_int()
                        .ok_or(Error::new_from_js("value", "bigint"))?
                        .to_i64()?
                )
                .map_err(|_| Error::Unknown)?;
            }
            Type::Array => {
                let array = value
                    .into_array()
                    .ok_or(Error::new_from_js("value", "array"))?;
                if depth > self.max_depth {
                    write!(out, "[Array]").map_err(|_| Error::Unknown)?;
                } else if key {
                    for (i, element) in array.iter().enumerate() {
                        if i > 0 {
                            write!(out, ",").map_err(|_| Error::Unknown)?;
                        }
                        self._format(out, element?, true, depth + 1)?;
                    }
                } else {
                    write!(out, "[ ").map_err(|_| Error::Unknown)?;
                    for (i, element) in array.iter().enumerate() {
                        if i > 0 {
                            write!(out, ", ").map_err(|_| Error::Unknown)?;
                        }
                        self._format(out, element?, false, depth + 1)?;
                    }
                    write!(out, " ]").map_err(|_| Error::Unknown)?;
                }
            }
            // rquickjs 0.12 split `Type::Proxy` out of `Type::Object`; a proxy is still a
            // JS_TAG_OBJECT, so `into_object`/`props` work on it and its traps get to answer.
            Type::Object | Type::Proxy => {
                if depth > self.max_depth {
                    write!(out, "[Object]").map_err(|_| Error::Unknown)?;
                } else if key {
                    write!(out, "[object Object]").map_err(|_| Error::Unknown)?;
                } else {
                    let object = value
                        .into_object()
                        .ok_or(Error::new_from_js("value", "object"))?;
                    write!(out, "{{ ").map_err(|_| Error::Unknown)?;
                    for prop in object.props() {
                        let (key, val) = prop?;
                        self._format(out, key, true, depth + 1)?;
                        write!(out, ": ").map_err(|_| Error::Unknown)?;
                        self._format(out, val, false, depth + 1)?;
                    }
                    write!(out, " }}").map_err(|_| Error::Unknown)?;
                }
            }
            Type::Symbol => {
                let symbol = value
                    .as_symbol()
                    .ok_or(Error::new_from_js("value", "symbol"))?;
                let description = match symbol.description()?.as_string() {
                    Some(description) => description.to_string()?,
                    None => String::default(),
                };
                write!(out, "Symbol({})", description).map_err(|_| Error::Unknown)?;
            }
            Type::Function => {
                let function = value
                    .as_function()
                    .ok_or(Error::new_from_js("value", "function"))?
                    .as_object()
                    .ok_or(Error::new_from_js("function", "object"))?;
                let name: Option<String> = function
                    .get("name")
                    .ok()
                    .filter(|name| name != "[object Object]");
                match name {
                    Some(name) => {
                        write!(out, "[Function: {}]", name).map_err(|_| Error::Unknown)?
                    }
                    None => write!(out, "[Function (anonymous)]").map_err(|_| Error::Unknown)?,
                }
            }
            Type::Null => {
                write!(out, "null",).map_err(|_| Error::Unknown)?;
            }
            Type::Undefined => {
                write!(out, "undefined",).map_err(|_| Error::Unknown)?;
            }
            _ => {}
        };

        Ok(())
    }
}

const TARGET: &str = "console";

/// A console object to print messages to the [`log`] crate.
///
/// # Example
/// ```rust
/// use rquickjs::{Context, Function, Object, Runtime};
/// use rquickjs_extra::console::{Console, Formatter};
///
/// fn main() {
///     let rt = Runtime::new().unwrap();
///     let ctx = Context::full(&rt).unwrap();
///
///     ctx.with(|ctx| {
///         let console = Console::new("hello", Formatter::default());
///         ctx.globals().set("console", console).unwrap();
///         let console: Object = ctx.globals().get("console").unwrap();
///         let log: Function = console.get("log").unwrap();
///         log.call::<_, ()>(("test",)).unwrap();
///     })
/// }
/// ```
///
/// [`log`]: https://docs.rs/log
#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(frozen)]
pub struct Console {
    target:    String,
    formatter: Formatter,
}

impl Console {
    pub fn new(target: &str, formatter: Formatter) -> Self {
        Self {
            target: target.to_string(),
            formatter,
        }
    }

    fn print(&self, values: Rest<Value<'_>>) -> Result<String> {
        let mut message = String::new();
        for (i, value) in values.0.into_iter().enumerate() {
            if i > 0 {
                write!(&mut message, ", ").map_err(|_| Error::Unknown)?
            }
            self.formatter.format(&mut message, value)?
        }
        Ok(message)
    }
}

#[rquickjs::methods]
impl Console {
    fn debug(&self, values: Rest<Value<'_>>) -> Result<()> {
        tracing::debug!(target = self.target, "{}", self.print(values)?);
        Ok(())
    }

    fn log(&self, values: Rest<Value<'_>>) -> Result<()> {
        tracing::info!(target = self.target, "{}", self.print(values)?);
        Ok(())
    }

    fn warn(&self, values: Rest<Value<'_>>) -> Result<()> {
        tracing::warn!(target = self.target, "{}", self.print(values)?);
        Ok(())
    }

    fn error(&self, values: Rest<Value<'_>>) -> Result<()> {
        tracing::error!(target = self.target, "{}", self.print(values)?);
        Ok(())
    }
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod console {
    use rquickjs::{Ctx, Result, module::Exports};

    use crate::{Console, Formatter, TARGET};

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        globals.set("console", Console::new(TARGET, Formatter::new(3)))?;
        Ok(())
    }
}
