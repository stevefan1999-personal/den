use derive_more::derive::{Deref, DerefMut, From, Into};
use indexmap::indexmap;
use rquickjs::{class::Trace, Coerced, Ctx, Exception, FromJs, IntoJs, JsLifetime, Result, Value};
use typed_builder::TypedBuilder;
use wasmtime::{AsContext, AsContextMut, GlobalType, Val, ValType};

#[derive(Clone, Trace, JsLifetime, Deref, DerefMut, From, Into)]
#[rquickjs::class]
pub struct Global {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Global,
}

impl Global {
    pub(crate) fn from_type<'js>(
        ty: GlobalType,
        v: &Value<'js>,
        store: impl AsContextMut,
        ctx: &Ctx<'js>,
    ) -> Result<Self> {
        let val: Val = match ty.content() {
            ValType::I32 => (*Coerced::<i32>::from_js(ctx, v.clone())?).into(),
            ValType::I64 => (*Coerced::<i64>::from_js(ctx, v.clone())?).into(),
            ValType::F32 => f32::from_js(ctx, v.clone())?.into(),
            ValType::F64 => (*Coerced::<f64>::from_js(ctx, v.clone())?).into(),
            x if x.matches(&ValType::FUNCREF) && v.is_null() => Val::null_func_ref(),
            x if x.matches(&ValType::EXTERNREF) && v.is_null() => Val::null_extern_ref(),
            x if x.matches(&ValType::FUNCREF) => {
                return Err(Exception::throw_type(ctx, "not a valid func ref"))
            }

            _ => unreachable!(),
        };
        let inner: wasmtime::Global = wasmtime::Global::new(store, ty, val).map_err(|x| {
            Exception::throw_internal(ctx, &format!("wasm linker global new error: {}", x))
        })?;
        Ok(Self { inner })
    }
}

#[rquickjs::methods]
impl Global {
    #[qjs(constructor)]
    pub fn new<'js>(desc: GlobalDescriptor, value: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let store = ctx.userdata::<crate::store::Store>().unwrap();

        let value = match (desc.value.as_str(), value.type_of()) {
            ("i32", rquickjs::Type::Int) => value.as_int().unwrap().into(),
            ("i32", rquickjs::Type::Bool) => wasmtime::Val::I32(value.as_bool().unwrap().into()),
            ("i64", rquickjs::Type::BigInt) => value.as_big_int().unwrap().clone().to_i64()?.into(),
            ("f32", rquickjs::Type::Float) => (value.as_float().unwrap() as f32).into(),
            ("f64", rquickjs::Type::Float) => value.as_float().unwrap().into(),
            ("v128", _) => return Err(ctx.throw("TODO".into_js(&ctx)?)),
            ("externref", _) => return Err(ctx.throw("TODO".into_js(&ctx)?)),
            ("anyfunc", _) => return Err(ctx.throw("TODO".into_js(&ctx)?)),
            (described_type, value_type) => {
                return Err(Exception::throw_internal(
                    &ctx,
                    &format!(
                        "mismatched type, expected {}, found {}",
                        described_type, value_type
                    ),
                ));
            }
        };

        let inner = wasmtime::Global::new(
            store.borrow_mut().as_context_mut(),
            GlobalType::new(
                value.ty(store.borrow().as_context()).unwrap(),
                if desc.mutable.unwrap_or(false) {
                    wasmtime::Mutability::Var
                } else {
                    wasmtime::Mutability::Const
                },
            ),
            value,
        )
        .map_err(|x| Exception::throw_internal(&ctx, &format!("wasm global new error: {}", x)))?;

        Ok(Global { inner })
    }
}

#[derive(Clone, TypedBuilder)]
pub struct GlobalDescriptor {
    value:   String,
    mutable: Option<bool>,
}

impl<'js> FromJs<'js> for GlobalDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if let Some(this) = value.into_object() {
            let value_type: String = this.get("value")?;
            if !matches!(
                value_type.as_str(),
                "i32" | "i64" | "f32" | "f64" | "v128" | "externref" | "anyref"
            ) {
                Err(Exception::throw_internal(
                    ctx,
                    &format!("value must be one of valid wasm type, found {value_type}"),
                ))
            } else {
                Ok(Self {
                    value:   value_type,
                    mutable: this.get("mutable")?,
                })
            }
        } else {
            Err(ctx.throw("not an object".into_js(ctx)?))
        }
    }
}

impl<'js> IntoJs<'js> for GlobalDescriptor {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        indexmap! {
            "value" => self.value.into_js(ctx)?,
            "mutable" => self.mutable.into_js(ctx)?,
        }
        .into_js(ctx)
    }
}
