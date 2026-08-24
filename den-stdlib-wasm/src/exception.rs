//! `WebAssembly.Exception` — a thrown wasm exception, seen from JS.
//!
//! The payload is held on the JS side rather than as a store-allocated
//! exception object: nothing in den can throw a wasm exception into or out of
//! a module yet (that needs `JSTag` and the host-function propagation rule),
//! and every member the interface exposes — `getArg`, `is`, `stack` — is
//! answerable from the tag and the payload alone.

use rquickjs::{
    Class, Coerced, Constructor, Ctx, Exception as JsException, JsLifetime, Object, Result, Value,
    class::Trace, prelude::Opt,
};

use crate::{
    tag::Tag,
    utils::{EnforceRange, WasmValue},
};

#[derive(Trace)]
#[rquickjs::class]
pub struct Exception<'js> {
    tag:     Class<'js, Tag>,
    #[qjs(skip_trace)]
    payload: Vec<WasmValue>,
    #[qjs(skip_trace)]
    stack:   Option<String>,
}

// SAFETY: the only `'js` data is the `Class` handle, which changes lifetime
// with the struct.
unsafe impl<'js> JsLifetime<'js> for Exception<'js> {
    type Changed<'to> = Exception<'to>;
}

#[rquickjs::methods]
impl<'js> Exception<'js> {
    #[qjs(constructor)]
    pub fn new(
        tag: Class<'js, Tag>, payload: Vec<Value<'js>>, options: Opt<Object<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        // `borrow` would abort the process if anything else held the cell; a JS
        // constructor argument is never allowed to reach a panic, however hard the
        // outstanding `borrow_mut` is to arrange today.
        let parameters = tag
            .try_borrow()
            .map_err(|_| JsException::throw_type(&ctx, "this WebAssembly.Tag is already in use"))?
            .parameters(&ctx)?;
        if parameters.len() != payload.len() {
            return Err(JsException::throw_type(
                &ctx,
                &format!(
                    "this tag takes {} payload values, but {} were given",
                    parameters.len(),
                    payload.len()
                ),
            ));
        }

        let payload = parameters
            .iter()
            .zip(&payload)
            .map(|(ty, value)| WasmValue::from_js(&ctx, value, ty))
            .collect::<Result<Vec<_>>>()?;

        // `traceStack` asks for a stack "in an implementation-defined format"; the one
        // JS itself would have produced is the only one worth having.
        let trace_stack = options
            .0
            .map(|options| options.get::<_, Option<Coerced<bool>>>("traceStack"))
            .transpose()?
            .flatten()
            .is_some_and(|trace_stack| trace_stack.0);
        let stack = trace_stack
            .then(|| {
                ctx.globals()
                    .get::<_, Constructor>("Error")?
                    .construct::<_, Object>(())?
                    .get::<_, Option<String>>("stack")
            })
            .transpose()?
            .flatten();

        Ok(Self {
            tag,
            payload,
            stack,
        })
    }

    /// The `index`th payload value, but only for the tag this exception was
    /// thrown with: a `Tag` is what proves the caller knows the payload's
    /// types.
    ///
    /// The index is the IDL's `[EnforceRange] unsigned long`, so `NaN` and
    /// `-1` are the `TypeError` WebIDL raises before `getArg`'s own steps run,
    /// not the index `0` and the `RangeError` `Coerced<u64>` used to make of
    /// them.
    #[qjs(rename = "getArg")]
    pub fn get_arg(
        &self, tag: Class<'js, Tag>, index: EnforceRange, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        if !self.is(tag) {
            return Err(JsException::throw_type(
                &ctx,
                "this exception was not thrown with the given tag",
            ));
        }
        let index = usize::try_from(index.0)
            .ok()
            .filter(|index| *index < self.payload.len());
        match index {
            Some(index) => self.payload[index].to_js(&ctx),
            None => {
                Err(JsException::throw_range(
                    &ctx,
                    "the payload index is out of range",
                ))
            }
        }
    }

    pub fn is(&self, tag: Class<'js, Tag>) -> bool { self.tag == tag }

    /// A string when the exception was created with `traceStack`, `undefined`
    /// otherwise.
    #[qjs(get, enumerable)]
    pub fn stack(&self) -> Option<String> { self.stack.clone() }
}

#[cfg(test)]
mod tests {
    use rquickjs::FromJs;

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
        .map_err(|_: rquickjs::Error| pending_error_name(ctx))
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
}
