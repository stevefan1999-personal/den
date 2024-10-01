use rquickjs::runtime::UserData;

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
        class::Trace, module::Exports, prelude::Opt, ArrayBuffer, Ctx, Exception, IntoJs, Result,
        TypedArray, Value,
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

    #[derive(Trace, Clone)]
    #[rquickjs::class()]
    pub struct ResultObject<'js> {
        #[qjs(get, enumerable)]
        pub module:   crate::module::Module,
        #[qjs(get, enumerable)]
        pub instance: crate::instance::Instance<'js>,
    }

    #[rquickjs::methods]
    impl ResultObject<'_> {
        #[qjs(constructor)]
        pub fn new() {}
    }

    #[rquickjs::function]
    pub async fn instantiate<'js>(
        module_or_buffer_source: Either<Module, Either<TypedArray<'js, u8>, ArrayBuffer<'js>>>,
        import_object: Opt<IndexMap<String, IndexMap<String, Value<'js>>>>,
        engine: Opt<crate::engine::Engine>,
        store: Opt<crate::store::Store<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<ResultObject<'js>> {
        let module = match module_or_buffer_source {
            Either::Left(module) => module,
            Either::Right(buffer_source) => Module::new(buffer_source, engine, ctx.clone())?,
        };
        let instance = Instance::new(&module, import_object, store, ctx.clone())?;
        Ok(ResultObject { module, instance })
    }

    #[rquickjs::function]
    pub fn validate<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: Opt<crate::engine::Engine>,
        ctx: Ctx<'js>,
    ) -> Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        let engine = engine.0.unwrap_or(
            ctx.userdata::<crate::WasmtimeRuntimeData>()
                .unwrap()
                .engine
                .clone(),
        );
        let engine = engine.borrow();
        Ok(wasmtime::Module::validate(&engine, buf).is_ok())
    }

    // Unfortunately...this is not supported by wasmtime/wasmi yet
    #[rquickjs::function]
    pub async fn compile<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: Opt<crate::engine::Engine>,
        ctx: Ctx<'js>,
    ) -> Result<Module> {
        // https://webassembly.github.io/spec/js-api/#compile-a-webassembly-module
        if validate(buffer_source.clone(), Opt(engine.clone()), ctx.clone())? {
            Module::new(buffer_source, engine, ctx)
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
        let store = crate::store::Store::new(engine.clone(), ctx.clone());
        ctx.store_userdata(crate::WasmtimeRuntimeData { engine, store })?;
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

#[derive(Clone)]
pub struct WasmtimeRuntimeData<'js> {
    pub(crate) engine: crate::engine::Engine,
    pub(crate) store:  crate::store::Store<'js>,
}

unsafe impl<'js> UserData<'js> for WasmtimeRuntimeData<'js> {
    type Static = WasmtimeRuntimeData<'static>;
}

pub mod utils;
