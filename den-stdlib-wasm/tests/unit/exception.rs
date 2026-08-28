use rquickjs::FromJs as _;

use super::*;
use crate::{
    memory::testing::{js, pending_error_name, with_wasm_context},
    tag::TagType,
};

/// The name of the JS error a construction attempt failed with.
fn error_of<T>(created: core::result::Result<T, String>) -> String {
    created.err().unwrap_or_else(|| "no error".to_owned())
}

/// Both the tag and an exception carrying `payload`, or the name of the JS
/// error that stopped us.
fn exception<'js>(
    ctx: &Ctx<'js>, parameters: &str, payload: &str, options: Option<&str>,
) -> core::result::Result<(Class<'js, Tag>, Exception<'js>), String> {
    (|| {
        let ty = TagType::from_js(ctx, js(ctx, parameters))?;
        let tag = Class::instance(ctx.clone(), Tag::new(ty, ctx.clone())?)?;
        let payload = Vec::<Value>::from_js(ctx, js(ctx, payload))?;
        let options = options.map(|options| {
            Object::from_js(ctx, js(ctx, options)).expect("the options are an object")
        });
        let exception = Exception::new(tag.clone(), payload, Opt(options), ctx.clone())?;
        Ok((tag, exception))
    })()
    .map_err(|_error: rquickjs::Error| pending_error_name(ctx))
}

#[test]
fn get_arg_returns_the_payload_value_for_the_matching_tag() {
    with_wasm_context(|ctx| {
        let created = exception(ctx, "({ parameters: ['i32', 'i64'] })", "[7, 8n]", None);
        let (tag, exception) = created.expect("exception");

        assert!(exception.is(tag.clone()));
        let first = exception
            .get_arg(tag.clone(), EnforceRange(0), ctx.clone())
            .expect("first argument");
        assert_eq!(first.as_int(), Some(7));

        let _ = exception
            .get_arg(tag, EnforceRange(2), ctx.clone())
            .expect_err("out of range");
        assert_eq!(pending_error_name(ctx), "RangeError");
    })
}

#[test]
fn another_tag_neither_matches_nor_can_read_the_payload() {
    with_wasm_context(|ctx| {
        let created = exception(ctx, "({ parameters: ['i32'] })", "[1]", None);
        let (_, thrown) = created.expect("exception");
        let (other, _) = exception(ctx, "({ parameters: ['i32'] })", "[1]", None)
            .expect("a second, unrelated tag");

        assert!(!thrown.is(other.clone()));
        let _ = thrown
            .get_arg(other, EnforceRange(0), ctx.clone())
            .expect_err("wrong tag");
        assert_eq!(pending_error_name(ctx), "TypeError");
    })
}

#[test]
fn a_tag_that_is_already_in_use_is_a_type_error_rather_than_a_panic() {
    with_wasm_context(|ctx| {
        let (tag, _) = exception(ctx, "({ parameters: [] })", "[]", None).expect("exception");
        // Stands in for whatever else holds the class cell — a `Tag` method taking
        // `&mut self`, say — which `Class::borrow` would answer with an abort.
        let held = tag.borrow_mut();

        let refused = Exception::new(tag.clone(), Vec::new(), Opt(None), ctx.clone());
        assert!(refused.is_err());
        assert_eq!(pending_error_name(ctx), "TypeError");
        drop(held);
    })
}

#[test]
fn a_payload_of_the_wrong_length_is_a_type_error() {
    with_wasm_context(|ctx| {
        for (parameters, payload) in [
            ("({ parameters: ['i32'] })", "[]"),
            ("({ parameters: ['i32'] })", "[1, 2]"),
            // The payload is coerced by the tag's types, so a Number is not an i64.
            ("({ parameters: ['i64'] })", "[1]"),
        ] {
            assert_eq!(
                error_of(exception(ctx, parameters, payload, None)),
                "TypeError",
                "{parameters} {payload}"
            );
        }
    })
}

#[test]
fn the_stack_is_only_captured_when_trace_stack_is_asked_for() {
    with_wasm_context(|ctx| {
        let (_, plain) = exception(ctx, "({ parameters: [] })", "[]", None).expect("exception");
        assert_eq!(plain.stack(), None);

        let (_, traced) = exception(
            ctx,
            "({ parameters: [] })",
            "[]",
            Some("({ traceStack: true })"),
        )
        .expect("exception");
        // The stack is the current JS call stack, and this exception is built straight
        // from Rust, so it can be empty — the point is that *a* string was captured.
        assert!(traced.stack().is_some());
    })
}
