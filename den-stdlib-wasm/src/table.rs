//! `WebAssembly.Table`.

use derive_more::derive::{From, Into};
use rquickjs::{
    Ctx, Exception, FromJs, JsLifetime, Object, Result, Value, class::Trace, prelude::Opt,
};

use crate::{
    backend,
    memory::{DescriptorObject, ValueTypeName},
    store::Store,
    utils::{EnforceRange, WasmValue},
};

/// A `TableDescriptor`. `element` is kept as written so that the `TypeError`
/// for an unknown element type is raised where the spec raises it — when the
/// table is created.
#[derive(Clone, Debug)]
pub struct TableDescriptor {
    initial: u32,
    maximum: Option<u32>,
    element: String,
}

impl<'js> FromJs<'js> for TableDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the table descriptor")?;
        Ok(Self {
            initial: object.required::<EnforceRange>(ctx, "initial")?.0,
            maximum: object
                .get::<_, Option<EnforceRange>>("maximum")?
                .map(|maximum| maximum.0),
            element: object.required(ctx, "element")?,
        })
    }
}

impl TableDescriptor {
    /// `ToValueType(descriptor["element"])`.
    fn element_type(&self, ctx: &Ctx<'_>) -> Result<backend::ValType> {
        ValueTypeName::resolve(
            ctx,
            &self.element,
            ValueTypeName::TABLE_ELEMENT,
            "table element type",
        )
    }
}

/// The element representation the active backend's table API speaks: wasmtime
/// takes and returns `Ref`, wasmi takes and returns `Val`. Everything else
/// about the two APIs matches, so this is the whole of the per-backend code in
/// this file.
struct TableElement(
    #[cfg(feature = "wasmtime")] ::wasmtime::Ref,
    #[cfg(not(feature = "wasmtime"))] backend::Val,
);

#[cfg(feature = "wasmtime")]
impl TableElement {
    fn from_wasm_value(ctx: &Ctx<'_>, value: backend::Val) -> Result<Self> {
        value
            .ref_()
            .map(Self)
            .ok_or_else(|| Exception::throw_type(ctx, "a table element must be a reference value"))
    }

    fn into_wasm_value(self) -> backend::Val {
        self.0.into()
    }
}

#[cfg(not(feature = "wasmtime"))]
impl TableElement {
    fn from_wasm_value(_ctx: &Ctx<'_>, value: backend::Val) -> Result<Self> {
        Ok(Self(value))
    }

    fn into_wasm_value(self) -> backend::Val {
        self.0
    }
}

#[derive(Trace, JsLifetime, Clone, Debug, From, Into)]
#[rquickjs::class]
pub struct Table {
    #[qjs(skip_trace)]
    pub(crate) inner: backend::Table,
}

impl Table {
    /// The table's element type as a backend `ValType`: wasmtime's
    /// `TableType::element` yields a `&RefType`, wasmi's yields a `ValType`.
    #[cfg(feature = "wasmtime")]
    fn element_type(&self, store: &backend::Store) -> backend::ValType {
        backend::ValType::Ref(self.inner.ty(store).element().clone())
    }

    #[cfg(not(feature = "wasmtime"))]
    fn element_type(&self, store: &backend::Store) -> backend::ValType {
        self.inner.ty(store).element()
    }

    /// `value` missing means `DefaultValue(elementtype)`, which for both table
    /// element types is a null reference.
    fn element<'js>(
        ctx: &Ctx<'js>,
        value: Option<&Value<'js>>,
        ty: &backend::ValType,
    ) -> Result<TableElement> {
        let value = match value {
            Some(value) => WasmValue::from_js(ctx, value, ty)?,
            None => {
                WasmValue::default_for(ty)
                    .map_err(|error| Exception::throw_type(ctx, &error.to_string()))?
            }
        };
        TableElement::from_wasm_value(ctx, value.into_inner())
    }
}

#[rquickjs::methods]
impl Table {
    #[qjs(constructor)]
    pub fn new<'js>(
        descriptor: TableDescriptor,
        value: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let element = descriptor.element_type(&ctx)?;
        let initial = Self::element(&ctx, value.0.as_ref(), &element)?;

        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            backend::new_table(
                store,
                &element,
                descriptor.initial,
                descriptor.maximum,
                Some(initial.into_wasm_value()),
            )
            .map(Self::from)
            .map_err(|error| {
                Exception::throw_range(&ctx, &format!("cannot allocate table: {error}"))
            })
        })
    }

    #[qjs(get, enumerable)]
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
        let element = backend::val_type_name(&element).ok_or_else(|| {
            Exception::throw_type(&ctx, "this table's element type has no JS name")
        })?;
        let ty = Object::new(ctx.clone())?;
        ty.set("element", element)?;
        ty.set("minimum", minimum)?;
        if let Some(maximum) = maximum {
            ty.set("maximum", maximum)?;
        }
        Ok(ty)
    }

    pub fn get<'js>(&self, index: EnforceRange, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let element = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            self.inner
                .get(&mut *store, index.size())
                .map(TableElement)
                .ok_or_else(|| {
                    Exception::throw_range(
                        &ctx,
                        &format!("table index {} is out of range", index.0),
                    )
                })
        })?;
        WasmValue::from(element.into_wasm_value()).to_js(&ctx)
    }

    pub fn set<'js>(
        &self,
        index: EnforceRange,
        value: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let store = Store::from_ctx(&ctx)?;
        // The element type is read under its own short borrow. `ToWebAssemblyValue`
        // runs arbitrary JS and allocates externrefs in the store, neither of which
        // may happen while the store is borrowed — and the spec coerces (step 4)
        // *before* the write bounds-checks (step 7), so a bad value is a `TypeError`
        // even when the index is out of range.
        let ty = store.with_mut(&ctx, |store| Ok(self.element_type(store)))?;
        let element = Self::element(&ctx, value.0.as_ref(), &ty)?;
        store.with_mut(&ctx, |store| {
            self.inner
                .set(&mut *store, index.size(), element.0)
                .map_err(|_| {
                    Exception::throw_range(
                        &ctx,
                        &format!("table index {} is out of range", index.0),
                    )
                })
        })
    }

    /// Grow by `delta` elements, returning the length *before* the growth.
    pub fn grow<'js>(
        &self,
        delta: EnforceRange,
        value: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<u64> {
        let store = Store::from_ctx(&ctx)?;
        // Same order as `set`, and for the same reason: the filler is coerced
        // before the table is asked to grow.
        let ty = store.with_mut(&ctx, |store| Ok(self.element_type(store)))?;
        let element = Self::element(&ctx, value.0.as_ref(), &ty)?;
        store.with_mut(&ctx, |store| {
            self.inner
                .grow(&mut *store, delta.size(), element.0)
                .map_err(|error| {
                    Exception::throw_range(&ctx, &format!("cannot grow table: {error}"))
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::Function;

    use super::*;
    use crate::{
        memory::testing::{js, pending_error_name, with_wasm_context},
        utils::HostReferences,
    };

    /// A host function wrapped as the JS API's Exported Function, which is the
    /// only thing a `funcref` may hold.
    fn exported_function<'js>(ctx: &Ctx<'js>) -> Function<'js> {
        let store = Store::from_ctx(ctx).expect("store");
        let func = store
            .with_mut(ctx, |store| {
                Ok(backend::Func::wrap(&mut *store, |value: i32| value + 1))
            })
            .expect("host function");
        HostReferences::exported_function(ctx, func, None).expect("exported function")
    }

    fn table(ctx: &Ctx<'_>, descriptor: &str) -> core::result::Result<Table, String> {
        TableDescriptor::from_js(ctx, js(ctx, descriptor))
            .and_then(|descriptor| Table::new(descriptor, Opt(None), ctx.clone()))
            .map_err(|_| pending_error_name(ctx))
    }

    #[test]
    fn an_anyfunc_table_is_created_full_of_null_function_references() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'anyfunc', initial: 2 })").expect("funcref table");
            assert_eq!(table.length(ctx.clone()).unwrap(), 2);
            for index in 0..2 {
                let element = table
                    .get(EnforceRange(index), ctx.clone())
                    .expect("in range");
                assert!(element.is_null(), "element {index} is not a null reference");
            }
        })
    }

    #[test]
    fn set_and_get_round_trip_a_null_reference() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'externref', initial: 1 })").expect("table");
            table
                .set(EnforceRange(0), Opt(Some(js(ctx, "null"))), ctx.clone())
                .expect("null is a valid externref");
            assert!(table.get(EnforceRange(0), ctx.clone()).unwrap().is_null());
        })
    }

    #[test]
    fn indexing_past_the_end_is_a_range_error_in_both_directions() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'anyfunc', initial: 1 })").expect("table");

            let _ = table
                .get(EnforceRange(1), ctx.clone())
                .expect_err("out of range");
            assert_eq!(pending_error_name(ctx), "RangeError");

            let _ = table
                .set(EnforceRange(1), Opt(None), ctx.clone())
                .expect_err("out of range");
            assert_eq!(pending_error_name(ctx), "RangeError");
        })
    }

    #[test]
    fn grow_returns_the_previous_length_and_refuses_to_pass_the_maximum() {
        with_wasm_context(|ctx| {
            let table =
                table(ctx, "({ element: 'anyfunc', initial: 1, maximum: 3 })").expect("table");

            assert_eq!(
                table.grow(EnforceRange(2), Opt(None), ctx.clone()).unwrap(),
                1
            );
            assert_eq!(table.length(ctx.clone()).unwrap(), 3);

            let _ = table
                .grow(EnforceRange(1), Opt(None), ctx.clone())
                .expect_err("over the maximum");
            assert_eq!(pending_error_name(ctx), "RangeError");
        })
    }

    #[test]
    fn a_descriptor_element_that_is_not_a_table_kind_is_a_type_error() {
        with_wasm_context(|ctx| {
            for descriptor in [
                "({ element: 'i32', initial: 1 })",
                "({ element: 'anyref', initial: 1 })",
                "({ element: 'nope', initial: 1 })",
                "({ initial: 1 })",
            ] {
                assert_eq!(
                    table(ctx, descriptor).unwrap_err(),
                    "TypeError",
                    "{descriptor}"
                );
            }
        })
    }

    #[test]
    fn funcref_is_accepted_as_an_alias_of_anyfunc() {
        with_wasm_context(|ctx| {
            assert!(table(ctx, "({ element: 'funcref', initial: 1 })").is_ok());
        })
    }

    #[test]
    fn a_plain_js_function_is_not_a_valid_element() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'anyfunc', initial: 1 })").expect("table");
            let _ = table
                .set(
                    EnforceRange(0),
                    Opt(Some(js(ctx, "(() => {})"))),
                    ctx.clone(),
                )
                .expect_err("a plain function has no function address");
            assert_eq!(pending_error_name(ctx), "TypeError");
        })
    }

    #[test]
    fn an_exported_function_round_trips_through_a_funcref_table_and_stays_callable() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'anyfunc', initial: 1 })").expect("table");
            let function = exported_function(ctx);

            table
                .set(
                    EnforceRange(0),
                    Opt(Some(function.clone().into_value())),
                    ctx.clone(),
                )
                .expect("an Exported Function is a valid funcref");
            let read = table.get(EnforceRange(0), ctx.clone()).expect("in range");
            // The spec's Exported Function cache: one funcref, one JS object.
            assert_eq!(read, function.into_value());

            ctx.globals().set("f", read).expect("bind");
            assert_eq!(
                ctx.eval::<i32, _>("f(41)")
                    .expect("the element is callable"),
                42
            );
        })
    }

    #[test]
    fn an_externref_table_preserves_the_identity_of_the_js_value_it_holds() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'externref', initial: 1 })").expect("table");
            let subject = js(ctx, "globalThis.subject = { tag: 'host value' }");

            table
                .set(EnforceRange(0), Opt(Some(subject.clone())), ctx.clone())
                .expect("any JS value is a valid externref");
            let read = table.get(EnforceRange(0), ctx.clone()).expect("in range");
            assert_eq!(read, subject);

            ctx.globals().set("read", read).expect("bind");
            assert!(
                ctx.eval::<bool, _>("read === globalThis.subject")
                    .expect("identity"),
                "the externref handed back a copy rather than the object"
            );
        })
    }

    #[test]
    fn set_coerces_the_value_before_it_bounds_checks_the_index() {
        with_wasm_context(|ctx| {
            let table = table(ctx, "({ element: 'anyfunc', initial: 1 })").expect("table");
            // Out of range *and* an invalid value. `Table.prototype.set` runs
            // `ToWebAssemblyValue` in step 4 and only writes in step 7, so the
            // `TypeError` is the one that must escape.
            let _ = table
                .set(
                    EnforceRange(9),
                    Opt(Some(js(ctx, "(() => {})"))),
                    ctx.clone(),
                )
                .expect_err("neither the index nor the value is acceptable");
            assert_eq!(pending_error_name(ctx), "TypeError");
        })
    }

    #[test]
    fn a_size_web_idl_rejects_is_a_type_error_rather_than_a_silent_zero() {
        with_wasm_context(|ctx| {
            // `Coerced<u64>` read every one of these as a size in range, so a `NaN`
            // descriptor quietly allocated an empty table.
            for descriptor in [
                "({ element: 'anyfunc', initial: NaN })",
                "({ element: 'anyfunc', initial: Infinity })",
                "({ element: 'anyfunc', initial: -1 })",
                "({ element: 'anyfunc', initial: 4294967296 })",
                "({ element: 'anyfunc', initial: 1, maximum: -1 })",
            ] {
                assert_eq!(
                    table(ctx, descriptor).unwrap_err(),
                    "TypeError",
                    "{descriptor}"
                );
            }
        })
    }

    #[test]
    fn type_reports_the_descriptor_the_table_was_created_with() {
        with_wasm_context(|ctx| {
            let bounded =
                table(ctx, "({ element: 'anyfunc', initial: 2, maximum: 5 })").expect("table");
            ctx.globals()
                .set("type", bounded.table_type(ctx.clone()).expect("type()"))
                .expect("bind");
            assert_eq!(
                ctx.eval::<String, _>("`${type.element}:${type.minimum}:${type.maximum}`")
                    .expect("render"),
                "anyfunc:2:5"
            );

            let unbounded = table(ctx, "({ element: 'externref', initial: 1 })").expect("table");
            ctx.globals()
                .set("type", unbounded.table_type(ctx.clone()).expect("type()"))
                .expect("bind");
            assert_eq!(
                ctx.eval::<String, _>("`${type.element}:${type.minimum}:${'maximum' in type}`")
                    .expect("render"),
                "externref:1:false"
            );
        })
    }
}
