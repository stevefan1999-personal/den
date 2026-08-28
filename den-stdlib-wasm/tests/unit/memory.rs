/// One wasm page, the unit memory sizes are counted in.
const PAGE_SIZE: usize = 0x0001_0000;

use super::{
    testing::{js, pending_error_name, with_wasm_context},
    *,
};

fn memory(ctx: &Ctx<'_>, descriptor: &str) -> core::result::Result<Memory, String> {
    MemoryDescriptor::from_js(ctx, js(ctx, descriptor))
        .and_then(|descriptor| Memory::new(descriptor, ctx.clone()))
        .map_err(|_error| pending_error_name(ctx))
}

#[test]
fn buffer_aliases_the_linear_memory_and_is_the_same_object_between_grows() {
    with_wasm_context(|ctx| {
        let memory = memory(ctx, "({ initial: 2 })").expect("two pages");
        let buffer = memory.buffer(ctx.clone()).expect("buffer");
        assert_eq!(buffer.len(), 2 * PAGE_SIZE);
        let again = memory.buffer(ctx.clone()).expect("buffer");
        assert_eq!(buffer.as_value(), again.as_value());
    })
}

#[test]
fn an_empty_memory_still_has_a_zero_length_buffer() {
    with_wasm_context(|ctx| {
        let memory = memory(ctx, "({ initial: 0 })").expect("no pages");
        assert_eq!(memory.buffer(ctx.clone()).expect("buffer").len(), 0);
    })
}

#[test]
fn grow_returns_the_previous_page_count_and_detaches_the_old_buffer() {
    with_wasm_context(|ctx| {
        let memory = memory(ctx, "({ initial: 1, maximum: 4 })").expect("one page");
        let stale = memory.buffer(ctx.clone()).expect("buffer");
        assert_eq!(stale.len(), PAGE_SIZE);

        let previous = memory.grow(Coerced(2), ctx.clone()).expect("grow");
        assert_eq!(previous, 1);
        assert_eq!(stale.as_bytes(), None, "the old buffer must be detached");

        let fresh = memory.buffer(ctx.clone()).expect("buffer");
        assert_eq!(fresh.len(), 3 * PAGE_SIZE);
        assert_ne!(stale.as_value(), fresh.as_value());
    })
}

#[test]
fn growing_past_the_maximum_is_a_range_error() {
    with_wasm_context(|ctx| {
        let memory = memory(ctx, "({ initial: 1, maximum: 2 })").expect("one page");
        let _ = memory
            .grow(Coerced(5), ctx.clone())
            .expect_err("over maximum");
        assert_eq!(pending_error_name(ctx), "RangeError");
    })
}

#[test]
fn a_descriptor_without_initial_is_a_type_error() {
    with_wasm_context(|ctx| {
        assert_eq!(memory(ctx, "({ maximum: 1 })").unwrap_err(), "TypeError");
        assert_eq!(memory(ctx, "(1)").unwrap_err(), "TypeError");
    })
}

#[test]
fn minimum_is_accepted_as_an_alias_of_initial_but_not_alongside_it() {
    with_wasm_context(|ctx| {
        let wasm_memory = memory(ctx, "({ minimum: 2 })").expect("minimum");
        assert_eq!(
            wasm_memory.buffer(ctx.clone()).expect("buffer").len(),
            2 * PAGE_SIZE
        );
        assert_eq!(
            memory(ctx, "({ initial: 1, minimum: 1 })").unwrap_err(),
            "TypeError"
        );
    })
}

#[test]
fn a_maximum_below_the_initial_size_is_a_range_error() {
    with_wasm_context(|ctx| {
        assert_eq!(
            memory(ctx, "({ initial: 4, maximum: 1 })").unwrap_err(),
            "RangeError"
        );
    })
}

#[test]
fn shared_memory_is_refused_whatever_this_build_cannot_alias() {
    with_wasm_context(|ctx| {
        assert_eq!(
            memory(ctx, "({ initial: 1, shared: true })").unwrap_err(),
            "TypeError"
        );
        assert_eq!(
            memory(ctx, "({ initial: 1, maximum: 2, shared: true })").unwrap_err(),
            "TypeError"
        );
    })
}
