//! The two built-in queuing strategies.

use rquickjs::{
    Coerced, Ctx, Function, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::Opt,
};

/// `QueuingStrategyInit.highWaterMark` is a required WebIDL member: a missing
/// one is a TypeError, not a silent NaN.
fn high_water_mark<'js>(ctx: &Ctx<'js>, init: Opt<Value<'js>>) -> Result<f64> {
    let Some(object) = init.0.filter(Value::is_object).and_then(Value::into_object) else {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "a queuing strategy requires { highWaterMark }",
        ));
    };
    let mark: Value = object.get("highWaterMark")?;
    if mark.is_undefined() {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "highWaterMark is required",
        ));
    }
    Ok(Coerced::<f64>::from_js(ctx, mark)?.0)
}

use rquickjs::FromJs as _;

macro_rules! queuing_strategy {
    ($name:ident, $tag:literal, $size:path) => {
        #[derive(JsLifetime)]
        #[rquickjs::class]
        pub struct $name<'js> {
            mark: f64,
            size: Function<'js>,
        }

        impl<'js> Trace<'js> for $name<'js> {
            fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.size.trace(tracer); }
        }

        #[rquickjs::methods(rename_all = "camelCase")]
        impl<'js> $name<'js> {
            #[qjs(constructor)]
            pub fn new(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
                Ok(Self {
                    mark: high_water_mark(&ctx, init)?,
                    size: $size(&ctx)?,
                })
            }

            #[qjs(get)]
            pub const fn high_water_mark(&self) -> f64 { self.mark }

            #[qjs(get)]
            pub fn size(&self) -> Function<'js> { self.size.clone() }

            #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
            pub const fn to_string_tag() -> &'static str { $tag }
        }
    };
}

queuing_strategy!(
    CountQueuingStrategy,
    "CountQueuingStrategy",
    crate::streams::count_size
);
queuing_strategy!(
    ByteLengthQueuingStrategy,
    "ByteLengthQueuingStrategy",
    crate::streams::byte_length_size
);
