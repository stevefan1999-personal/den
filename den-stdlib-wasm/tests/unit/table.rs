use rquickjs::Function;
use wasmtime::Func;

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
            Ok(Func::wrap(&mut *store, |value: i32| value + 1))
        })
        .expect("host function");
    HostReferences::exported_function(ctx, func, None).expect("exported function")
}

fn table(ctx: &Ctx<'_>, descriptor: &str) -> core::result::Result<Table, String> {
    TableDescriptor::from_js(ctx, js(ctx, descriptor))
        .and_then(|descriptor| Table::new(descriptor, Opt(None), ctx.clone()))
        .map_err(|_error| pending_error_name(ctx))
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
        let table = table(ctx, "({ element: 'anyfunc', initial: 1, maximum: 3 })").expect("table");

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
fn minimum_is_accepted_as_an_alias_of_initial_but_not_alongside_it() {
    with_wasm_context(|ctx| {
        let wasm_table = table(ctx, "({ element: 'anyfunc', minimum: 3 })").expect("minimum");
        assert_eq!(wasm_table.length(ctx.clone()).unwrap(), 3);
        assert_eq!(
            table(ctx, "({ element: 'anyfunc', initial: 1, minimum: 1 })").unwrap_err(),
            "TypeError"
        );
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
            "funcref:2:5"
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
