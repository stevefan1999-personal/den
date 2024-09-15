pub mod error;
pub mod global;
pub mod instance;
pub mod memory;
pub mod module;
pub mod table;
pub mod tag;

#[rquickjs::module]
pub mod wasm {
    use either::Either;
    use rquickjs::{
        class::Trace, module::Exports, prelude::Opt, ArrayBuffer, Ctx, Object, TypedArray,
    };

    pub use crate::{
        error::{CompileError, Exception, LinkError, RuntimeError},
        global::Global,
        instance::Instance,
        memory::Memory,
        module::Module,
        table::Table,
        tag::Tag,
    };

    #[derive(Trace, Clone)]
    #[rquickjs::class()]
    pub struct ResultObject {
        #[qjs(get)]
        pub module:   crate::module::Module,
        #[qjs(get)]
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
        import_object: Opt<Object<'js>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<ResultObject> {
        let module = match module_or_buffer_source {
            Either::Left(module) => module,
            Either::Right(buffer_source) => Module::new(buffer_source, ctx.clone())?,
        };
        let instance = Instance::new(&module, import_object, ctx)?;
        Ok(ResultObject { module, instance })
    }

    #[rquickjs::function]
    pub fn validate<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        let mut config = wasmtime::Config::default();
        config.consume_fuel(true);

        let engine = wasmtime::Engine::new(&config).map_err(|x| {
            rquickjs::Exception::throw_internal(&ctx, &format!("wasm engine creation error: {}", x))
        })?;

        Ok(wasmtime::Module::validate(&engine, buf).is_ok())
    }

    // Unfortunately...this is not supported by wasmtime/wasmi yet
    #[rquickjs::function]
    pub async fn compile<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Module> {
        // https://webassembly.github.io/spec/js-api/#compile-a-webassembly-module
        if validate(buffer_source.clone(), ctx.clone())? {
            Module::new(buffer_source, ctx)
        } else {
            Err(rquickjs::Exception::throw_internal(
                &ctx,
                &format!("wasm compile error: unknown"),
            ))
        }
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> rquickjs::Result<()> {
        let wasm = Object::new(ctx.clone())?;
        // for (k, v) in exports.iter() {
        //     wasm.set(k.to_str()?, v)?;
        // }

        ctx.globals().set("WebAssembly", wasm)?;

        Ok(())
    }
}
