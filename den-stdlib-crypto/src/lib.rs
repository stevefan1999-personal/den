use rquickjs::{ArrayBuffer, Ctx, Exception, Object, Result, TypedArray};
use uuid::Uuid;

#[rquickjs::function]
pub fn get_random_values<'js>(array: Object<'js>, ctx: Ctx<'js>) -> Result<Object<'js>> {
    {
        let array = if let Ok(array) = TypedArray::<u8>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u16>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u32>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<u64>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i8>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i16>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i32>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Ok(array) = TypedArray::<i64>::from_object(array.clone()) {
            array.arraybuffer()
        } else if let Some(array) = ArrayBuffer::from_object(array.clone()) {
            Ok(array)
        } else {
            Err(Exception::throw_type(&ctx, "not a typed array"))
        }?;

        // `as_raw` is the only mutable view rquickjs 0.12 offers: `as_bytes` hands back
        // a shared `&[u8]`, so writing through it would mean casting away its
        // immutability. It returns `None` for a detached buffer, which JS can
        // trigger at will.
        let Some(raw) = array.as_raw() else {
            return Err(Exception::throw_type(&ctx, "array buffer is detached"));
        };
        // SAFETY: `raw` is QuickJS's own live allocation for this buffer. Nothing else
        // aliases it here — no JS runs between `as_raw` and the end of the
        // fill, so the buffer cannot be detached or resized underneath us.
        let dest = unsafe { core::slice::from_raw_parts_mut(raw.ptr.as_ptr(), raw.len) };
        rand::fill(dest);
    }
    Ok(array)
}

#[rquickjs::function(rename = "randomUUID")]
pub fn random_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[rquickjs::module]
pub mod crypto {
    use indexmap::indexmap;
    use rquickjs::{Ctx, IntoJs, Result, module::Exports};

    #[qjs(declare)]
    pub fn declare(declare: &rquickjs::module::Declarations) -> Result<()> {
        declare.declare("getRandomValues")?.declare("randomUUID")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
        e.export("getRandomValues", super::js_get_random_values.into_js(ctx)?)?
            .export("randomUUID", super::js_random_uuid.into_js(ctx)?)?;

        ctx.globals().set(
            "crypto",
            indexmap! {
                "getRandomValues" => super::js_get_random_values.into_js(ctx)?,
                "randomUUID" => super::js_random_uuid.into_js(ctx)?,
            },
        )?;

        Ok(())
    }
}
