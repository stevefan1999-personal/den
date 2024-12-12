pub mod engine;
pub mod error;
pub mod global;
pub mod instance;
pub mod memory;
pub mod module;
pub mod store;
pub mod table;
pub mod tag;

#[rquickjs::module]
pub mod wasm {
    use std::clone::Clone;

    use either::Either;
    use indexmap::{indexmap, IndexMap};
    use rquickjs::{
        class::Trace, module::Exports, prelude::Opt, ArrayBuffer, Ctx, Exception, IntoJs,
        JsLifetime, Result, TypedArray, Value,
    };

    pub use crate::{
        error::{CompileError, Exception as WasmException, LinkError, RuntimeError},
        global::Global,
        instance::Instance,
        memory::Memory,
        module::Module,
        table::Table,
        tag::Tag,
    };

    #[derive(Trace, JsLifetime, Clone)]
    #[rquickjs::class()]
    pub struct ResultObject {
        #[qjs(get, enumerable)]
        pub module:   crate::module::Module,
        #[qjs(get, enumerable)]
        pub instance: crate::instance::Instance,
    }

    #[rquickjs::methods]
    impl ResultObject {
        #[qjs(constructor)]
        pub fn new() {}
    }

    #[rquickjs::function]
    pub async fn instantiate<'js>(
        module_or_buffer_source: Either<Module, Either<TypedArray<'js, u8>, ArrayBuffer<'js>>>,
        import_object: Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
        ctx: Ctx<'js>,
    ) -> Result<ResultObject> {
        let module = match module_or_buffer_source {
            Either::Left(module) => module,
            Either::Right(buffer_source) => Module::new2(buffer_source, &ctx)?,
        };
        let instance = Instance::new(&module, import_object, ctx)?;
        Ok(ResultObject { module, instance })
    }

    fn validate_inner<'js>(
        buffer_source: &Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: &crate::engine::Engine,
    ) -> Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        Ok(wasmtime::Module::validate(&engine, buf).is_ok())
    }

    #[rquickjs::function]
    pub fn validate<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<bool> {
        validate_inner(
            &buffer_source,
            &ctx.userdata::<crate::engine::Engine>().unwrap(),
        )
    }

    // Unfortunately...this is not supported by wasmtime/wasmi yet
    #[rquickjs::function]
    pub async fn compile<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Module> {
        let engine = &ctx.userdata::<crate::engine::Engine>().unwrap();
        if validate_inner(&buffer_source, &engine)? {
            Module::new_inner(buffer_source, &ctx)
        } else {
            Err(ctx.throw(crate::error::CompileError::new().into_js(&ctx)?))
        }
    }

    // Unfortunately...this is not supported by wasmtime/wasmi yet
    #[rquickjs::function]
    pub fn wat2wasm(source: String, ctx: Ctx<'_>) -> Result<TypedArray<'_, u8>> {
        match wabt::wat2wasm(source) {
            Ok(data) => TypedArray::new(ctx, data),
            Err(e) => {
                Err(Exception::throw_internal(
                    &ctx,
                    &format!("wat2wasm error: {e}"),
                ))
            }
        }
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        let engine = crate::engine::Engine::new();
        let store = crate::store::Store::new(&engine, ctx.clone());
        ctx.store_userdata(store)?;
        ctx.store_userdata(engine)?;
        ctx.globals().set(
            "WebAssembly",
            indexmap! {
                "instantiate" => js_instantiate.into_js(ctx)?,
                "validate" => js_validate.into_js(ctx)?,
                "compile" => js_compile.into_js(ctx)?,
                "wat2wasm" => js_wat2wasm.into_js(ctx)?,
                "Module" => indexmap! {
                    "imports" => crate::module::Module::js_imports.into_js(ctx)?,
                    "exports" => crate::module::Module::js_exports.into_js(ctx)?,
                    "customSections" => crate::module::Module::js_custom_sections.into_js(ctx)?,

                }.into_js(ctx)?,
            },
        )?;

        Ok(())
    }
}

pub mod utils;
