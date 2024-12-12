use derive_more::{derive::DerefMut, Deref, From, Into};
use indexmap::indexmap;
use rquickjs::{class::Trace, Ctx, Exception, FromJs, IntoJs, JsLifetime, Result, Value};
use typed_builder::TypedBuilder;
use wasmtime::{Ref, RefType, TableType};

use crate::store::StoreData;

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

#[derive(Trace, JsLifetime, Clone, DerefMut, Deref, From, Into)]
#[rquickjs::class]
pub struct Table {
    #[qjs(skip_trace)]
    pub(crate) inner: wasmtime::Table,
}

#[rquickjs::methods]
impl Table {
    #[qjs(skip)]
    pub(crate) fn new_inner<'js>(
        desc: TableDescriptor,
        store: &mut wasmtime::Store<StoreData>,
        ctx: &Ctx<'js>,
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
                    ctx,
                    &format!("Either externref or anyfunc is accepted for element type, found {x}"),
                ));
            }
        };

        let inner = wasmtime::Table::new(store, ty, init).map_err(|x| {
            Exception::throw_internal(ctx, &format!("wasm linker memory new error: {}", x))
        })?;

        Ok(Self { inner })
    }

    #[qjs(constructor)]
    pub fn new<'js>(desc: TableDescriptor, ctx: Ctx<'js>) -> Result<Self> {
        Self::new_inner(
            desc,
            &mut *ctx.userdata::<crate::store::Store>().unwrap().borrow_mut(),
            &ctx,
        )
    }
}
