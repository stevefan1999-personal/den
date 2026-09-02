use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Array, Class, Ctx, FromJs as _, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};

use crate::{
    ErrorSink, GPUBindGroup, GPUBindGroupLayout, GPUBuffer, GPUDevice, GPUPipelineLayout,
    GPUShaderModule, JsU32, JsU64, Recorder, buffer_slice, format, illegal_constructor, label,
    query::{self, GPUQuerySet},
    texture::{GPUTexture, GPUTextureView},
    type_error,
};

struct RenderPipelineInfo {
    dummy:           bool,
    layout_groups:   crate::PipelineLayoutGroups,
    empty_bgl:       wgpu::BindGroupLayout,
    bgls:            Rc<[wgpu::BindGroupLayout]>,
    max_bind_groups: u32,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPipeline")]
pub struct GPURenderPipeline<'js> {
    device:           Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::RenderPipeline,
    #[qjs(skip_trace)]
    info:             Rc<RenderPipelineInfo>,
    #[qjs(skip_trace)]
    pub(crate) label: Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPURenderPipeline<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub(crate) fn dummy(&self) -> bool { self.info.dummy }

    pub fn get_bind_group_layout(&self, index: JsU32, ctx: Ctx<'js>) -> Result<GPUBindGroupLayout> {
        if index.0 >= self.info.max_bind_groups {
            self.device
                .borrow()
                .errors
                .validation("bind group layout index is out of range");
        }
        let entries = self
            .info
            .layout_groups
            .get(index.0 as usize)
            .cloned()
            .unwrap_or_else(|| Rc::from(Vec::new()));
        let inner = if !self.info.dummy && !entries.is_empty() {
            self.inner.get_bind_group_layout(index.0)
        } else {
            self.info
                .bgls
                .get(index.0 as usize)
                .cloned()
                .unwrap_or_else(|| self.info.empty_bgl.clone())
        };
        crate::flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(String::new())),
            entries,
        })
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPassEncoder")]
pub struct GPURenderPassEncoder<'js> {
    device: Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    label:  RefCell<String>,
    #[qjs(skip_trace)]
    pass:   Recorder<wgpu::RenderPass<'static>>,
}

impl<'js> GPURenderPassEncoder<'js> {
    pub(crate) fn new(
        device: Class<'js, GPUDevice<'js>>, label: String,
        pass: Recorder<wgpu::RenderPass<'static>>,
    ) -> Self {
        Self {
            device,
            label: RefCell::new(label),
            pass,
        }
    }

    fn errors(&self) -> ErrorSink { self.device.borrow().errors.clone() }

    fn record(
        &self, ctx: &Ctx<'js>, operation: impl FnOnce(&mut wgpu::RenderPass<'static>),
    ) -> Result<()> {
        self.pass.record(&self.errors(), operation);
        crate::flush_uncaptured(&self.device, ctx)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPURenderPassEncoder<'js> {
    #[qjs(constructor)]
    pub fn new_illegal(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn set_pipeline(
        &self, pipeline: Class<'js, GPURenderPipeline<'js>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        self.record(&ctx, |pass| pass.set_pipeline(&pipeline.borrow().inner))
    }

    pub fn set_bind_group(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let group = bind_group.map(|group| group.borrow().inner.clone());
        self.record(&ctx, |pass| {
            pass.set_bind_group(index.0, group.as_ref(), &offsets);
        })
    }

    pub fn set_vertex_buffer(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.map(|buffer| buffer.borrow().inner.clone());
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        self.record(&ctx, |pass| {
            match buffer
                .as_ref()
                .map(|buffer| buffer_slice(buffer, offset, size))
            {
                None | Some(Ok(None)) => {
                    pass.set_vertex_buffer(slot.0, Option::<wgpu::BufferSlice<'_>>::None);
                }
                Some(Ok(Some(slice))) => pass.set_vertex_buffer(slot.0, slice),
                Some(Err(message)) => self.pass.defer(message),
            }
        })
    }

    pub fn set_index_buffer(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let format = format::index_format(&index_format, &ctx)?;
        let buffer = buffer.borrow().inner.clone();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        self.record(&ctx, |pass| {
            match buffer_slice(&buffer, offset, size) {
                Ok(Some(slice)) => pass.set_index_buffer(slice, format),
                Ok(None) => {}
                Err(message) => self.pass.defer(message),
            }
        })
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        self.record(&ctx, |pass| {
            pass.draw(
                first_vertex..first_vertex.saturating_add(vertex_count.0),
                first_instance..first_instance.saturating_add(instance_count),
            );
        })
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'js>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        self.record(&ctx, |pass| {
            pass.draw_indexed(
                first_index..first_index.saturating_add(index_count.0),
                base_vertex,
                first_instance..first_instance.saturating_add(instance_count),
            );
        })
    }

    pub fn draw_indirect(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        self.record(&ctx, |pass| pass.draw_indirect(&buffer, offset.0))
    }

    pub fn draw_indexed_indirect(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        self.record(&ctx, |pass| pass.draw_indexed_indirect(&buffer, offset.0))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "GPURenderPassEncoder.setViewport matches the WebGPU IDL"
    )]
    pub fn set_viewport(
        &self, x: f64, y: f64, width: f64, height: f64, min_depth: f64, max_depth: f64,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        self.record(&ctx, |pass| {
            pass.set_viewport(
                x as f32,
                y as f32,
                width as f32,
                height as f32,
                min_depth as f32,
                max_depth as f32,
            );
        })
    }

    pub fn set_scissor_rect(
        &self, x: JsU32, y: JsU32, width: JsU32, height: JsU32, ctx: Ctx<'js>,
    ) -> Result<()> {
        self.record(&ctx, |pass| {
            pass.set_scissor_rect(x.0, y.0, width.0, height.0);
        })
    }

    pub fn set_blend_constant(&self, color: Value<'js>, ctx: Ctx<'js>) -> Result<()> {
        let color = format::color(Some(color), &ctx)?;
        self.record(&ctx, |pass| pass.set_blend_constant(color))
    }

    pub fn set_stencil_reference(&self, reference: JsU32, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.set_stencil_reference(reference.0))
    }

    pub fn begin_occlusion_query(&self, query_index: JsU32, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.begin_occlusion_query(query_index.0))
    }

    pub fn end_occlusion_query(&self, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, wgpu::RenderPass::end_occlusion_query)
    }

    pub fn execute_bundles(&self, bundles: Array<'js>, ctx: Ctx<'js>) -> Result<()> {
        let bundles = bundles
            .iter::<Class<GPURenderBundle>>()
            .map(|bundle| Ok(bundle?.borrow().inner.clone()))
            .collect::<Result<Vec<_>>>()?;
        self.record(&ctx, |pass| pass.execute_bundles(bundles.iter()))
    }

    pub fn end(&self, ctx: Ctx<'js>) -> Result<()> {
        // Dropping the pass ends it; wgpu validates the recorded commands
        // here and reports through the sink.
        self.pass.end(&self.errors(), drop);
        crate::flush_uncaptured(&self.device, &ctx)
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, wgpu::RenderPass::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.insert_debug_marker(&value))
    }

    pub fn set_immediates(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let bytes = crate::immediates_bytes(data, data_offset, size, &ctx)?;
        self.record(&ctx, |pass| pass.set_immediates(offset.0, &bytes))
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundle")]
pub struct GPURenderBundle {
    #[qjs(skip_trace)]
    inner: wgpu::RenderBundle,
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPURenderBundle {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

/// Handles whose borrows `extend_borrow` stretched to `'static`.
#[expect(dead_code, reason = "held only to outlive the encoder")]
enum Retained {
    Pipeline(wgpu::RenderPipeline),
    Buffer(wgpu::Buffer),
}

/// `RenderBundleEncoder<'a>` demands `&'a` borrows of the pipeline and the
/// index/indirect buffers, but trunk wgpu-core clones the resource `Arc` at
/// call time (`command/bundle.rs`, `RenderCommand<ArcReferences>`) and never
/// keeps the borrow. Every caller pushes a clone into `Retained` first, so the
/// resource also outlives the encoder by construction.
///
/// SAFETY: `value` is alive for the duration of the call that receives the
/// extended reference, and nothing dereferences it afterwards.
unsafe fn extend_borrow<T>(value: &T) -> &'static T { unsafe { &*std::ptr::from_ref(value) } }

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundleEncoder")]
pub struct GPURenderBundleEncoder<'js> {
    device:   Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    label:    RefCell<String>,
    #[qjs(skip_trace)]
    encoder:  Recorder<wgpu::RenderBundleEncoder<'static>>,
    #[qjs(skip_trace)]
    retained: RefCell<Vec<Retained>>,
}

impl<'js> GPURenderBundleEncoder<'js> {
    fn errors(&self) -> ErrorSink { self.device.borrow().errors.clone() }

    fn record(
        &self, ctx: &Ctx<'js>, operation: impl FnOnce(&mut wgpu::RenderBundleEncoder<'static>),
    ) -> Result<()> {
        self.encoder.record(&self.errors(), operation);
        crate::flush_uncaptured(&self.device, ctx)
    }

    fn retain_buffer(&self, buffer: &wgpu::Buffer) -> &'static wgpu::Buffer {
        self.retained
            .borrow_mut()
            .push(Retained::Buffer(buffer.clone()));
        // SAFETY: see `extend_borrow`; the clone above outlives the encoder.
        unsafe { extend_borrow(buffer) }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPURenderBundleEncoder<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    // The safe `wgpu` crate exposes no debug groups on bundle encoders, so
    // these are no-ops; wgpu-core never sees them.
    pub fn push_debug_group(&self, _value: String) {}

    pub fn pop_debug_group(&self) {}

    pub fn insert_debug_marker(&self, _value: String) {}

    pub fn set_pipeline(
        &self, pipeline: Class<'js, GPURenderPipeline<'js>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let pipeline = pipeline.borrow().inner.clone();
        self.retained
            .borrow_mut()
            .push(Retained::Pipeline(pipeline.clone()));
        // SAFETY: see `extend_borrow`; the clone above outlives the encoder.
        let pipeline = unsafe { extend_borrow(&pipeline) };
        self.record(&ctx, |encoder| encoder.set_pipeline(pipeline))
    }

    pub fn set_bind_group(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let group = bind_group.map(|group| group.borrow().inner.clone());
        self.record(&ctx, |encoder| {
            encoder.set_bind_group(index.0, group.as_ref(), &offsets);
        })
    }

    pub fn set_vertex_buffer(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.map(|buffer| buffer.borrow().inner.clone());
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        self.record(&ctx, |encoder| {
            match buffer
                .as_ref()
                .map(|buffer| buffer_slice(buffer, offset, size))
            {
                None | Some(Ok(None)) => {
                    encoder.set_vertex_buffer(slot.0, Option::<wgpu::BufferSlice<'_>>::None);
                }
                Some(Ok(Some(slice))) => encoder.set_vertex_buffer(slot.0, slice),
                Some(Err(message)) => self.encoder.defer(message),
            }
        })
    }

    pub fn set_index_buffer(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let format = format::index_format(&index_format, &ctx)?;
        let buffer = self.retain_buffer(&buffer.borrow().inner);
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        self.record(&ctx, |encoder| {
            match buffer_slice(buffer, offset, size) {
                Ok(Some(slice)) => encoder.set_index_buffer(slice, format),
                Ok(None) => {}
                Err(message) => self.encoder.defer(message),
            }
        })
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        self.record(&ctx, |encoder| {
            encoder.draw(
                first_vertex..first_vertex.saturating_add(vertex_count.0),
                first_instance..first_instance.saturating_add(instance_count),
            );
        })
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'js>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        self.record(&ctx, |encoder| {
            encoder.draw_indexed(
                first_index..first_index.saturating_add(index_count.0),
                base_vertex,
                first_instance..first_instance.saturating_add(instance_count),
            );
        })
    }

    pub fn draw_indirect(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = self.retain_buffer(&buffer.borrow().inner);
        self.record(&ctx, |encoder| encoder.draw_indirect(buffer, offset.0))
    }

    pub fn draw_indexed_indirect(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = self.retain_buffer(&buffer.borrow().inner);
        self.record(&ctx, |encoder| {
            encoder.draw_indexed_indirect(buffer, offset.0);
        })
    }

    pub fn set_immediates(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let bytes = crate::immediates_bytes(data, data_offset, size, &ctx)?;
        self.record(&ctx, |encoder| encoder.set_immediates(offset.0, &bytes))
    }

    pub fn finish(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPURenderBundle> {
        let label = descriptor
            .0
            .flatten()
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let errors = self.errors();
        let inner = self.encoder.end(&errors, |encoder| {
            encoder.finish(&wgpu::RenderBundleDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
            })
        });
        crate::flush_uncaptured(&self.device, &ctx)?;
        // A second finish has no encoder left; the error was reported above
        // and an empty bundle stands in for the invalid one wgpu would return.
        let inner = inner.unwrap_or_else(|| {
            self.device
                .borrow()
                .device
                .create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor::default())
                .finish(&wgpu::RenderBundleDescriptor::default())
        });
        Ok(GPURenderBundle {
            inner,
            label: Rc::new(RefCell::new(label)),
        })
    }
}

pub fn new_bundle_encoder<'js>(
    device: &Class<'js, GPUDevice<'js>>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<GPURenderBundleEncoder<'js>> {
    let label = label(&descriptor)?;
    let features = device.borrow().device.features();
    let color_formats = descriptor
        .get::<_, Array>("colorFormats")
        .map_err(|_error| type_error(ctx, "colorFormats must be an array"))?
        .iter::<Option<String>>()
        .map(|name| {
            name?
                .map(|name| format::texture_format_with_features(&name, features, ctx))
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    let depth_stencil = descriptor
        .get::<_, Option<String>>("depthStencilFormat")?
        .map(|name| {
            Ok::<_, rquickjs::Error>(wgpu::RenderBundleDepthStencil {
                format:            format::texture_format_with_features(&name, features, ctx)?,
                depth_read_only:   descriptor
                    .get::<_, Option<bool>>("depthReadOnly")?
                    .unwrap_or_default(),
                stencil_read_only: descriptor
                    .get::<_, Option<bool>>("stencilReadOnly")?
                    .unwrap_or_default(),
            })
        })
        .transpose()?;
    let sample_count = descriptor
        .get::<_, Option<JsU32>>("sampleCount")?
        .map_or(1, |value| value.0);
    let encoder =
        device
            .borrow()
            .device
            .create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                color_formats: &color_formats,
                depth_stencil,
                sample_count,
                multiview: None,
            });
    Ok(GPURenderBundleEncoder {
        device:   device.clone(),
        label:    RefCell::new(label),
        encoder:  Recorder::open("GPURenderBundleEncoder", encoder),
        retained: RefCell::new(Vec::new()),
    })
}

pub fn create_pipeline<'js>(
    device_class: &Class<'js, GPUDevice<'js>>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(GPURenderPipeline<'js>, bool)> {
    let device = device_class.borrow();
    let label = label(&descriptor)?;
    let layout_value: Value = descriptor.get("layout")?;
    let (layout, layout_id, layout_groups, layout_immediate) = if layout_value.is_undefined()
        || layout_value
            .as_string()
            .map(rquickjs::String::to_string)
            .transpose()?
            .as_deref()
            == Some("auto")
    {
        (
            None,
            device.id,
            None,
            device.device.limits().max_immediate_size,
        )
    } else {
        let layout_js = Class::<GPUPipelineLayout>::from_js(ctx, layout_value)?;
        let layout = layout_js.borrow();
        (
            Some(layout.inner.clone()),
            layout.device_id,
            Some(layout.groups.clone()),
            layout.immediate_size,
        )
    };
    let vertex: Object = descriptor.get("vertex")?;
    let vertex_js = crate::class_value::<GPUShaderModule>(&vertex, "module", ctx)?;
    let vertex_invalid = vertex_js.borrow().invalid;
    let vertex_code = vertex_js.borrow().code.clone();
    let vertex_id = vertex_js.borrow().device_id;
    let vertex_module = vertex_js.borrow().inner.clone();
    let vertex_entry = vertex.get::<_, Option<String>>("entryPoint")?;
    let vertex_constants = format::pipeline_constants(vertex.get("constants")?, ctx)?;
    let vertex_constant_pairs = vertex_constants
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect::<Vec<_>>();
    let buffers_value: Option<Array> = vertex.get("buffers")?;
    let mut owned_buffers = Vec::new();
    if let Some(buffers) = buffers_value {
        for buffer in buffers.iter::<Option<Object>>() {
            let Some(buffer) = buffer? else {
                owned_buffers.push(None);
                continue;
            };
            let array_stride = buffer.get::<_, JsU64>("arrayStride")?.0;
            let step_mode = match buffer
                .get::<_, Option<String>>("stepMode")?
                .as_deref()
                .unwrap_or("vertex")
            {
                "vertex" => wgpu::VertexStepMode::Vertex,
                "instance" => wgpu::VertexStepMode::Instance,
                value => {
                    return Err(type_error(
                        ctx,
                        format!("invalid GPUVertexStepMode {value}"),
                    ));
                }
            };
            let attributes = buffer
                .get::<_, Array>("attributes")
                .map_err(|_error| type_error(ctx, "vertex attributes must be an array"))?;
            let mut native_attributes = Vec::with_capacity(attributes.len());
            for attribute in attributes.iter::<Object>() {
                let attribute = attribute?;
                native_attributes.push(wgpu::VertexAttribute {
                    format:          format::vertex_format(
                        &attribute.get::<_, String>("format")?,
                        ctx,
                    )?,
                    offset:          attribute.get::<_, JsU64>("offset")?.0,
                    shader_location: attribute.get::<_, JsU32>("shaderLocation")?.0,
                });
            }
            owned_buffers.push(Some((array_stride, step_mode, native_attributes)));
        }
    }
    let vertex_buffers = owned_buffers
        .iter()
        .map(|buffer| {
            buffer.as_ref().map(|(stride, step, attributes)| {
                wgpu::VertexBufferLayout {
                    array_stride: *stride,
                    step_mode:    *step,
                    attributes:   attributes.as_slice(),
                }
            })
        })
        .collect::<Vec<_>>();
    let primitive = primitive_state(descriptor.get("primitive")?, ctx)?;
    let depth_stencil = depth_stencil_state(descriptor.get("depthStencil")?, ctx)?;
    let multisample = multisample_state(descriptor.get("multisample")?, ctx)?;
    let fragment_object: Option<Object> = descriptor.get("fragment")?;
    let fragment_js;
    let fragment_module;
    let fragment_entry;
    let fragment_constants;
    let fragment_constant_pairs;
    let fragment_targets;
    let mut fragment_invalid = false;
    let mut fragment_id = device.id;
    let mut fragment_code = None;
    let mut write_mask_invalid = false;
    let fragment = if let Some(fragment) = fragment_object {
        fragment_js = crate::class_value::<GPUShaderModule>(&fragment, "module", ctx)?;
        fragment_invalid = fragment_js.borrow().invalid;
        fragment_code = Some(fragment_js.borrow().code.clone());
        fragment_id = fragment_js.borrow().device_id;
        fragment_module = fragment_js.borrow().inner.clone();
        fragment_entry = fragment.get::<_, Option<String>>("entryPoint")?;
        fragment_constants = format::pipeline_constants(fragment.get("constants")?, ctx)?;
        fragment_constant_pairs = fragment_constants
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect::<Vec<_>>();
        let targets = fragment
            .get::<_, Array>("targets")
            .map_err(|_error| type_error(ctx, "fragment targets must be an array"))?;
        let mut native_targets = Vec::with_capacity(targets.len());
        for target in targets.iter::<Option<Object>>() {
            let (state, mask_invalid) = color_target(target?, ctx)?;
            write_mask_invalid |= mask_invalid;
            native_targets.push(state);
        }
        fragment_targets = native_targets;
        Some(wgpu::FragmentState {
            module:              &fragment_module,
            entry_point:         fragment_entry.as_deref(),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants:                        &fragment_constant_pairs,
                zero_initialize_workgroup_memory: true,
            },
            targets:             &fragment_targets,
        })
    } else {
        fragment_entry = None;
        fragment_constants = Vec::new();
        fragment_targets = Vec::new();
        None
    };
    let vertex_stage_invalid = format::render_stage_invalid(
        &vertex_code,
        format::ShaderStage::Vertex,
        vertex_entry.as_deref(),
        &vertex_constants,
    );
    let fragment_stage_invalid = fragment_code.as_deref().is_some_and(|code| {
        format::render_stage_invalid(
            code,
            format::ShaderStage::Fragment,
            fragment_entry.as_deref(),
            &fragment_constants,
        )
    });
    let mismatched = vertex_id != device.id || layout_id != device.id || fragment_id != device.id;
    let no_target = fragment_targets.iter().all(Option::is_none) && depth_stencil.is_none();
    let depth_format_invalid = depth_stencil.as_ref().is_some_and(|state| {
        !state.format.has_depth_aspect() && !state.format.has_stencil_aspect()
    });
    let color_format_invalid = fragment_targets.iter().any(|target| {
        target.as_ref().is_some_and(|state| {
            state.format.has_depth_aspect() || state.format.has_stencil_aspect()
        })
    });
    let sample_invalid = !matches!(multisample.count, 1 | 4);
    let depth_stencil_invalid = depth_stencil.as_ref().is_some_and(depth_stencil_invalid);
    let features = device.device.features();
    if let Some(state) = depth_stencil.as_ref()
        && !features.contains(state.format.required_features())
    {
        return Err(type_error(
            ctx,
            "depth stencil format requires missing features",
        ));
    }
    if fragment_targets
        .iter()
        .flatten()
        .any(|state| !features.contains(state.format.required_features()))
    {
        return Err(type_error(
            ctx,
            "color target format requires missing features",
        ));
    }
    let limits = device.reported_limits();
    let native_limits = device.device.limits();
    let blend_invalid = fragment_targets.iter().any(|target| {
        target
            .as_ref()
            .is_some_and(|state| color_target_invalid(state, features))
    });
    let storage_invalid = format::storage_texture_access_unsupported(&vertex_code, features)
        || fragment_code
            .as_deref()
            .is_some_and(|code| format::storage_texture_access_unsupported(code, features));
    let frag_depth_invalid = fragment_code
        .as_deref()
        .is_some_and(|code| format::writes_frag_depth(code, fragment_entry.as_deref()))
        && depth_stencil
            .as_ref()
            .is_none_or(|state| !state.format.has_depth_aspect());
    let depth_bias_invalid = depth_stencil.as_ref().is_some_and(|state| {
        let biased =
            state.bias.constant != 0 || state.bias.slope_scale != 0.0 || state.bias.clamp != 0.0;
        biased
            && !matches!(
                primitive.topology,
                wgpu::PrimitiveTopology::TriangleList | wgpu::PrimitiveTopology::TriangleStrip
            )
    });
    let strip_index_invalid = primitive.strip_index_format.is_some()
        && !matches!(
            primitive.topology,
            wgpu::PrimitiveTopology::LineStrip | wgpu::PrimitiveTopology::TriangleStrip
        );
    let unclipped_invalid =
        primitive.unclipped_depth && !features.contains(wgpu::Features::DEPTH_CLIP_CONTROL);
    let a2c_count_invalid = multisample.alpha_to_coverage_enabled && multisample.count != 4;
    let sample_mask_a2c = multisample.alpha_to_coverage_enabled
        && fragment_code
            .as_deref()
            .is_some_and(|code| format::writes_sample_mask(code, fragment_entry.as_deref()));
    let dual_src_invalid = format::writes_blend_src(fragment_code.as_deref().unwrap_or(""))
        && !features.contains(wgpu::Features::DUAL_SOURCE_BLENDING);
    let too_many_targets = fragment_targets.len() as u32 > limits.max_color_attachments;
    let color_bytes_invalid = color_bytes_per_sample_invalid(
        &fragment_targets,
        limits.max_color_attachment_bytes_per_sample,
    );
    let vertex_buffers_invalid = vertex_buffers_invalid(&owned_buffers, &limits);
    let vertex_attrs: Vec<(u32, wgpu::VertexFormat)> = owned_buffers
        .iter()
        .flatten()
        .flat_map(|(_stride, _step, attributes)| {
            attributes
                .iter()
                .map(|attribute| (attribute.shader_location, attribute.format))
        })
        .collect();
    let vertex_io_invalid = format::vertex_inputs_invalid(&vertex_code, &vertex_attrs);
    let inter_stage_invalid = format::inter_stage_invalid(
        &vertex_code,
        fragment_code.as_deref(),
        limits.max_inter_stage_shader_variables,
        primitive.topology == wgpu::PrimitiveTopology::PointList,
        fragment_entry.as_deref(),
    );
    let skip_native_inter_stage = !inter_stage_invalid
        && native_limits.max_inter_stage_shader_variables < limits.max_inter_stage_shader_variables
        && format::inter_stage_invalid(
            &vertex_code,
            fragment_code.as_deref(),
            native_limits.max_inter_stage_shader_variables,
            primitive.topology == wgpu::PrimitiveTopology::PointList,
            fragment_entry.as_deref(),
        );
    let naga_interp_skip = format::inter_stage_naga_skip(
        &vertex_code,
        fragment_code.as_deref(),
        fragment_entry.as_deref(),
    );
    let fragment_io_invalid = fragment_code.as_deref().is_some_and(|code| {
        format::fragment_color_io_invalid(
            code,
            &fragment_targets,
            features,
            fragment_entry.as_deref(),
        )
    });
    let layout_mismatch = layout_groups.as_deref().is_some_and(|groups| {
        crate::layout_shader_mismatch(&vertex_code, groups, wgpu::ShaderStages::VERTEX)
            || fragment_code.as_deref().is_some_and(|code| {
                crate::layout_shader_mismatch(code, groups, wgpu::ShaderStages::FRAGMENT)
            })
    });
    let integer_filter = layout_groups
        .as_deref()
        .is_some_and(crate::layout_filtering_nonfilterable);
    let stored_groups = layout_groups.unwrap_or_else(|| {
        crate::auto_layout_groups(&[
            (wgpu::ShaderStages::VERTEX, vertex_code.as_ref()),
            (
                wgpu::ShaderStages::FRAGMENT,
                fragment_code.as_deref().unwrap_or(""),
            ),
        ])
    });
    let immediate_unusable = format::naga_immediate_unusable(&vertex_code)
        || fragment_code
            .as_deref()
            .is_some_and(format::naga_immediate_unusable);
    let clip_unfeatured = vertex_code.contains("clip_distances")
        && !features.contains(wgpu::Features::CLIP_DISTANCES);
    let immediate_over = format::immediate_byte_size(&vertex_code) > layout_immediate
        || fragment_code
            .as_deref()
            .is_some_and(|code| format::immediate_byte_size(code) > layout_immediate);
    let invalid = vertex_invalid
        || fragment_invalid
        || mismatched
        || vertex_stage_invalid
        || fragment_stage_invalid
        || no_target
        || depth_format_invalid
        || color_format_invalid
        || sample_invalid
        || depth_stencil_invalid
        || blend_invalid
        || storage_invalid
        || frag_depth_invalid
        || depth_bias_invalid
        || strip_index_invalid
        || unclipped_invalid
        || a2c_count_invalid
        || sample_mask_a2c
        || dual_src_invalid
        || too_many_targets
        || color_bytes_invalid
        || write_mask_invalid
        || vertex_buffers_invalid
        || vertex_io_invalid
        || inter_stage_invalid
        || fragment_io_invalid
        || layout_mismatch
        || clip_unfeatured
        || immediate_unusable
        || immediate_over
        || integer_filter;
    let has_external = format::uses_external_texture(&vertex_code)
        || fragment_code
            .as_deref()
            .is_some_and(format::uses_external_texture);
    let no_frag_out = fragment_code
        .as_deref()
        .is_some_and(|code| format::fragment_has_no_color_outputs(code, fragment_entry.as_deref()));
    let empty_bgl = device.fallbacks.bind_group_layout.clone();
    let skip_storage = vertex_code.contains("texture_storage_")
        || fragment_code
            .as_deref()
            .is_some_and(|code| code.contains("texture_storage_"));
    let skip_immediate = vertex_code.contains("var<immediate")
        || fragment_code
            .as_deref()
            .is_some_and(|code| code.contains("var<immediate"));
    let bgls = crate::bind_group_layouts_from_groups(&device.device, &stored_groups, &empty_bgl);
    let max_bind_groups = device.reported_limits().max_bind_groups;
    let make = |inner: wgpu::RenderPipeline, dummy: bool| {
        GPURenderPipeline {
            device: device_class.clone(),
            inner,
            info: Rc::new(RenderPipelineInfo {
                dummy,
                layout_groups: stored_groups.clone(),
                empty_bgl: empty_bgl.clone(),
                bgls: bgls.clone(),
                max_bind_groups,
            }),
            label: Rc::new(RefCell::new(label.clone())),
        }
    };
    if invalid
        || has_external
        || naga_interp_skip
        || no_frag_out
        || skip_storage
        || skip_immediate
        || skip_native_inter_stage
    {
        return Ok((
            make(device.fallbacks.render_pipeline.clone(), true),
            invalid,
        ));
    }
    device.errors.push(crate::GPUErrorKind::Validation);
    let inner = device
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            layout: layout.as_ref(),
            vertex: wgpu::VertexState {
                module:              &vertex_module,
                entry_point:         vertex_entry.as_deref(),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants:                        &vertex_constant_pairs,
                    zero_initialize_workgroup_memory: true,
                },
                buffers:             &vertex_buffers,
            },
            primitive,
            depth_stencil,
            multisample,
            fragment,
            multiview_mask: None,
            cache: None,
        });
    let scoped = device.errors.pop().unwrap_or_default();
    let invalid = scoped.is_some();
    if let Some(error) = scoped {
        device.errors.capture(error);
    }
    Ok((make(inner, invalid), invalid))
}

fn primitive_state(object: Option<Object<'_>>, ctx: &Ctx<'_>) -> Result<wgpu::PrimitiveState> {
    let Some(object) = object else {
        return Ok(wgpu::PrimitiveState::default());
    };
    let topology = object
        .get::<_, Option<String>>("topology")?
        .as_deref()
        .map_or(Ok(wgpu::PrimitiveTopology::TriangleList), |name| {
            format::primitive_topology(name, ctx)
        })?;
    let strip_index_format = object
        .get::<_, Option<String>>("stripIndexFormat")?
        .map(|name| format::index_format(&name, ctx))
        .transpose()?;
    let front_face = match object
        .get::<_, Option<String>>("frontFace")?
        .as_deref()
        .unwrap_or("ccw")
    {
        "ccw" => wgpu::FrontFace::Ccw,
        "cw" => wgpu::FrontFace::Cw,
        value => return Err(type_error(ctx, format!("invalid GPUFrontFace {value}"))),
    };
    let cull_mode = match object.get::<_, Option<String>>("cullMode")?.as_deref() {
        None | Some("none") => None,
        Some("front") => Some(wgpu::Face::Front),
        Some("back") => Some(wgpu::Face::Back),
        Some(value) => return Err(type_error(ctx, format!("invalid GPUCullMode {value}"))),
    };
    Ok(wgpu::PrimitiveState {
        topology,
        strip_index_format,
        front_face,
        cull_mode,
        unclipped_depth: object
            .get::<_, Option<bool>>("unclippedDepth")?
            .unwrap_or_default(),
        polygon_mode: wgpu::PolygonMode::Fill,
        conservative: false,
    })
}

fn multisample_state(object: Option<Object<'_>>, _ctx: &Ctx<'_>) -> Result<wgpu::MultisampleState> {
    let Some(object) = object else {
        return Ok(wgpu::MultisampleState::default());
    };
    Ok(wgpu::MultisampleState {
        count:                     object
            .get::<_, Option<JsU32>>("count")?
            .map_or(1, |value| value.0),
        mask:                      u64::from(
            object
                .get::<_, Option<JsU32>>("mask")?
                .map_or(u32::MAX, |value| value.0),
        ),
        alpha_to_coverage_enabled: object
            .get::<_, Option<bool>>("alphaToCoverageEnabled")?
            .unwrap_or_default(),
    })
}

fn depth_stencil_invalid(state: &wgpu::DepthStencilState) -> bool {
    let has_depth = state.format.has_depth_aspect();
    let has_stencil = state.format.has_stencil_aspect();
    if !has_depth && !has_stencil {
        return true;
    }
    let depth_write = state.depth_write_enabled == Some(true);
    let depth_test = state
        .depth_compare
        .is_some_and(|compare| compare != wgpu::CompareFunction::Always);
    if (depth_write || depth_test) && !has_depth {
        return true;
    }
    if has_depth && state.depth_write_enabled.is_none() {
        return true;
    }
    let stencil_default = state.stencil.front == wgpu::StencilFaceState::default()
        && state.stencil.back == wgpu::StencilFaceState::default();
    if !stencil_default && !has_stencil {
        return true;
    }
    if has_depth && (depth_write || !stencil_default) && state.depth_compare.is_none() {
        return true;
    }
    false
}

fn depth_stencil_state(
    object: Option<Object<'_>>, ctx: &Ctx<'_>,
) -> Result<Option<wgpu::DepthStencilState>> {
    let Some(object) = object else {
        return Ok(None);
    };
    let stencil_face = |key: &str| -> Result<wgpu::StencilFaceState> {
        let Some(face) = object.get::<_, Option<Object>>(key)? else {
            return Ok(wgpu::StencilFaceState::default());
        };
        Ok(wgpu::StencilFaceState {
            compare:       face
                .get::<_, Option<String>>("compare")?
                .as_deref()
                .map_or(Ok(wgpu::CompareFunction::Always), |name| {
                    format::compare_function(name, ctx)
                })?,
            fail_op:       face
                .get::<_, Option<String>>("failOp")?
                .as_deref()
                .map_or(Ok(wgpu::StencilOperation::Keep), |name| {
                    format::stencil_operation(name, ctx)
                })?,
            depth_fail_op: face
                .get::<_, Option<String>>("depthFailOp")?
                .as_deref()
                .map_or(Ok(wgpu::StencilOperation::Keep), |name| {
                    format::stencil_operation(name, ctx)
                })?,
            pass_op:       face
                .get::<_, Option<String>>("passOp")?
                .as_deref()
                .map_or(Ok(wgpu::StencilOperation::Keep), |name| {
                    format::stencil_operation(name, ctx)
                })?,
        })
    };
    Ok(Some(wgpu::DepthStencilState {
        format:              format::texture_format(&object.get::<_, String>("format")?, ctx)?,
        depth_write_enabled: object.get("depthWriteEnabled")?,
        depth_compare:       object
            .get::<_, Option<String>>("depthCompare")?
            .map(|name| format::compare_function(&name, ctx))
            .transpose()?,
        stencil:             wgpu::StencilState {
            front:      stencil_face("stencilFront")?,
            back:       stencil_face("stencilBack")?,
            read_mask:  object
                .get::<_, Option<JsU32>>("stencilReadMask")?
                .map_or(u32::MAX, |value| value.0),
            write_mask: object
                .get::<_, Option<JsU32>>("stencilWriteMask")?
                .map_or(u32::MAX, |value| value.0),
        },
        bias:                wgpu::DepthBiasState {
            constant:    format::signed_i32(object.get("depthBias")?, ctx, 0)?,
            slope_scale: object
                .get::<_, Option<f64>>("depthBiasSlopeScale")?
                .unwrap_or(0.0) as f32,
            clamp:       object
                .get::<_, Option<f64>>("depthBiasClamp")?
                .unwrap_or(0.0) as f32,
        },
    }))
}

fn color_target(
    object: Option<Object<'_>>, ctx: &Ctx<'_>,
) -> Result<(Option<wgpu::ColorTargetState>, bool)> {
    let Some(object) = object else {
        return Ok((None, false));
    };
    let blend = object
        .get::<_, Option<Object>>("blend")?
        .map(|blend| -> Result<wgpu::BlendState> {
            let component = |key: &str| -> Result<wgpu::BlendComponent> {
                let Some(component) = blend.get::<_, Option<Object>>(key)? else {
                    return Ok(wgpu::BlendComponent::default());
                };
                Ok(wgpu::BlendComponent {
                    src_factor: component
                        .get::<_, Option<String>>("srcFactor")?
                        .as_deref()
                        .map_or(Ok(wgpu::BlendFactor::One), |name| {
                            format::blend_factor(name, ctx)
                        })?,
                    dst_factor: component
                        .get::<_, Option<String>>("dstFactor")?
                        .as_deref()
                        .map_or(Ok(wgpu::BlendFactor::Zero), |name| {
                            format::blend_factor(name, ctx)
                        })?,
                    operation:  component
                        .get::<_, Option<String>>("operation")?
                        .as_deref()
                        .map_or(Ok(wgpu::BlendOperation::Add), |name| {
                            format::blend_operation(name, ctx)
                        })?,
                })
            };
            Ok(wgpu::BlendState {
                color: component("color")?,
                alpha: component("alpha")?,
            })
        })
        .transpose()?;
    let write_bits = object
        .get::<_, Option<JsU32>>("writeMask")?
        .map_or(0xf, |value| value.0);
    let mask_invalid = write_bits & !0xf != 0;
    Ok((
        Some(wgpu::ColorTargetState {
            format: format::texture_format(&object.get::<_, String>("format")?, ctx)?,
            blend,
            write_mask: wgpu::ColorWrites::from_bits_truncate(write_bits & 0xf),
        }),
        mask_invalid,
    ))
}

fn color_target_invalid(state: &wgpu::ColorTargetState, features: wgpu::Features) -> bool {
    let format_features = state.format.guaranteed_format_features(features);
    let not_renderable = !format_features
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT);
    let blend_bad = state.blend.as_ref().is_some_and(|blend| {
        !format_features
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
            || blend_component_invalid(&blend.color, features)
            || blend_component_invalid(&blend.alpha, features)
    });
    not_renderable || blend_bad
}

fn blend_component_invalid(component: &wgpu::BlendComponent, features: wgpu::Features) -> bool {
    let dual = is_dual_source(component.src_factor) || is_dual_source(component.dst_factor);
    let dual_without_feature = dual && !features.contains(wgpu::Features::DUAL_SOURCE_BLENDING);
    let min_max = matches!(
        component.operation,
        wgpu::BlendOperation::Min | wgpu::BlendOperation::Max
    ) && (component.src_factor != wgpu::BlendFactor::One
        || component.dst_factor != wgpu::BlendFactor::One);
    dual_without_feature || min_max
}

const fn is_dual_source(factor: wgpu::BlendFactor) -> bool {
    matches!(
        factor,
        wgpu::BlendFactor::Src1
            | wgpu::BlendFactor::OneMinusSrc1
            | wgpu::BlendFactor::Src1Alpha
            | wgpu::BlendFactor::OneMinusSrc1Alpha
    )
}

fn color_bytes_per_sample_invalid(targets: &[Option<wgpu::ColorTargetState>], limit: u32) -> bool {
    let mut total = 0_u32;
    targets.iter().flatten().any(|state| {
        let Some(cost) = state.format.target_pixel_byte_cost() else {
            return true;
        };
        let Some(alignment) = state.format.target_component_alignment() else {
            return true;
        };
        total = total.next_multiple_of(alignment).saturating_add(cost);
        total > limit
    })
}

fn vertex_buffers_invalid(
    buffers: &[Option<(u64, wgpu::VertexStepMode, Vec<wgpu::VertexAttribute>)>],
    limits: &wgpu::Limits,
) -> bool {
    if buffers.len() as u32 > limits.max_vertex_buffers {
        return true;
    }
    let stride_invalid = buffers.iter().flatten().any(|(stride, _step, attributes)| {
        *stride > u64::from(limits.max_vertex_buffer_array_stride)
            || *stride % 4 != 0
            || attributes.iter().any(|attribute| {
                let size = attribute.format.size();
                let contain = if *stride == 0 {
                    u64::from(limits.max_vertex_buffer_array_stride)
                } else {
                    *stride
                };
                attribute.shader_location >= limits.max_vertex_attributes
                    || attribute.offset % size.min(4) != 0
                    || attribute.offset.saturating_add(size) > contain
            })
    });
    if stride_invalid {
        return true;
    }
    let mut locations: Vec<u32> = buffers
        .iter()
        .flatten()
        .flat_map(|(_stride, _step, attributes)| {
            attributes.iter().map(|attribute| attribute.shader_location)
        })
        .collect();
    let attr_count = locations.len();
    locations.sort_unstable();
    locations.dedup();
    attr_count as u32 > limits.max_vertex_attributes || locations.len() != attr_count
}

fn texture_or_view<'js>(value: Value<'js>, ctx: &Ctx<'js>, key: &str) -> Result<wgpu::TextureView> {
    if let Ok(view) = Class::<GPUTextureView>::from_js(ctx, value.clone()) {
        return Ok(view.borrow().inner.clone());
    }
    if let Ok(texture) = Class::<GPUTexture>::from_js(ctx, value) {
        return Ok(texture.borrow().default_view());
    }
    Err(type_error(
        ctx,
        format!("{key} must be a GPUTexture or GPUTextureView"),
    ))
}

fn attachment_view<'js>(
    attachment: &Object<'js>, key: &str, ctx: &Ctx<'js>,
) -> Result<wgpu::TextureView> {
    let value = attachment
        .get::<_, Value>(key)
        .map_err(|_error| type_error(ctx, format!("{key} must be a WebGPU object")))?;
    texture_or_view(value, ctx, key)
}

struct OwnedColorAttachment {
    view:        wgpu::TextureView,
    resolve:     Option<wgpu::TextureView>,
    ops:         wgpu::Operations<wgpu::Color>,
    depth_slice: Option<u32>,
}

/// The safe `wgpu` API folds "no ops" and "read-only" into one `None`, so the
/// pairing rules the spec checks on the descriptor are reported by den; the
/// message is deferred to `end()` where wgpu reports its own pass errors.
fn depth_stencil_ops<'js, V: Copy>(
    attachment: &Object<'js>, aspect: &str, clear: Option<V>, read_only: bool, ctx: &Ctx<'js>,
) -> Result<std::result::Result<Option<wgpu::Operations<V>>, String>> {
    let load = attachment.get::<_, Option<String>>(format!("{aspect}LoadOp"))?;
    let store = attachment.get::<_, Option<String>>(format!("{aspect}StoreOp"))?;
    let (load, store) = match (load, store) {
        (Some(load), Some(store)) => (load, store),
        (None, None) => return Ok(Ok(None)),
        _ => {
            return Ok(Err(format!(
                "{aspect}LoadOp and {aspect}StoreOp must be set together"
            )));
        }
    };
    let load = match (load.as_str(), clear) {
        ("load", _) => wgpu::LoadOp::Load,
        ("clear", Some(clear)) => wgpu::LoadOp::Clear(clear),
        ("clear", None) => {
            return Ok(Err(format!(
                "{aspect}ClearValue must be in range when {aspect}LoadOp is clear"
            )));
        }
        (value, _) => return Err(type_error(ctx, format!("invalid GPULoadOp {value}"))),
    };
    let ops = wgpu::Operations {
        load,
        store: format::store_op(&store, ctx)?,
    };
    Ok(Ok((!read_only).then_some(ops)))
}

pub fn begin_render_pass<'js>(
    encoder: &mut wgpu::CommandEncoder, descriptor: &Object<'js>, ctx: &Ctx<'js>,
) -> Result<(wgpu::RenderPass<'static>, Option<String>)> {
    let label = label(descriptor)?;
    let color_value = descriptor
        .get::<_, Array>("colorAttachments")
        .map_err(|_error| type_error(ctx, "colorAttachments must be an array"))?;
    let mut owned_colors = Vec::with_capacity(color_value.len());
    for attachment in color_value.iter::<Option<Object>>() {
        let Some(attachment) = attachment? else {
            owned_colors.push(None);
            continue;
        };
        let resolve = match attachment.get::<_, Option<Value>>("resolveTarget")? {
            Some(value) if !value.is_null() && !value.is_undefined() => {
                Some(texture_or_view(value, ctx, "resolveTarget")?)
            }
            _ => None,
        };
        let clear = format::color(attachment.get("clearValue")?, ctx)?;
        let load = match attachment.get::<_, String>("loadOp")?.as_str() {
            "load" => wgpu::LoadOp::Load,
            "clear" => wgpu::LoadOp::Clear(clear),
            value => return Err(type_error(ctx, format!("invalid GPULoadOp {value}"))),
        };
        owned_colors.push(Some(OwnedColorAttachment {
            view: attachment_view(&attachment, "view", ctx)?,
            resolve,
            ops: wgpu::Operations {
                load,
                store: format::store_op(&attachment.get::<_, String>("storeOp")?, ctx)?,
            },
            depth_slice: attachment
                .get::<_, Option<JsU32>>("depthSlice")?
                .map(|value| value.0),
        }));
    }
    let color_attachments = owned_colors
        .iter()
        .map(|attachment| {
            attachment.as_ref().map(|attachment| {
                wgpu::RenderPassColorAttachment {
                    view:           &attachment.view,
                    resolve_target: attachment.resolve.as_ref(),
                    ops:            attachment.ops,
                    depth_slice:    attachment.depth_slice,
                }
            })
        })
        .collect::<Vec<_>>();
    let mut deferred = None;
    let owned_depth = descriptor
        .get::<_, Option<Object>>("depthStencilAttachment")?
        .map(|attachment| {
            let view = attachment_view(&attachment, "view", ctx)?;
            let depth_clear = attachment
                .get::<_, Option<f64>>("depthClearValue")?
                .filter(|value| (0.0..=1.0).contains(value))
                .map(|value| value as f32);
            let depth_read_only = attachment
                .get::<_, Option<bool>>("depthReadOnly")?
                .unwrap_or_default();
            let stencil_read_only = attachment
                .get::<_, Option<bool>>("stencilReadOnly")?
                .unwrap_or_default();
            let stencil_clear = attachment
                .get::<_, Option<JsU32>>("stencilClearValue")?
                .map_or(0, |value| value.0);
            let depth_ops =
                depth_stencil_ops(&attachment, "depth", depth_clear, depth_read_only, ctx)?;
            let stencil_ops = depth_stencil_ops(
                &attachment,
                "stencil",
                Some(stencil_clear),
                stencil_read_only,
                ctx,
            )?;
            let (depth_ops, stencil_ops) = match (depth_ops, stencil_ops) {
                (Ok(depth_ops), Ok(stencil_ops)) => (depth_ops, stencil_ops),
                (Err(message), _) | (_, Err(message)) => {
                    deferred = Some(message);
                    (None, None)
                }
            };
            Ok::<_, rquickjs::Error>((view, depth_ops, stencil_ops))
        })
        .transpose()?;
    let depth_stencil_attachment = owned_depth.as_ref().map(|(view, depth_ops, stencil_ops)| {
        wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: *depth_ops,
            stencil_ops: *stencil_ops,
        }
    });
    let timestamp = descriptor
        .get::<_, Option<Object>>("timestampWrites")?
        .map(|writes| query::timestamp_writes_from(&writes, ctx))
        .transpose()?;
    let timestamp_writes = timestamp.as_ref().map(|(query_set, beginning, end)| {
        wgpu::RenderPassTimestampWrites {
            query_set,
            beginning_of_pass_write_index: *beginning,
            end_of_pass_write_index: *end,
        }
    });
    let occlusion = descriptor
        .get::<_, Option<Class<GPUQuerySet>>>("occlusionQuerySet")?
        .map(|query_set| query_set.borrow().inner.clone());
    let pass = encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            color_attachments: &color_attachments,
            depth_stencil_attachment,
            timestamp_writes,
            occlusion_query_set: occlusion.as_ref(),
            multiview_mask: None,
        })
        .forget_lifetime();
    Ok((pass, deferred))
}
