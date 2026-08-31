//! `WebAssembly.Table`.

use indexmap::indexmap;
use rquickjs::{
    Coerced, Ctx, Exception, FromJs, IntoJs as _, JsLifetime, Object, Result, Value, class::Trace,
    prelude::Opt,
};
use wasmtime::{Ref, Table as WasmTable, TableType, ValType};

use crate::{
    backend,
    memory::{DescriptorObject as _, ValueTypeName},
    store::Store,
    utils::{EnforceRange, WasmValue},
};

/// A `TableDescriptor`. `element` is kept as written so that the `TypeError`
/// for an unknown element type is raised where the spec raises it — when the
/// table is created.
#[expect(
    clippy::module_name_repetitions,
    reason = "WebAssembly names this dictionary TableDescriptor"
)]
#[derive(Clone, Debug)]
pub struct TableDescriptor {
    initial: u32,
    maximum: Option<u32>,
    element: String,
}

impl<'js> FromJs<'js> for TableDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the table descriptor")?;
        let element = object.required::<Coerced<String>>(ctx, "element")?.0;
        let initial = object.initial_or_minimum(ctx)?;
        let maximum = object
            .get::<_, Option<EnforceRange>>("maximum")?
            .map(|maximum| maximum.0);
        Ok(Self {
            initial,
            maximum,
            element,
        })
    }
}

impl TableDescriptor {
    /// `ToValueType(descriptor["element"])`.
    fn element_type(&self, ctx: &Ctx<'_>) -> Result<ValType> {
        ValueTypeName::table(ctx, &self.element)
    }

    fn validate(&self, ctx: &Ctx<'_>) -> Result<()> {
        if self.maximum.is_some_and(|maximum| self.initial > maximum) {
            return Err(Exception::throw_range(
                ctx,
                "table minimum exceeds its maximum",
            ));
        }
        Ok(())
    }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class]
pub struct Table {
    #[qjs(skip_trace)]
    pub(crate) inner: WasmTable,
}

impl Table {
    /// The table's element type as a `ValType`: `TableType::element` yields a
    /// `&RefType`.
    fn element_type(&self, store: &backend::Store) -> ValType {
        ValType::Ref(self.inner.ty(store).element().clone())
    }

    /// `value` missing means `DefaultValue(elementtype)`, which for both table
    /// element types is a null reference.
    fn element<'js>(ctx: &Ctx<'js>, value: Option<&Value<'js>>, ty: &ValType) -> Result<Ref> {
        let value = match value {
            Some(value) => WasmValue::from_js(ctx, value, ty)?,
            None => {
                WasmValue::default_for(ty).map_err(|error| Exception::throw_type(ctx, error))?
            }
        };
        value
            .into_inner()
            .ref_()
            .ok_or_else(|| Exception::throw_type(ctx, "a table element must be a reference value"))
    }
}

#[rquickjs::methods]
impl Table {
    #[qjs(constructor)]
    pub fn new<'js>(
        descriptor: TableDescriptor, value: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let element = descriptor.element_type(&ctx)?;
        descriptor.validate(&ctx)?;
        let initial = Self::element(&ctx, value.0.as_ref(), &element)?;
        let ValType::Ref(element) = element else {
            return Err(Exception::throw_type(
                &ctx,
                "a table element type must be a reference type",
            ));
        };

        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            WasmTable::new(
                store,
                TableType::new(element, descriptor.initial, descriptor.maximum),
                initial,
            )
            .map(|inner| Self { inner })
            .map_err(|error| {
                Exception::throw_range(&ctx, &format!("cannot allocate table: {error}"))
            })
        })
    }

    #[qjs(get, enumerable, configurable)]
    pub fn length(&self, ctx: Ctx<'_>) -> Result<u64> {
        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| Ok(self.inner.size(&*store)))
    }

    /// The js-types reflection method: `{ element, minimum, maximum? }`, with
    /// `maximum` present only when the table has one.
    #[qjs(rename = "type")]
    pub fn table_type<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let (element, minimum, maximum) = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            let ty = self.inner.ty(&*store);
            Ok((self.element_type(store), ty.minimum(), ty.maximum()))
        })?;
        // The dictionary is built outside the borrow: a `set` walks the prototype
        // chain, so a setter planted on `Object.prototype` would run JS here.
        let element = ValueTypeName::get(&element).ok_or_else(|| {
            Exception::throw_type(&ctx, "this table's element type has no JS name")
        })?;
        let mut ty = indexmap! {
            "element" => element.into_js(&ctx)?,
            "minimum" => minimum.into_js(&ctx)?,
        };
        if let Some(maximum) = maximum {
            ty.insert("maximum", maximum.into_js(&ctx)?);
        }
        ty.into_js(&ctx)?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "table type is not an object"))
    }

    pub fn get<'js>(&self, index: EnforceRange, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let element = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            self.inner.get(&mut *store, index.size()).ok_or_else(|| {
                Exception::throw_range(&ctx, &format!("table index {} is out of range", index.0))
            })
        })?;
        WasmValue(element.into()).to_js(&ctx)
    }

    pub fn set<'js>(
        &self, index: EnforceRange, value: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let store = Store::from_ctx(&ctx)?;
        // The element type is read under its own short borrow. `ToWebAssemblyValue`
        // runs arbitrary JS and allocates externrefs in the store, neither of which
        // may happen while the store is borrowed — and the spec coerces (step 4)
        // *before* the write bounds-checks (step 7), so a bad value is a `TypeError`
        // even when the index is out of range.
        let ty = store.with_mut(&ctx, |store| Ok(self.element_type(store)))?;
        let value = match value.0 {
            Some(value) => Some(value),
            None if matches!(&ty, ValType::Ref(reference) if reference.matches(&wasmtime::RefType::EXTERNREF)) => {
                Some(Value::new_undefined(ctx.clone()))
            }
            None => None,
        };
        let element = Self::element(&ctx, value.as_ref(), &ty)?;
        store.with_mut(&ctx, |store| {
            self.inner
                .set(&mut *store, index.size(), element)
                .map_err(|_error| {
                    Exception::throw_range(
                        &ctx,
                        &format!("table index {} is out of range", index.0),
                    )
                })
        })
    }

    /// Grow by `delta` elements, returning the length *before* the growth.
    pub fn grow<'js>(
        &self, delta: EnforceRange, value: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<u64> {
        let store = Store::from_ctx(&ctx)?;
        // Same order as `set`, and for the same reason: the filler is coerced
        // before the table is asked to grow.
        let ty = store.with_mut(&ctx, |store| Ok(self.element_type(store)))?;
        let element = Self::element(&ctx, value.0.as_ref(), &ty)?;
        store.with_mut(&ctx, |store| {
            self.inner
                .grow(&mut *store, delta.size(), element)
                .map_err(|error| {
                    Exception::throw_range(&ctx, &format!("cannot grow table: {error}"))
                })
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/table.rs"]
mod tests;
