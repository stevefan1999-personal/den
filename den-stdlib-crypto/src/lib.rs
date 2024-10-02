use rand::RngCore;
use rquickjs::ArrayBuffer;
use uuid::Uuid;

#[rquickjs::function]
pub fn get_random_values<'js>(array: ArrayBuffer<'js>) -> ArrayBuffer<'js> {
    let dest = array.as_bytes().unwrap();
    let dest = unsafe { core::slice::from_raw_parts_mut(dest.as_ptr() as *mut u8, dest.len()) };
    rand::thread_rng().fill_bytes(dest);
    array
}

#[rquickjs::function(rename = "randomUUID")]
pub fn random_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[rquickjs::module]
pub mod crypto {
    use indexmap::indexmap;
    use rquickjs::{module::Exports, Ctx, IntoJs, Result};

    #[qjs(declare)]
    pub fn declare(declare: &rquickjs::module::Declarations) -> Result<()> {
        declare.declare("getRandomValues")?.declare("randomUUID")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
        e.export(
            "getRandomValues",
            super::js_get_random_values.into_js(&ctx)?,
        )?
        .export("randomUUID", super::js_random_uuid.into_js(&ctx)?)?;

        ctx.globals().set(
            "crypto",
            indexmap! {
                "getRandomValues" => super::js_get_random_values.into_js(&ctx)?,
                "randomUUID" => super::js_random_uuid.into_js(&ctx)?,
            },
        )?;

        Ok(())
    }
}