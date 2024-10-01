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
    use std::{clone::Clone, ops::Index, sync::{Arc, Mutex}};

    use either::Either;
    use indexmap::{indexmap, IndexMap};
    use rquickjs::{
        class::Trace, module::Exports, prelude::Opt, runtime::UserData, ArrayBuffer, Ctx,
        Exception, IntoJs, Result, TypedArray, Value,
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
    ) -> Result<IndexMap<&'js str, Value<'js>>> {
        let module = match module_or_buffer_source {
            Either::Left(module) => module,
            Either::Right(buffer_source) => Module::new(buffer_source, engine, ctx.clone())?,
        };
        let instance = Instance::new(&module, import_object, store, ctx.clone())?.into_js(&ctx)?;
        Ok(indexmap! { "module" => module.into_js(&ctx)?, "instance" => instance })
    }

    #[rquickjs::function]
    pub fn validate<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: Opt<crate::engine::Engine>,
        ctx: Ctx<'js>
    ) -> Result<bool> {
        // https://webassembly.github.io/spec/js-api/#dom-webassembly-validate
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        let engine = engine.clone().unwrap_or(ctx.userdata::<crate::MyUserData>().unwrap().engine.clone()).clone();
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
    pub fn wat2wasm<'js>(source: String, ctx: Ctx<'js>) -> Result<TypedArray<'js, u8>> {
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
        let engine = wasmtime::Engine::default();
        let store = Arc::new(Mutex::new(wasmtime::Store::new(&engine, ctx.clone())));
        ctx.store_userdata(crate::MyUserData { engine: engine.into(), store: store.into() })?;
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
pub struct MyUserData<'js> {
    pub(crate) engine: crate::engine::Engine,
    pub(crate) store:  crate::store::Store<'js>,
}

unsafe impl<'js> UserData<'js> for MyUserData<'js> {
    type Static = MyUserData<'static>;
}

pub mod utils {
    use derive_more::derive::{Deref, DerefMut, From, Into};
    use rquickjs::{prelude::*, Ctx, Result, Value};

    #[derive(Clone, Copy, From, Into, Deref, DerefMut)]
    pub struct WasmValueConverter(wasmtime::Val);

    impl<'js> IntoJs<'js> for WasmValueConverter {
        fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
            match self.0 {
                wasmtime::Val::I32(x) => Ok(x.into_js(ctx)?),
                wasmtime::Val::I64(x) => Ok(x.into_js(ctx)?),
                wasmtime::Val::F32(x) => Ok(x.into_js(ctx)?),
                wasmtime::Val::F64(x) => Ok(x.into_js(ctx)?),
                _ => Err(rquickjs::Exception::throw_type(ctx, "TODO")),
                // x => Ok(Persistent::save(ctx, WasmValue::new(x))?),
            }
        }
    }

    impl<'js> FromJs<'js> for WasmValueConverter {
        fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
            match value.type_of() {
                rquickjs::Type::Uninitialized
                | rquickjs::Type::Undefined
                | rquickjs::Type::Null => Ok(Self(wasmtime::Val::null_any_ref())),
                rquickjs::Type::Bool => {
                    Ok(Self(wasmtime::Val::I32(value.as_bool().unwrap().into())))
                }
                rquickjs::Type::Int if value.is_int() => Ok(Self(value.as_int().unwrap().into())),
                rquickjs::Type::Float => Ok(Self(value.as_number().unwrap().into())),
                rquickjs::Type::BigInt => {
                    Ok(Self(value.into_big_int().unwrap().to_i64()?.into()))
                }
                _ => Err(rquickjs::Exception::throw_type(ctx, "not implemented")),
            }
        }
    }
}

pub mod func {
    use derive_more::derive::{Deref, DerefMut, From, Into};
    use rquickjs::class::Trace;
    #[derive(Trace, Clone, Deref, DerefMut, From, Into)]
    #[rquickjs::class]
    pub struct Function {
        #[qjs(skip_trace)]
        inner: wasmtime::Func,
    }

    #[rquickjs::methods]
    impl Function {
        #[qjs(constructor)]
        pub fn new() {}
    }
}
