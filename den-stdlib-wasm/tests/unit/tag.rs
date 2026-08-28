use super::*;
use crate::memory::testing::{js, pending_error_name, with_wasm_context};

fn tag(ctx: &Ctx<'_>, ty: &str) -> core::result::Result<Tag, String> {
    TagType::from_js(ctx, js(ctx, ty))
        .and_then(|ty| Tag::new(ty, ctx.clone()))
        .map_err(|_error| pending_error_name(ctx))
}

#[test]
fn a_tag_reports_the_parameter_types_it_was_created_with() {
    with_wasm_context(|ctx| {
        let created = tag(ctx, "({ parameters: ['i32', 'f64'] })");
        let ty = created.expect("tag").tag_type(ctx.clone()).expect("type()");
        ctx.globals().set("type", ty).expect("bind");
        let parameters: String = ctx
            .eval("type.parameters.join(',')")
            .expect("parameters is an array");
        assert_eq!(parameters, "i32,f64");
    })
}

#[test]
fn a_tag_type_with_an_unknown_parameter_type_is_a_type_error() {
    with_wasm_context(|ctx| {
        for ty in [
            "({ parameters: ['nope'] })",
            "({ parameters: ['v128'] })",
            "({})",
            "(1)",
        ] {
            assert_eq!(tag(ctx, ty).unwrap_err(), "TypeError", "{ty}");
        }
    })
}
