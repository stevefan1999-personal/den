//! ProgressEvent and CloseEvent. Constructors return a real JS `Event` so
//! `dispatchEvent` in den:worker still sees EVENT_STATE; the class wrapper
//! then sets `[[Prototype]]` to this class.

use den_util::coerce_string;
use rquickjs::{
    Coerced, Ctx, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    function::Opt,
};

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct ProgressEvent {}

#[rquickjs::methods]
impl ProgressEvent {
    #[qjs(constructor)]
    pub fn construct<'js>(
        ctx: Ctx<'js>, type_: String, options: Opt<Option<Object<'js>>>,
    ) -> Result<Object<'js>> {
        let options = options.0.flatten();
        let event = if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("Event")
        {
            ctor.construct::<_, Object>((type_.as_str(), options.clone()))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("type", type_.as_str())?;
            object
        };
        let (length_computable, loaded, total) = Self::fields(options.as_ref())?;
        event.set("lengthComputable", length_computable)?;
        event.set("loaded", loaded)?;
        event.set("total", total)?;
        Ok(event)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "ProgressEvent" }

    #[qjs(skip)]
    fn fields(options: Option<&Object<'_>>) -> Result<(bool, f64, f64)> {
        let Some(options) = options else {
            return Ok((false, 0.0, 0.0));
        };
        let length_computable = options.get::<_, Coerced<bool>>("lengthComputable")?.0;
        let loaded = Self::number(options, "loaded")?;
        let total = Self::number(options, "total")?;
        Ok((length_computable, loaded, total))
    }

    #[qjs(skip)]
    fn number(options: &Object<'_>, name: &str) -> Result<f64> {
        let number = options.get::<_, Coerced<f64>>(name)?.0;
        Ok(if number.is_finite() { number } else { 0.0 })
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct CloseEvent {}

#[rquickjs::methods]
impl CloseEvent {
    #[qjs(constructor)]
    pub fn construct<'js>(
        ctx: Ctx<'js>, type_: String, options: Opt<Option<Object<'js>>>,
    ) -> Result<Object<'js>> {
        let options = options.0.flatten();
        let event = if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("Event")
        {
            ctor.construct::<_, Object>((type_.as_str(), options.clone()))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("type", type_.as_str())?;
            object
        };
        let (code, reason, was_clean) = Self::fields(options.as_ref())?;
        event.set("code", code)?;
        event.set("reason", reason)?;
        event.set("wasClean", was_clean)?;
        Ok(event)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "CloseEvent" }

    #[qjs(skip)]
    fn fields(options: Option<&Object<'_>>) -> Result<(f64, String, bool)> {
        let Some(options) = options else {
            return Ok((0.0, String::new(), false));
        };
        let code = options.get::<_, Coerced<f64>>("code")?.0;
        let code = if code.is_finite() { code } else { 0.0 };
        let reason: Value = options.get("reason")?;
        let reason = if reason.is_undefined() {
            String::new()
        } else {
            coerce_string(options.ctx(), reason)?
        };
        let was_clean = options.get::<_, Coerced<bool>>("wasClean")?.0;
        Ok((code, reason, was_clean))
    }
}
