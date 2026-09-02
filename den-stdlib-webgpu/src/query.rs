use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use rquickjs::{Ctx, JsLifetime, Object, Result, class::Trace};

use crate::{ErrorSink, JsU32, illegal_constructor, label, type_error};

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUQuerySet")]
pub struct GPUQuerySet {
    #[qjs(skip_trace)]
    pub inner:            wgpu::QuerySet,
    #[qjs(skip_trace)]
    pub(crate) label:     Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) count:     u32,
    #[qjs(skip_trace)]
    pub(crate) invalid:   bool,
    #[qjs(skip_trace)]
    pub(crate) ty:        &'static str,
    #[qjs(skip_trace)]
    pub(crate) device_id: u64,
    #[qjs(skip_trace)]
    pub(crate) destroyed: Rc<Cell<bool>>,
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

    pub fn destroy(&self) {
        self.destroyed.set(true);
        self.inner.destroy();
    }
}

impl GPUQuerySet {
    pub(crate) fn from_descriptor(
        device: &wgpu::Device, descriptor: Object<'_>, ctx: &Ctx<'_>, errors: &ErrorSink,
        device_id: u64,
    ) -> Result<Self> {
        const MAX_QUERY_COUNT: u32 = 4096;
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
        let too_large = count > MAX_QUERY_COUNT;
        if too_large {
            errors.validation("query set count exceeds the maximum");
        }
        let timestamp_unfeatured = matches!(ty, wgpu::QueryType::Timestamp)
            && !device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        if timestamp_unfeatured {
            return Err(type_error(
                ctx,
                "timestamp-query is not enabled on this device",
            ));
        }
        let skip_native = too_large;
        // wgpu rejects count 0; the spec/CTS treat it as valid.
        let gpu_count = if skip_native || count == 0 { 1 } else { count };
        let inner = if skip_native {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty:    wgpu::QueryType::Occlusion,
                count: 1,
            })
        } else {
            crate::catch_gpu(errors, || {
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: (!label.is_empty()).then_some(label.as_str()),
                    ty,
                    count: gpu_count,
                })
            })
            .unwrap_or_else(|| {
                device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: None,
                    ty:    wgpu::QueryType::Occlusion,
                    count: 1,
                })
            })
        };
        Ok(Self {
            inner,
            label: Rc::new(RefCell::new(label)),
            count,
            invalid: skip_native,
            ty: match ty {
                wgpu::QueryType::Occlusion => "occlusion",
                wgpu::QueryType::Timestamp => "timestamp",
                wgpu::QueryType::PipelineStatistics(_) => "pipeline-statistics",
            },
            device_id,
            destroyed: Rc::new(Cell::new(false)),
        })
    }
}

pub fn timestamp_writes_from<'js>(
    writes: &Object<'js>, ctx: &Ctx<'js>, device_id: u64,
) -> Result<(wgpu::QuerySet, Option<u32>, Option<u32>, bool)> {
    let query = crate::class_value::<GPUQuerySet>(writes, "querySet", ctx)?;
    let query = query.borrow();
    let beginning = writes
        .get::<_, Option<crate::JsU32>>("beginningOfPassWriteIndex")?
        .map(|value| value.0);
    let end = writes
        .get::<_, Option<crate::JsU32>>("endOfPassWriteIndex")?
        .map(|value| value.0);
    let index_oob = beginning.is_some_and(|index| index >= query.count)
        || end.is_some_and(|index| index >= query.count);
    let invalid = query.invalid
        || query.ty != "timestamp"
        || query.device_id != device_id
        || index_oob
        || beginning == end;
    Ok((query.inner.clone(), beginning, end, invalid))
}
