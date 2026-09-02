use std::{cell::RefCell, rc::Rc};

use rquickjs::{Ctx, JsLifetime, Object, Result, class::Trace};

use crate::{JsU32, illegal_constructor, label, type_error};

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUQuerySet")]
pub struct GPUQuerySet {
    #[qjs(skip_trace)]
    pub inner:        wgpu::QuerySet,
    #[qjs(skip_trace)]
    pub(crate) label: Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) count: u32,
    #[qjs(skip_trace)]
    pub(crate) ty:    &'static str,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUQuerySet {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable, rename = "type")]
    pub const fn ty(&self) -> &'static str { self.ty }

    #[qjs(get, configurable)]
    pub const fn count(&self) -> u32 { self.count }

    pub fn destroy(&self) { self.inner.destroy(); }
}

impl GPUQuerySet {
    pub(crate) fn from_descriptor(
        device: &wgpu::Device, descriptor: Object<'_>, ctx: &Ctx<'_>,
    ) -> Result<Self> {
        let label = label(&descriptor)?;
        let ty_name: String = descriptor.get("type")?;
        let (ty, ty_str) = match ty_name.as_str() {
            "occlusion" => (wgpu::QueryType::Occlusion, "occlusion"),
            "timestamp" => (wgpu::QueryType::Timestamp, "timestamp"),
            value => {
                return Err(type_error(ctx, format!("invalid GPUQueryType {value}")));
            }
        };
        let count = descriptor.get::<_, JsU32>("count")?.0;
        let inner = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            ty,
            count,
        });
        Ok(Self {
            inner,
            label: Rc::new(RefCell::new(label)),
            count,
            ty: ty_str,
        })
    }
}

pub fn timestamp_writes_from<'js>(
    writes: &Object<'js>, ctx: &Ctx<'js>,
) -> Result<(wgpu::QuerySet, Option<u32>, Option<u32>)> {
    let query = crate::class_value::<GPUQuerySet>(writes, "querySet", ctx)?;
    let beginning = writes
        .get::<_, Option<JsU32>>("beginningOfPassWriteIndex")?
        .map(|value| value.0);
    let end = writes
        .get::<_, Option<JsU32>>("endOfPassWriteIndex")?
        .map(|value| value.0);
    Ok((query.borrow().inner.clone(), beginning, end))
}
