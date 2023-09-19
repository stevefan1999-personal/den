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
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &mut Exports<'js>) -> rquickjs::Result<()> {
        let wasm = Object::new(ctx.clone())?;
        for (k, v) in exports.iter() {
            wasm.set(k.to_str()?, v)?;
        }

        ctx.globals().set("WebAssembly", wasm)?;

        Ok(())
    }
}
