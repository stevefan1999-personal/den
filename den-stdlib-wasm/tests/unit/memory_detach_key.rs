use super::{
    testing::{pending_error_name, with_wasm_context},
    *,
};

/// One wasm page, the unit memory sizes are counted in.
const PAGE_SIZE: usize = 0x0001_0000;

fn one_page(ctx: &Ctx<'_>) -> Memory {
    Memory::new(
        MemoryDescriptor {
            initial: 1,
            maximum: None,
            shared:  false,
        },
        ctx.clone(),
    )
    .expect("one page")
}

/// `transfer` reaches `js_realloc` on the wasm linear memory base, so
/// before the buffer was sealed this corrupted the heap and crashed the
/// process rather than merely misbehaving.
#[test]
fn the_buffer_refuses_every_method_that_would_detach_it() {
    with_wasm_context(|ctx| {
        let memory = one_page(ctx);
        ctx.globals()
            .set("buf", memory.buffer(ctx.clone()).expect("buffer"))
            .expect("bind");

        for call in [
            "buf.transfer()",
            "buf.transfer(1)",
            "buf.transferToFixedLength()",
            "buf.transferToImmutable()",
            "buf.resize(0)",
        ] {
            let outcome: rquickjs::Result<rquickjs::Value> = ctx.eval(call);
            assert!(outcome.is_err(), "{call} must not succeed");
            assert_eq!(pending_error_name(ctx), "TypeError", "{call}");
        }

        // Sealing must not have cost the buffer its ordinary behaviour.
        let length: usize = ctx.eval("buf.byteLength").expect("byteLength");
        assert_eq!(length, PAGE_SIZE);
        let written: u8 = ctx
            .eval("(new Uint8Array(buf))[7] = 42, (new Uint8Array(buf))[7]")
            .expect("writable");
        assert_eq!(written, 42);
    })
}

/// Every detaching method must be shadowed. Guarding `transfer` alone still
/// leaves `resize` able to hand QuickJS the wasm allocator's foreign pointer.
#[test]
fn every_detaching_method_is_an_own_sealed_guard() {
    with_wasm_context(|ctx| {
        let memory = one_page(ctx);
        ctx.globals()
            .set("buf", memory.buffer(ctx.clone()).expect("buffer"))
            .expect("bind");

        let descriptors: String = ctx
            .eval(include_str!("../fixtures/unit/memory_guard_descriptors.js"))
            .expect("descriptors");
        insta::assert_snapshot!("sealed_memory_guard_descriptors", descriptors);
    })
}

/// Script must not be able to `delete` a guard to reach the original method on
/// `ArrayBuffer.prototype`.
#[test]
fn the_guards_are_neither_writable_nor_configurable() {
    with_wasm_context(|ctx| {
        let memory = one_page(ctx);
        ctx.globals()
            .set("buf", memory.buffer(ctx.clone()).expect("buffer"))
            .expect("bind");

        // Non-configurable, so `delete` reports failure — as a thrown
        // TypeError under strict mode, which is how rquickjs evaluates.
        let deleted: String = ctx
            .eval(
                "(() => { try { return String(delete buf.transfer) } catch (e) { return e.name } \
                 })()",
            )
            .expect("delete");
        assert!(
            deleted == "false" || deleted == "TypeError",
            "the guard must not be deletable, got {deleted}"
        );

        let still_refused: bool = ctx
            .eval("(() => { try { buf.transfer(1); return false } catch { return true } })()")
            .expect("transfer");
        assert!(still_refused, "transfer must still be refused");
    })
}
