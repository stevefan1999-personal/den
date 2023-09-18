pub mod error;
pub mod global;
pub mod instance;
pub mod memory;
pub mod module;
pub mod table;
pub mod tag;

#[rquickjs::module]
pub mod wasm {
    use rquickjs::{module::Exports, Ctx, Object};

    pub use crate::{
        error::{CompileError, Exception, LinkError, RuntimeError},
        global::Global,
        instance::Instance,
        memory::Memory,
        module::Module,
        table::Table,
        tag::Tag,
    };

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _exports: &mut Exports<'js>) -> rquickjs::Result<()> {
        let wasm = Object::new(ctx.clone())?;
        wasm.set(
            "Global",
            rquickjs::Class::<Global>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "Instance",
            rquickjs::Class::<Instance>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "Memory",
            rquickjs::Class::<Memory>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "Module",
            rquickjs::Class::<Module>::create_constructor(ctx)?,
        )?;
        wasm.set("Table", rquickjs::Class::<Table>::create_constructor(ctx)?)?;
        wasm.set("Tag", rquickjs::Class::<Tag>::create_constructor(ctx)?)?;
        wasm.set(
            "Exception",
            rquickjs::Class::<Exception>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "LinkError",
            rquickjs::Class::<LinkError>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "RuntimeError",
            rquickjs::Class::<RuntimeError>::create_constructor(ctx)?,
        )?;
        wasm.set(
            "CompileError",
            rquickjs::Class::<CompileError>::create_constructor(ctx)?,
        )?;

        ctx.globals().set("WebAssembly", wasm)?;

        Ok(())
    }
}
