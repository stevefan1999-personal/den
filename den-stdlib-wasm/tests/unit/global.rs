use super::*;
use crate::memory::testing::{js, pending_error_name, with_wasm_context};

fn global(
    ctx: &Ctx<'_>, descriptor: &str, value: Option<&str>,
) -> core::result::Result<Global, String> {
    GlobalDescriptor::from_js(ctx, js(ctx, descriptor))
        .and_then(|descriptor| {
            Global::new(
                descriptor,
                Opt(value.map(|value| js(ctx, value))),
                ctx.clone(),
            )
        })
        .map_err(|_error| pending_error_name(ctx))
}

/// The global's value as JS renders it, e.g. `"number:42"`.
fn rendered(ctx: &Ctx<'_>, global: &Global) -> String {
    let value = global.get_value(ctx.clone()).expect("readable global");
    ctx.globals().set("value", value).expect("bind");
    ctx.eval("`${typeof value}:${value}`").expect("render")
}

#[test]
fn the_constructor_coerces_the_initial_value_to_the_declared_type() {
    with_wasm_context(|ctx| {
        // ToInt32("3") is 3: the target type drives the coercion, not the JS type.
        let from_string = global(ctx, "({ value: 'i32' })", Some("'3'")).expect("i32 global");
        assert_eq!(rendered(ctx, &from_string), "number:3");
        // `1` is a perfectly good f64, even though it arrives as an integer.
        let from_integer = global(ctx, "({ value: 'f64' })", Some("1")).expect("f64 global");
        assert_eq!(rendered(ctx, &from_integer), "number:1");
    })
}

#[test]
fn an_omitted_initial_value_is_the_default_for_the_type() {
    with_wasm_context(|ctx| {
        assert_eq!(
            rendered(ctx, &global(ctx, "({ value: 'i32' })", None).unwrap()),
            "number:0"
        );
        assert_eq!(
            rendered(ctx, &global(ctx, "({ value: 'anyfunc' })", None).unwrap()),
            "object:null"
        );
    })
}

#[test]
fn an_i64_global_reads_and_writes_as_a_bigint() {
    with_wasm_context(|ctx| {
        let global = global(
            ctx,
            "({ value: 'i64', mutable: true })",
            Some("9223372036854775807n"),
        )
        .expect("i64 global");
        assert_eq!(rendered(ctx, &global), "bigint:9223372036854775807");

        global
            .set_value(Opt(Some(js(ctx, "-1n"))), ctx.clone())
            .expect("mutable");
        assert_eq!(rendered(ctx, &global), "bigint:-1");

        // A Number is not a BigInt, and `ToWebAssemblyValue` says so.
        let _ = global
            .set_value(Opt(Some(js(ctx, "1"))), ctx.clone())
            .expect_err("Number is not an i64");
        assert_eq!(pending_error_name(ctx), "TypeError");
    })
}

#[test]
fn writing_an_immutable_global_is_a_type_error() {
    with_wasm_context(|ctx| {
        let global = global(ctx, "({ value: 'i32' })", Some("7")).expect("immutable global");
        let _ = global
            .set_value(Opt(Some(js(ctx, "8"))), ctx.clone())
            .expect_err("immutable");
        assert_eq!(pending_error_name(ctx), "TypeError");
        assert_eq!(rendered(ctx, &global), "number:7");
    })
}

#[test]
fn value_of_agrees_with_the_value_getter() {
    with_wasm_context(|ctx| {
        let global = global(ctx, "({ value: 'f32' })", Some("0.5")).expect("f32 global");
        let by_getter = global.get_value(ctx.clone()).unwrap();
        let by_value_of = global.value_of(ctx.clone()).unwrap();
        assert_eq!(
            by_getter.as_float().unwrap(),
            by_value_of.as_float().unwrap()
        );
    })
}

#[test]
fn a_descriptor_value_that_is_not_a_value_type_is_a_type_error() {
    with_wasm_context(|ctx| {
        for descriptor in [
            // "anyref" is not in the spec's `ValueType` enum, however much it looks like one.
            "({ value: 'anyref' })",
            // v128 has no JS representation, so a v128 global could never be read.
            "({ value: 'v128' })",
            "({ value: 'nope' })",
            "({})",
        ] {
            assert_eq!(
                global(ctx, descriptor, None).unwrap_err(),
                "TypeError",
                "{descriptor}"
            );
        }
    })
}

#[test]
fn anyfunc_is_a_valid_global_type() {
    with_wasm_context(|ctx| {
        assert!(global(ctx, "({ value: 'anyfunc' })", Some("null")).is_ok());
        assert!(global(ctx, "({ value: 'externref' })", Some("null")).is_ok());
    })
}

#[test]
fn an_externref_global_hands_back_the_very_js_value_it_was_given() {
    with_wasm_context(|ctx| {
        let global = global(
            ctx,
            "({ value: 'externref', mutable: true })",
            Some("globalThis.first = { tag: 'first' }"),
        )
        .expect("externref global");
        ctx.globals()
            .set("read", global.get_value(ctx.clone()).expect("readable"))
            .expect("bind");
        assert!(
            ctx.eval::<bool, _>("read === globalThis.first")
                .expect("identity")
        );

        global
            .set_value(
                Opt(Some(js(ctx, "globalThis.second = ['second']"))),
                ctx.clone(),
            )
            .expect("mutable");
        ctx.globals()
            .set("read", global.get_value(ctx.clone()).expect("readable"))
            .expect("bind");
        assert!(
            ctx.eval::<bool, _>("read === globalThis.second")
                .expect("identity")
        );
    })
}

#[test]
fn type_reports_the_descriptor_the_global_was_created_with() {
    with_wasm_context(|ctx| {
        for (descriptor, expected) in [
            ("({ value: 'f64', mutable: true })", "f64:true"),
            ("({ value: 'i32' })", "i32:false"),
            ("({ value: 'externref' })", "externref:false"),
        ] {
            let global = global(ctx, descriptor, None).expect("global");
            ctx.globals()
                .set("type", global.global_type(ctx.clone()).expect("type()"))
                .expect("bind");
            assert_eq!(
                ctx.eval::<String, _>("`${type.value}:${type.mutable}`")
                    .expect("render"),
                expected,
                "{descriptor}"
            );
        }
    })
}
