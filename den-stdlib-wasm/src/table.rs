use derive_more::{derive::DerefMut, Deref, From, Into};
use indexmap::indexmap;
use rquickjs::{class::Trace, prelude::Opt, Ctx, Exception, FromJs, IntoJs, Result, Value};
use typed_builder::TypedBuilder;
use wasmtime::{Ref, RefType, TableType};

use crate::WasmtimeRuntimeData;

#[derive(Clone, From, Into, TypedBuilder)]
pub struct TableDescriptor {
    initial: u32,
    maximum: Option<u32>,
    element: String,
}

impl<'js> FromJs<'js> for TableDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if let Some(this) = value.into_object() {
            Ok(Self {
                initial: this.get("initial")?,
                maximum: this.get("maximum")?,
                element: this.get("element")?,
            })
        } else {
            Err(ctx.throw("not an object".into_js(ctx)?))
        }
    }
}

impl<'js> IntoJs<'js> for TableDescriptor {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        indexmap! {
            "initial" => self.initial.into_js(ctx)?,
            "maximum" => self.maximum.into_js(ctx)?,
            "element" => self.element.into_js(ctx)?,
        }
        .into_js(ctx)
    }
}

#[derive(Trace, Clone, DerefMut, Deref, From, Into)]
#[rquickjs::class]
pub struct Table {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Table,
}

#[rquickjs::methods]
impl Table {
    #[qjs(constructor)]
    pub fn new<'js>(
        desc: TableDescriptor,
        Opt(store): Opt<crate::store::Store<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let (ty, init) = match desc.element.as_str() {
            "externref" => {
                (
                    TableType::new(RefType::EXTERNREF, desc.initial, desc.maximum),
                    Ref::Extern(None),
                )
            }
            "anyfunc" => {
                (
                    TableType::new(RefType::FUNCREF, desc.initial, desc.maximum),
                    Ref::Any(None),
                )
            }
            x => {
                return Err(Exception::throw_internal(
                    &ctx,
                    &format!("Either externref or anyfunc is accepted for element type, found {x}"),
                ));
            }
        };

        let store = store.unwrap_or(ctx.userdata::<WasmtimeRuntimeData>().unwrap().store.clone());
        let mut store = store.borrow_mut();
        let inner = wasmtime::Table::new(&mut *store, ty, init).map_err(|x| {
            Exception::throw_internal(&ctx, &format!("wasm linker memory new error: {}", x))
        })?;

        Ok(Self { inner })
    }
}
