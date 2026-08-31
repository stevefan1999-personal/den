//! ProgressEvent and CloseEvent. Constructors return a real JS `Event` so
//! `dispatchEvent` in den:worker still sees EVENT_STATE; the class wrapper
//! then sets `[[Prototype]]` to this class.

use den_util::coerce_string;
use rquickjs::{
    Ctx, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace, function::Opt,
};

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct ProgressEvent {}

#[rquickjs::methods]
impl ProgressEvent {
    #[qjs(constructor)]
    pub fn construct<'js>(
        ctx: Ctx<'js>, type_: String, options: Opt<Object<'js>>,
    ) -> Result<Object<'js>> {
        let event = if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("Event")
        {
            ctor.construct::<_, Object>((type_.as_str(), options.0.clone()))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("type", type_.as_str())?;
            object
        };
        let (length_computable, loaded, total) = Self::fields(options.0.as_ref());
        event.set("lengthComputable", length_computable)?;
        event.set("loaded", loaded)?;
        event.set("total", total)?;
        Ok(event)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "ProgressEvent" }

    #[qjs(skip)]
    fn fields(options: Option<&Object<'_>>) -> (bool, f64, f64) {
        let Some(options) = options else {
            return (false, 0.0, 0.0);
        };
        let length_computable = options
            .get::<_, Value>("lengthComputable")
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let loaded = Self::number(options, "loaded");
        let total = Self::number(options, "total");
        (length_computable, loaded, total)
    }

    #[qjs(skip)]
    fn number(options: &Object<'_>, name: &str) -> f64 {
        options
            .get::<_, Value>(name)
            .ok()
            .and_then(|value| value.as_number())
            .filter(|number| number.is_finite())
            .unwrap_or(0.0)
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct CloseEvent {}

#[rquickjs::methods]
impl CloseEvent {
    #[qjs(constructor)]
    pub fn construct<'js>(
        ctx: Ctx<'js>, type_: String, options: Opt<Object<'js>>,
    ) -> Result<Object<'js>> {
        let event = if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("Event")
        {
            ctor.construct::<_, Object>((type_.as_str(), options.0.clone()))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("type", type_.as_str())?;
            object
        };
        let (code, reason, was_clean) = Self::fields(options.0.as_ref());
        event.set("code", code)?;
        event.set("reason", reason)?;
        event.set("wasClean", was_clean)?;
        Ok(event)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "CloseEvent" }

    #[qjs(skip)]
    fn fields(options: Option<&Object<'_>>) -> (f64, String, bool) {
        let Some(options) = options else {
            return (0.0, String::new(), false);
        };
        let code = options
            .get::<_, Value>("code")
            .ok()
            .and_then(|value| {
                if value.is_undefined() {
                    None
                } else {
                    value.as_number()
                }
            })
            .filter(|number| number.is_finite())
            .unwrap_or(0.0);
        let reason = options
            .get::<_, Value>("reason")
            .ok()
            .and_then(|value| {
                if value.is_undefined() {
                    None
                } else {
                    coerce_string(options.ctx(), value).ok()
                }
            })
            .unwrap_or_default();
        let was_clean = options
            .get::<_, Value>("wasClean")
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        (code, reason, was_clean)
    }
}
