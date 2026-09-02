use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Array, Class, Ctx, FromJs as _, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};

use crate::{
    ErrorSink, GPUBindGroup, GPUBindGroupLayout, GPUBuffer, GPUDevice, GPUPipelineLayout,
    GPUShaderModule, JsU32, JsU64, format, illegal_constructor, label,
    query::{self, GPUQuerySet},
    texture::{GPUTexture, GPUTextureView},
    type_error,
};

const DRAW_INDIRECT_BYTES: u64 = 16;
const DRAW_INDEXED_INDIRECT_BYTES: u64 = 20;

fn set_vertex_bound(bounds: &mut Vec<Option<u64>>, slot: usize, value: Option<u64>) {
    if bounds.len() <= slot {
        bounds.resize(slot + 1, None);
    }
    if let Some(bound) = bounds.get_mut(slot) {
        *bound = value;
    }
}

#[derive(Clone, Copy)]
struct VertexLayout {
    pub array_stride: u64,
    pub step_mode:    wgpu::VertexStepMode,
    pub last_stride:  u64,
}

fn vertex_required_bytes(layout: &VertexLayout, count: u32) -> u64 {
    if count == 0 {
        0
    } else if layout.array_stride == 0 {
        layout.last_stride
    } else {
        u64::from(count - 1)
            .saturating_mul(layout.array_stride)
            .saturating_add(layout.last_stride)
    }
}

fn vertex_buffers_oob(
    layouts: &[Option<VertexLayout>], bounds: &[Option<u64>], vertex_end: u32, instance_end: u32,
    indexed: bool,
) -> bool {
    layouts.iter().enumerate().any(|(slot, layout)| {
        let Some(layout) = layout else {
            return false;
        };
        let count = match layout.step_mode {
            wgpu::VertexStepMode::Vertex if indexed && layout.array_stride != 0 => return false,
            wgpu::VertexStepMode::Vertex => vertex_end,
            wgpu::VertexStepMode::Instance => instance_end,
        };
        let bound = bounds.get(slot).copied().flatten().unwrap_or(0);
        bound < vertex_required_bytes(layout, count)
    })
}

struct RenderPipelineInfo {
    invalid:            bool,
    device_id:          u64,
    is_strip:           bool,
    strip_index_format: Option<wgpu::IndexFormat>,
    dummy:              bool,
    layout_groups:      crate::PipelineLayoutGroups,
    auto_layout:        bool,
    auto_layout_id:     u64,
    empty_bgl:          wgpu::BindGroupLayout,
    bgls:               Rc<[wgpu::BindGroupLayout]>,
    errors:             crate::ErrorSink,
    max_bind_groups:    u32,
    immediate_slots:    u64,
    vertex_layouts:     Rc<[Option<VertexLayout>]>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderPipeline")]
pub struct GPURenderPipeline {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::RenderPipeline,
    #[qjs(skip_trace)]
    info:             Rc<RenderPipelineInfo>,
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

    pub(crate) fn dummy(&self) -> bool { self.info.dummy }

    pub fn get_bind_group_layout(&self, index: JsU32) -> GPUBindGroupLayout {
        if index.0 >= self.info.max_bind_groups {
            self.info
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
        GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(String::new())),
            kinds: crate::kinds_from_entries(&entries),
            entries,
            auto: self.info.auto_layout,
            auto_layout_id: self.info.auto_layout_id,
        }
    }
}

struct RenderPassState {
    parent:             Rc<RefCell<crate::EncoderState>>,
    open:               bool,
    pass:               Option<wgpu::RenderPass<'static>>,
    index_count:        Option<u64>,
    index_format:       Option<wgpu::IndexFormat>,
    index_native:       bool,
    is_strip:           bool,
    strip_index_format: Option<wgpu::IndexFormat>,
    pipeline_bound:     bool,
    native_pipeline:    bool,
    depth_read_only:    Option<bool>,
    stencil_read_only:  Option<bool>,
    color_formats:      Rc<[Option<wgpu::TextureFormat>]>,
    depth_format:       Option<wgpu::TextureFormat>,
    sample_count:       u32,
    occlusion_native:   bool,
    occlusion_count:    u32,
    occlusion_open:     bool,
    occlusion_used:     Vec<u32>,
    attachment_uses:    Vec<crate::TextureUse>,
    bound_groups:       crate::BindGroupSet,
    pipeline_groups:    crate::PipelineLayoutGroups,
    pipeline_auto:      bool,
    pipeline_auto_id:   u64,
    vertex_layouts:     Rc<[Option<VertexLayout>]>,
    vertex_bounds:      Vec<Option<u64>>,
    immediate_required: u64,
    immediate_filled:   u64,
}

impl RenderPassState {
    fn end(&mut self) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        let pass = self.pass.take();
        let mut parent = self.parent.borrow_mut();
        if let Some(pass) = pass {
            if parent.encoder.is_none() {
                let _pass = std::mem::ManuallyDrop::new(pass);
            } else {
                drop(pass);
            }
        }
        parent.open_passes = parent.open_passes.saturating_sub(1);
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
    errors: ErrorSink,
    #[qjs(skip_trace)]
    label:  Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state:  Rc<RefCell<RenderPassState>>,
}

impl GPURenderPassEncoder {
    fn mark_invalid(&self) { self.state.borrow_mut().parent.borrow_mut().invalid = true; }

    fn skip_native_draw(&self) -> bool {
        let (mismatch, missing, skip) = {
            let state = self.state.borrow();
            let mismatch = !state.pipeline_bound
                || crate::pipeline_bind_groups_mismatch(
                    &state.pipeline_groups,
                    state.pipeline_auto,
                    state.pipeline_auto_id,
                    &state.bound_groups,
                );
            let skip = !state.native_pipeline
                || state.immediate_required != 0
                || state.parent.borrow().invalid;
            (
                mismatch,
                crate::immediates_unfilled(state.immediate_required, state.immediate_filled),
                skip,
            )
        };
        if mismatch || missing {
            self.mark_invalid();
            true
        } else {
            skip
        }
    }

    fn draw_vertex_oob(&self, vertex_end: u32, instance_end: u32, indexed: bool) -> bool {
        let state = self.state.borrow();
        vertex_buffers_oob(
            &state.vertex_layouts,
            &state.vertex_bounds,
            vertex_end,
            instance_end,
            indexed,
        )
    }

    fn ensure_open(&self) -> bool {
        if self.state.borrow().open {
            true
        } else {
            self.errors
                .validation("GPURenderPassEncoder is already ended");
            false
        }
    }

    pub(crate) fn new(
        parent: Rc<RefCell<crate::EncoderState>>, errors: ErrorSink, begun: BegunRenderPass,
    ) -> Self {
        Self {
            errors,
            label: Rc::new(RefCell::new(begun.label)),
            state: Rc::new(RefCell::new(RenderPassState {
                parent,
                open: true,
                pass: begun.pass,
                index_count: None,
                index_format: None,
                index_native: false,
                is_strip: false,
                strip_index_format: None,
                pipeline_bound: false,
                native_pipeline: false,
                depth_read_only: begun.depth_read_only,
                stencil_read_only: begun.stencil_read_only,
                color_formats: begun.color_formats,
                depth_format: begun.depth_format,
                sample_count: begun.sample_count,
                occlusion_native: begun.occlusion_native,
                occlusion_count: begun.occlusion_count,
                occlusion_open: false,
                occlusion_used: Vec::new(),
                attachment_uses: begun.attachment_uses,
                bound_groups: crate::BindGroupSet::default(),
                pipeline_groups: crate::empty_layout_groups(),
                pipeline_auto: false,
                pipeline_auto_id: 0,
                vertex_layouts: Rc::from(Vec::new()),
                vertex_bounds: Vec::new(),
                immediate_required: 0,
                immediate_filled: 0,
            })),
        }
    }

    pub(crate) fn finished(
        label: String, parent: Rc<RefCell<crate::EncoderState>>, errors: ErrorSink,
    ) -> Self {
        Self {
            errors,
            label: Rc::new(RefCell::new(label)),
            state: Rc::new(RefCell::new(RenderPassState {
                parent,
                open: false,
                pass: None,
                index_count: None,
                index_format: None,
                index_native: false,
                is_strip: false,
                strip_index_format: None,
                pipeline_bound: false,
                native_pipeline: false,
                depth_read_only: None,
                stencil_read_only: None,
                color_formats: Rc::from(Vec::new()),
                depth_format: None,
                sample_count: 1,
                occlusion_native: false,
                occlusion_count: 0,
                occlusion_open: false,
                occlusion_used: Vec::new(),
                attachment_uses: Vec::new(),
                bound_groups: crate::BindGroupSet::default(),
                pipeline_groups: crate::empty_layout_groups(),
                pipeline_auto: false,
                pipeline_auto_id: 0,
                vertex_layouts: Rc::from(Vec::new()),
                vertex_bounds: Vec::new(),
                immediate_required: 0,
                immediate_filled: 0,
            })),
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "JS methods return Result; keep one helper shape"
    )]
    fn with_pass<T: Default>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::RenderPass<'static>) -> T,
    ) -> Result<T> {
        let _ = ctx;
        if !self.ensure_open() {
            return Ok(T::default());
        }
        Ok(self
            .state
            .borrow_mut()
            .pass
            .as_mut()
            .map_or_else(T::default, operation))
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
        if !self.ensure_open() {
            return Ok(());
        }
        let pipeline = pipeline.borrow();
        let device_id = self.state.borrow().parent.borrow().device_id;
        if pipeline.info.invalid || pipeline.info.device_id != device_id {
            self.mark_invalid();
            return Ok(());
        }
        let native = !pipeline.info.dummy;
        let inner = native.then(|| pipeline.inner.clone());
        {
            let mut state = self.state.borrow_mut();
            state.is_strip = pipeline.info.is_strip;
            state.strip_index_format = pipeline.info.strip_index_format;
            state.pipeline_bound = true;
            state.pipeline_groups = pipeline.info.layout_groups.clone();
            state.pipeline_auto = pipeline.info.auto_layout;
            state.pipeline_auto_id = pipeline.info.auto_layout_id;
            state.vertex_layouts = pipeline.info.vertex_layouts.clone();
            state.immediate_required = pipeline.info.immediate_slots;
            state.native_pipeline = native;
        }
        drop(pipeline);
        if let Some(inner) = inner {
            self.with_pass(&ctx, |pass| pass.set_pipeline(&inner))?;
        }
        Ok(())
    }

    pub fn set_bind_group<'js>(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let native = bind_group.as_ref().and_then(|group| {
            let group = group.borrow();
            (!group.invalid).then(|| group.inner.clone())
        });
        let bind_group = bind_group.as_ref().map(|group| group.borrow());
        let parent = self.state.borrow().parent.clone();
        let (device_id, max_bind_groups) = {
            let parent = parent.borrow();
            (parent.device_id, parent.max_bind_groups)
        };
        let applied = {
            let mut state = self.state.borrow_mut();
            state.bound_groups.apply(
                index.0,
                bind_group.as_deref(),
                &offsets,
                device_id,
                max_bind_groups,
            )
        };
        {
            let mut parent = parent.borrow_mut();
            parent.used_destroyed.extend(applied.used_destroyed);
            parent.used_mapped.extend(applied.used_mapped);
        }
        if applied.invalid {
            self.mark_invalid();
            return Ok(());
        }
        let skip_native = {
            let state = self.state.borrow();
            let extras = bind_group.as_ref().map_or_else(
                || std::rc::Rc::from(Vec::new()),
                |group| crate::bind_group_texture_uses(group),
            );
            crate::skip_native_texture_bind(&state.attachment_uses, &extras)
        };
        drop(bind_group);
        if skip_native {
            return Ok(());
        }
        self.with_pass(&ctx, |pass| {
            pass.set_bind_group(index.0, native.as_ref(), &offsets);
        })
    }

    pub fn set_vertex_buffer<'js>(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let Some(buffer) = buffer else {
            set_vertex_bound(
                &mut self.state.borrow_mut().vertex_bounds,
                slot.0 as usize,
                None,
            );
            return self.with_pass(&ctx, |pass| {
                pass.set_vertex_buffer(slot.0, Option::<wgpu::BufferSlice<'_>>::None);
            });
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let parent = self.state.borrow().parent.clone();
        let (device_id, max_vertex_buffers) = {
            let parent = parent.borrow();
            (parent.device_id, parent.max_vertex_buffers)
        };
        let bind = crate::buffer_bind(
            &buffer,
            offset,
            size.0.flatten().map(|value| value.0),
            wgpu::BufferUsages::VERTEX,
            4,
            device_id,
            slot.0 >= max_vertex_buffers,
        );
        parent.borrow_mut().used_mapped.push(buffer.state.clone());
        if bind.invalid {
            parent.borrow_mut().invalid = true;
            return Ok(());
        }
        let Some((start, end)) = bind.slice else {
            set_vertex_bound(
                &mut self.state.borrow_mut().vertex_bounds,
                slot.0 as usize,
                Some(0),
            );
            return Ok(());
        };
        set_vertex_bound(
            &mut self.state.borrow_mut().vertex_bounds,
            slot.0 as usize,
            Some(end.saturating_sub(start)),
        );
        let inner = buffer.inner.clone();
        drop(buffer);
        self.with_pass(&ctx, |pass| {
            pass.set_vertex_buffer(slot.0, static_slice(&inner, start, end));
        })
    }

    pub fn set_index_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let format = format::index_format(&index_format, &ctx)?;
        let stride = match format {
            wgpu::IndexFormat::Uint16 => 2,
            wgpu::IndexFormat::Uint32 => 4,
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let parent = self.state.borrow().parent.clone();
        let device_id = parent.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset,
            size.0.flatten().map(|value| value.0),
            wgpu::BufferUsages::INDEX,
            stride,
            device_id,
            false,
        );
        parent.borrow_mut().used_mapped.push(buffer.state.clone());
        if bind.invalid {
            parent.borrow_mut().invalid = true;
            let mut state = self.state.borrow_mut();
            state.index_count = Some(0);
            state.index_format = Some(format);
            state.index_native = false;
            return Ok(());
        }
        self.state.borrow_mut().index_format = Some(format);
        let Some((start, end)) = bind.slice else {
            let mut state = self.state.borrow_mut();
            state.index_count = Some(0);
            state.index_native = false;
            return Ok(());
        };
        {
            let mut state = self.state.borrow_mut();
            state.index_count = Some(index_count(end - start, stride));
            state.index_native = true;
        }
        let inner = buffer.inner.clone();
        drop(buffer);
        self.with_pass(&ctx, |pass| {
            pass.set_index_buffer(static_slice(&inner, start, end), format);
        })
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let Some(vertex_end) = first_vertex.checked_add(vertex_count.0) else {
            self.mark_invalid();
            return Ok(());
        };
        let Some(instance_end) = first_instance.checked_add(instance_count) else {
            self.mark_invalid();
            return Ok(());
        };
        if self.draw_vertex_oob(vertex_end, instance_end, false) {
            self.mark_invalid();
            return Ok(());
        }
        if self.skip_native_draw() {
            return Ok(());
        }
        self.with_pass(&ctx, |pass| {
            pass.draw(first_vertex..vertex_end, first_instance..instance_end);
        })
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'_>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        let Some(index_end) = first_index.checked_add(index_count.0) else {
            self.mark_invalid();
            return Ok(());
        };
        let Some(instance_end) = first_instance.checked_add(instance_count) else {
            self.mark_invalid();
            return Ok(());
        };
        let (available, strip_mismatch) = {
            let state = self.state.borrow();
            (
                state.index_count.unwrap_or(0),
                state.is_strip && state.strip_index_format != state.index_format,
            )
        };
        if u64::from(index_end) > available || strip_mismatch {
            self.mark_invalid();
            return Ok(());
        }
        if self.draw_vertex_oob(index_count.0.min(1), instance_end, true) {
            self.mark_invalid();
            return Ok(());
        }
        // Zero-sized index buffers never get a native binding; wgpu leftover
        // "Index buffer must be set" if we still issue drawIndexed.
        if self.skip_native_draw() || !self.state.borrow().index_native || index_count.0 == 0 {
            return Ok(());
        }
        self.with_pass(&ctx, |pass| {
            pass.draw_indexed(
                first_index..index_end,
                base_vertex,
                first_instance..instance_end,
            );
        })
    }

    pub fn draw_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let buffer = buffer.borrow();
        let parent = self.state.borrow().parent.clone();
        let device_id = parent.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset.0,
            Some(DRAW_INDIRECT_BYTES),
            wgpu::BufferUsages::INDIRECT,
            4,
            device_id,
            false,
        );
        parent.borrow_mut().used_mapped.push(buffer.state.clone());
        if bind.invalid {
            self.mark_invalid();
            return Ok(());
        }
        if self.skip_native_draw() {
            return Ok(());
        }
        let inner = buffer.inner.clone();
        let offset = offset.0;
        drop(buffer);
        self.with_pass(&ctx, |pass| {
            pass.draw_indirect(&inner, offset);
        })
    }

    pub fn draw_indexed_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let buffer = buffer.borrow();
        let parent = self.state.borrow().parent.clone();
        let device_id = parent.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset.0,
            Some(DRAW_INDEXED_INDIRECT_BYTES),
            wgpu::BufferUsages::INDIRECT,
            4,
            device_id,
            false,
        );
        parent.borrow_mut().used_mapped.push(buffer.state.clone());
        if bind.invalid {
            self.mark_invalid();
            return Ok(());
        }
        let strip_mismatch = {
            let state = self.state.borrow();
            state.is_strip && state.strip_index_format != state.index_format
        };
        if strip_mismatch {
            self.mark_invalid();
            return Ok(());
        }
        if self.skip_native_draw() || !self.state.borrow().index_native {
            return Ok(());
        }
        let inner = buffer.inner.clone();
        let offset = offset.0;
        drop(buffer);
        self.with_pass(&ctx, |pass| {
            pass.draw_indexed_indirect(&inner, offset);
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
        if !self.ensure_open() {
            return Ok(());
        }
        let query_index = query_index.0;
        let (out_of_range, nested, duplicate, native) = {
            let mut state = self.state.borrow_mut();
            let out_of_range = query_index >= state.occlusion_count;
            let nested = state.occlusion_open;
            let duplicate = state.occlusion_used.contains(&query_index);
            if !out_of_range && !nested && !duplicate {
                state.occlusion_open = true;
                state.occlusion_used.push(query_index);
            }
            (out_of_range, nested, duplicate, state.occlusion_native)
        };
        if out_of_range || nested || duplicate {
            self.mark_invalid();
            return Ok(());
        }
        if !native {
            let _ = ctx;
            return Ok(());
        }
        self.with_pass(&ctx, |pass| pass.begin_occlusion_query(query_index))
    }

    pub fn end_occlusion_query(&self, ctx: Ctx<'_>) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let (was_open, native) = {
            let mut state = self.state.borrow_mut();
            let was_open = state.occlusion_open;
            state.occlusion_open = false;
            (was_open, state.occlusion_native)
        };
        if !was_open {
            self.mark_invalid();
            return Ok(());
        }
        if !native {
            let _ = ctx;
            return Ok(());
        }
        self.with_pass(&ctx, wgpu::RenderPass::end_occlusion_query)
    }

    pub fn execute_bundles<'js>(&self, bundles: Array<'js>, ctx: Ctx<'js>) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let mut used_mapped = Vec::new();
        let mut used_destroyed = Vec::new();
        let mut mismatch = false;
        let mut replays = Vec::new();
        let (pass_depth, pass_stencil, pass_colors, pass_depth_format, pass_samples, device_id) = {
            let state = self.state.borrow();
            (
                state.depth_read_only,
                state.stencil_read_only,
                state.color_formats.clone(),
                state.depth_format,
                state.sample_count,
                state.parent.borrow().device_id,
            )
        };
        for bundle in bundles.iter::<Class<GPURenderBundle>>() {
            let bundle = bundle?;
            let bundle = bundle.borrow();
            let info = &bundle.info;
            used_mapped.extend(info.used_mapped.iter().cloned());
            used_destroyed.extend(info.used_destroyed.iter().cloned());
            mismatch |= info.device_id != device_id
                || info.color_formats.as_ref() != pass_colors.as_ref()
                || info.depth_format != pass_depth_format
                || info.sample_count != pass_samples
                || bundle_pass_readonly_mismatch(
                    pass_depth,
                    pass_stencil,
                    info.depth_read_only,
                    info.stencil_read_only,
                );
            replays.push(info.commands.clone());
        }
        {
            let parent = self.state.borrow().parent.clone();
            let mut parent = parent.borrow_mut();
            parent.used_mapped.extend(used_mapped);
            parent.used_destroyed.extend(used_destroyed);
        }
        if mismatch {
            self.mark_invalid();
        }
        self.state.borrow_mut().immediate_filled = 0;
        let parent_invalid = self.state.borrow().parent.borrow().invalid;
        if !mismatch && !parent_invalid {
            self.with_pass(&ctx, |pass| {
                for commands in &replays {
                    for command in commands.iter() {
                        command.replay(pass);
                    }
                }
            })?;
        }
        Ok(())
    }

    pub fn end(&self, ctx: Ctx<'_>) -> Result<()> {
        let _ = ctx;
        if self.state.borrow().occlusion_open {
            self.mark_invalid();
        }
        {
            let state = self.state.borrow();
            let mut uses = state.attachment_uses.clone();
            uses.extend(
                state
                    .bound_groups
                    .slots
                    .iter()
                    .filter_map(Option::as_ref)
                    .flat_map(|slot| slot.textures.iter().copied()),
            );
            if crate::texture_uses_conflict(&uses, false) {
                drop(state);
                self.mark_invalid();
            }
        }
        if !self.state.borrow_mut().end() {
            self.errors
                .validation("GPURenderPassEncoder is already ended");
        }
        Ok(())
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

    pub fn set_immediates<'js>(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let bytes = crate::immediates_bytes(data, data_offset, size, &ctx)?;
        let max = self.state.borrow().parent.borrow().max_immediate_size;
        if crate::immediates_range_invalid(offset.0, bytes.len(), max) {
            self.mark_invalid();
        } else {
            let slots = crate::immediate_slots_from_range(offset.0, bytes.len());
            self.state.borrow_mut().immediate_filled |= slots;
        }
        Ok(())
    }
}

fn bundle_pass_readonly_mismatch(
    pass_depth: Option<bool>, pass_stencil: Option<bool>, bundle_depth: Option<bool>,
    bundle_stencil: Option<bool>,
) -> bool {
    let depth = match (pass_depth, bundle_depth) {
        (Some(pass), Some(bundle)) => pass && bundle != pass,
        _ => false,
    };
    let stencil = match (pass_stencil, bundle_stencil) {
        (Some(pass), Some(bundle)) => pass && bundle != pass,
        _ => false,
    };
    depth || stencil
}

enum BundleCmd {
    SetPipeline(wgpu::RenderPipeline),
    SetVertexBuffer {
        slot:   u32,
        buffer: wgpu::Buffer,
        start:  u64,
        end:    u64,
    },
    UnsetVertexBuffer {
        slot: u32,
    },
    SetIndexBuffer {
        buffer: wgpu::Buffer,
        format: wgpu::IndexFormat,
        start:  u64,
        end:    u64,
    },
    Draw {
        first_vertex:   u32,
        vertex_end:     u32,
        first_instance: u32,
        instance_end:   u32,
    },
    DrawIndexed {
        first_index:    u32,
        index_end:      u32,
        base_vertex:    i32,
        first_instance: u32,
        instance_end:   u32,
    },
    SetBindGroup {
        index:   u32,
        group:   Option<wgpu::BindGroup>,
        offsets: Vec<u32>,
    },
}

struct BundleInfo {
    used_mapped:       Rc<[Rc<RefCell<crate::BufferMapState>>]>,
    used_destroyed:    Rc<[Rc<std::cell::Cell<bool>>]>,
    device_id:         u64,
    color_formats:     Rc<[Option<wgpu::TextureFormat>]>,
    depth_format:      Option<wgpu::TextureFormat>,
    sample_count:      u32,
    depth_read_only:   Option<bool>,
    stencil_read_only: Option<bool>,
    commands:          Rc<[BundleCmd]>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundle")]
pub struct GPURenderBundle {
    #[qjs(skip_trace)]
    info:  Rc<BundleInfo>,
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

struct BundleEncoderState {
    encoder:            Option<wgpu::RenderBundleEncoder<'static>>,
    open:               bool,
    debug_groups:       usize,
    invalid:            bool,
    device_id:          u64,
    max_vertex_buffers: u32,
    max_immediate_size: u32,
    max_bind_groups:    u32,
    index_count:        Option<u64>,
    index_format:       Option<wgpu::IndexFormat>,
    is_strip:           bool,
    strip_index_format: Option<wgpu::IndexFormat>,
    used_mapped:        Vec<Rc<RefCell<crate::BufferMapState>>>,
    used_destroyed:     Vec<Rc<std::cell::Cell<bool>>>,
    color_formats:      Rc<[Option<wgpu::TextureFormat>]>,
    depth_format:       Option<wgpu::TextureFormat>,
    sample_count:       u32,
    depth_read_only:    Option<bool>,
    stencil_read_only:  Option<bool>,
    pipeline_bound:     bool,
    native_pipeline:    bool,
    bound_groups:       crate::BindGroupSet,
    pipeline_groups:    crate::PipelineLayoutGroups,
    pipeline_auto:      bool,
    pipeline_auto_id:   u64,
    vertex_layouts:     Rc<[Option<VertexLayout>]>,
    vertex_bounds:      Vec<Option<u64>>,
    immediate_required: u64,
    immediate_filled:   u64,
    commands:           Vec<BundleCmd>,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPURenderBundleEncoder")]
pub struct GPURenderBundleEncoder {
    #[qjs(skip_trace)]
    errors: ErrorSink,
    #[qjs(skip_trace)]
    label:  Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state:  Rc<RefCell<BundleEncoderState>>,
}

impl GPURenderBundleEncoder {
    fn ensure_open(&self) -> bool {
        if self.state.borrow().open {
            true
        } else {
            self.errors
                .validation("GPURenderBundleEncoder is already finished");
            false
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "JS methods return Result; keep one helper shape"
    )]
    fn with_encoder<T: Default>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::RenderBundleEncoder<'static>) -> T,
    ) -> Result<T> {
        let _ = (ctx, operation);
        if !self.ensure_open() {
            return Ok(T::default());
        }
        Ok(T::default())
    }

    fn skip_native_bundle_draw(&self) -> bool {
        let state = self.state.borrow();
        !state.native_pipeline || state.immediate_required != 0 || state.invalid
    }

    fn draw_invalid(&self, vertex_end: u32, instance_end: u32, indexed: bool) -> bool {
        let state = self.state.borrow();
        if !state.pipeline_bound {
            return true;
        }
        crate::pipeline_bind_groups_mismatch(
            &state.pipeline_groups,
            state.pipeline_auto,
            state.pipeline_auto_id,
            &state.bound_groups,
        ) || vertex_buffers_oob(
            &state.vertex_layouts,
            &state.vertex_bounds,
            vertex_end,
            instance_end,
            indexed,
        ) || crate::immediates_unfilled(state.immediate_required, state.immediate_filled)
    }
}

fn index_count(bytes: u64, stride: u64) -> u64 {
    match stride {
        4 => bytes >> 2,
        _ => bytes >> 1,
    }
}

fn static_slice(buffer: &wgpu::Buffer, offset: u64, end: u64) -> wgpu::BufferSlice<'static> {
    // SAFETY: same as `static_ref`; the slice is consumed by the encoder call.
    unsafe { std::mem::transmute(buffer.slice(offset..end)) }
}

impl BundleCmd {
    fn replay(&self, pass: &mut wgpu::RenderPass<'_>) {
        match self {
            Self::SetPipeline(pipeline) => pass.set_pipeline(pipeline),
            Self::SetVertexBuffer {
                slot,
                buffer,
                start,
                end,
            } => pass.set_vertex_buffer(*slot, static_slice(buffer, *start, *end)),
            Self::UnsetVertexBuffer { slot } => {
                pass.set_vertex_buffer(*slot, Option::<wgpu::BufferSlice<'_>>::None);
            }
            Self::SetIndexBuffer {
                buffer,
                format,
                start,
                end,
            } => pass.set_index_buffer(static_slice(buffer, *start, *end), *format),
            Self::Draw {
                first_vertex,
                vertex_end,
                first_instance,
                instance_end,
            } => pass.draw(*first_vertex..*vertex_end, *first_instance..*instance_end),
            Self::DrawIndexed {
                first_index,
                index_end,
                base_vertex,
                first_instance,
                instance_end,
            } => {
                pass.draw_indexed(
                    *first_index..*index_end,
                    *base_vertex,
                    *first_instance..*instance_end,
                );
            }
            Self::SetBindGroup {
                index,
                group,
                offsets,
            } => pass.set_bind_group(*index, group.as_ref(), offsets),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPURenderBundleEncoder {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        let _ = value;
        self.state.borrow_mut().debug_groups += 1;
        self.with_encoder(&ctx, |_| {})
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'_>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.debug_groups == 0 {
            state.invalid = true;
        } else {
            state.debug_groups -= 1;
        }
        drop(state);
        self.with_encoder(&ctx, |_| {})
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        let _ = value;
        self.with_encoder(&ctx, |_| {})
    }

    pub fn set_pipeline<'js>(
        &self, pipeline: Class<'js, GPURenderPipeline>, _ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let pipeline = pipeline.borrow();
        let device_id = self.state.borrow().device_id;
        if pipeline.info.invalid || pipeline.info.device_id != device_id {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        let native = !pipeline.info.dummy;
        let inner = native.then(|| pipeline.inner.clone());
        {
            let mut state = self.state.borrow_mut();
            state.is_strip = pipeline.info.is_strip;
            state.strip_index_format = pipeline.info.strip_index_format;
            state.pipeline_bound = true;
            state.pipeline_groups = pipeline.info.layout_groups.clone();
            state.pipeline_auto = pipeline.info.auto_layout;
            state.pipeline_auto_id = pipeline.info.auto_layout_id;
            state.vertex_layouts = pipeline.info.vertex_layouts.clone();
            state.native_pipeline = native;
            state.immediate_required = pipeline.info.immediate_slots;
        }
        drop(pipeline);
        if let Some(inner) = inner {
            self.state
                .borrow_mut()
                .commands
                .push(BundleCmd::SetPipeline(inner));
        }
        Ok(())
    }

    pub fn set_bind_group<'js>(
        &self, index: JsU32, bind_group: Option<Class<'js, GPUBindGroup>>,
        dynamic_offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let offsets = crate::dynamic_offsets(&ctx, dynamic_offsets, start, length)?;
        let native = bind_group.as_ref().and_then(|group| {
            let group = group.borrow();
            (!group.invalid).then(|| group.inner.clone())
        });
        let bind_group = bind_group.as_ref().map(|group| group.borrow());
        let applied = {
            let mut state = self.state.borrow_mut();
            let device_id = state.device_id;
            let max_bind_groups = state.max_bind_groups;
            state.bound_groups.apply(
                index.0,
                bind_group.as_deref(),
                &offsets,
                device_id,
                max_bind_groups,
            )
        };
        {
            let mut state = self.state.borrow_mut();
            state.used_destroyed.extend(applied.used_destroyed);
            state.used_mapped.extend(applied.used_mapped);
            if applied.invalid {
                state.invalid = true;
            } else {
                state.commands.push(BundleCmd::SetBindGroup {
                    index: index.0,
                    group: native,
                    offsets,
                });
            }
        }
        let _ = ctx;
        Ok(())
    }

    pub fn set_vertex_buffer<'js>(
        &self, slot: JsU32, buffer: Option<Class<'js, GPUBuffer>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let Some(buffer) = buffer else {
            set_vertex_bound(
                &mut self.state.borrow_mut().vertex_bounds,
                slot.0 as usize,
                None,
            );
            self.state
                .borrow_mut()
                .commands
                .push(BundleCmd::UnsetVertexBuffer { slot: slot.0 });
            return Ok(());
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let (device_id, max_vertex_buffers) = {
            let state = self.state.borrow();
            (state.device_id, state.max_vertex_buffers)
        };
        let bind = crate::buffer_bind(
            &buffer,
            offset,
            size.0.flatten().map(|value| value.0),
            wgpu::BufferUsages::VERTEX,
            4,
            device_id,
            slot.0 >= max_vertex_buffers,
        );
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        if bind.invalid {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        let Some((start, end)) = bind.slice else {
            set_vertex_bound(
                &mut self.state.borrow_mut().vertex_bounds,
                slot.0 as usize,
                Some(0),
            );
            return Ok(());
        };
        set_vertex_bound(
            &mut self.state.borrow_mut().vertex_bounds,
            slot.0 as usize,
            Some(end.saturating_sub(start)),
        );
        let inner = buffer.inner.clone();
        drop(buffer);
        let _ = ctx;
        self.state
            .borrow_mut()
            .commands
            .push(BundleCmd::SetVertexBuffer {
                slot: slot.0,
                buffer: inner,
                start,
                end,
            });
        Ok(())
    }

    pub fn set_index_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, index_format: String, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let format = format::index_format(&index_format, &ctx)?;
        let stride = match format {
            wgpu::IndexFormat::Uint16 => 2,
            wgpu::IndexFormat::Uint32 => 4,
        };
        let buffer = buffer.borrow();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let device_id = self.state.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset,
            size.0.flatten().map(|value| value.0),
            wgpu::BufferUsages::INDEX,
            stride,
            device_id,
            false,
        );
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        if bind.invalid {
            let mut state = self.state.borrow_mut();
            state.invalid = true;
            state.index_count = Some(0);
            state.index_format = Some(format);
            return Ok(());
        }
        self.state.borrow_mut().index_format = Some(format);
        let Some((start, end)) = bind.slice else {
            self.state.borrow_mut().index_count = Some(0);
            return Ok(());
        };
        self.state.borrow_mut().index_count = Some(index_count(end - start, stride));
        let inner = buffer.inner.clone();
        drop(buffer);
        let _ = ctx;
        self.state
            .borrow_mut()
            .commands
            .push(BundleCmd::SetIndexBuffer {
                buffer: inner,
                format,
                start,
                end,
            });
        Ok(())
    }

    pub fn draw(
        &self, vertex_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_vertex: Opt<Option<JsU32>>, first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let first_vertex = first_vertex.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let Some(vertex_end) = first_vertex.checked_add(vertex_count.0) else {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        };
        let Some(instance_end) = first_instance.checked_add(instance_count) else {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        };
        if self.draw_invalid(vertex_end, instance_end, false) {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if !self.skip_native_bundle_draw() {
            self.state.borrow_mut().commands.push(BundleCmd::Draw {
                first_vertex,
                vertex_end,
                first_instance,
                instance_end,
            });
        }
        let _ = ctx;
        Ok(())
    }

    pub fn draw_indexed(
        &self, index_count: JsU32, instance_count: Opt<Option<JsU32>>,
        first_index: Opt<Option<JsU32>>, base_vertex: Opt<Value<'_>>,
        first_instance: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let first_index = first_index.0.flatten().map_or(0, |value| value.0);
        let instance_count = instance_count.0.flatten().map_or(1, |value| value.0);
        let first_instance = first_instance.0.flatten().map_or(0, |value| value.0);
        let base_vertex = format::signed_i32(base_vertex.0, &ctx, 0)?;
        let Some(index_end) = first_index.checked_add(index_count.0) else {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        };
        let Some(instance_end) = first_instance.checked_add(instance_count) else {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        };
        let (available, strip_mismatch) = {
            let state = self.state.borrow();
            (
                state.index_count.unwrap_or(0),
                state.is_strip && state.strip_index_format != state.index_format,
            )
        };
        if u64::from(index_end) > available || strip_mismatch {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if self.draw_invalid(index_count.0.min(1), instance_end, true) {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if !self.skip_native_bundle_draw() && available > 0 {
            self.state
                .borrow_mut()
                .commands
                .push(BundleCmd::DrawIndexed {
                    first_index,
                    index_end,
                    base_vertex,
                    first_instance,
                    instance_end,
                });
        }
        let _ = ctx;
        Ok(())
    }

    pub fn set_immediates<'js>(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let bytes = crate::immediates_bytes(data, data_offset, size, &ctx)?;
        let max = self.state.borrow().max_immediate_size;
        if crate::immediates_range_invalid(offset.0, bytes.len(), max) {
            self.state.borrow_mut().invalid = true;
        } else {
            let slots = crate::immediate_slots_from_range(offset.0, bytes.len());
            self.state.borrow_mut().immediate_filled |= slots;
        }
        Ok(())
    }

    pub fn draw_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let buffer = buffer.borrow();
        let device_id = self.state.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset.0,
            Some(DRAW_INDIRECT_BYTES),
            wgpu::BufferUsages::INDIRECT,
            4,
            device_id,
            false,
        );
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        if bind.invalid {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if self.draw_invalid(0, 0, false) {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        let _ = ctx;
        Ok(())
    }

    pub fn draw_indexed_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let buffer = buffer.borrow();
        let device_id = self.state.borrow().device_id;
        let bind = crate::buffer_bind(
            &buffer,
            offset.0,
            Some(DRAW_INDEXED_INDIRECT_BYTES),
            wgpu::BufferUsages::INDIRECT,
            4,
            device_id,
            false,
        );
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        if bind.invalid {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if bind.slice.is_none() {
            return Ok(());
        }
        let strip_mismatch = {
            let state = self.state.borrow();
            state.is_strip && state.strip_index_format != state.index_format
        };
        if strip_mismatch {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        if self.draw_invalid(0, 0, true) {
            self.state.borrow_mut().invalid = true;
            return Ok(());
        }
        let _ = ctx;
        Ok(())
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
        let mut state = self.state.borrow_mut();
        if !state.open {
            self.errors
                .validation("GPURenderBundleEncoder is already finished");
            drop(state);
            return Ok(GPURenderBundle {
                info:  Rc::new(BundleInfo {
                    used_mapped:       Rc::from(Vec::new()),
                    used_destroyed:    Rc::from(Vec::new()),
                    device_id:         0,
                    color_formats:     Rc::from(Vec::new()),
                    depth_format:      None,
                    sample_count:      1,
                    depth_read_only:   None,
                    stencil_read_only: None,
                    commands:          Rc::from(Vec::new()),
                }),
                label: Rc::new(RefCell::new(label)),
            });
        }
        state.open = false;
        if state.invalid || state.debug_groups != 0 {
            self.errors
                .validation("GPURenderBundleEncoder debug groups are unbalanced");
        }
        let skip_replay = state.invalid || state.debug_groups != 0;
        let encoder = state.encoder.take();
        let used_mapped = Rc::<[Rc<RefCell<crate::BufferMapState>>]>::from(std::mem::take(
            &mut state.used_mapped,
        ));
        let used_destroyed =
            Rc::<[Rc<std::cell::Cell<bool>>]>::from(std::mem::take(&mut state.used_destroyed));
        let commands = if skip_replay {
            Rc::from(Vec::new())
        } else {
            Rc::from(std::mem::take(&mut state.commands))
        };
        let device_id = state.device_id;
        let color_formats = state.color_formats.clone();
        let depth_format = state.depth_format;
        let sample_count = state.sample_count;
        let depth_read_only = state.depth_read_only;
        let stencil_read_only = state.stencil_read_only;
        drop(state);
        drop(encoder);
        let _ = ctx;
        Ok(GPURenderBundle {
            info:  Rc::new(BundleInfo {
                used_mapped,
                used_destroyed,
                device_id,
                color_formats,
                depth_format,
                sample_count,
                depth_read_only,
                stencil_read_only,
                commands,
            }),
            label: Rc::new(RefCell::new(label)),
        })
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "bundle encoder construction needs device limits plus the JS descriptor"
)]
pub fn new_bundle_encoder(
    device: &wgpu::Device, descriptor: Object<'_>, ctx: &Ctx<'_>, errors: ErrorSink,
    device_id: u64, max_vertex_buffers: u32, max_immediate_size: u32, max_bind_groups: u32,
) -> Result<GPURenderBundleEncoder> {
    let label = label(&descriptor)?;
    let color_formats = descriptor
        .get::<_, Array>("colorFormats")
        .map_err(|_error| type_error(ctx, "colorFormats must be an array"))?;
    let mut formats = Vec::with_capacity(color_formats.len());
    for format in color_formats.iter::<Option<String>>() {
        let parsed = format?
            .map(|name| format::texture_format(&name, ctx))
            .transpose()?;
        if let Some(format) = parsed
            && !device.features().contains(format.required_features())
        {
            return Err(type_error(ctx, "color format requires missing features"));
        }
        formats.push(parsed);
    }
    let depth_stencil = match descriptor.get::<_, Option<String>>("depthStencilFormat")? {
        Some(name) => {
            let format = format::texture_format(&name, ctx)?;
            if !device.features().contains(format.required_features()) {
                return Err(type_error(
                    ctx,
                    "depth stencil format requires missing features",
                ));
            }
            Some(wgpu::RenderBundleDepthStencil {
                format,
                depth_read_only: descriptor
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
    let _ = device;
    Ok(GPURenderBundleEncoder {
        errors,
        label: Rc::new(RefCell::new(label)),
        state: Rc::new(RefCell::new(BundleEncoderState {
            // wgpu 30 `RenderBundleEncoder::finish` is fatal on a worker, and
            // Drop of an unused encoder has SIGSEGV'd CTS cartesian bundle
            // cases. Commands are software-recorded and replayed in
            // executeBundles.
            encoder: None,
            open: true,
            debug_groups: 0,
            invalid: false,
            device_id,
            max_vertex_buffers,
            max_immediate_size,
            max_bind_groups,
            index_count: None,
            index_format: None,
            is_strip: false,
            strip_index_format: None,
            used_mapped: Vec::new(),
            used_destroyed: Vec::new(),
            color_formats: Rc::from(formats),
            depth_format: depth_stencil.as_ref().map(|state| state.format),
            sample_count,
            depth_read_only: depth_stencil.as_ref().map(|state| state.depth_read_only),
            stencil_read_only: depth_stencil.as_ref().map(|state| state.stencil_read_only),
            pipeline_bound: false,
            native_pipeline: false,
            bound_groups: crate::BindGroupSet::default(),
            pipeline_groups: crate::empty_layout_groups(),
            pipeline_auto: false,
            pipeline_auto_id: 0,
            vertex_layouts: Rc::from(Vec::new()),
            vertex_bounds: Vec::new(),
            immediate_required: 0,
            immediate_filled: 0,
            commands: Vec::new(),
        })),
    })
}

pub fn create_pipeline<'js>(
    device: &GPUDevice<'_>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(GPURenderPipeline, bool)> {
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
    let auto_layout = layout.is_none();
    let auto_layout_id = if auto_layout {
        crate::next_resource_id()
    } else {
        0
    };
    let stored_groups = layout_groups.unwrap_or_else(|| {
        crate::auto_layout_groups(&[
            (wgpu::ShaderStages::VERTEX, vertex_code.as_ref()),
            (
                wgpu::ShaderStages::FRAGMENT,
                fragment_code.as_deref().unwrap_or(""),
            ),
        ])
    });
    let vertex_layouts = Rc::from(
        owned_buffers
            .iter()
            .map(|buffer| {
                buffer.as_ref().map(|(stride, step, attributes)| {
                    let last_stride = attributes
                        .iter()
                        .map(|attribute| attribute.offset.saturating_add(attribute.format.size()))
                        .max()
                        .unwrap_or(0);
                    VertexLayout {
                        array_stride: *stride,
                        step_mode: *step,
                        last_stride,
                    }
                })
            })
            .collect::<Vec<_>>(),
    );
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
    let immediate_slots = {
        let vertex_bytes = format::immediate_byte_size(&vertex_code);
        let fragment_bytes = fragment_code
            .as_deref()
            .map_or(0, format::immediate_byte_size);
        let vertex_slots = if vertex_bytes > format::NAGA_IMMEDIATE_SLOT_BYTES {
            format::immediate_slots_mask(vertex_bytes)
        } else {
            format::immediate_slots_used(
                &vertex_code,
                vertex_entry.as_deref(),
                format::ShaderStage::Vertex,
            )
        };
        let fragment_slots = if fragment_bytes > format::NAGA_IMMEDIATE_SLOT_BYTES {
            format::immediate_slots_mask(fragment_bytes)
        } else {
            fragment_code.as_deref().map_or(0, |code| {
                format::immediate_slots_used(
                    code,
                    fragment_entry.as_deref(),
                    format::ShaderStage::Fragment,
                )
            })
        };
        vertex_slots | fragment_slots
    };
    let skip_immediate = vertex_code.contains("var<immediate")
        || fragment_code
            .as_deref()
            .is_some_and(|code| code.contains("var<immediate"));
    if invalid
        || has_external
        || naga_interp_skip
        || no_frag_out
        || skip_storage
        || skip_immediate
        || skip_native_inter_stage
    {
        let bgls =
            crate::bind_group_layouts_from_groups(&device.device, &stored_groups, &empty_bgl);
        return Ok((
            GPURenderPipeline {
                inner: device.fallbacks.render_pipeline.clone(),
                info:  Rc::new(RenderPipelineInfo {
                    invalid,
                    device_id: device.id,
                    is_strip: matches!(
                        primitive.topology,
                        wgpu::PrimitiveTopology::LineStrip | wgpu::PrimitiveTopology::TriangleStrip
                    ),
                    strip_index_format: primitive.strip_index_format,
                    dummy: true,
                    layout_groups: stored_groups,
                    auto_layout,
                    auto_layout_id,
                    empty_bgl,
                    bgls,
                    errors: device.errors.clone(),
                    max_bind_groups: device.reported_limits().max_bind_groups,
                    immediate_slots,
                    vertex_layouts,
                }),
                label: Rc::new(RefCell::new(label)),
            },
            invalid,
        ));
    }
    let bgls = crate::bind_group_layouts_from_groups(&device.device, &stored_groups, &empty_bgl);
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
    Ok((
        GPURenderPipeline {
            inner,
            info: Rc::new(RenderPipelineInfo {
                invalid,
                device_id: device.id,
                is_strip: matches!(
                    primitive.topology,
                    wgpu::PrimitiveTopology::LineStrip | wgpu::PrimitiveTopology::TriangleStrip
                ),
                strip_index_format: primitive.strip_index_format,
                dummy: invalid,
                layout_groups: stored_groups,
                auto_layout,
                auto_layout_id,
                empty_bgl,
                bgls,
                errors: device.errors.clone(),
                max_bind_groups: device.reported_limits().max_bind_groups,
                immediate_slots,
                vertex_layouts,
            }),
            label: Rc::new(RefCell::new(label)),
        },
        invalid,
    ))
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

fn pass_attachment_texture_use<'js>(
    attachment: &Object<'js>, key: &str, ctx: &Ctx<'js>, write: bool,
) -> Result<Option<crate::TextureUse>> {
    let value = attachment.get::<_, Value>(key)?;
    if let Ok(view) = Class::<GPUTextureView>::from_js(ctx, value.clone()) {
        return Ok(Some(crate::TextureUse::from_view(
            &view.borrow(),
            write,
            crate::TextureUseKind::Attachment,
        )));
    }
    if let Ok(texture) = Class::<GPUTexture>::from_js(ctx, value) {
        return Ok(Some(crate::TextureUse::from_texture(
            &texture.borrow(),
            write,
            crate::TextureUseKind::Attachment,
        )));
    }
    Ok(None)
}

fn pass_attachment_view<'js>(
    attachment: &Object<'js>, key: &str, ctx: &Ctx<'js>,
) -> Result<(
    wgpu::TextureView,
    wgpu::TextureFormat,
    u32,
    wgpu::TextureUsages,
)> {
    let value = attachment
        .get::<_, Value>(key)
        .map_err(|_error| type_error(ctx, format!("{key} must be a WebGPU object")))?;
    if let Ok(view) = Class::<GPUTextureView>::from_js(ctx, value.clone()) {
        let view = view.borrow();
        return Ok((
            view.inner.clone(),
            view.format,
            view.sample_count().max(1),
            view.usage,
        ));
    }
    if let Ok(texture) = Class::<GPUTexture>::from_js(ctx, value) {
        let texture = texture.borrow();
        return Ok((
            texture.default_view(),
            texture.format,
            texture.samples.max(1),
            texture.usage,
        ));
    }
    Err(type_error(
        ctx,
        format!("{key} must be a GPUTexture or GPUTextureView"),
    ))
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

pub struct BegunRenderPass {
    pub label:               String,
    pub pass:                Option<wgpu::RenderPass<'static>>,
    pub invalid:             bool,
    pub depth_read_only:     Option<bool>,
    pub stencil_read_only:   Option<bool>,
    pub color_formats:       Rc<[Option<wgpu::TextureFormat>]>,
    pub depth_format:        Option<wgpu::TextureFormat>,
    pub sample_count:        u32,
    pub occlusion_native:    bool,
    pub occlusion_count:     u32,
    pub occlusion_destroyed: Option<Rc<std::cell::Cell<bool>>>,
    pub attachment_uses:     Vec<crate::TextureUse>,
}

pub fn begin_render_pass<'js>(
    encoder: &mut wgpu::CommandEncoder, descriptor: Object<'js>, ctx: &Ctx<'js>, device_id: u64,
    native: bool,
) -> Result<BegunRenderPass> {
    let label = label(&descriptor)?;
    let color_value = descriptor
        .get::<_, Array>("colorAttachments")
        .map_err(|_error| type_error(ctx, "colorAttachments must be an array"))?;
    let mut owned_colors = Vec::with_capacity(color_value.len());
    let mut color_formats = Vec::with_capacity(color_value.len());
    let mut attachment_uses = Vec::new();
    let mut sample_count = 1;
    for attachment in color_value.iter::<Option<Object>>() {
        let Some(attachment) = attachment? else {
            owned_colors.push(None);
            color_formats.push(None);
            continue;
        };
        let (view, format, samples, _usage) = pass_attachment_view(&attachment, "view", ctx)?;
        if let Some(used) = pass_attachment_texture_use(&attachment, "view", ctx, true)? {
            attachment_uses.push(used);
        }
        color_formats.push(Some(format));
        sample_count = samples;
        let resolve = match attachment.get::<_, Option<Value>>("resolveTarget")? {
            Some(value) if !value.is_null() && !value.is_undefined() => {
                if let Ok(view) = Class::<GPUTextureView>::from_js(ctx, value.clone()) {
                    Some(view.borrow().inner.clone())
                } else if let Ok(texture) = Class::<GPUTexture>::from_js(ctx, value) {
                    Some(texture.borrow().default_view())
                } else {
                    return Err(type_error(
                        ctx,
                        "resolveTarget must be a GPUTexture or GPUTextureView",
                    ));
                }
            }
            _ => None,
        };
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
    let mut depth_format = None;
    let mut ds_invalid = false;
    let owned_depth = descriptor
        .get::<_, Option<Object>>("depthStencilAttachment")?
        .map(|attachment| {
            let (view, format, samples, usage) = pass_attachment_view(&attachment, "view", ctx)?;
            let depth_read_only = attachment
                .get::<_, Option<bool>>("depthReadOnly")?
                .unwrap_or_default();
            let stencil_read_only = attachment
                .get::<_, Option<bool>>("stencilReadOnly")?
                .unwrap_or_default();
            let has_depth = format.has_depth_aspect();
            let has_stencil = format.has_stencil_aspect();
            if has_depth
                && let Some(used) =
                    pass_attachment_texture_use(&attachment, "view", ctx, !depth_read_only)?
            {
                attachment_uses.push(used.with_aspect(wgpu::TextureAspect::DepthOnly));
            }
            if has_stencil
                && let Some(used) =
                    pass_attachment_texture_use(&attachment, "view", ctx, !stencil_read_only)?
            {
                attachment_uses.push(used.with_aspect(wgpu::TextureAspect::StencilOnly));
            }
            depth_format = Some(format);
            if color_formats.iter().all(Option::is_none) {
                sample_count = samples;
            }
            let depth_load = attachment.get::<_, Option<String>>("depthLoadOp")?;
            let depth_store = attachment.get::<_, Option<String>>("depthStoreOp")?;
            let stencil_load = attachment.get::<_, Option<String>>("stencilLoadOp")?;
            let stencil_store = attachment.get::<_, Option<String>>("stencilStoreOp")?;
            let has_both_depth = depth_load.is_some() && depth_store.is_some();
            let has_neither_depth = depth_load.is_none() && depth_store.is_none();
            let has_both_stencil = stencil_load.is_some() && stencil_store.is_some();
            let has_neither_stencil = stencil_load.is_none() && stencil_store.is_none();
            let has_depth_settings = has_both_depth && !depth_read_only;
            let has_stencil_settings = has_both_stencil && !stencil_read_only;
            let good_aspect =
                (!has_depth_settings || has_depth) && (!has_stencil_settings || has_stencil);
            let good_depth = if has_depth && !depth_read_only {
                has_both_depth
            } else {
                has_neither_depth
            };
            let good_stencil = if has_stencil && !stencil_read_only {
                has_both_stencil
            } else {
                has_neither_stencil
            };
            let transient = usage.contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT);
            let good_transient = !transient
                || ((!has_depth
                    || (depth_load.as_deref() == Some("clear")
                        && depth_store.as_deref() == Some("discard")))
                    && (!has_stencil
                        || (stencil_load.as_deref() == Some("clear")
                            && stencil_store.as_deref() == Some("discard"))));
            let depth_clear = attachment.get::<_, Option<f64>>("depthClearValue")?;
            let clear_invalid = depth_load.as_deref() == Some("clear")
                && !depth_clear.is_some_and(|value| (0.0..=1.0).contains(&value));
            if !good_aspect || !good_depth || !good_stencil || !good_transient || clear_invalid {
                ds_invalid = true;
            }
            let depth_ops = if depth_read_only || !has_depth {
                None
            } else {
                match (depth_load.as_deref(), depth_store.as_deref()) {
                    (Some(load), Some(store)) => {
                        let load = match load {
                            "clear" => wgpu::LoadOp::Clear(depth_clear.unwrap_or(0.0) as f32),
                            "load" => wgpu::LoadOp::Load,
                            value => {
                                return Err(type_error(ctx, format!("invalid GPULoadOp {value}")));
                            }
                        };
                        Some(wgpu::Operations {
                            load,
                            store: format::store_op(store, ctx)?,
                        })
                    }
                    _ => None,
                }
            };
            let stencil_ops = if stencil_read_only || !has_stencil {
                None
            } else {
                match (stencil_load.as_deref(), stencil_store.as_deref()) {
                    (Some(load), Some(store)) => {
                        let load = match load {
                            "clear" => {
                                wgpu::LoadOp::Clear(
                                    attachment
                                        .get::<_, Option<JsU32>>("stencilClearValue")?
                                        .map_or(0, |value| value.0),
                                )
                            }
                            "load" => wgpu::LoadOp::Load,
                            value => {
                                return Err(type_error(ctx, format!("invalid GPULoadOp {value}")));
                            }
                        };
                        Some(wgpu::Operations {
                            load,
                            store: format::store_op(store, ctx)?,
                        })
                    }
                    _ => None,
                }
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
    let mut timestamp_invalid = false;
    let timestamp_writes = match descriptor.get::<_, Option<Object>>("timestampWrites")? {
        Some(writes) => {
            let (query, beginning, end, invalid) =
                query::timestamp_writes_from(&writes, ctx, device_id)?;
            timestamp_invalid = invalid;
            timestamp_query = query;
            if invalid {
                None
            } else {
                Some(wgpu::RenderPassTimestampWrites {
                    query_set:                     &timestamp_query,
                    beginning_of_pass_write_index: beginning,
                    end_of_pass_write_index:       end,
                })
            }
        }
        None => None,
    };
    let occlusion = descriptor.get::<_, Option<Class<GPUQuerySet>>>("occlusionQuerySet")?;
    let mut occlusion_invalid = false;
    let occlusion_destroyed = occlusion
        .as_ref()
        .map(|query| query.borrow().destroyed.clone());
    let occlusion_count = occlusion.as_ref().map_or(0, |query| query.borrow().count);
    let occlusion_inner = occlusion.as_ref().and_then(|query| {
        let query = query.borrow();
        if query.invalid || query.ty != "occlusion" || query.device_id != device_id {
            occlusion_invalid = true;
            None
        } else if query.destroyed.get() {
            None
        } else {
            Some(query.inner.clone())
        }
    });
    let pass = (native && !ds_invalid).then(|| {
        encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                color_attachments: &color_attachments,
                depth_stencil_attachment,
                timestamp_writes,
                occlusion_query_set: occlusion_inner.as_ref(),
                multiview_mask: None,
            })
            .forget_lifetime()
    });
    let (depth_read_only, stencil_read_only) =
        if let Some(attachment) = descriptor.get::<_, Option<Object>>("depthStencilAttachment")? {
            (
                Some(
                    attachment
                        .get::<_, Option<bool>>("depthReadOnly")?
                        .unwrap_or_default(),
                ),
                Some(
                    attachment
                        .get::<_, Option<bool>>("stencilReadOnly")?
                        .unwrap_or_default(),
                ),
            )
        } else {
            (None, None)
        };
    Ok(BegunRenderPass {
        label,
        pass,
        invalid: timestamp_invalid || occlusion_invalid || ds_invalid,
        depth_read_only,
        stencil_read_only,
        color_formats: Rc::from(color_formats),
        depth_format,
        sample_count,
        occlusion_native: native && occlusion_inner.is_some(),
        occlusion_count,
        occlusion_destroyed,
        attachment_uses,
    })
}
