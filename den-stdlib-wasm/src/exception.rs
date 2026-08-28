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
#[path = "../tests/unit/exception.rs"]
mod tests;
