//! JSR `@std/assert@1.0.19` plus Insta-backed snapshots as `den:assert`.
//! Import-only; no globals.

use std::{
    any::Any,
    cmp::Ordering,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
};

use den_util::{instance_of_global, json_stringify};
pub use insta;
use rquickjs::{
    Class, Coerced, Ctx, Error, Filter, FromJs as _, Function, Object, Result, Value, class::Trace,
    function::Opt, prelude::This,
};

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "AssertionError")]
pub struct AssertionError {
    #[qjs(get, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl AssertionError {
    #[qjs(constructor)]
    pub fn new(message: Opt<String>) -> Self {
        Self {
            message: message.0.unwrap_or_default(),
        }
    }

    #[qjs(get)]
    pub const fn name(&self) -> &'static str { "AssertionError" }
}

fn throw_assertion(ctx: &Ctx<'_>, message: String) -> Error {
    match Class::<AssertionError>::instance(ctx.clone(), AssertionError { message }) {
        Ok(error) => ctx.throw(error.into_value()),
        Err(error) => error,
    }
}

fn is_truthy<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    Ok(Coerced::<bool>::from_js(ctx, value.clone())?.0)
}

fn same_value<'js>(ctx: &Ctx<'js>, left: &Value<'js>, right: &Value<'js>) -> Result<bool> {
    let object: Object = ctx.globals().get("Object")?;
    let is: Function = object.get("is")?;
    is.call((left.clone(), right.clone()))
}

fn equal<'js>(ctx: &Ctx<'js>, left: &Value<'js>, right: &Value<'js>) -> Result<bool> {
    equal_seen(ctx, left, right, &mut Vec::new())
}

fn equal_seen<'js>(
    ctx: &Ctx<'js>, left: &Value<'js>, right: &Value<'js>, seen: &mut Vec<(Value<'js>, Value<'js>)>,
) -> Result<bool> {
    if same_value(ctx, left, right)? {
        return Ok(true);
    }
    if left.type_of() != right.type_of() {
        return Ok(false);
    }
    if !left.is_object() {
        return Ok(false);
    }
    for (seen_left, seen_right) in seen.iter() {
        if same_value(ctx, seen_left, left)? && same_value(ctx, seen_right, right)? {
            return Ok(true);
        }
    }
    seen.push((left.clone(), right.clone()));
    let outcome = equal_objects(ctx, left, right, seen)?;
    seen.pop();
    Ok(outcome)
}

fn equal_objects<'js>(
    ctx: &Ctx<'js>, left: &Value<'js>, right: &Value<'js>, seen: &mut Vec<(Value<'js>, Value<'js>)>,
) -> Result<bool> {
    if let (Some(left_array), Some(right_array)) = (left.as_array(), right.as_array()) {
        if left_array.len() != right_array.len() {
            return Ok(false);
        }
        for index in 0..left_array.len() {
            let left_item: Value = left_array.get(index)?;
            let right_item: Value = right_array.get(index)?;
            if !equal_seen(ctx, &left_item, &right_item, seen)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let left_date = instance_of_global(ctx, left, "Date")?;
    let right_date = instance_of_global(ctx, right, "Date")?;
    if left_date || right_date {
        if !(left_date && right_date) {
            return Ok(false);
        }
        let left_object = left
            .as_object()
            .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "expected a Date"))?;
        let right_object = right
            .as_object()
            .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "expected a Date"))?;
        let left_time: f64 = left_object
            .get::<_, Function>("getTime")?
            .call((This(left.clone()),))?;
        let right_time: f64 = right_object
            .get::<_, Function>("getTime")?
            .call((This(right.clone()),))?;
        return Ok(left_time.to_bits() == right_time.to_bits());
    }
    let left_regexp = instance_of_global(ctx, left, "RegExp")?;
    let right_regexp = instance_of_global(ctx, right, "RegExp")?;
    if left_regexp || right_regexp {
        if !(left_regexp && right_regexp) {
            return Ok(false);
        }
        let left_object = left
            .as_object()
            .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "expected a RegExp"))?;
        let right_object = right
            .as_object()
            .ok_or_else(|| rquickjs::Exception::throw_type(ctx, "expected a RegExp"))?;
        return Ok(left_object.get::<_, String>("source")?
            == right_object.get::<_, String>("source")?
            && left_object.get::<_, String>("flags")?
                == right_object.get::<_, String>("flags")?);
    }
    let Some(left_object) = left.as_object() else {
        return Ok(false);
    };
    let Some(right_object) = right.as_object() else {
        return Ok(false);
    };
    let left_keys = own_keys(left_object)?;
    let right_keys = own_keys(right_object)?;
    if left_keys.len() != right_keys.len() {
        return Ok(false);
    }
    for key in &left_keys {
        if !right_keys.iter().any(|other| other == key) {
            return Ok(false);
        }
        let left_value: Value = left_object.get(key.as_str())?;
        let right_value: Value = right_object.get(key.as_str())?;
        if !equal_seen(ctx, &left_value, &right_value, seen)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn own_keys(object: &Object<'_>) -> Result<Vec<String>> {
    object
        .own_keys::<String>(Filter::new().string().enum_only())
        .collect()
}

fn stringify<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> String {
    if let Ok(json) = json_stringify(ctx, value)
        && let Some(string) = json.as_string()
        && let Ok(text) = string.to_string()
    {
        return text;
    }
    Coerced::<String>::from_js(ctx, value.clone())
        .map_or_else(|_| value.type_of().as_str().to_owned(), |coerced| coerced.0)
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "snapshot assertion failed".to_owned())
}

fn assert_insta_snapshot(name: &str, value: &str) -> std::result::Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err("snapshot name must contain only ASCII letters, digits, '_' or '-'".into());
    }
    let path = std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join("snapshots");
    let snapshot_file = path.join(format!("{name}.snap"));
    let exists = snapshot_file.is_file();
    let matches = if exists {
        let snapshot = insta::Snapshot::from_file(&snapshot_file).ok();
        let current = insta::internals::TextSnapshotContents::new(
            value.to_owned(),
            insta::TextSnapshotKind::File,
        );
        snapshot
            .as_ref()
            .and_then(insta::Snapshot::as_text)
            .is_some_and(|stored| stored.matches_latest(&current))
    } else {
        false
    };
    if !matches {
        let update = std::env::var("INSTA_UPDATE").unwrap_or_else(|_| "auto".to_owned());
        let in_place = match update.as_str() {
            "always" | "force" => Some(true),
            "unseen" => Some(!exists),
            "new" => Some(false),
            "auto" if std::env::var_os("CI").is_none() => Some(false),
            "auto" | "no" => None,
            _ => return Err(format!("invalid INSTA_UPDATE mode: {update}")),
        };
        if let Some(in_place) = in_place {
            fs::create_dir_all(&path).map_err(|error| error.to_string())?;
            let target = if in_place {
                snapshot_file
            } else {
                snapshot_file.with_extension("snap.new")
            };
            fs::write(
                target,
                format!("---\nsource: den-stdlib-assert/src/lib.rs\n---\n{value}\n"),
            )
            .map_err(|error| error.to_string())?;
            if in_place {
                return Ok(());
            }
            return Err("snapshot differs; review the generated .snap.new file".into());
        }
        return Err("snapshot differs and INSTA_UPDATE forbids updates".into());
    }

    let workspace = insta::_get_workspace_root!();
    let mut settings = insta::Settings::clone_current();
    settings.set_snapshot_path(path);
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);

    // ponytail: Insta's public macro emits `#[allow]`, which this workspace
    // forbids. The exact dependency pin protects this intentionally narrow use
    // of its macro support until Insta offers a public non-macro assertion.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        settings.bind(|| {
            insta::_macro_support::assert_snapshot(
                (name, value).into(),
                workspace.as_path(),
                "assertSnapshot",
                module_path!(),
                file!(),
                line!(),
                "value",
            )
        })
    }));
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(payload) => Err(panic_message(payload.as_ref())),
    }
}

fn message_or(msg: Opt<String>, fallback: String) -> String { msg.0.unwrap_or(fallback) }

fn compare<'js>(ctx: &Ctx<'js>, left: &Value<'js>, right: &Value<'js>) -> Result<Ordering> {
    if let (Some(left_int), Some(right_int)) = (left.as_int(), right.as_int()) {
        return Ok(left_int.cmp(&right_int));
    }
    if let (Some(left_number), Some(right_number)) = (
        left.as_float().or_else(|| left.as_int().map(f64::from)),
        right.as_float().or_else(|| right.as_int().map(f64::from)),
    ) {
        return Ok(left_number.total_cmp(&right_number));
    }
    let left_text = Coerced::<String>::from_js(ctx, left.clone())?.0;
    let right_text = Coerced::<String>::from_js(ctx, right.clone())?.0;
    Ok(left_text.cmp(&right_text))
}

fn matches_class<'js>(value: &Value<'js>, class: &Value<'js>) -> bool {
    value
        .as_object()
        .is_some_and(|object| class.is_function() && object.is_instance_of(class))
}

fn error_message<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> String {
    if let Some(object) = value.as_object()
        && let Ok(message) = object.get::<_, String>("message")
    {
        return message;
    }
    stringify(ctx, value)
}

fn check_error<'js>(
    ctx: &Ctx<'js>, thrown: &Value<'js>, class_or_msg: Opt<Value<'js>>,
    includes_or_msg: Opt<Value<'js>>, msg: Opt<String>,
) -> Result<()> {
    let (class, includes, header) = split_error_args(class_or_msg, includes_or_msg, msg);
    if let Some(class) = class.as_ref()
        && !matches_class(thrown, class)
    {
        return Err(throw_assertion(
            ctx,
            message_or(Opt(header), "Expected error to be a specific type".into()),
        ));
    }
    if let Some(includes) = includes {
        let text = error_message(ctx, thrown);
        let holds = if let Some(object) = includes.as_object()
            && instance_of_global(ctx, &includes, "RegExp")?
        {
            let tester: Function = object.get("test")?;
            tester.call::<_, bool>((This(includes.clone()), text.clone()))?
        } else {
            let needle = Coerced::<String>::from_js(ctx, includes)?.0;
            text.contains(&needle)
        };
        if !holds {
            return Err(throw_assertion(
                ctx,
                message_or(
                    Opt(header),
                    format!("Expected error message to include match: {text}"),
                ),
            ));
        }
    }
    Ok(())
}

fn split_error_args<'js>(
    class_or_msg: Opt<Value<'js>>, includes_or_msg: Opt<Value<'js>>, msg: Opt<String>,
) -> (Option<Value<'js>>, Option<Value<'js>>, Option<String>) {
    match class_or_msg.0 {
        None => (None, None, msg.0),
        Some(first) if first.is_function() => {
            let includes = includes_or_msg.0.filter(|value| !value.is_undefined());
            (Some(first), includes, msg.0)
        }
        Some(first) => {
            let header = first
                .as_string()
                .and_then(|string| string.to_string().ok())
                .or(msg.0);
            (None, None, header)
        }
    }
}

#[rquickjs::module]
pub mod assert {
    #![expect(
        non_snake_case,
        reason = "JSR export names; rquickjs 0.12 exports the rust ident"
    )]
    use rquickjs::{
        Class, Ctx, Error, Function, Object, Result, Value, function::Opt, module::Exports,
        prelude::This,
    };

    pub use super::AssertionError;

    fn fail_with(ctx: &Ctx<'_>, message: String) -> Result<()> {
        Err(super::throw_assertion(ctx, message))
    }

    #[rquickjs::function]
    pub fn assert<'js>(ctx: Ctx<'js>, expr: Value<'js>, msg: Opt<String>) -> Result<()> {
        if super::is_truthy(&ctx, &expr)? {
            return Ok(());
        }
        fail_with(&ctx, super::message_or(msg, "Assertion failed".into()))
    }

    #[rquickjs::function]
    pub fn assertFalse<'js>(ctx: Ctx<'js>, expr: Value<'js>, msg: Opt<String>) -> Result<()> {
        if !super::is_truthy(&ctx, &expr)? {
            return Ok(());
        }
        fail_with(&ctx, super::message_or(msg, "Expected false".into()))
    }

    #[rquickjs::function]
    pub fn equal<'js>(ctx: Ctx<'js>, left: Value<'js>, right: Value<'js>) -> Result<bool> {
        super::equal(&ctx, &left, &right)
    }

    #[rquickjs::function]
    pub fn assertEquals<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::equal(&ctx, &actual, &expected)? {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(
                msg,
                format!(
                    "Values are not equal: {} !== {}",
                    super::stringify(&ctx, &actual),
                    super::stringify(&ctx, &expected)
                ),
            ),
        )
    }

    #[rquickjs::function]
    pub fn assertNotEquals<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if !super::equal(&ctx, &actual, &expected)? {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected values to differ".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertStrictEquals<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::same_value(&ctx, &actual, &expected)? {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected strictly equal".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertNotStrictEquals<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if !super::same_value(&ctx, &actual, &expected)? {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected strictly unequal".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertExists<'js>(ctx: Ctx<'js>, actual: Value<'js>, msg: Opt<String>) -> Result<()> {
        if actual.is_null() || actual.is_undefined() {
            return fail_with(
                &ctx,
                super::message_or(msg, "Expected value to exist".into()),
            );
        }
        Ok(())
    }

    #[expect(
        clippy::float_arithmetic,
        reason = "assertAlmostEquals is defined as IEEE-754 distance"
    )]
    #[rquickjs::function]
    pub fn assertAlmostEquals(
        ctx: Ctx<'_>, actual: f64, expected: f64, tolerance: Opt<f64>, msg: Opt<String>,
    ) -> Result<()> {
        let tolerance = tolerance.0.unwrap_or(1.0e-7);
        let delta = (actual - expected).abs();
        if delta <= tolerance {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(
                msg,
                format!("Expected {actual} ≈ {expected} (±{tolerance})"),
            ),
        )
    }

    #[rquickjs::function]
    pub fn assertGreater<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::compare(&ctx, &actual, &expected)? == std::cmp::Ordering::Greater {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected actual > expected".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertGreaterOrEqual<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::compare(&ctx, &actual, &expected)? != std::cmp::Ordering::Less {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected actual >= expected".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertLess<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::compare(&ctx, &actual, &expected)? == std::cmp::Ordering::Less {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected actual < expected".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertLessOrEqual<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::compare(&ctx, &actual, &expected)? != std::cmp::Ordering::Greater {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected actual <= expected".into()),
        )
    }

    #[rquickjs::function]
    pub fn assertInstanceOf<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected_type: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if super::matches_class(&actual, &expected_type) {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(
                msg,
                "Expected value to be an instance of the given type".into(),
            ),
        )
    }

    #[rquickjs::function]
    pub fn assertNotInstanceOf<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, unexpected_type: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if !super::matches_class(&actual, &unexpected_type) {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(
                msg,
                "Expected value not to be an instance of the given type".into(),
            ),
        )
    }

    #[rquickjs::function]
    pub fn assertMatch(
        ctx: Ctx<'_>, actual: String, expected: Object<'_>, msg: Opt<String>,
    ) -> Result<()> {
        let tester: Function = expected.get("test")?;
        let holds: bool = tester.call((This(expected.clone().into_value()), actual.clone()))?;
        if holds {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, format!("{actual} did not match")),
        )
    }

    #[rquickjs::function]
    pub fn assertNotMatch(
        ctx: Ctx<'_>, actual: String, expected: Object<'_>, msg: Opt<String>,
    ) -> Result<()> {
        let tester: Function = expected.get("test")?;
        let holds: bool = tester.call((This(expected.clone().into_value()), actual.clone()))?;
        if !holds {
            return Ok(());
        }
        fail_with(&ctx, super::message_or(msg, format!("{actual} matched")))
    }

    #[rquickjs::function]
    pub fn assertStringIncludes(
        ctx: Ctx<'_>, actual: String, expected: String, msg: Opt<String>,
    ) -> Result<()> {
        if actual.contains(&expected) {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, format!("{actual:?} does not include {expected:?}")),
        )
    }

    #[rquickjs::function]
    pub fn assertArrayIncludes<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        let Some(haystack) = actual.as_array() else {
            return fail_with(
                &ctx,
                super::message_or(msg, "Expected an array-like".into()),
            );
        };
        let needles: Vec<Value> = if let Some(array) = expected.as_array() {
            let mut items = Vec::new();
            for index in 0..array.len() {
                items.push(array.get(index)?);
            }
            items
        } else {
            vec![expected]
        };
        for needle in needles {
            let mut found = false;
            for index in 0..haystack.len() {
                let item: Value = haystack.get(index)?;
                if super::equal(&ctx, &item, &needle)? {
                    found = true;
                    break;
                }
            }
            if !found {
                return fail_with(
                    &ctx,
                    super::message_or(msg, "Expected array to include value".into()),
                );
            }
        }
        Ok(())
    }

    #[rquickjs::function]
    pub fn assertObjectMatch<'js>(
        ctx: Ctx<'js>, actual: Value<'js>, expected: Value<'js>, msg: Opt<String>,
    ) -> Result<()> {
        if object_match(&ctx, &actual, &expected)? {
            return Ok(());
        }
        fail_with(
            &ctx,
            super::message_or(msg, "Expected object to match".into()),
        )
    }

    /// Compare a string against a named Insta snapshot under `./snapshots` in
    /// the process working directory.
    #[rquickjs::function]
    pub fn assertSnapshot(ctx: Ctx<'_>, actual: String, name: String) -> Result<()> {
        super::assert_insta_snapshot(&name, &actual).map_err(|error| {
            super::throw_assertion(&ctx, format!("Snapshot {name:?} failed: {error}"))
        })
    }

    fn object_match<'js>(
        ctx: &Ctx<'js>, actual: &Value<'js>, expected: &Value<'js>,
    ) -> Result<bool> {
        let Some(expected_object) = expected.as_object() else {
            return super::equal(ctx, actual, expected);
        };
        let Some(actual_object) = actual.as_object() else {
            return Ok(false);
        };
        for key in super::own_keys(expected_object)? {
            let expected_value: Value = expected_object.get(key.as_str())?;
            let actual_value: Value = actual_object.get(key.as_str())?;
            if actual_value.is_undefined() {
                return Ok(false);
            }
            if expected_value.as_object().is_some() && !expected_value.is_array() {
                if !object_match(ctx, &actual_value, &expected_value)? {
                    return Ok(false);
                }
            } else if !super::equal(ctx, &actual_value, &expected_value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    #[rquickjs::function]
    pub fn assertIsError<'js>(
        ctx: Ctx<'js>, error: Value<'js>, error_class: Opt<Value<'js>>,
        msg_matches: Opt<Value<'js>>, msg: Opt<String>,
    ) -> Result<()> {
        if !den_util::instance_of_global(&ctx, &error, "Error")? {
            return fail_with(&ctx, super::message_or(msg, "Expected an Error".into()));
        }
        super::check_error(&ctx, &error, error_class, msg_matches, msg)
    }

    #[rquickjs::function]
    pub fn assertThrows<'js>(
        ctx: Ctx<'js>, func: Function<'js>, class_or_msg: Opt<Value<'js>>,
        includes_or_msg: Opt<Value<'js>>, msg: Opt<String>,
    ) -> Result<Value<'js>> {
        match func.call::<_, Value>(()) {
            Err(Error::Exception) => {
                let thrown = ctx.catch();
                super::check_error(&ctx, &thrown, class_or_msg, includes_or_msg, msg)?;
                Ok(thrown)
            }
            Ok(_) => {
                Err(super::throw_assertion(
                    &ctx,
                    super::message_or(msg, "Expected function to throw".into()),
                ))
            }
            Err(error) => Err(error),
        }
    }

    #[rquickjs::function]
    pub async fn assertRejects<'js>(
        ctx: Ctx<'js>, func: Function<'js>, class_or_msg: Opt<Value<'js>>,
        includes_or_msg: Opt<Value<'js>>, msg: Opt<String>,
    ) -> Result<Value<'js>> {
        let produced: Value = match func.call(()) {
            Ok(value) => value,
            Err(Error::Exception) => {
                drop(ctx.catch());
                return Err(super::throw_assertion(
                    &ctx,
                    super::message_or(msg, "Function did not return a promise".into()),
                ));
            }
            Err(error) => return Err(error),
        };
        let resolved = rquickjs::promise::MaybePromise::from_value(produced)
            .into_future::<Value>()
            .await;
        match resolved {
            Err(Error::Exception) => {
                let thrown = ctx.catch();
                super::check_error(&ctx, &thrown, class_or_msg, includes_or_msg, msg)?;
                Ok(thrown)
            }
            Ok(_) => {
                Err(super::throw_assertion(
                    &ctx,
                    super::message_or(msg, "Expected promise to reject".into()),
                ))
            }
            Err(error) => Err(error),
        }
    }

    #[rquickjs::function]
    pub fn fail(ctx: Ctx<'_>, msg: Opt<String>) -> Result<()> {
        fail_with(&ctx, super::message_or(msg, "Failed assertion".into()))
    }

    #[rquickjs::function]
    pub fn unimplemented(ctx: Ctx<'_>, msg: Opt<String>) -> Result<()> {
        fail_with(&ctx, super::message_or(msg, "unimplemented".into()))
    }

    #[rquickjs::function]
    pub fn unreachable(ctx: Ctx<'_>, msg: Opt<String>) -> Result<()> {
        fail_with(&ctx, super::message_or(msg, "unreachable".into()))
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let Some(proto) = Class::<AssertionError>::prototype(ctx)? else {
            return Ok(());
        };
        let error_ctor: rquickjs::Constructor = ctx.globals().get("Error")?;
        let error_proto: Object = error_ctor.get("prototype")?;
        proto.set_prototype(Some(&error_proto))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/assert.rs"]
mod tests;
