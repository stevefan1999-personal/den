use rquickjs::{Context, Runtime};

use super::*;
use crate::memory::testing::{js, with_wasm_context};

fn with_context<R, F: FnOnce(&Ctx<'_>) -> R>(f: F) -> R {
    let runtime = Runtime::new().expect("runtime");
    let context = Context::full(&runtime).expect("context");
    context.with(|ctx| f(&ctx))
}

fn ty(name: &str) -> ValType {
    match name {
        "i32" => ValType::I32,
        "i64" => ValType::I64,
        "f32" => ValType::F32,
        "f64" => ValType::F64,
        "v128" => ValType::V128,
        "anyfunc" => ValType::FUNCREF,
        "externref" => ValType::EXTERNREF,
        _ => panic!("unknown value type: {name}"),
    }
}

/// The `name` of whatever JS error is currently pending, e.g.
/// `"TypeError"`.
fn pending_error_name(ctx: &Ctx<'_>) -> String {
    ctx.catch()
        .as_exception()
        .and_then(|exception| exception.get::<_, String>("name").ok())
        .unwrap_or_else(|| "not a JS error".to_owned())
}

/// JS to wasm, reporting the *class* of a thrown JS error so the tests pin
/// the spec's error type rather than its wording.
fn from_js(ctx: &Ctx<'_>, source: &str, type_name: &str) -> core::result::Result<Val, String> {
    let value = ctx
        .eval::<Value, _>(source)
        .map_err(|_error| pending_error_name(ctx))?;
    WasmValue::from_js(ctx, &value, &ty(type_name))
        .map(WasmValue::into_inner)
        .map_err(|_error| pending_error_name(ctx))
}

/// wasm to JS, rendered as `"<typeof>:<value>"` by JS itself.
fn to_js_source(ctx: &Ctx<'_>, value: Val) -> core::result::Result<String, String> {
    WasmValue(value)
        .to_js(ctx)
        .and_then(|value| {
            ctx.globals().set("value", value)?;
            ctx.eval::<String, _>("`${typeof value}:${value}`")
        })
        .map_err(|_error| pending_error_name(ctx))
}

#[test]
fn i32_round_trips_through_to_int32_semantics() {
    with_context(|ctx| {
        assert!(matches!(from_js(ctx, "-5", "i32"), Ok(Val::I32(-5))));
        // ToInt32 truncates and wraps rather than rejecting.
        assert!(matches!(
            from_js(ctx, "2147483648", "i32"),
            Ok(Val::I32(i32::MIN))
        ));
        assert!(matches!(from_js(ctx, "1.9", "i32"), Ok(Val::I32(1))));
        assert_eq!(to_js_source(ctx, Val::I32(-5)).unwrap(), "number:-5");
    })
}

#[test]
fn i64_uses_bigint_in_both_directions_including_the_extremes() {
    with_context(|ctx| {
        assert!(matches!(from_js(ctx, "1n", "i64"), Ok(Val::I64(1))));
        assert!(matches!(
            from_js(ctx, "-9223372036854775808n", "i64"),
            Ok(Val::I64(i64::MIN))
        ));
        assert!(matches!(
            from_js(ctx, "9223372036854775807n", "i64"),
            Ok(Val::I64(i64::MAX))
        ));
        assert_eq!(
            to_js_source(ctx, Val::I64(i64::MAX)).unwrap(),
            "bigint:9223372036854775807"
        );
        assert_eq!(
            to_js_source(ctx, Val::I64(i64::MIN)).unwrap(),
            "bigint:-9223372036854775808"
        );
    })
}

#[test]
fn i64_rejects_a_number_with_a_type_error() {
    with_context(|ctx| {
        let error = from_js(ctx, "1", "i64").expect_err("Number is not a BigInt");
        assert_eq!(error, "TypeError");
    })
}

#[test]
fn i64_accepts_the_values_to_big_int_accepts() {
    with_context(|ctx| {
        assert!(matches!(from_js(ctx, "'42'", "i64"), Ok(Val::I64(42))));
        assert!(matches!(from_js(ctx, "true", "i64"), Ok(Val::I64(1))));
        assert!(from_js(ctx, "undefined", "i64").is_err());
        // ToBigInt64 wraps modulo 2^64: 2^63 is i64::MIN, not a conversion failure.
        assert!(matches!(
            from_js(ctx, "2n ** 63n", "i64"),
            Ok(Val::I64(i64::MIN))
        ));
    })
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "WebAssembly f32 conversion must preserve the exact narrowed value"
)]
fn f32_narrows_to_nearest_and_keeps_the_value_not_the_bits() {
    with_context(|ctx| {
        let Ok(value) = from_js(ctx, "0.1", "f32") else {
            panic!("0.1 is a valid f32");
        };
        assert!(matches!(value, Val::F32(bits) if f32::from_bits(bits) == 0.1_f32));
        // The JS side must see the widened f32, never the raw bit pattern.
        assert_eq!(
            to_js_source(ctx, value).unwrap(),
            format!("number:{}", f64::from(0.1_f32))
        );
        assert_eq!(to_js_source(ctx, Val::from(1.0_f32)).unwrap(), "number:1");
    })
}

#[test]
fn f64_preserves_nan_and_both_infinities() {
    with_context(|ctx| {
        for (source, expected) in [
            ("Infinity", "number:Infinity"),
            ("-Infinity", "number:-Infinity"),
            ("NaN", "number:NaN"),
        ] {
            let value = from_js(ctx, source, "f64").expect("valid f64");
            assert_eq!(to_js_source(ctx, value).unwrap(), expected);
            let value = from_js(ctx, source, "f32").expect("valid f32");
            assert_eq!(to_js_source(ctx, value).unwrap(), expected);
        }
    })
}

#[test]
fn undefined_and_null_follow_to_number_for_numeric_types() {
    with_context(|ctx| {
        // ToNumber(undefined) is NaN, ToInt32(NaN) is 0, ToNumber(null) is +0.
        assert!(matches!(from_js(ctx, "undefined", "i32"), Ok(Val::I32(0))));
        assert!(matches!(from_js(ctx, "null", "i32"), Ok(Val::I32(0))));
        assert_eq!(
            to_js_source(ctx, from_js(ctx, "undefined", "f64").unwrap()).unwrap(),
            "number:NaN"
        );
        assert_eq!(
            to_js_source(ctx, from_js(ctx, "null", "f64").unwrap()).unwrap(),
            "number:0"
        );
    })
}

#[test]
fn null_is_a_reference_of_every_reference_type() {
    with_context(|ctx| {
        let value = from_js(ctx, "null", "anyfunc").expect("null funcref");
        assert!(matches!(value, Val::FuncRef(None)));
        assert_eq!(to_js_source(ctx, value).unwrap(), "object:null");
    })
}

#[test]
fn a_plain_js_function_has_no_function_address_to_store_in_a_funcref() {
    with_context(|ctx| {
        // `ToWebAssemblyValue(v, funcref)` needs `v.[[FunctionAddress]]`, which
        // only an Exported Function has.
        assert_eq!(
            from_js(ctx, "(() => {})", "anyfunc").unwrap_err(),
            "TypeError"
        );
        assert_eq!(from_js(ctx, "({})", "anyfunc").unwrap_err(), "TypeError");
    })
}

/// `[EnforceRange] unsigned long`, applied to a JS snippet.
fn enforce_range(ctx: &Ctx<'_>, source: &str) -> core::result::Result<u32, String> {
    let value = ctx
        .eval::<Value, _>(source)
        .map_err(|_error| pending_error_name(ctx))?;
    EnforceRange::from_js(ctx, value)
        .map(|range| range.0)
        .map_err(|_error| pending_error_name(ctx))
}

#[test]
fn enforce_range_truncates_toward_zero_like_web_idl() {
    with_context(|ctx| {
        assert_eq!(enforce_range(ctx, "1.9").unwrap(), 1);
        // `IntegerPart` truncates toward zero, so -0 and -0.5 both land on 0
        // and stay in range — only a value that truncates *past* zero is
        // rejected.
        assert_eq!(enforce_range(ctx, "-0").unwrap(), 0);
        assert_eq!(enforce_range(ctx, "-0.5").unwrap(), 0);
        assert_eq!(enforce_range(ctx, "'7'").unwrap(), 7);
        assert_eq!(enforce_range(ctx, "true").unwrap(), 1);
        assert_eq!(enforce_range(ctx, "4294967295").unwrap(), u32::MAX);
        // The `valueOf` hook still runs; only the *result* is range-checked.
        assert_eq!(enforce_range(ctx, "({ valueOf: () => 3 })").unwrap(), 3);
    })
}

#[test]
fn enforce_range_rejects_nan_infinity_and_everything_out_of_range() {
    with_context(|ctx| {
        // `Coerced<u64>` reads every one of these as a number in range, which is
        // how a `NaN` descriptor used to allocate an empty table instead of
        // throwing.
        for source in [
            "NaN",
            "undefined",
            "Infinity",
            "-Infinity",
            "-1",
            "4294967296",
            "1e30",
        ] {
            assert_eq!(
                enforce_range(ctx, source).unwrap_err(),
                "TypeError",
                "{source}"
            );
        }
    })
}

#[test]
fn v128_is_a_type_error_in_both_directions() {
    with_context(|ctx| {
        assert_eq!(
            from_js(ctx, "0", "v128").expect_err("v128 is not representable"),
            "TypeError"
        );
        assert_eq!(
            to_js_source(ctx, Val::V128(wasmtime::V128::from(0_u128)))
                .expect_err("v128 is not representable"),
            "TypeError"
        );
    })
}

/// Two distinct host functions must not share a cache key, or reading one
/// `funcref` would hand JS the other one's callable. This is the guard on
/// [`Reference::identity`].
#[test]
fn func_identity_tells_two_functions_apart() {
    with_wasm_context(|ctx| {
        let store = Store::from_ctx(ctx).expect("store");
        store
            .with_mut(ctx, |store| {
                let first = Func::wrap(&mut *store, || {});
                let second = Func::wrap(&mut *store, || {});
                assert_eq!(
                    Reference::identity(store, &first),
                    Reference::identity(store, &first),
                    "the same function must keep one identity"
                );
                assert_ne!(
                    Reference::identity(store, &first),
                    Reference::identity(store, &second),
                    "two functions must not collide"
                );
                Ok(())
            })
            .expect("identities");
    })
}

#[test]
fn an_externref_carries_any_js_value_across_and_back_unchanged() {
    with_wasm_context(|ctx| {
        for source in [
            "({ tag: 'host value' })",
            "'a string'",
            "42",
            "Symbol.iterator",
        ] {
            let original = js(ctx, source);
            let reference = WasmValue::from_js(ctx, &original, &ty("externref"))
                .expect("every JS value is a valid externref");
            assert_eq!(
                reference.to_js(ctx).expect("readable externref"),
                original,
                "{source}"
            );
        }
    })
}

#[test]
fn reading_the_same_funcref_twice_yields_the_same_js_function() {
    with_wasm_context(|ctx| {
        let store = Store::from_ctx(ctx).expect("store");
        let func = store
            .with_mut(ctx, |store| Ok(Func::wrap(&mut *store, || {})))
            .expect("host function");
        let reference = WasmValue(Reference::function(func));
        let first = reference.to_js(ctx).expect("readable funcref");
        let second = reference.to_js(ctx).expect("readable funcref");
        assert_eq!(first, second, "the Exported Function cache is not caching");
        // …and that object converts back to the same function address.
        let round_tripped = WasmValue::from_js(ctx, &first, &ty("anyfunc"))
            .expect("an Exported Function is a valid funcref");
        assert_eq!(round_tripped.to_js(ctx).expect("readable funcref"), first);
    })
}

#[test]
fn default_values_exist_for_every_type_but_v128() {
    assert!(matches!(
        WasmValue::default_for(&ty("i32")).map(WasmValue::into_inner),
        Ok(Val::I32(0))
    ));
    assert!(matches!(
        WasmValue::default_for(&ty("i64")).map(WasmValue::into_inner),
        Ok(Val::I64(0))
    ));
    assert!(matches!(
        WasmValue::default_for(&ty("f32")).unwrap().0,
        Val::F32(0)
    ));
    assert!(matches!(
        WasmValue::default_for(&ty("anyfunc")).unwrap().0,
        Val::FuncRef(None)
    ));
    assert!(WasmValue::default_for(&ty("v128")).is_err());
}
