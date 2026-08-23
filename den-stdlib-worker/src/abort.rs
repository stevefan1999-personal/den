//! DOM `AbortController` / `AbortSignal` as `#[rquickjs::class]` types.
//!
//! `AbortSignal.prototype` is reparented onto `EventTarget.prototype` so
//! `signal instanceof EventTarget` holds. Listener state lives on the hidden
//! EventTarget slot [`crate::events::EventTarget::resolve`] attaches.

use rquickjs::{
    Class, Ctx, Exception, Function, IntoJs, JsLifetime, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{FuncArg, Opt, This},
};

use crate::events::{Event, EventTarget, define_event_handler, inherit, new_dom_exception};

const ABORT_MESSAGE: &str = "This operation was aborted";
const TIMEOUT_MESSAGE: &str = "The operation was aborted due to timeout";

fn default_reason<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    new_dom_exception(ctx, ABORT_MESSAGE, "AbortError")
}

fn timeout_reason<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    new_dom_exception(ctx, TIMEOUT_MESSAGE, "TimeoutError")
}

fn is_aborted(source: &Value<'_>) -> Result<bool> {
    let Some(object) = source.as_object() else {
        return Ok(false);
    };
    Ok(object.get::<_, bool>("aborted").unwrap_or(false))
}

fn source_reason<'js>(ctx: &Ctx<'js>, source: &Value<'js>) -> Result<Value<'js>> {
    let Some(object) = source.as_object() else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    object.get("reason")
}

fn collect_sources<'js>(ctx: &Ctx<'js>, signals: Value<'js>) -> Result<Vec<Value<'js>>> {
    let Some(array) = signals.as_array() else {
        return Err(Exception::throw_type(
            ctx,
            "AbortSignal.any: signals is not iterable",
        ));
    };
    let mut sources = Vec::with_capacity(array.len());
    for index in 0..array.len() {
        sources.push(array.get(index)?);
    }
    Ok(sources)
}

fn add_listener<'js>(source: &Value<'js>, listener: &Function<'js>) -> Result<()> {
    let Some(object) = source.as_object() else {
        return Ok(());
    };
    let Ok(add) = object.get::<_, Function<'js>>("addEventListener") else {
        return Ok(());
    };
    add.call::<_, ()>((This(source.clone()), "abort", listener.clone()))
}

fn remove_listener<'js>(source: &Value<'js>, listener: &Function<'js>) {
    let Some(object) = source.as_object() else {
        return;
    };
    let Ok(remove) = object.get::<_, Function<'js>>("removeEventListener") else {
        return;
    };
    let _ = remove.call::<_, ()>((This(source.clone()), "abort", listener.clone()));
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct AbortSignal<'js> {
    aborted: bool,
    reason:  Value<'js>,
}

impl<'js> AbortSignal<'js> {
    fn fresh(ctx: &Ctx<'js>) -> Self {
        Self {
            aborted: false,
            reason:  Value::new_undefined(ctx.clone()),
        }
    }

    fn abort_inner(
        ctx: &Ctx<'js>, this: &Class<'js, Self>, reason: Option<Value<'js>>, dispatch: bool,
    ) -> Result<bool> {
        if this.try_borrow()?.aborted {
            return Ok(false);
        }
        let reason = match reason {
            Some(value) if !value.is_undefined() => value,
            _ => default_reason(ctx)?,
        };
        {
            let mut signal = this.try_borrow_mut()?;
            signal.aborted = true;
            signal.reason = reason;
        }
        if dispatch {
            let event = Class::instance(
                ctx.clone(),
                Event::new(ctx.clone(), "abort".into_js(ctx)?, Opt(None))?,
            )?;
            EventTarget::dispatch(ctx, this.as_inner().as_value(), event.into_value(), false)?;
        }
        Ok(true)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> AbortSignal<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Self { Self::fresh(&ctx) }

    #[qjs(get)]
    pub fn aborted(&self) -> bool { self.aborted }

    #[qjs(get)]
    pub fn reason(&self) -> Value<'js> { self.reason.clone() }

    pub fn throw_if_aborted(&self, ctx: Ctx<'js>) -> Result<()> {
        if self.aborted {
            return Err(ctx.throw(self.reason.clone()));
        }
        Ok(())
    }

    #[qjs(static)]
    pub fn abort(ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<Class<'js, Self>> {
        let signal = Class::instance(ctx.clone(), Self::fresh(&ctx))?;
        Self::abort_inner(&ctx, &signal, reason.0, false)?;
        Ok(signal)
    }

    #[qjs(static)]
    pub fn timeout(ctx: Ctx<'js>, ms: Value<'js>) -> Result<Class<'js, Self>> {
        let signal = Class::instance(ctx.clone(), Self::fresh(&ctx))?;
        let set_timeout: Function<'js> = ctx.globals().get("setTimeout")?;
        let callback = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, function: FuncArg<Function<'js>>| -> Result<()> {
                let aborting: Class<'js, Self> = function.0.get("_signal")?;
                let reason = timeout_reason(&ctx)?;
                Self::abort_inner(&ctx, &aborting, Some(reason), true)?;
                Ok(())
            },
        )?;
        callback.set("_signal", signal.clone())?;
        set_timeout.call::<_, ()>((callback, ms))?;
        Ok(signal)
    }

    #[qjs(static)]
    pub fn any(ctx: Ctx<'js>, signals: Value<'js>) -> Result<Class<'js, Self>> {
        let combined = Class::instance(ctx.clone(), Self::fresh(&ctx))?;
        let sources = collect_sources(&ctx, signals)?;
        for source in &sources {
            if is_aborted(source)? {
                Self::abort_inner(&ctx, &combined, Some(source_reason(&ctx, source)?), false)?;
                return Ok(combined);
            }
        }
        let listener = Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, function: FuncArg<Function<'js>>| -> Result<()> {
                let combined: Class<'js, Self> = function.0.get("_combined")?;
                let sources: Vec<Value<'js>> = function.0.get("_sources")?;
                for source in &sources {
                    if !is_aborted(source)? {
                        continue;
                    }
                    if Self::abort_inner(&ctx, &combined, Some(source_reason(&ctx, source)?), true)?
                    {
                        for other in &sources {
                            remove_listener(other, &function.0);
                        }
                    }
                    break;
                }
                Ok(())
            },
        )?;
        listener.set("_combined", combined.clone())?;
        listener.set("_sources", sources.clone())?;
        for source in &sources {
            add_listener(source, &listener)?;
        }
        Ok(combined)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "AbortSignal" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct AbortController<'js> {
    signal: Class<'js, AbortSignal<'js>>,
}

#[rquickjs::methods]
impl<'js> AbortController<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self {
            signal: Class::instance(ctx.clone(), AbortSignal::fresh(&ctx))?,
        })
    }

    #[qjs(get)]
    pub fn signal(&self) -> Class<'js, AbortSignal<'js>> { self.signal.clone() }

    pub fn abort(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<()> {
        AbortSignal::abort_inner(&ctx, &self.signal, reason.0, true)?;
        Ok(())
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "AbortController" }
}

/// Prototype chain and `onabort`.
pub fn finish<'js>(ctx: &Ctx<'js>) -> Result<()> {
    inherit::<AbortSignal, EventTarget>(ctx)?;
    if let Some(proto) = Class::<AbortSignal>::prototype(ctx)? {
        define_event_handler(ctx.clone(), proto, "onabort".to_owned(), Opt(None))?;
    }
    Ok(())
}
