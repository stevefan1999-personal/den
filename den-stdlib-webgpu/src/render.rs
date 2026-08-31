use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Array, Class, Ctx, FromJs as _, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};

use crate::{
    GPUBindGroup, GPUBindGroupLayout, GPUBuffer, GPUPipelineLayout, GPUShaderModule, JsU32, JsU64,
    format, illegal_constructor, label, query::GPUQuerySet, texture::GPUTextureView, type_error,
};

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPipeline")]
pub struct GPURenderPipeline {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::RenderPipeline,
    #[qjs(skip_trace)]
    pub(crate) label: Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPURenderPipeline {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn get_bind_group_layout(&self, index: JsU32) -> GPUBindGroupLayout {
        GPUBindGroupLayout {
            inner: self.inner.get_bind_group_layout(index.0),
            label: Rc::new(RefCell::new(String::new())),
        }
    }
}

struct RenderPassState {
    parent: Rc<RefCell<crate::EncoderState>>,
    pass:   Option<wgpu::RenderPass<'static>>,
}

impl RenderPassState {
    fn end(&mut self) -> bool {
        let Some(pass) = self.pass.take() else {
            return false;
        };
        drop(pass);
        self.parent.borrow_mut().open_passes -= 1;
        true
    }
}

impl Drop for RenderPassState {
    fn drop(&mut self) { self.end(); }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPassEncoder")]
pub struct GPURenderPassEncoder {
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state: Rc<RefCell<RenderPassState>>,
}

impl GPURenderPassEncoder {
    pub(crate) fn new(
        label: String, parent: Rc<RefCell<crate::EncoderState>>, pass: wgpu::RenderPass<'static>,
    ) -> Self {
        Self {
            label: Rc::new(RefCell::new(label)),
            state: Rc::new(RefCell::new(RenderPassState {
                parent,
                pass: Some(pass),
            })),
        }
    }

    fn with_pass<T>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::RenderPass<'static>) -> T,
    ) -> Result<T> {
        self.state
            .borrow_mut()
            .pass
            .as_mut()
            .map(operation)
            .ok_or_else(|| crate::invalid_state(ctx, "GPURenderPassEncoder is already ended"))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPURenderPassEncoder {
    #[qjs(constructor)]
    pub fn new_illegal(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn set_pipeline<'js>(
        &self, pipeline: Class<'js, GPURenderPipeline>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let pipeline = pipeline.borrow();
        self.with_pass(&ctx, |pass| pass.set_pipeline(&pipeline.inner))
    }

    pub fn set_bind_group<'js>(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let bind_group = bind_group.as_ref().map(|group| group.borrow());
        self.with_pass(&ctx, |pass| {
            pass.set_bind_group(
                index.0,
                bind_group.as_ref().map(|group| &group.inner),
                &offsets,
            );
        })
    }

    pub fn set_vertex_buffer<'js>(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let Some(buffer) = buffer else {
            return self.with_pass(&ctx, |pass| pass.set_vertex_buffer(slot.0, None));
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        self.with_pass(&ctx, |pass| {
            pass.set_vertex_buffer(
                slot.0,
                Some(
                    buffer.inner.slice(
                        offset
                            ..size
                                .0
                                .flatten()
                                .map_or(buffer.size, |value| offset + value.0),
                    ),
                ),
            );
        })
    }

    pub fn set_index_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let format = format::index_format(&index_format, &ctx)?;
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        self.with_pass(&ctx, |pass| {
            pass.set_index_buffer(
                buffer.inner.slice(
                    offset
                        ..size
                            .0
                            .flatten()
                            .map_or(buffer.size, |value| offset + value.0),
                ),
                format,
            );
        })
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        self.with_pass(&ctx, |pass| {
            pass.draw(
                first_vertex..first_vertex + vertex_count.0,
                first_instance..first_instance + instance_count,
            );
        })
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'_>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        self.with_pass(&ctx, |pass| {
            pass.draw_indexed(
                first_index..first_index + index_count.0,
                base_vertex,
                first_instance..first_instance + instance_count,
            );
        })
    }

    pub fn draw_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        self.with_pass(&ctx, |pass| pass.draw_indirect(&buffer.inner, offset.0))
    }

    pub fn draw_indexed_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        self.with_pass(&ctx, |pass| {
            pass.draw_indexed_indirect(&buffer.inner, offset.0)
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "GPURenderPassEncoder.setViewport matches the WebGPU IDL"
    )]
    pub fn set_viewport(
        &self, x: f64, y: f64, width: f64, height: f64, min_depth: f64, max_depth: f64,
        ctx: Ctx<'_>,
    ) -> Result<()> {
        self.with_pass(&ctx, |pass| {
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
        &self, x: JsU32, y: JsU32, width: JsU32, height: JsU32, ctx: Ctx<'_>,
    ) -> Result<()> {
        self.with_pass(&ctx, |pass| {
            pass.set_scissor_rect(x.0, y.0, width.0, height.0);
        })
    }

    pub fn set_blend_constant<'js>(&self, color: Value<'js>, ctx: Ctx<'js>) -> Result<()> {
        let color = format::color(Some(color), &ctx)?;
        self.with_pass(&ctx, |pass| pass.set_blend_constant(color))
    }

    pub fn set_stencil_reference(&self, reference: JsU32, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.set_stencil_reference(reference.0))
    }

    pub fn begin_occlusion_query(&self, query_index: JsU32, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.begin_occlusion_query(query_index.0))
    }

    pub fn end_occlusion_query(&self, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, wgpu::RenderPass::end_occlusion_query)
    }

    pub fn execute_bundles<'js>(&self, bundles: Array<'js>, ctx: Ctx<'js>) -> Result<()> {
        let owned = bundles
            .iter::<Class<GPURenderBundle>>()
            .map(|bundle| Ok(bundle?.borrow().inner.clone()))
            .collect::<Result<Vec<_>>>()?;
        self.with_pass(&ctx, |pass| pass.execute_bundles(owned.iter()))
    }

    pub fn end(&self, ctx: Ctx<'_>) -> Result<()> {
        if self.state.borrow_mut().end() {
            Ok(())
        } else {
            Err(crate::invalid_state(
                &ctx,
                "GPURenderPassEncoder is already ended",
            ))
        }
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, wgpu::RenderPass::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.insert_debug_marker(&value))
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundle")]
pub struct GPURenderBundle {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::RenderBundle,
    #[qjs(skip_trace)]
    label:            Rc<RefCell<String>>,
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

struct BundleEncoderState {
    encoder: Option<wgpu::RenderBundleEncoder<'static>>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundleEncoder")]
pub struct GPURenderBundleEncoder {
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state: Rc<RefCell<BundleEncoderState>>,
}

impl GPURenderBundleEncoder {
    fn with_encoder<T>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::RenderBundleEncoder<'static>) -> T,
    ) -> Result<T> {
        self.state
            .borrow_mut()
            .encoder
            .as_mut()
            .map(operation)
            .ok_or_else(|| crate::invalid_state(ctx, "GPURenderBundleEncoder is already finished"))
    }
}

fn static_ref<T>(value: &T) -> &'static T {
    // SAFETY: wgpu's RenderBundleEncoder only copies native handles during the
    // call; the JS-owned GPU objects keep the resources alive afterwards.
    unsafe { &*(std::ptr::from_ref(value)) }
}

fn static_slice(buffer: &wgpu::Buffer, offset: u64, end: u64) -> wgpu::BufferSlice<'static> {
    // SAFETY: same as `static_ref`; the slice is consumed by the encoder call.
    unsafe { std::mem::transmute(buffer.slice(offset..end)) }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPURenderBundleEncoder {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn set_pipeline<'js>(
        &self, pipeline: Class<'js, GPURenderPipeline>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let pipeline = pipeline.borrow().inner.clone();
        self.with_encoder(&ctx, |encoder| {
            encoder.set_pipeline(static_ref(&pipeline));
        })
    }

    pub fn set_bind_group<'js>(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let bind_group = bind_group.map(|group| group.borrow().inner.clone());
        self.with_encoder(&ctx, |encoder| {
            encoder.set_bind_group(index.0, bind_group.as_ref().map(static_ref), &offsets);
        })
    }

    pub fn set_vertex_buffer<'js>(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let Some(buffer) = buffer else {
            return self.with_encoder(&ctx, |encoder| encoder.set_vertex_buffer(slot.0, None));
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let end = size
            .0
            .flatten()
            .map_or(buffer.size, |value| offset + value.0);
        let inner = buffer.inner.clone();
        self.with_encoder(&ctx, |encoder| {
            encoder.set_vertex_buffer(slot.0, Some(static_slice(&inner, offset, end)));
        })
    }

    pub fn set_index_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let format = format::index_format(&index_format, &ctx)?;
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let end = size
            .0
            .flatten()
            .map_or(buffer.size, |value| offset + value.0);
        let inner = buffer.inner.clone();
        self.with_encoder(&ctx, |encoder| {
            encoder.set_index_buffer(static_slice(&inner, offset, end), format);
        })
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        self.with_encoder(&ctx, |encoder| {
            encoder.draw(
                first_vertex..first_vertex + vertex_count.0,
                first_instance..first_instance + instance_count,
            );
        })
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'_>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        self.with_encoder(&ctx, |encoder| {
            encoder.draw_indexed(
                first_index..first_index + index_count.0,
                base_vertex,
                first_instance..first_instance + instance_count,
            );
        })
    }

    pub fn draw_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        self.with_encoder(&ctx, |encoder| {
            encoder.draw_indirect(static_ref(&buffer), offset.0)
        })
    }

    pub fn draw_indexed_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        self.with_encoder(&ctx, |encoder| {
            encoder.draw_indexed_indirect(static_ref(&buffer), offset.0)
        })
    }

    pub fn finish<'js>(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPURenderBundle> {
        let label = descriptor
            .0
            .flatten()
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let encoder = self.state.borrow_mut().encoder.take().ok_or_else(|| {
            crate::invalid_state(&ctx, "GPURenderBundleEncoder is already finished")
        })?;
        Ok(GPURenderBundle {
            inner: encoder.finish(&wgpu::RenderBundleDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
            }),
            label: Rc::new(RefCell::new(label)),
        })
    }
}

pub fn new_bundle_encoder(
    device: &wgpu::Device, descriptor: Object<'_>, ctx: &Ctx<'_>,
) -> Result<GPURenderBundleEncoder> {
    let label = label(&descriptor)?;
    let color_formats = descriptor
        .get::<_, Array>("colorFormats")
        .map_err(|_error| type_error(ctx, "colorFormats must be an array"))?;
    let mut formats = Vec::with_capacity(color_formats.len());
    for format in color_formats.iter::<Option<String>>() {
        formats.push(
            format?
                .map(|name| format::texture_format(&name, ctx))
                .transpose()?,
        );
    }
    let depth_stencil = match descriptor.get::<_, Option<String>>("depthStencilFormat")? {
        Some(name) => {
            Some(wgpu::RenderBundleDepthStencil {
                format:            format::texture_format(&name, ctx)?,
                depth_read_only:   descriptor
                    .get::<_, Option<bool>>("depthReadOnly")?
                    .unwrap_or_default(),
                stencil_read_only: descriptor
                    .get::<_, Option<bool>>("stencilReadOnly")?
                    .unwrap_or_default(),
            })
        }
        None => None,
    };
    let sample_count = descriptor
        .get::<_, Option<JsU32>>("sampleCount")?
        .map_or(1, |value| value.0.max(1));
    let encoder = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
        label: (!label.is_empty()).then_some(label.as_str()),
        color_formats: &formats,
        depth_stencil,
        sample_count,
        multiview: None,
    });
    Ok(GPURenderBundleEncoder {
        label: Rc::new(RefCell::new(label)),
        state: Rc::new(RefCell::new(BundleEncoderState {
            // SAFETY: the encoder is stored for the JS object's lifetime and
            // only used while that object is alive.
            encoder: Some(unsafe {
                std::mem::transmute::<
                    wgpu::RenderBundleEncoder<'_>,
                    wgpu::RenderBundleEncoder<'static>,
                >(encoder)
            }),
        })),
    })
}

pub fn create_pipeline<'js>(
    device: &wgpu::Device, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<GPURenderPipeline> {
    let label = label(&descriptor)?;
    let layout_value: Value = descriptor.get("layout")?;
    let layout = if layout_value.is_undefined()
        || layout_value
            .as_string()
            .map(rquickjs::String::to_string)
            .transpose()?
            .as_deref()
            == Some("auto")
    {
        None
    } else {
        Some(
            Class::<GPUPipelineLayout>::from_js(ctx, layout_value)?
                .borrow()
                .inner
                .clone(),
        )
    };
    let vertex: Object = descriptor.get("vertex")?;
    let vertex_module = crate::class_value::<GPUShaderModule>(&vertex, "module", ctx)?
        .borrow()
        .inner
        .clone();
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
    let fragment_module;
    let fragment_entry;
    let fragment_constants;
    let fragment_constant_pairs;
    let fragment_targets;
    let fragment = if let Some(fragment) = fragment_object {
        fragment_module = crate::class_value::<GPUShaderModule>(&fragment, "module", ctx)?
            .borrow()
            .inner
            .clone();
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
            native_targets.push(color_target(target?, ctx)?);
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
        None
    };
    Ok(GPURenderPipeline {
        inner: device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        }),
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

fn multisample_state(object: Option<Object<'_>>, _ctx: &Ctx<'_>) -> Result<wgpu::MultisampleState> {
    let Some(object) = object else {
        return Ok(wgpu::MultisampleState::default());
    };
    Ok(wgpu::MultisampleState {
        count:                     object
            .get::<_, Option<JsU32>>("count")?
            .map_or(1, |value| value.0.max(1)),
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
    Ok(Some(wgpu::ColorTargetState {
        format: format::texture_format(&object.get::<_, String>("format")?, ctx)?,
        blend,
        write_mask: wgpu::ColorWrites::from_bits(
            object
                .get::<_, Option<JsU32>>("writeMask")?
                .map_or(0xf, |value| value.0),
        )
        .unwrap_or(wgpu::ColorWrites::ALL),
    }))
}

struct OwnedColorAttachment {
    view:        wgpu::TextureView,
    resolve:     Option<wgpu::TextureView>,
    ops:         wgpu::Operations<wgpu::Color>,
    depth_slice: Option<u32>,
}

struct OwnedDepthStencil {
    view:        wgpu::TextureView,
    depth_ops:   Option<wgpu::Operations<f32>>,
    stencil_ops: Option<wgpu::Operations<u32>>,
}

pub fn begin_render_pass<'js>(
    encoder: &mut wgpu::CommandEncoder, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(String, wgpu::RenderPass<'static>)> {
    let label = label(&descriptor)?;
    let color_value = descriptor
        .get::<_, Array>("colorAttachments")
        .map_err(|_error| type_error(ctx, "colorAttachments must be an array"))?;
    let mut owned_colors = Vec::with_capacity(color_value.len());
    for attachment in color_value.iter::<Option<Object>>() {
        let Some(attachment) = attachment? else {
            owned_colors.push(None);
            continue;
        };
        let view = crate::class_value::<GPUTextureView>(&attachment, "view", ctx)?
            .borrow()
            .inner
            .clone();
        let resolve = attachment
            .get::<_, Option<Class<GPUTextureView>>>("resolveTarget")?
            .map(|view| view.borrow().inner.clone());
        let load = match attachment.get::<_, String>("loadOp")?.as_str() {
            "load" => wgpu::LoadOp::Load,
            "clear" => wgpu::LoadOp::Clear(format::color(attachment.get("clearValue")?, ctx)?),
            value => return Err(type_error(ctx, format!("invalid GPULoadOp {value}"))),
        };
        let store = format::store_op(&attachment.get::<_, String>("storeOp")?, ctx)?;
        owned_colors.push(Some(OwnedColorAttachment {
            view,
            resolve,
            ops: wgpu::Operations { load, store },
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
    let owned_depth = descriptor
        .get::<_, Option<Object>>("depthStencilAttachment")?
        .map(|attachment| {
            let view = crate::class_value::<GPUTextureView>(&attachment, "view", ctx)?
                .borrow()
                .inner
                .clone();
            let depth_read_only = attachment
                .get::<_, Option<bool>>("depthReadOnly")?
                .unwrap_or_default();
            let stencil_read_only = attachment
                .get::<_, Option<bool>>("stencilReadOnly")?
                .unwrap_or_default();
            let depth_ops = if depth_read_only {
                None
            } else {
                let load = match attachment
                    .get::<_, Option<String>>("depthLoadOp")?
                    .as_deref()
                {
                    Some("clear") => {
                        wgpu::LoadOp::Clear(
                            attachment
                                .get::<_, Option<f64>>("depthClearValue")?
                                .unwrap_or(0.0) as f32,
                        )
                    }
                    Some("load") | None => wgpu::LoadOp::Load,
                    Some(value) => {
                        return Err(type_error(ctx, format!("invalid GPULoadOp {value}")));
                    }
                };
                let store = attachment
                    .get::<_, Option<String>>("depthStoreOp")?
                    .as_deref()
                    .map_or(Ok(wgpu::StoreOp::Store), |name| format::store_op(name, ctx))?;
                Some(wgpu::Operations { load, store })
            };
            let stencil_ops = if stencil_read_only {
                None
            } else {
                let load = match attachment
                    .get::<_, Option<String>>("stencilLoadOp")?
                    .as_deref()
                {
                    Some("clear") => {
                        wgpu::LoadOp::Clear(
                            attachment
                                .get::<_, Option<JsU32>>("stencilClearValue")?
                                .map_or(0, |value| value.0),
                        )
                    }
                    Some("load") | None => wgpu::LoadOp::Load,
                    Some(value) => {
                        return Err(type_error(ctx, format!("invalid GPULoadOp {value}")));
                    }
                };
                let store = attachment
                    .get::<_, Option<String>>("stencilStoreOp")?
                    .as_deref()
                    .map_or(Ok(wgpu::StoreOp::Store), |name| format::store_op(name, ctx))?;
                Some(wgpu::Operations { load, store })
            };
            Ok(OwnedDepthStencil {
                view,
                depth_ops,
                stencil_ops,
            })
        })
        .transpose()?;
    let depth_stencil_attachment = owned_depth.as_ref().map(|attachment| {
        wgpu::RenderPassDepthStencilAttachment {
            view:        &attachment.view,
            depth_ops:   attachment.depth_ops,
            stencil_ops: attachment.stencil_ops,
        }
    });
    let timestamp_query;
    let timestamp_writes = match descriptor.get::<_, Option<Object>>("timestampWrites")? {
        Some(writes) => {
            timestamp_query = crate::class_value::<GPUQuerySet>(&writes, "querySet", ctx)?
                .borrow()
                .inner
                .clone();
            Some(wgpu::RenderPassTimestampWrites {
                query_set:                     &timestamp_query,
                beginning_of_pass_write_index: writes
                    .get::<_, Option<JsU32>>("beginningOfPassWriteIndex")?
                    .map(|value| value.0),
                end_of_pass_write_index:       writes
                    .get::<_, Option<JsU32>>("endOfPassWriteIndex")?
                    .map(|value| value.0),
            })
        }
        None => None,
    };
    let occlusion = descriptor.get::<_, Option<Class<GPUQuerySet>>>("occlusionQuerySet")?;
    let occlusion_inner = occlusion.as_ref().map(|query| query.borrow().inner.clone());
    let pass = encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            color_attachments: &color_attachments,
            depth_stencil_attachment,
            timestamp_writes,
            occlusion_query_set: occlusion_inner.as_ref(),
            multiview_mask: None,
        })
        .forget_lifetime();
    Ok((label, pass))
}
