//! `WebAssembly.Global`.

use indexmap::indexmap;
use rquickjs::{
    Coerced, Ctx, Exception, FromJs, IntoJs as _, JsLifetime, Object, Result, Value, class::Trace,
    prelude::Opt,
};
use wasmtime::{Global as WasmGlobal, GlobalType, Mutability, ValType};

use crate::{
    memory::{DescriptorObject as _, ValueTypeName},
    store::Store,
    utils::WasmValue,
};

/// A `GlobalDescriptor`.
#[derive(Clone, Debug)]
pub struct GlobalDescriptor {
    value:   String,
    mutable: bool,
}

impl<'js> FromJs<'js> for GlobalDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the global descriptor")?;
        let mutable = object
            .get::<_, Option<Coerced<bool>>>("mutable")?
            .is_some_and(|mutable| mutable.0);
        let value = object.required::<Coerced<String>>(ctx, "value")?.0;
        Ok(Self { value, mutable })
    }
}

impl GlobalDescriptor {
    /// `ToValueType(descriptor["value"])`.
    fn value_type(&self, ctx: &Ctx<'_>) -> Result<ValType> {
        ValueTypeName::value(ctx, &self.value, "global value type")
    }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class]
pub struct Global {
    #[qjs(skip_trace)]
    pub(crate) inner: WasmGlobal,
}

#[rquickjs::methods]
impl Global {
    #[qjs(constructor)]
    pub fn new<'js>(
        descriptor: GlobalDescriptor, value: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let ty = descriptor.value_type(&ctx)?;
        let initial = match value.0.filter(|value| !value.is_undefined()) {
            Some(value) => WasmValue::from_js(&ctx, &value, &ty)?,
            None => {
                WasmValue::default_for(&ty).map_err(|error| Exception::throw_type(&ctx, error))?
            }
        };

        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            let mutability = if descriptor.mutable {
                Mutability::Var
            } else {
                Mutability::Const
            };
            WasmGlobal::new(store, GlobalType::new(ty, mutability), initial.into_inner())
                .map(|inner| Self { inner })
                .map_err(|error| {
                    Exception::throw_type(&ctx, &format!("cannot create global: {error}"))
                })
        })
    }

    /// `GetGlobalValue(this)`. An `i64` global reads as a `BigInt`; a `v128`
    /// one is a `TypeError`, which [`WasmValue::to_js`] raises.
    #[qjs(get, enumerable, configurable, rename = "value")]
    pub fn get_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let value = Store::from_ctx(&ctx)?
            .with_mut(&ctx, |store| Ok(WasmValue(self.inner.get(&mut *store))))?;
        value.to_js(&ctx)
    }

    /// The setter is where immutability is enforced: wasmtime's own error for
    /// writing a constant global is indistinguishable from a type mismatch,
    /// and the spec wants a `TypeError` raised *before* the write is attempted.
    #[qjs(set, enumerable, configurable, rename = "value")]
    pub fn set_value<'js>(&self, value: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<()> {
        let store = Store::from_ctx(&ctx)?;
        // The type is read under its own short borrow: `ToWebAssemblyValue` below
        // runs arbitrary JS and allocates externrefs in the store, neither of which
        // may happen while the store is borrowed.
        let ty = store.with_mut(&ctx, |store| Ok(self.inner.ty(&*store)))?;
        if !matches!(ty.mutability(), Mutability::Var) {
            return Err(Exception::throw_type(
                &ctx,
                "cannot set the value of an immutable WebAssembly.Global",
            ));
        }
        let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let value = WasmValue::from_js(&ctx, &value, ty.content())?;
        store.with_mut(&ctx, |store| {
            self.inner
                .set(&mut *store, value.into_inner())
                .map_err(|error| {
                    Exception::throw_type(&ctx, &format!("cannot set global: {error}"))
                })
        })
    }

    /// The js-types reflection method: `{ mutable, value }`.
    #[qjs(rename = "type")]
    pub fn global_type<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let declared = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| Ok(self.inner.ty(&*store)))?;
        // Built outside the borrow: a `set` walks the prototype chain, so a setter
        // planted on `Object.prototype` would run JS here.
        let value = ValueTypeName::get(declared.content()).ok_or_else(|| {
            Exception::throw_type(&ctx, "this global's value type has no JS name")
        })?;
        indexmap! {
            "mutable" => matches!(declared.mutability(), Mutability::Var).into_js(&ctx)?,
            "value" => value.into_js(&ctx)?,
        }
        .into_js(&ctx)?
        .into_object()
        .ok_or_else(|| Exception::throw_type(&ctx, "global type is not an object"))
    }

    #[qjs(rename = "valueOf")]
    pub fn value_of<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> { self.get_value(ctx) }
}

#[cfg(test)]
#[path = "../tests/unit/global.rs"]
mod tests;
