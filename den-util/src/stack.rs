//! Structured JavaScript stacks and source-map correction.
//!
//! quickjs-ng calls `Error.prepareStackTrace(error, call_sites)` while it still
//! has structured frames. Installing one native formatter here avoids parsing
//! an already-flattened stack string and gives every den subsystem the same
//! corrected `error.stack`.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fmt::{self, Write as _},
};

use oxc_sourcemap::SourceMap;
use rquickjs::{
    Array, Coerced, Constructor, Ctx, Error, Exception, FromJs, Function, JsLifetime, Object,
    Result, Value, function::This, object::Property,
};
use url::Url;

const MAPPED_STACK: &str = "\0den:mapped-stack";
const MAPPED_FRAMES: &str = "\0den:mapped-frames";

/// One source location extracted from a formatted stack.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Location {
    pub filename: String,
    pub line: u32,
    pub column: u32,
}

/// An owned JavaScript failure that remains useful after leaving the realm.
#[derive(Clone, Debug)]
pub struct JsError {
    name: Option<String>,
    message: String,
    stack: Option<String>,
    rendered: String,
    location: Option<Location>,
}

impl JsError {
    pub fn from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Self {
        let error = value
            .as_exception()
            .map(rquickjs::Exception::as_object)
            .or_else(|| {
                (crate::instance_of_global(ctx, value, "DOMException").unwrap_or(false))
                    .then(|| value.as_object())
                    .flatten()
            });
        if let Some(error) = error {
            let name = optional(ctx, error.get::<_, Option<Coerced<String>>>("name"))
                .flatten()
                .map(|Coerced(name)| name);
            let message = optional(ctx, error.get::<_, Option<Coerced<String>>>("message"))
                .flatten()
                .map_or_else(String::new, |Coerced(message)| message);
            let stack = optional(ctx, error.get::<_, Option<Coerced<String>>>("stack"))
                .flatten()
                .map(|Coerced(stack)| stack);
            let rendered = format_error(ctx, error);
            let location = first_location(&rendered);
            return Self {
                name,
                message,
                stack,
                rendered,
                location,
            };
        }
        let message = optional(ctx, Coerced::<String>::from_js(ctx, value.clone()))
            .map_or_else(|| "unknown error".to_owned(), |Coerced(text)| text);
        Self {
            name: None,
            rendered: message.clone(),
            message,
            stack: None,
            location: None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn stack(&self) -> Option<&str> {
        self.stack.as_deref()
    }

    pub const fn location(&self) -> Option<&Location> {
        self.location.as_ref()
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::error::Error for JsError {}

const RETAINED_EVAL_SOURCES: usize = 256;

#[derive(Default)]
struct SourceMaps(RefCell<Registry>);

#[derive(Default)]
struct Registry {
    scripts: HashMap<String, Script>,
    evals: VecDeque<String>,
}

// SAFETY: the registry contains owned Rust strings and integers only.
unsafe impl JsLifetime<'_> for SourceMaps {
    type Changed<'to> = SourceMaps;
}

struct Script {
    generated_source: String,
    layers: Vec<SourceMap<'static>>,
}

/// Install den's structured stack formatter on this realm.
///
/// `Error.prepareStackTrace` remains writable, matching Node: application code
/// can deliberately replace it after startup.
pub fn install(ctx: &Ctx<'_>) -> Result<()> {
    if ctx.userdata::<SourceMaps>().is_none() {
        ctx.store_userdata(SourceMaps::default())
            .map_err(|_error| Exception::throw_internal(ctx, "stack registry is already in use"))?;
    }
    let error: Object<'_> = ctx.globals().get("Error")?;
    let prepare =
        Function::new(ctx.clone(), prepare_stack_trace)?.with_name("prepareStackTrace")?;
    error.set("prepareStackTrace", prepare)
}

/// Register the exact code QuickJS compiled and each map layer leading back to
/// its authored source. Layers are applied in order.
pub fn register_source<I>(
    ctx: &Ctx<'_>, filename: &str, generated_source: String, source_maps: I,
) -> Result<()>
where
    I: IntoIterator<Item = SourceMap<'static>>,
{
    if ctx.userdata::<SourceMaps>().is_none() {
        install(ctx)?;
    }
    let registry = ctx
        .userdata::<SourceMaps>()
        .ok_or_else(|| Exception::throw_internal(ctx, "stack registry is missing"))?;
    let mut registry = registry
        .0
        .try_borrow_mut()
        .map_err(|_error| Exception::throw_internal(ctx, "stack registry is busy"))?;
    let layers: Vec<_> = source_maps.into_iter().collect();
    if layers.is_empty()
        && registry
            .scripts
            .get(filename)
            .is_some_and(|script| script.generated_source == generated_source)
    {
        return Ok(());
    }
    let is_new_eval = filename.starts_with("<eval:") && !registry.scripts.contains_key(filename);
    registry.scripts.insert(
        filename.to_owned(),
        Script {
            generated_source,
            layers,
        },
    );
    if is_new_eval {
        registry.evals.push_back(filename.to_owned());
        // ponytail: keep recent REPL maps bounded; switch to weak script keys if
        // rquickjs exposes bytecode identity.
        while registry.evals.len() > RETAINED_EVAL_SOURCES {
            if let Some(expired) = registry.evals.pop_front() {
                registry.scripts.remove(&expired);
            }
        }
    }
    Ok(())
}

/// Last source-map directive in a script, if present.
pub fn source_mapping_url(source: &str) -> Option<&str> {
    source.lines().rev().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("//# sourceMappingURL=")
            .or_else(|| line.strip_prefix("//@ sourceMappingURL="))
            .or_else(|| {
                let value = line.strip_prefix("/*# sourceMappingURL=")?;
                value.strip_suffix("*/")
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// Decode an inline data URL. Invalid maps are deliberately ignored so stack
/// construction remains total.
pub fn inline_source_map(url: &str, source_url: &Url) -> Option<SourceMap<'static>> {
    let data = url.strip_prefix("data:")?;
    let (metadata, payload) = data.split_once(',')?;
    let bytes = if metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        base64_simd::STANDARD
            .decode_to_vec(payload.as_bytes())
            .ok()?
    } else {
        percent_encoding::percent_decode_str(payload).collect()
    };
    let json = String::from_utf8(bytes).ok()?;
    parse_source_map(&json, source_url)
}

/// Parse a v3 source map and resolve each source against the map's URL.
pub fn parse_source_map(json: &str, map_url: &Url) -> Option<SourceMap<'static>> {
    let mut map = SourceMap::from_json_string(json).ok()?.into_owned();
    let base = map
        .get_source_root()
        .and_then(|root| map_url.join(root).ok())
        .unwrap_or_else(|| map_url.clone());
    let sources: Vec<_> = map
        .get_sources()
        .map(|source| {
            base.join(source)
                .map_or_else(|_error| source.to_owned(), String::from)
        })
        .collect();
    map.set_sources(sources);
    Some(map)
}

/// Create an ordinary Error after repairing the eagerly prepared header.
///
/// rquickjs's `Exception::from_message` calls `JS_NewError` before assigning
/// `message`; quickjs-ng therefore invokes `prepareStackTrace` too early. This
/// helper makes host-created errors agree with `new Error(message)`.
pub fn error_from_message<'js>(ctx: &Ctx<'js>, message: &str) -> Result<Exception<'js>> {
    let error = Exception::from_message(ctx.clone(), message)?;
    refresh_header(ctx, error.as_object())?;
    Ok(error)
}

/// Throw an ordinary host Error with a current, mapped stack header.
pub fn throw_error(ctx: &Ctx<'_>, message: &str) -> Error {
    match error_from_message(ctx, message) {
        Ok(error) => error.throw(),
        Err(error) => error,
    }
}

/// Rebuild an Error that crossed a worker-thread boundary as owned text.
pub fn error_from_parts<'js>(
    ctx: &Ctx<'js>, name: Option<&str>, message: &str, stack: &str,
) -> Result<Value<'js>> {
    let constructor_name = name.map_or("Error", builtin_error_name);
    let constructor: Constructor<'js> = ctx.globals().get(constructor_name)?;
    let error: Object<'js> = constructor.construct((message,))?;
    if let Some(name) = name {
        error.set("name", name)?;
    }
    if !stack.is_empty() {
        error.set("stack", stack)?;
        let header = match (name.unwrap_or("Error"), message) {
            ("", "") => String::new(),
            ("", message) => message.to_owned(),
            (name, "") => name.to_owned(),
            (name, message) => format!("{name}: {message}"),
        };
        if let Some(frames) = stack.strip_prefix(&header) {
            error.set(MAPPED_FRAMES, frames)?;
            error.set(MAPPED_STACK, true)?;
        } else {
            error.set(MAPPED_STACK, false)?;
        }
    }
    Ok(error.into_value())
}

/// Format a thrown value the same way console and uncaught-error reporting do.
pub fn format_thrown<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> String {
    JsError::from_value(ctx, value).to_string()
}

/// Find the first concrete `file:line:column` frame in a formatted stack.
pub fn first_location(stack: &str) -> Option<Location> {
    stack.lines().find_map(|line| {
        let frame = line.trim().strip_prefix("at ")?;
        let location = frame
            .rsplit_once('(')
            .map_or(frame, |(_, inside)| inside.trim_end_matches(')'));
        let (head, column) = location.rsplit_once(':')?;
        let (filename, line) = head.rsplit_once(':')?;
        Some(Location {
            filename: filename.to_owned(),
            line: line.parse().ok()?,
            column: column.parse().ok()?,
        })
    })
}

fn prepare_stack_trace<'js>(ctx: Ctx<'js>, error: Object<'js>, sites: Array<'js>) -> String {
    let header = error_header(&ctx, &error);
    let mut frames = String::new();
    for site in sites
        .iter::<Object<'js>>()
        .filter_map(|site| optional(&ctx, site))
    {
        let mut frame = StackFrame {
            function: call::<Option<String>>(&ctx, &site, "getFunctionName").flatten(),
            filename: call::<Option<String>>(&ctx, &site, "getFileName").flatten(),
            line: call::<i32>(&ctx, &site, "getLineNumber").unwrap_or(-1),
            column: call::<i32>(&ctx, &site, "getColumnNumber").unwrap_or(-1),
            native: call::<bool>(&ctx, &site, "isNative").unwrap_or(false),
        };
        remap(&ctx, &mut frame);
        let _ = write_frame(&mut frames, &frame);
    }
    let _ = optional(
        &ctx,
        error.prop(
            MAPPED_FRAMES,
            Property::from(frames.clone()).writable().configurable(),
        ),
    );
    let _ = optional(
        &ctx,
        error.prop(MAPPED_STACK, Property::from(true).writable().configurable()),
    );
    format!("{header}{frames}")
}

struct StackFrame {
    function: Option<String>,
    filename: Option<String>,
    line: i32,
    column: i32,
    native: bool,
}

fn remap(ctx: &Ctx<'_>, frame: &mut StackFrame) {
    let (Some(filename), Ok(line), Ok(column)) = (
        frame.filename.clone(),
        u32::try_from(frame.line),
        u32::try_from(frame.column),
    ) else {
        return;
    };
    if line == 0 || column == 0 {
        return;
    }
    let Some(registry) = ctx.userdata::<SourceMaps>() else {
        return;
    };
    let Ok(registry) = registry.0.try_borrow() else {
        return;
    };

    let mut filename = filename;
    let mut line = line - 1;
    let mut column = column - 1;
    let mut visited = HashSet::new();
    while visited.insert(filename.clone()) {
        let Some(script) = registry.scripts.get(&filename) else {
            break;
        };
        column = byte_column_to_utf16(&script.generated_source, line, column);
        let previous_filename = filename.clone();
        for layer in &script.layers {
            let lookup = layer.generate_lookup_table();
            let Some(mapping) = layer.lookup_source_view_token(&lookup, line, column) else {
                break;
            };
            let Some(source) = mapping.get_source() else {
                break;
            };
            filename = source.to_owned();
            line = mapping.get_src_line();
            column = mapping.get_src_col();
            if let Some(name) = mapping.get_name() {
                frame.function = Some(name.to_owned());
            }
        }
        if filename == previous_filename {
            break;
        }
    }

    frame.filename = Some(filename);
    frame.line = i32::try_from(line.saturating_add(1)).unwrap_or(i32::MAX);
    frame.column = i32::try_from(column.saturating_add(1)).unwrap_or(i32::MAX);
}

fn byte_column_to_utf16(source: &str, line: u32, byte_column: u32) -> u32 {
    let Some(line) = source.split('\n').nth(line as usize) else {
        return byte_column;
    };
    let line = line.strip_suffix('\r').unwrap_or(line);
    let byte_column = (byte_column as usize).min(line.len());
    let boundary = (0..=byte_column)
        .rev()
        .find(|offset| line.is_char_boundary(*offset))
        .unwrap_or_default();
    line.get(..boundary).map_or(byte_column as u32, |prefix| {
        prefix.encode_utf16().count() as u32
    })
}

fn write_frame(out: &mut String, frame: &StackFrame) -> std::fmt::Result {
    out.push_str("\n    at ");
    let function = frame.function.as_deref().filter(|name| !name.is_empty());
    if frame.native {
        out.push_str(function.unwrap_or("<anonymous>"));
        return out.write_str(" (native)");
    }
    match (function, frame.filename.as_deref()) {
        (Some(function), Some(filename)) => {
            write!(out, "{function} ({filename}")?;
            write_position(out, frame)?;
            out.push(')');
        }
        (None, Some(filename)) => {
            out.push_str(filename);
            write_position(out, frame)?;
        }
        (Some(function), None) => out.push_str(function),
        (None, None) => out.push_str("<anonymous>"),
    }
    Ok(())
}

fn write_position(out: &mut String, frame: &StackFrame) -> std::fmt::Result {
    if frame.line > 0 {
        write!(out, ":{}", frame.line)?;
        if frame.column > 0 {
            write!(out, ":{}", frame.column)?;
        }
    }
    Ok(())
}

fn format_error(ctx: &Ctx<'_>, error: &Object<'_>) -> String {
    let header = error_header(ctx, error);
    let stack = optional(ctx, error.get::<_, Option<Coerced<String>>>("stack"))
        .flatten()
        .map(|Coerced(stack)| stack)
        .unwrap_or_default();
    if stack.is_empty() {
        return header;
    }
    if optional(ctx, error.get::<_, bool>(MAPPED_STACK)).unwrap_or(false) {
        let frames = optional(ctx, error.get::<_, String>(MAPPED_FRAMES)).unwrap_or_default();
        return format!("{header}{frames}");
    }
    if stack
        .lines()
        .next()
        .is_some_and(|line| line.trim_start().starts_with("at "))
    {
        format!("{header}\n{}", stack.trim_end())
    } else {
        stack
    }
}

fn refresh_header(ctx: &Ctx<'_>, error: &Object<'_>) -> Result<()> {
    if !optional(ctx, error.get::<_, bool>(MAPPED_STACK)).unwrap_or(false) {
        return Ok(());
    }
    let header = error_header(ctx, error);
    let frames = error
        .get::<_, Option<String>>(MAPPED_FRAMES)?
        .unwrap_or_default();
    let stack = format!("{header}{frames}");
    error.set("stack", stack)
}

fn error_header(ctx: &Ctx<'_>, error: &Object<'_>) -> String {
    let name = optional(ctx, error.get::<_, Option<Coerced<String>>>("name"))
        .flatten()
        .map_or_else(|| "Error".to_owned(), |Coerced(name)| name);
    let message = optional(ctx, error.get::<_, Option<Coerced<String>>>("message"))
        .flatten()
        .map_or_else(String::new, |Coerced(message)| message);
    match (name.is_empty(), message.is_empty()) {
        (true, true) => String::new(),
        (true, false) => message,
        (false, true) => name,
        (false, false) => format!("{name}: {message}"),
    }
}

fn call<'js, T: FromJs<'js>>(ctx: &Ctx<'js>, site: &Object<'js>, name: &str) -> Option<T> {
    let function = optional(ctx, site.get::<_, Function<'js>>(name))?;
    optional(ctx, function.call::<_, T>((This(site.clone()),)))
}

fn optional<T>(ctx: &Ctx<'_>, result: Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(Error::Exception) => {
            drop(ctx.catch());
            None
        }
        Err(_) => None,
    }
}

fn builtin_error_name(name: &str) -> &str {
    if matches!(
        name,
        "Error"
            | "EvalError"
            | "RangeError"
            | "ReferenceError"
            | "SyntaxError"
            | "TypeError"
            | "URIError"
    ) {
        name
    } else {
        "Error"
    }
}
