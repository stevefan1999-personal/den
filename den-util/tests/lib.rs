use den_util::{
    BufferSource, ClassId as _, ObjectExt as _, OwnedCtx, Probe as _, coerce_string, construct,
    instance_of_global, new_dom_exception, throw_dom_exception,
};
use rquickjs::{ArrayBuffer, Context, Error, FromJs as _, Object, Runtime, Value};

type TestResult = rquickjs::Result<()>;

fn realm(body: impl FnOnce(rquickjs::Ctx<'_>) -> TestResult) -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(body)
}

#[test]
fn buffer_source_copies_array_buffers_typed_arrays_and_data_views() -> TestResult {
    realm(|ctx| {
        // An ArrayBuffer is not a view, and yields the whole buffer.
        let array_buffer: ArrayBuffer = ctx.eval("new Uint8Array([1,2,3]).buffer")?;
        let array_buffer = array_buffer.into_value();
        assert!(!BufferSource::is_array_buffer_view(&ctx, &array_buffer)?);
        assert_eq!(
            BufferSource::from_js(&ctx, array_buffer.clone())?.bytes(),
            &[1, 2, 3]
        );
        assert_eq!(
            BufferSource::from_js(&ctx, array_buffer)?.into_bytes(),
            vec![1, 2, 3]
        );

        let typed_array: Value = ctx.eval("new Uint8Array([4,5,6])")?;
        assert!(BufferSource::is_array_buffer_view(&ctx, &typed_array)?);
        assert_eq!(BufferSource::from_js(&ctx, typed_array)?.bytes(), &[
            4, 5, 6
        ]);

        // A DataView reads through its own window, not the whole buffer.
        let data_view: Value = ctx.eval("new DataView(new Uint8Array([7,8,9]).buffer, 1, 2)")?;
        assert!(BufferSource::is_array_buffer_view(&ctx, &data_view)?);
        assert_eq!(BufferSource::view_bytes(&ctx, &data_view)?, vec![8, 9]);
        assert_eq!(BufferSource::from_js(&ctx, data_view)?.bytes(), &[8, 9]);

        // A typed array over a slice of a buffer sees only its own window.
        let window: Value = ctx.eval("new Uint8Array(new Uint8Array([1,2,3,4]).buffer, 1, 2)")?;
        assert_eq!(BufferSource::from_js(&ctx, window)?.bytes(), &[2, 3]);

        // The copy is taken up front, so mutating the source afterwards cannot
        // change what the caller already read.
        let source: Value = ctx.eval("globalThis.shared = new Uint8Array([10, 20])")?;
        let copied = BufferSource::from_js(&ctx, source)?;
        ctx.eval::<(), _>("shared[0] = 99")?;
        assert_eq!(copied.bytes(), &[10, 20]);

        // Anything else is a TypeError, which stays pending until it is taken.
        let plain: Value = ctx.eval("({})")?;
        assert!(BufferSource::from_js(&ctx, plain).is_err());
        assert!(ctx.catch().is_object());
        Ok(())
    })
}

#[test]
fn coerce_string_casts_primitives_and_objects() -> TestResult {
    realm(|ctx| {
        for (source, expected) in [
            ("42", "42"),
            ("true", "true"),
            ("undefined", "undefined"),
            ("null", "null"),
            ("({ toString: () => 'custom' })", "custom"),
        ] {
            let value: Value = ctx.eval(source)?;
            assert_eq!(coerce_string(&ctx, value)?, expected, "coercing {source}");
        }
        Ok(())
    })
}

#[test]
fn construct_and_instance_of_global_read_the_realm() -> TestResult {
    realm(|ctx| {
        let constructed: Object = construct(&ctx, "Object", ())?;
        let constructed = constructed.into_value();
        assert!(instance_of_global(&ctx, &constructed, "Object")?);
        assert!(!instance_of_global(&ctx, &constructed, "Error")?);

        // Every Error is also an Object: the check walks the prototype chain.
        let error: Value = ctx.eval("new TypeError('raw')")?;
        assert!(instance_of_global(&ctx, &error, "TypeError")?);
        assert!(instance_of_global(&ctx, &error, "Object")?);

        // A missing global, a non-function global and a primitive are all
        // false rather than errors.
        assert!(!instance_of_global(&ctx, &error, "NoSuchGlobal")?);
        ctx.globals().set("notAConstructor", 1)?;
        assert!(!instance_of_global(&ctx, &error, "notAConstructor")?);
        let primitive: Value = ctx.eval("1")?;
        assert!(!instance_of_global(&ctx, &primitive, "Object")?);

        // Constructing a missing global fails, unlike the tolerant check
        // above, but it fails converting `undefined` on the Rust side rather
        // than throwing, so it leaves nothing pending on the context.
        let missing = construct::<_, Value>(&ctx, "NoSuchGlobal", ());
        assert!(matches!(
            missing,
            Err(Error::FromJs {
                to: "constructor",
                ..
            })
        ));
        assert!(!ctx.has_exception());
        Ok(())
    })
}

#[test]
fn probe_clears_only_the_exception_it_swallowed() -> TestResult {
    realm(|ctx| {
        let outcome = ctx.probe(|| {
            ctx.eval::<(), _>("throw new TypeError('boom')").ok()?;
            None::<()>
        });
        assert!(outcome.is_none());
        assert!(!ctx.has_exception(), "probe leaves no pending exception");

        // A successful attempt is passed straight through.
        assert_eq!(ctx.probe(|| Some(7)), Some(7));
        Ok(())
    })
}

#[test]
fn has_own_ignores_the_prototype_chain() -> TestResult {
    realm(|ctx| {
        let object: Object =
            ctx.eval("Object.assign(Object.create({ inherited: 1 }), { own: 2 })")?;
        assert!(object.has_own("own")?);
        assert!(!object.has_own("inherited")?);
        assert!(!object.has_own("missing")?);
        // `contains_key` is the prototype-walking counterpart it must not be.
        assert!(object.contains_key("inherited")?);
        Ok(())
    })
}

#[test]
fn dom_exceptions_carry_their_name_and_message() -> TestResult {
    realm(|ctx| {
        let exception = new_dom_exception(&ctx, "bad", "TypeError")?;
        let exception = exception.as_object().expect("DOMException is an object");
        assert_eq!(exception.get::<_, String>("name")?, "TypeError");
        assert_eq!(exception.get::<_, String>("message")?, "bad");

        // Throwing leaves the same DOMException pending on the context.
        let error = throw_dom_exception(&ctx, "TypeError", "bad value");
        assert!(matches!(error, Error::Exception));
        let thrown = ctx.catch();
        let thrown = thrown.as_object().expect("the thrown value is an object");
        assert_eq!(thrown.get::<_, String>("name")?, "TypeError");
        assert_eq!(thrown.get::<_, String>("message")?, "bad value");
        Ok(())
    })
}

#[test]
fn class_id_separates_natives_from_plain_objects() -> TestResult {
    realm(|ctx| {
        let plain: Value = ctx.eval("({})")?;
        let array: Value = ctx.eval("[]")?;
        assert_ne!(plain.class_id(), 0);
        assert_ne!(array.class_id(), plain.class_id());
        Ok(())
    })
}

#[test]
fn owned_ctx_keeps_the_context_alive_past_its_borrow() -> TestResult {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    let owned = context.with(|ctx| -> rquickjs::Result<OwnedCtx> {
        ctx.globals().set("parked", 42)?;
        Ok(OwnedCtx::new(&ctx))
    })?;
    // The borrow that minted it is gone; the handle still reaches the realm.
    owned.with(|ctx| -> TestResult {
        assert_eq!(ctx.globals().get::<_, i32>("parked")?, 42);
        Ok(())
    })
}
