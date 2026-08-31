//! Live `process.env` as a Proxy whose traps read and write the real
//! environment.

use rquickjs::{
    Ctx, FromJs as _, Function, IntoJs as _, Object, Proxy, Result, Value,
    atom::PredefinedAtom,
    prelude::*,
    proxy::{ProxyHandler, ProxyProperty, ProxyReceiver, ProxyTarget},
};

/// Environment variables as seen by `process.env`.
pub struct Env;

impl Env {
    pub fn get(name: &str) -> Option<String> { std::env::var(name).ok() }

    pub fn set(name: &str, value: &str) {
        // SAFETY: `process.env` is a process-wide map, the same contract Node
        // and txiki expose. Writes from JS are serialized by the realm's event
        // loop; other threads racing `getenv` is inherent to the API.
        unsafe { std::env::set_var(name, value) }
    }

    pub fn unset(name: &str) {
        // SAFETY: same contract as [`Env::set`].
        unsafe { std::env::remove_var(name) }
    }

    pub fn keys() -> Vec<String> { std::env::vars().map(|(name, _)| name).collect() }

    /// A Proxy whose get/set/delete/ownKeys talk to the process environment.
    pub fn proxy<'js>(ctx: Ctx<'js>) -> Result<Proxy<'js>> {
        let handler = ProxyHandler::new(ctx.clone())?
            .with_getter(
                |_target: ProxyTarget<'js>,
                 property: ProxyProperty<'js>,
                 _receiver: ProxyReceiver<'js>| {
                    if !property.is_string() {
                        return Ok(None);
                    }
                    Ok(Self::get(&property.to_string()?))
                },
            )?
            .with_setter(
                |_target: ProxyTarget<'js>,
                 property: ProxyProperty<'js>,
                 value: Value<'js>,
                 _receiver: ProxyReceiver<'js>| {
                    if !property.is_string() {
                        return Ok(true);
                    }
                    let name = property.to_string()?;
                    let ctx = value.ctx().clone();
                    let Coerced(text) = Coerced::<String>::from_js(&ctx, value)?;
                    Self::set(&name, &text);
                    Ok(true)
                },
            )?
            .with_has(|_target: ProxyTarget<'js>, property: ProxyProperty<'js>| {
                if !property.is_string() {
                    return Ok(false);
                }
                Ok(Self::get(&property.to_string()?).is_some())
            })?
            .with_delete(|_target: ProxyTarget<'js>, property: ProxyProperty<'js>| {
                if property.is_string() {
                    Self::unset(&property.to_string()?);
                }
                Ok(true)
            })?;

        // `ownKeys` / `getOwnPropertyDescriptor` are not on the builder; without
        // them `Object.keys(process.env)` would walk an empty target.
        let handler = Object::from_value(handler.into_js(&ctx)?)?;
        handler.set(
            PredefinedAtom::OwnKeys,
            Function::new(ctx.clone(), |_target: ProxyTarget<'js>| Self::keys())?,
        )?;
        handler.set(
            PredefinedAtom::GetOwnPropertyDescriptor,
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, _target: ProxyTarget<'js>, property: ProxyProperty<'js>| {
                    Self::descriptor(ctx, property)
                },
            )?,
        )?;

        Proxy::new(
            ctx.clone(),
            Object::new(ctx)?,
            ProxyHandler::from_object(handler)?,
        )
    }

    fn descriptor<'js>(ctx: Ctx<'js>, property: ProxyProperty<'js>) -> Result<Value<'js>> {
        if !property.is_string() {
            return Ok(Value::new_undefined(ctx));
        }
        let Some(value) = Self::get(&property.to_string()?) else {
            return Ok(Value::new_undefined(ctx));
        };
        let descriptor = Object::new(ctx)?;
        descriptor.set("value", value)?;
        descriptor.set("writable", true)?;
        descriptor.set("enumerable", true)?;
        descriptor.set("configurable", true)?;
        Ok(descriptor.into_value())
    }
}
