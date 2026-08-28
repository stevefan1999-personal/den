//! The two built-in queuing strategies.

use rquickjs::{
    Coerced, Ctx, Function, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::Opt,
};

fn high_water_mark<'js>(ctx: &Ctx<'js>, init: Opt<Value<'js>>) -> Result<f64> {
    let Some(object) = init.0.and_then(Value::into_object) else {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "a queuing strategy requires { highWaterMark }",
        ));
    };
    Ok(Coerced::<f64>::from(object.get::<_, Coerced<f64>>("highWaterMark")?).0)
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct CountQueuingStrategy<'js> {
    mark: f64,
    size: Function<'js>,
}

impl<'js> Trace<'js> for CountQueuingStrategy<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.size.trace(tracer); }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> CountQueuingStrategy<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
        Ok(Self {
            mark: high_water_mark(&ctx, init)?,
            size: Function::new(ctx, || 1.0)?,
        })
    }

    #[qjs(get)]
    pub fn high_water_mark(&self) -> f64 { self.mark }

    #[qjs(get)]
    pub fn size(&self) -> Function<'js> { self.size.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "CountQueuingStrategy" }
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct ByteLengthQueuingStrategy<'js> {
    mark: f64,
    size: Function<'js>,
}

impl<'js> Trace<'js> for ByteLengthQueuingStrategy<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.size.trace(tracer); }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ByteLengthQueuingStrategy<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
        Ok(Self {
            mark: high_water_mark(&ctx, init)?,
            size: Function::new(ctx, |chunk: rquickjs::Object| -> Result<f64> {
                Ok(chunk.get::<_, Coerced<f64>>("byteLength")?.0)
            })?,
        })
    }

    #[qjs(get)]
    pub fn high_water_mark(&self) -> f64 { self.mark }

    #[qjs(get)]
    pub fn size(&self) -> Function<'js> { self.size.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ByteLengthQueuingStrategy" }
}
