use derive_more::Deref;
use either::Either;
use getset::Getters;
use rquickjs::{class::Trace, ArrayBuffer, Ctx, Object, TypedArray};
use wasmtime::ExternType;

#[derive(Trace, Getters, Deref, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    #[getset(get)]
    pub(crate) module: wasmtime::Module,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Module {
    #[qjs(constructor)]
    pub fn new<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Self> {
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

        let module = wasmtime::Module::from_binary(&engine, buf).map_err(|x| {
            rquickjs::Exception::throw_internal(&ctx, &format!("wasm module creation error: {}", x))
        })?;

        Ok(Self { module })
    }

    #[qjs(static)]
    pub fn imports<'js>(module: &Module, ctx: Ctx<'js>) -> rquickjs::Result<Vec<Object<'js>>> {
        module
            .imports()
            .map(|import| {
                let obj = Object::new(ctx.clone())?;
                obj.set("module", import.module())?;
                obj.set("name", import.name())?;
                obj.set("kind", extern_type_to_str(import.ty()))?;
                Ok(obj)
            })
            .collect::<rquickjs::Result<Vec<_>>>()
    }

    #[qjs(static)]
    pub fn exports<'js>(module: &Module, ctx: Ctx<'js>) -> rquickjs::Result<Vec<Object<'js>>> {
        module
            .exports()
            .map(|import| {
                let obj = Object::new(ctx.clone())?;
                obj.set("name", import.name())?;
                obj.set("kind", extern_type_to_str(import.ty()))?;
                Ok(obj)
            })
            .collect::<rquickjs::Result<Vec<_>>>()
    }

    #[qjs(static)]
    pub fn custom_sections<'js>(
        _module: &Module,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Vec<Object<'js>>> {
        Err(rquickjs::Exception::throw_internal(&ctx, "not implemented"))
    }
}

fn extern_type_to_str(x: ExternType) -> &'static str {
    match x {
        wasmtime::ExternType::Func(_) => "function",
        wasmtime::ExternType::Global(_) => "global",
        wasmtime::ExternType::Table(_) => "table",
        wasmtime::ExternType::Memory(_) => "memory",
    }
}
