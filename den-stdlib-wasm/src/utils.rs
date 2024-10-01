use derive_more::derive::{Deref, DerefMut, From, Into};
use rquickjs::{prelude::*, Ctx, Result, Value};
use wasmtime::{RefType, Val, ValType};

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
            rquickjs::Type::Uninitialized | rquickjs::Type::Undefined | rquickjs::Type::Null => {
                Ok(Self(wasmtime::Val::null_any_ref()))
            }
            rquickjs::Type::Bool => Ok(Self(wasmtime::Val::I32(value.as_bool().unwrap().into()))),
            rquickjs::Type::Int if value.is_int() => Ok(Self(value.as_int().unwrap().into())),
            rquickjs::Type::Float => Ok(Self(value.as_number().unwrap().into())),
            rquickjs::Type::BigInt => Ok(Self(value.into_big_int().unwrap().to_i64()?.into())),
            _ => Err(rquickjs::Exception::throw_type(ctx, "not implemented")),
        }
    }
}

pub(crate) fn get_default_value_for_val_type(x: &ValType) -> std::result::Result<Val, ()> {
    match x {
        wasmtime::ValType::I32 => Ok(Val::I32(0)),
        wasmtime::ValType::I64 => Ok(Val::I64(0)),
        wasmtime::ValType::F32 => Ok(Val::F32(0)),
        wasmtime::ValType::F64 => Ok(Val::F64(0)),
        wasmtime::ValType::V128 => Ok(Val::V128(0_u128.into())),
        wasmtime::ValType::Ref(ref_type) if ref_type.matches(&RefType::FUNCREF) => {
            Ok(Val::null_func_ref())
        }
        wasmtime::ValType::Ref(ref_type) if ref_type.matches(&RefType::EXTERNREF) => {
            Ok(Val::null_extern_ref())
        }
        wasmtime::ValType::Ref(ref_type) if ref_type.matches(&RefType::ANYREF) => {
            Ok(Val::null_any_ref())
        }
        _ => Err(()),
    }
}
