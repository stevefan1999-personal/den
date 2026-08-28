use den_core::engine::Engine;
use rquickjs::{Ctx, Exception, Function, Result, TypedArray};

fn assemble<'js>(source: String, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
    let bytes = wat::parse_str(&source)
        .map_err(|error| Exception::throw_type(&ctx, &format!("wat2wasm error: {error}")))?;
    TypedArray::new_copy(ctx, bytes)
}

pub async fn engine() -> Engine {
    let engine = Engine::new().await;
    engine
        .context
        .with(|ctx| {
            ctx.globals().set(
                "wat2wasm",
                Function::new(ctx.clone(), assemble)?.with_name("wat2wasm")?,
            )
        })
        .await
        .expect("install the test-only WAT assembler");
    engine
}
