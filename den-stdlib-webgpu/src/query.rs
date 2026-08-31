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

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, rename = "type")]
    pub const fn ty(&self) -> &'static str { self.ty }

    #[qjs(get)]
    pub const fn count(&self) -> u32 { self.count }

    pub fn destroy(&self) { self.inner.destroy(); }
}

impl GPUQuerySet {
    pub(crate) fn from_descriptor(
        device: &wgpu::Device, descriptor: Object<'_>, ctx: &Ctx<'_>,
    ) -> Result<Self> {
        let label = label(&descriptor)?;
        let ty_name: String = descriptor.get("type")?;
        let ty = match ty_name.as_str() {
            "occlusion" => wgpu::QueryType::Occlusion,
            "timestamp" => wgpu::QueryType::Timestamp,
            value => {
                return Err(type_error(ctx, format!("invalid GPUQueryType {value}")));
            }
        };
        let count = descriptor.get::<_, JsU32>("count")?.0;
        Ok(Self {
            inner: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                ty,
                count,
            }),
            label: Rc::new(RefCell::new(label)),
            count,
            ty: match ty {
                wgpu::QueryType::Occlusion => "occlusion",
                wgpu::QueryType::Timestamp => "timestamp",
                wgpu::QueryType::PipelineStatistics(_) => "pipeline-statistics",
            },
        })
    }
}
