use std::{collections::HashSet, fmt::Write as _, io};

use den_util::{instance_of_global, stack::format_thrown};
use rquickjs::{
    Coerced, Ctx, Error, FromJs as _, Function, JsLifetime, Object, Result, Type, Value,
    class::Trace,
    function::{Opt, Rest},
};

/// Bounded, cycle-safe value formatting shared by every console method.
#[derive(Clone, Debug, Trace, JsLifetime)]
pub struct Formatter {
    max_depth: usize,
}

impl Default for Formatter {
    fn default() -> Self { Self::new(10) }
}

impl Formatter {
    pub const fn new(max_depth: usize) -> Self { Self { max_depth } }

    pub fn format<W: std::fmt::Write>(&self, out: &mut W, value: Value<'_>) -> Result<()> {
        self.format_value(out, value, false, 0, &mut HashSet::new())
    }

    /// Apply console substitutions to the first string and inspect every
    /// argument left over.
    pub fn format_values<'js, I>(&self, values: I) -> Result<String>
    where
        I: IntoIterator<Item = Value<'js>>,
    {
        let values: Vec<_> = values.into_iter().collect();
        let Some(first) = values.first() else {
            return Ok(String::new());
        };

        let mut out = String::new();
        let mut next = 0;
        if let Some(format) = first.as_string() {
            next = 1;
            self.substitute(&mut out, &format.to_string()?, &values, &mut next)?;
        }
        for value in values.into_iter().skip(next) {
            if !out.is_empty() {
                out.push(' ');
            }
            self.format(&mut out, value)?;
        }
        Ok(out)
    }

    fn substitute(
        &self, out: &mut String, format: &str, values: &[Value<'_>], next: &mut usize,
    ) -> Result<()> {
        let mut chars = format.chars();
        while let Some(character) = chars.next() {
            if character != '%' {
                out.push(character);
                continue;
            }
            let Some(specifier) = chars.next() else {
                out.push('%');
                break;
            };
            if specifier == '%' {
                out.push('%');
                continue;
            }
            if !matches!(specifier, 's' | 'd' | 'i' | 'f' | 'o' | 'O' | 'c') {
                out.push('%');
                out.push(specifier);
                continue;
            }
            let Some(value) = values.get(*next) else {
                out.push('%');
                out.push(specifier);
                continue;
            };
            *next += 1;
            match specifier {
                's' => {
                    let Coerced(text) = Coerced::<String>::from_js(value.ctx(), value.clone())?;
                    out.push_str(&text);
                }
                'd' => {
                    let Coerced(number) = Coerced::<f64>::from_js(value.ctx(), value.clone())?;
                    Self::write_number(out, number)?;
                }
                'i' => Self::write_number(out, Self::parse_number(value, "parseInt")?)?,
                'f' => Self::write_number(out, Self::parse_number(value, "parseFloat")?)?,
                'o' | 'O' => self.format(out, value.clone())?,
                // CSS styling has no terminal representation, but still consumes
                // its argument like browser consoles do.
                'c' => {}
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn parse_number(value: &Value<'_>, function: &str) -> Result<f64> {
        let parse: Function<'_> = value.ctx().globals().get(function)?;
        parse.call((value.clone(),))
    }

    fn write_number(out: &mut String, number: f64) -> Result<()> {
        let text = if number.is_nan() {
            "NaN".to_owned()
        } else if number == f64::INFINITY {
            "Infinity".to_owned()
        } else if number == f64::NEG_INFINITY {
            "-Infinity".to_owned()
        } else {
            number.to_string()
        };
        out.write_str(&text).map_err(|_error| Error::Unknown)
    }

    fn format_value<'js, W: std::fmt::Write>(
        &self, out: &mut W, value: Value<'js>, key: bool, depth: usize,
        ancestors: &mut HashSet<Value<'js>>,
    ) -> Result<()> {
        if value.as_exception().is_some()
            || instance_of_global(value.ctx(), &value, "DOMException")?
        {
            return out
                .write_str(&format_thrown(value.ctx(), &value))
                .map_err(|_error| Error::Unknown);
        }

        match value.type_of() {
            Type::String => {
                out.write_str(
                    &value
                        .into_string()
                        .ok_or_else(|| Error::new_from_js("value", "string"))?
                        .to_string()?,
                )
                .map_err(|_error| Error::Unknown)?
            }
            Type::Int => {
                write!(
                    out,
                    "{}",
                    value
                        .as_int()
                        .ok_or_else(|| Error::new_from_js("value", "int"))?
                )
                .map_err(|_error| Error::Unknown)?
            }
            Type::Bool => {
                write!(
                    out,
                    "{}",
                    value
                        .as_bool()
                        .ok_or_else(|| Error::new_from_js("value", "bool"))?
                )
                .map_err(|_error| Error::Unknown)?
            }
            Type::Float => {
                write!(
                    out,
                    "{}",
                    value
                        .as_float()
                        .ok_or_else(|| Error::new_from_js("value", "float"))?
                )
                .map_err(|_error| Error::Unknown)?
            }
            Type::BigInt => {
                let Coerced(text) = Coerced::<String>::from_js(value.ctx(), value.clone())?;
                write!(out, "{text}n").map_err(|_error| Error::Unknown)?;
            }
            Type::Array => {
                if depth >= self.max_depth {
                    return out.write_str("[Array]").map_err(|_error| Error::Unknown);
                }
                if !ancestors.insert(value.clone()) {
                    return out.write_str("[Circular]").map_err(|_error| Error::Unknown);
                }
                let result: Result<()> = (|| {
                    let array = value
                        .as_array()
                        .ok_or_else(|| Error::new_from_js("value", "array"))?;
                    if key {
                        for (index, element) in array.iter().enumerate() {
                            if index > 0 {
                                out.write_char(',').map_err(|_error| Error::Unknown)?;
                            }
                            self.format_value(out, element?, true, depth + 1, ancestors)?;
                        }
                    } else {
                        out.write_str("[ ").map_err(|_error| Error::Unknown)?;
                        for (index, element) in array.iter().enumerate() {
                            if index > 0 {
                                out.write_str(", ").map_err(|_error| Error::Unknown)?;
                            }
                            self.format_value(out, element?, false, depth + 1, ancestors)?;
                        }
                        out.write_str(" ]").map_err(|_error| Error::Unknown)?;
                    }
                    Ok(())
                })();
                ancestors.remove(&value);
                result?;
            }
            Type::Object | Type::Promise | Type::Proxy => {
                if depth >= self.max_depth {
                    return out.write_str("[Object]").map_err(|_error| Error::Unknown);
                }
                if key {
                    return out
                        .write_str("[object Object]")
                        .map_err(|_error| Error::Unknown);
                }
                if !ancestors.insert(value.clone()) {
                    return out.write_str("[Circular]").map_err(|_error| Error::Unknown);
                }
                let result: Result<()> = (|| {
                    let object = value
                        .as_object()
                        .ok_or_else(|| Error::new_from_js("value", "object"))?;
                    out.write_str("{ ").map_err(|_error| Error::Unknown)?;
                    for (index, property) in object.props().enumerate() {
                        if index > 0 {
                            out.write_str(", ").map_err(|_error| Error::Unknown)?;
                        }
                        let (property, nested) = property?;
                        self.format_value(out, property, true, depth + 1, ancestors)?;
                        out.write_str(": ").map_err(|_error| Error::Unknown)?;
                        self.format_value(out, nested, false, depth + 1, ancestors)?;
                    }
                    out.write_str(" }").map_err(|_error| Error::Unknown)
                })();
                ancestors.remove(&value);
                result?;
            }
            Type::Symbol => {
                let symbol = value
                    .as_symbol()
                    .ok_or_else(|| Error::new_from_js("value", "symbol"))?;
                let description = match symbol.description()?.as_string() {
                    Some(description) => description.to_string()?,
                    None => String::new(),
                };
                write!(out, "Symbol({description})").map_err(|_error| Error::Unknown)?;
            }
            Type::Function | Type::Constructor => {
                let function = value
                    .as_function()
                    .ok_or_else(|| Error::new_from_js("value", "function"))?
                    .as_object()
                    .ok_or_else(|| Error::new_from_js("function", "object"))?;
                let name: Option<String> = function
                    .get("name")
                    .ok()
                    .filter(|name| name != "[object Object]");
                match name {
                    Some(name) => {
                        write!(out, "[Function: {name}]").map_err(|_error| Error::Unknown)?
                    }
                    None => {
                        out.write_str("[Function (anonymous)]")
                            .map_err(|_error| Error::Unknown)?
                    }
                }
            }
            Type::Null => out.write_str("null").map_err(|_error| Error::Unknown)?,
            Type::Undefined => {
                out.write_str("undefined")
                    .map_err(|_error| Error::Unknown)?
            }
            other => write!(out, "[{}]", other.as_str()).map_err(|_error| Error::Unknown)?,
        }
        Ok(())
    }
}

/// The JavaScript console. Output methods write directly to their conventional
/// standard stream so build profiles and tracing filters cannot erase them.
#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(frozen)]
pub struct Console {
    formatter: Formatter,
}

impl Console {
    pub const fn new(formatter: Formatter) -> Self { Self { formatter } }

    fn print(&self, Rest(values): Rest<Value<'_>>) -> Result<String> {
        self.formatter.format_values(values)
    }

    fn write_stdout(message: &str) -> Result<()> {
        let stdout = io::stdout();
        Self::write_line(&mut stdout.lock(), message)
    }

    fn write_stderr(message: &str) -> Result<()> {
        let stderr = io::stderr();
        Self::write_line(&mut stderr.lock(), message)
    }

    fn write_line(writer: &mut impl io::Write, message: &str) -> Result<()> {
        writeln!(writer, "{message}").map_err(Error::from)
    }

    fn capture_trace(ctx: &Ctx<'_>) -> Result<String> {
        let error: Object<'_> = ctx.globals().get("Error")?;
        let capture: Function<'_> = error.get("captureStackTrace")?;
        let console: Object<'_> = ctx.globals().get("console")?;
        let trace: Function<'_> = console.get("trace")?;
        let holder = Object::new(ctx.clone())?;
        capture.call::<_, ()>((holder.clone(), trace))?;
        let stack = holder
            .get::<_, Option<Coerced<String>>>("stack")?
            .map_or_else(String::new, |Coerced(stack)| stack);
        let stack = stack.trim_end();
        Ok(match stack.split_once('\n') {
            Some((first, frames)) if !first.trim_start().starts_with("at ") => frames.to_owned(),
            _ => stack.to_owned(),
        })
    }
}

#[rquickjs::methods]
impl Console {
    fn debug(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stdout(&self.print(values)?)
    }

    fn log(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stdout(&self.print(values)?)
    }

    fn info(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stdout(&self.print(values)?)
    }

    fn dir(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stdout(&self.print(values)?)
    }

    fn warn(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stderr(&self.print(values)?)
    }

    fn error(&self, values: Rest<Value<'_>>) -> Result<()> {
        Self::write_stderr(&self.print(values)?)
    }

    #[qjs(rename = "trace")]
    fn trace_<'js>(&self, ctx: Ctx<'js>, values: Rest<Value<'js>>) -> Result<()> {
        let detail = self.print(values)?;
        let mut message = if detail.is_empty() {
            "Trace".to_owned()
        } else {
            format!("Trace: {detail}")
        };
        let frames = Self::capture_trace(&ctx)?;
        if !frames.is_empty() {
            message.push('\n');
            message.push_str(&frames);
        }
        Self::write_stderr(&message)
    }

    #[qjs(rename = "assert")]
    fn assert_<'js>(
        &self, Opt(condition): Opt<Value<'js>>, values: Rest<Value<'js>>,
    ) -> Result<()> {
        let passed = match condition {
            Some(condition) => {
                let ctx = condition.ctx().clone();
                Coerced::<bool>::from_js(&ctx, condition)?.0
            }
            None => false,
        };
        if passed {
            return Ok(());
        }
        let detail = self.print(values)?;
        if detail.is_empty() {
            Self::write_stderr("Assertion failed")
        } else {
            Self::write_stderr(&format!("Assertion failed: {detail}"))
        }
    }
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod console {
    use rquickjs::{Ctx, Result, module::Exports};

    use crate::{Console, Formatter};

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        ctx.globals()
            .set("console", Console::new(Formatter::new(3)))
    }
}
