use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Array, Class, Ctx, FromJs as _, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};

use crate::{
    ErrorSink, GPUBindGroup, GPUBindGroupLayout, GPUBuffer, GPUDevice, JsU32, JsU64,
    ProgrammableStage, Recorder, buffer_slice, format, illegal_constructor, label,
    query::{self, GPUQuerySet},
    texture::{GPUTexture, GPUTextureView},
    type_error,
};

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPipeline")]
pub struct GPURenderPipeline<'js> {
    device:           Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::RenderPipeline,
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

    pub fn get_bind_group_layout(&self, index: JsU32, ctx: Ctx<'js>) -> Result<GPUBindGroupLayout> {
        let inner = self.inner.get_bind_group_layout(index.0);
        crate::flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(String::new())),
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
        let bytes = crate::data_window(data, data_offset, size, &ctx)?;
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
        let bytes = crate::data_window(data, data_offset, size, &ctx)?;
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

/// `GPUVertexBufferLayout` with the attribute storage wgpu's descriptor
/// borrows.
struct OwnedVertexBuffer {
    array_stride: u64,
    step_mode:    wgpu::VertexStepMode,
    attributes:   Vec<wgpu::VertexAttribute>,
}

impl OwnedVertexBuffer {
    fn read<'js>(buffer: &Object<'js>, ctx: &Ctx<'js>) -> Result<Self> {
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
            .map_err(|_error| type_error(ctx, "vertex attributes must be an array"))?
            .iter::<Object>()
            .map(|attribute| {
                let attribute = attribute?;
                Ok(wgpu::VertexAttribute {
                    format:          format::vertex_format(
                        &attribute.get::<_, String>("format")?,
                        ctx,
                    )?,
                    offset:          attribute.get::<_, JsU64>("offset")?.0,
                    shader_location: attribute.get::<_, JsU32>("shaderLocation")?.0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            array_stride: buffer.get::<_, JsU64>("arrayStride")?.0,
            step_mode,
            attributes,
        })
    }

    fn layout(&self) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: self.array_stride,
            step_mode:    self.step_mode,
            attributes:   &self.attributes,
        }
    }
}

pub fn create_pipeline<'js>(
    device_class: &Class<'js, GPUDevice<'js>>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<GPURenderPipeline<'js>> {
    let label = label(&descriptor)?;
    let layout = crate::pipeline_layout(&descriptor, ctx)?;
    let vertex: Object = descriptor.get("vertex")?;
    let vertex_stage = ProgrammableStage::read(&vertex, ctx)?;
    let vertex_constants = vertex_stage.constant_pairs();
    let owned_buffers = vertex
        .get::<_, Option<Array>>("buffers")?
        .map(|buffers| {
            buffers
                .iter::<Option<Object>>()
                .map(|buffer| {
                    buffer?
                        .map(|buffer| OwnedVertexBuffer::read(&buffer, ctx))
                        .transpose()
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let vertex_buffers = owned_buffers
        .iter()
        .map(|buffer| buffer.as_ref().map(OwnedVertexBuffer::layout))
        .collect::<Vec<_>>();
    let fragment_object: Option<Object> = descriptor.get("fragment")?;
    let fragment_stage = fragment_object
        .as_ref()
        .map(|fragment| ProgrammableStage::read(fragment, ctx))
        .transpose()?;
    let fragment_constants = fragment_stage
        .as_ref()
        .map(ProgrammableStage::constant_pairs)
        .unwrap_or_default();
    let fragment_targets = fragment_object
        .as_ref()
        .map(|fragment| {
            fragment
                .get::<_, Array>("targets")
                .map_err(|_error| type_error(ctx, "fragment targets must be an array"))?
                .iter::<Option<Object>>()
                .map(|target| color_target(target?, ctx))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let fragment = fragment_stage.as_ref().map(|stage| {
        wgpu::FragmentState {
            module:              &stage.module,
            entry_point:         stage.entry_point.as_deref(),
            compilation_options: ProgrammableStage::compilation_options(&fragment_constants),
            targets:             &fragment_targets,
        }
    });
    let inner =
        device_class
            .borrow()
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                layout: layout.as_ref(),
                vertex: wgpu::VertexState {
                    module:              &vertex_stage.module,
                    entry_point:         vertex_stage.entry_point.as_deref(),
                    compilation_options: ProgrammableStage::compilation_options(&vertex_constants),
                    buffers:             &vertex_buffers,
                },
                primitive: primitive_state(descriptor.get("primitive")?, ctx)?,
                depth_stencil: depth_stencil_state(descriptor.get("depthStencil")?, ctx)?,
                multisample: multisample_state(descriptor.get("multisample")?)?,
                fragment,
                multiview_mask: None,
                cache: None,
            });
    Ok(GPURenderPipeline {
        device: device_class.clone(),
        inner,
        label: Rc::new(RefCell::new(label)),
    })
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

fn multisample_state(object: Option<Object<'_>>) -> Result<wgpu::MultisampleState> {
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
) -> Result<Option<wgpu::ColorTargetState>> {
    let Some(object) = object else {
        return Ok(None);
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
    // Unknown write-mask bits are a device-timeline validation error, so
    // they are retained for wgpu to reject (deno does the same).
    let write_mask = object
        .get::<_, Option<JsU32>>("writeMask")?
        .map_or(wgpu::ColorWrites::ALL, |value| {
            wgpu::ColorWrites::from_bits_retain(value.0)
        });
    Ok(Some(wgpu::ColorTargetState {
        format: format::texture_format(&object.get::<_, String>("format")?, ctx)?,
        blend,
        write_mask,
    }))
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
