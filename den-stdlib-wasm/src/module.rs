use std::clone::Clone;

use derive_more::{derive::DerefMut, Deref, From, Into};
use either::Either;
use getset::Getters;
use indexmap::{indexmap, IndexMap};
use rquickjs::{class::Trace, prelude::*, ArrayBuffer, Ctx, Exception, Object, Result, TypedArray};
use wasmtime::ExternType;

use crate::WasmtimeRuntimeData;

#[derive(Trace, Getters, Deref, DerefMut, From, Into, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    #[getset(get)]
    pub(crate) inner: wasmtime::Module,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Module {
    #[qjs(constructor)]
    pub fn new<'js>(
        buffer_source: Either<TypedArray<'js, u8>, ArrayBuffer<'js>>,
        engine: Opt<crate::engine::Engine>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let buf = match buffer_source {
            Either::Left(ref x) => x.as_bytes(),
            Either::Right(ref x) => x.as_bytes(),
        }
        .unwrap();

        let engine = engine.clone().unwrap_or(
            ctx.userdata::<WasmtimeRuntimeData>()
                .unwrap()
                .engine
                .clone(),
        );

        let inner = wasmtime::Module::from_binary(&engine.borrow(), buf).map_err(|x| {
            Exception::throw_internal(&ctx, &format!("wasm module creation error: {}", x))
        })?;

        Ok(Self { inner })
    }

    #[qjs(static)]
    pub fn imports(module: &Module) -> Vec<IndexMap<&str, &str>> {
        module
            .imports()
            .map(|import| {
                indexmap! {
                    "module" => import.module(),
                    "name" => import.name(),
                    "kind" => extern_type_to_str(import.ty())
                }
            })
            .collect::<Vec<_>>()
    }

    #[qjs(static)]
    pub fn exports(module: &Module) -> Vec<IndexMap<&str, &str>> {
        module
            .exports()
            .map(|import| {
                indexmap! {
                    "name" => import.name(),
                    "kind" => extern_type_to_str(import.ty())
                }
            })
            .collect::<Vec<_>>()
    }

    #[qjs(static)]
    pub fn custom_sections<'js>(_module: &Module, ctx: Ctx<'js>) -> Result<Vec<Object<'js>>> {
        Err(Exception::throw_internal(&ctx, "not implemented"))
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
