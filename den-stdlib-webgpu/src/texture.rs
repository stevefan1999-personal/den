use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use rquickjs::{Class, Coerced, Ctx, JsLifetime, Object, Result, class::Trace, function::Opt};

use crate::{
    ErrorSink, JsU32, JsU64, MAX_HOST_ALLOCATION, format, illegal_constructor, label, type_error,
};

const TEXTURE_USAGE_MASK: u32 = 0x003f;

fn dummy_extent(format: wgpu::TextureFormat, dimension: wgpu::TextureDimension) -> wgpu::Extent3d {
    let (width, height) = format.block_dimensions();
    wgpu::Extent3d {
        width:                 width.max(1),
        height:                if dimension == wgpu::TextureDimension::D1 {
            1
        } else {
            height.max(1)
        },
        depth_or_array_layers: 1,
    }
}

pub struct FallbackResources {
    sampled:               wgpu::Texture,
    msaa:                  wgpu::Texture,
    storage:               wgpu::Texture,
    color_render:          wgpu::Texture,
    depth:                 wgpu::Texture,
    pub buffer:            wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group:        wgpu::BindGroup,
    pub shader_module:     wgpu::ShaderModule,
    pub compute_pipeline:  wgpu::ComputePipeline,
    pub render_pipeline:   wgpu::RenderPipeline,
}

impl FallbackResources {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let sampled = dummy_texture(device);
        let msaa = device.create_texture(&wgpu::TextureDescriptor {
            label:           None,
            size:            wgpu::Extent3d {
                width:                 1,
                height:                1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count:    4,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::Rgba8Unorm,
            usage:           wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats:    &[],
        });
        let storage = device.create_texture(&wgpu::TextureDescriptor {
            label:           None,
            size:            wgpu::Extent3d {
                width:                 1,
                height:                1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::R32Float,
            usage:           wgpu::TextureUsages::STORAGE_BINDING,
            view_formats:    &[],
        });
        let color_render = device.create_texture(&wgpu::TextureDescriptor {
            label:           None,
            size:            wgpu::Extent3d {
                width:                 1,
                height:                1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::Rgba8Unorm,
            usage:           wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats:    &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label:           None,
            size:            wgpu::Extent3d {
                width:                 1,
                height:                1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       wgpu::TextureDimension::D2,
            format:          wgpu::TextureFormat::Depth32Float,
            usage:           wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats:    &[],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label:              None,
            size:               16,
            usage:              wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:   None,
            entries: &[],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   None,
            layout:  &layout,
            entries: &[],
        });
        let bind_group_layout = layout;
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  None,
            source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
        });
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label:               None,
            layout:              None,
            module:              &shader_module,
            entry_point:         Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache:               None,
        });
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  None,
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    "@vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }\n",
                    "@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }\n",
                )
                .into(),
            ),
        });
        let color = wgpu::ColorTargetState {
            format:     wgpu::TextureFormat::Rgba8Unorm,
            blend:      None,
            write_mask: wgpu::ColorWrites::ALL,
        };
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:          None,
            layout:         None,
            vertex:         wgpu::VertexState {
                module:              &render_shader,
                entry_point:         Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers:             &[],
            },
            primitive:      wgpu::PrimitiveState::default(),
            depth_stencil:  None,
            multisample:    wgpu::MultisampleState::default(),
            fragment:       Some(wgpu::FragmentState {
                module:              &render_shader,
                entry_point:         Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets:             &[Some(color)],
            }),
            multiview_mask: None,
            cache:          None,
        });
        Self {
            sampled,
            msaa,
            storage,
            color_render,
            depth,
            buffer,
            bind_group_layout,
            bind_group,
            shader_module,
            compute_pipeline,
            render_pipeline,
        }
    }

    pub(crate) fn view(
        &self, samples: u32, usage: wgpu::TextureUsages, format: wgpu::TextureFormat,
    ) -> wgpu::TextureView {
        let texture = if samples == 4 {
            &self.msaa
        } else if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
            &self.storage
        } else if format.has_depth_aspect() || format.has_stencil_aspect() {
            &self.depth
        } else if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            &self.color_render
        } else {
            &self.sampled
        };
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }
}

fn dummy_texture(device: &wgpu::Device) -> wgpu::Texture {
    // Always a 1x1 RGBA8 binding texture. Invalid descriptors (MSAA+storage,
    // compressed 1x1, huge extents) must never reach the driver: wgpu/Vulkan
    // SIGSEGV instead of returning a GPUValidationError.
    device.create_texture(&wgpu::TextureDescriptor {
        label:           None,
        size:            wgpu::Extent3d {
            width:                 1,
            height:                1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       wgpu::TextureDimension::D2,
        format:          wgpu::TextureFormat::Rgba8Unorm,
        usage:           wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats:    &[],
    })
}

fn dimension_format_compatible(
    dimension: wgpu::TextureDimension, format: wgpu::TextureFormat, features: wgpu::Features,
) -> bool {
    if dimension == wgpu::TextureDimension::D3 {
        if format.is_bcn() && features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D) {
            return true;
        }
        if format.is_astc() && features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D)
        {
            return true;
        }
    }
    if matches!(
        dimension,
        wgpu::TextureDimension::D1 | wgpu::TextureDimension::D3
    ) && (format.is_compressed() || format.has_depth_aspect() || format.has_stencil_aspect())
    {
        return false;
    }
    true
}

fn default_view_dimension(
    dimension: wgpu::TextureDimension, array_layers: u32,
) -> wgpu::TextureViewDimension {
    match dimension {
        wgpu::TextureDimension::D1 => wgpu::TextureViewDimension::D1,
        wgpu::TextureDimension::D3 => wgpu::TextureViewDimension::D3,
        wgpu::TextureDimension::D2 if array_layers == 1 => wgpu::TextureViewDimension::D2,
        wgpu::TextureDimension::D2 => wgpu::TextureViewDimension::D2Array,
    }
}

fn texture_byte_size(
    size: wgpu::Extent3d, dimension: wgpu::TextureDimension, format: wgpu::TextureFormat,
    mip_levels: u32, samples: u32,
) -> u64 {
    let block = u64::from(format.block_copy_size(None).unwrap_or(4));
    let (block_width, block_height) = format.block_dimensions();
    let mut width = size.width.max(1);
    let mut height = size.height.max(1);
    let mut depth = size.depth_or_array_layers.max(1);
    let mut total = 0_u64;
    for _ in 0..mip_levels.max(1) {
        let blocks_x = u64::from(width.div_ceil(block_width));
        let blocks_y = u64::from(height.div_ceil(block_height));
        total = total.saturating_add(
            blocks_x
                .saturating_mul(blocks_y)
                .saturating_mul(u64::from(depth))
                .saturating_mul(block)
                .saturating_mul(u64::from(samples.max(1))),
        );
        width = width.saturating_div(2).max(1);
        height = height.saturating_div(2).max(1);
        if dimension == wgpu::TextureDimension::D3 {
            depth = depth.saturating_div(2).max(1);
        }
    }
    total
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUTexture")]
pub struct GPUTexture {
    #[qjs(skip_trace)]
    pub(crate) destroyed:  Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    errors:                ErrorSink,
    #[qjs(skip_trace)]
    fallbacks:             Rc<FallbackResources>,
    #[qjs(skip_trace)]
    pub(crate) inner:      wgpu::Texture,
    #[qjs(skip_trace)]
    pub(crate) is_dummy:   bool,
    #[qjs(skip_trace)]
    pub(crate) label:      Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) format:     wgpu::TextureFormat,
    #[qjs(skip_trace)]
    pub(crate) size:       wgpu::Extent3d,
    #[qjs(skip_trace)]
    pub(crate) dimension:  wgpu::TextureDimension,
    #[qjs(skip_trace)]
    pub(crate) mip_levels: u32,
    #[qjs(skip_trace)]
    pub(crate) samples:    u32,
    #[qjs(skip_trace)]
    pub(crate) usage:      wgpu::TextureUsages,
    #[qjs(skip_trace)]
    pub(crate) device_id:  u64,
    #[qjs(skip_trace)]
    features:              wgpu::Features,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUTexture {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable)]
    pub const fn width(&self) -> u32 { self.size.width }

    #[qjs(get, configurable)]
    pub const fn height(&self) -> u32 { self.size.height }

    #[qjs(get, configurable)]
    pub const fn depth_or_array_layers(&self) -> u32 { self.size.depth_or_array_layers }

    #[qjs(get, configurable)]
    pub const fn mip_level_count(&self) -> u32 { self.mip_levels }

    #[qjs(get, configurable)]
    pub const fn sample_count(&self) -> u32 { self.samples }

    #[qjs(get, configurable)]
    pub fn dimension(&self) -> &'static str {
        match self.dimension {
            wgpu::TextureDimension::D1 => "1d",
            wgpu::TextureDimension::D3 => "3d",
            wgpu::TextureDimension::D2 => "2d",
        }
    }

    #[qjs(get, configurable)]
    pub fn format(&self) -> String {
        serde_json::to_string(&self.format)
            .unwrap_or_else(|_error| "\"unknown\"".into())
            .trim_matches('"')
            .to_owned()
    }

    #[qjs(get, configurable)]
    pub const fn usage(&self) -> u32 { self.usage.bits() }

    pub fn create_view(
        &self, descriptor: Opt<Option<Object<'_>>>, ctx: Ctx<'_>,
    ) -> Result<GPUTextureView> {
        let descriptor = descriptor.0.flatten();
        let label = descriptor
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let specified_format = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>("format"))
            .transpose()?
            .flatten();
        let format = specified_format
            .as_deref()
            .map(|name| format::texture_format(name, &ctx))
            .transpose()?
            .unwrap_or(self.format);
        if specified_format.is_some() && !self.features.contains(format.required_features()) {
            return Err(type_error(
                &ctx,
                "texture view format requires missing features",
            ));
        }
        let dimension = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>("dimension"))
            .transpose()?
            .flatten();
        let aspect = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>("aspect"))
            .transpose()?
            .flatten();
        let base_mip_level = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<JsU32>>("baseMipLevel"))
            .transpose()?
            .flatten()
            .map_or(0, |value| value.0);
        let mip_level_count = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<JsU32>>("mipLevelCount"))
            .transpose()?
            .flatten()
            .map(|value| value.0);
        let base_array_layer = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<JsU32>>("baseArrayLayer"))
            .transpose()?
            .flatten()
            .map_or(0, |value| value.0);
        let array_layer_count = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<JsU32>>("arrayLayerCount"))
            .transpose()?
            .flatten()
            .map(|value| value.0);
        let view_dimension =
            format::view_dimension(dimension.as_deref(), &ctx)?.unwrap_or_else(|| {
                default_view_dimension(self.dimension, self.size.depth_or_array_layers)
            });
        let swizzle = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Coerced<String>>>("swizzle"))
            .transpose()?
            .flatten()
            .map(|value| value.0);
        if swizzle.as_deref().is_some_and(|swizzle| {
            swizzle.len() != 4
                || !swizzle
                    .bytes()
                    .all(|byte| matches!(byte, b'r' | b'g' | b'b' | b'a' | b'0' | b'1'))
        }) {
            return Err(type_error(&ctx, "invalid GPUTextureComponentSwizzle"));
        }
        let non_identity_swizzle = swizzle.as_deref().is_some_and(|value| value != "rgba");
        if non_identity_swizzle {
            self.errors
                .validation("non-identity texture component swizzle");
        }
        let usage_bits = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<JsU32>>("usage"))
            .transpose()?
            .flatten()
            .map(|value| value.0);
        let extra_usage = usage_bits.is_some_and(|bits| bits & !TEXTURE_USAGE_MASK != 0);
        let specified_usage = usage_bits
            .filter(|bits| *bits != 0)
            .map(|bits| wgpu::TextureUsages::from_bits_truncate(bits & TEXTURE_USAGE_MASK));
        let usage_invalid =
            extra_usage || specified_usage.is_some_and(|usage| !self.usage.contains(usage));
        if usage_invalid {
            self.errors
                .validation("texture view usage is not a subset of texture usage");
        }
        let view_usage = specified_usage.unwrap_or(self.usage);
        let view_aspect = format::aspect(aspect);
        let resolved_mips =
            mip_level_count.unwrap_or_else(|| self.mip_levels.saturating_sub(base_mip_level));
        // Invalid textures always produce an invalid view. Destroyed textures
        // stay valid JS objects: createView must not emit GPUValidationError
        // (CTS createView.texture_state /
        // createBindGroup.texture,resource_state).
        let (inner, is_dummy) = if self.is_dummy {
            self.errors.validation("GPUTexture is invalid");
            (
                self.fallbacks
                    .view(1, wgpu::TextureUsages::TEXTURE_BINDING, self.format),
                true,
            )
        } else if non_identity_swizzle || usage_invalid {
            (
                self.fallbacks.view(self.samples, self.usage, self.format),
                true,
            )
        } else {
            let view_desc = wgpu::TextureViewDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                format: specified_format.is_some().then_some(format),
                dimension: Some(view_dimension),
                usage: specified_usage,
                aspect: view_aspect,
                base_mip_level,
                mip_level_count,
                base_array_layer,
                array_layer_count,
                swizzle: wgpu::TextureComponentSwizzle::default(),
            };
            crate::catch_gpu(&self.errors, || self.inner.create_view(&view_desc)).map_or_else(
                || {
                    (
                        self.fallbacks.view(self.samples, self.usage, self.format),
                        true,
                    )
                },
                |inner| (inner, false),
            )
        };
        Ok(GPUTextureView {
            inner,
            is_dummy,
            destroyed: self.destroyed.clone(),
            label: Rc::new(RefCell::new(label)),
            format,
            dimension: view_dimension,
            mip_count: resolved_mips.max(1),
            base_mip: base_mip_level,
            base_layer: base_array_layer,
            layer_count: array_layer_count.unwrap_or_else(|| {
                match view_dimension {
                    wgpu::TextureViewDimension::D2Array | wgpu::TextureViewDimension::CubeArray => {
                        self.size
                            .depth_or_array_layers
                            .saturating_sub(base_array_layer)
                            .max(1)
                    }
                    _ => 1,
                }
            }),
            samples: pack_view_samples(self.samples, view_aspect),
            usage: view_usage,
        })
    }

    pub fn destroy(&self) {
        // Keep the wgpu texture alive so encode can still build a matching
        // pass. CTS texture/destroy only wraps queue.submit(); encode must
        // not emit GPUValidationError. Submit reads this flag.
        self.destroyed.set(true);
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUTextureView")]
pub struct GPUTextureView {
    #[qjs(skip_trace)]
    pub(crate) inner:       wgpu::TextureView,
    #[qjs(skip_trace)]
    pub(crate) is_dummy:    bool,
    #[qjs(skip_trace)]
    pub(crate) destroyed:   Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    pub(crate) label:       Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) format:      wgpu::TextureFormat,
    #[qjs(skip_trace)]
    pub(crate) dimension:   wgpu::TextureViewDimension,
    #[qjs(skip_trace)]
    pub(crate) mip_count:   u32,
    #[qjs(skip_trace)]
    pub(crate) base_mip:    u32,
    #[qjs(skip_trace)]
    pub(crate) base_layer:  u32,
    #[qjs(skip_trace)]
    pub(crate) layer_count: u32,
    #[qjs(skip_trace)]
    pub(crate) samples:     u32,
    #[qjs(skip_trace)]
    pub(crate) usage:       wgpu::TextureUsages,
}

fn pack_view_samples(samples: u32, aspect: wgpu::TextureAspect) -> u32 {
    let tag = match aspect {
        wgpu::TextureAspect::All => 0,
        wgpu::TextureAspect::DepthOnly => 1,
        wgpu::TextureAspect::StencilOnly => 2,
        wgpu::TextureAspect::Plane0 => 3,
        wgpu::TextureAspect::Plane1 => 4,
        wgpu::TextureAspect::Plane2 => 5,
    };
    (samples & 0xff) | (tag << 8)
}

impl GPUTextureView {
    pub(crate) fn sample_count(&self) -> u32 { self.samples & 0xff }

    pub(crate) fn aspect(&self) -> wgpu::TextureAspect {
        match self.samples >> 8 {
            1 => wgpu::TextureAspect::DepthOnly,
            2 => wgpu::TextureAspect::StencilOnly,
            3 => wgpu::TextureAspect::Plane0,
            4 => wgpu::TextureAspect::Plane1,
            5 => wgpu::TextureAspect::Plane2,
            _ => wgpu::TextureAspect::All,
        }
    }

    pub(crate) fn is_valid_external_texture(&self) -> bool {
        self.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING)
            && self.dimension == wgpu::TextureViewDimension::D2
            && self.mip_count == 1
            && self.sample_count() == 1
            && matches!(
                self.format,
                wgpu::TextureFormat::Rgba8Unorm
                    | wgpu::TextureFormat::Bgra8Unorm
                    | wgpu::TextureFormat::Rgba16Float
            )
    }
}

#[rquickjs::methods]
impl GPUTextureView {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUSampler")]
pub struct GPUSampler {
    #[qjs(skip_trace)]
    pub(crate) inner:   wgpu::Sampler,
    #[qjs(skip_trace)]
    pub(crate) invalid: bool,
    #[qjs(skip_trace)]
    pub(crate) label:   Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPUSampler {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

impl GPUTexture {
    pub(crate) fn from_descriptor<'js>(
        device: &wgpu::Device, fallbacks: &Rc<FallbackResources>, descriptor: Object<'js>,
        ctx: &Ctx<'js>, errors: &ErrorSink, device_id: u64,
    ) -> Result<Self> {
        let label = label(&descriptor)?;
        let size = format::extent3d(descriptor.get("size")?, ctx)?;
        let mip_levels = descriptor
            .get::<_, Option<JsU32>>("mipLevelCount")?
            .map_or(1, |value| value.0);
        let samples = descriptor
            .get::<_, Option<JsU32>>("sampleCount")?
            .map_or(1, |value| value.0);
        let dimension = format::dimension(descriptor.get("dimension")?);
        let format_name: String = descriptor.get("format")?;
        let tex_format = format::texture_format(&format_name, ctx)?;
        if !device.features().contains(tex_format.required_features()) {
            return Err(type_error(
                ctx,
                format!("texture format {format_name} requires missing features"),
            ));
        }
        let usage_bits = descriptor.get::<_, JsU32>("usage")?.0;
        let extra_usage = usage_bits & !TEXTURE_USAGE_MASK != 0;
        let usage = wgpu::TextureUsages::from_bits_truncate(usage_bits & TEXTURE_USAGE_MASK);
        let view_formats = descriptor
            .get::<_, Option<Vec<String>>>("viewFormats")?
            .unwrap_or_default();
        let view_formats = view_formats
            .iter()
            .map(|name| {
                let format = format::texture_format(name, ctx)?;
                if !device.features().contains(format.required_features()) {
                    return Err(type_error(
                        ctx,
                        format!("view format {name} requires missing features"),
                    ));
                }
                Ok(format)
            })
            .collect::<Result<Vec<_>>>()?;
        let device_features = device.features();
        let limits = device.limits();
        let format_features = tex_format.guaranteed_format_features(device_features);
        let mut invalid = extra_usage || usage.is_empty();
        if extra_usage {
            errors.validation("usage contains unknown GPUTextureUsage flags");
        }
        if usage.is_empty() && !extra_usage {
            errors.validation("usage must not be zero");
        }
        if usage.contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT)
            && usage
                != (wgpu::TextureUsages::TRANSIENT_ATTACHMENT
                    | wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            errors.validation("TRANSIENT_ATTACHMENT must be combined only with RENDER_ATTACHMENT");
            invalid = true;
        }
        let zero_size = size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0;
        if zero_size {
            errors.validation("texture size must be non-zero");
            invalid = true;
        }
        if !dimension_format_compatible(dimension, tex_format, device_features) {
            errors.validation("texture format is incompatible with dimension");
            invalid = true;
        }
        match dimension {
            wgpu::TextureDimension::D1 => {
                if size.width > limits.max_texture_dimension_1d
                    || size.height != 1
                    || size.depth_or_array_layers != 1
                {
                    errors.validation("1d texture size is invalid");
                    invalid = true;
                }
            }
            wgpu::TextureDimension::D2 => {
                if size.width > limits.max_texture_dimension_2d
                    || size.height > limits.max_texture_dimension_2d
                    || size.depth_or_array_layers > limits.max_texture_array_layers
                {
                    errors.validation("2d texture size exceeds device limits");
                    invalid = true;
                }
            }
            wgpu::TextureDimension::D3 => {
                if size.width > limits.max_texture_dimension_3d
                    || size.height > limits.max_texture_dimension_3d
                    || size.depth_or_array_layers > limits.max_texture_dimension_3d
                {
                    errors.validation("3d texture size exceeds device limits");
                    invalid = true;
                }
            }
        }
        let (block_width, block_height) = tex_format.block_dimensions();
        if tex_format.is_compressed()
            && (!size.width.is_multiple_of(block_width)
                || (dimension != wgpu::TextureDimension::D1
                    && !size.height.is_multiple_of(block_height)))
        {
            errors.validation("compressed texture size must be a multiple of the block size");
            invalid = true;
        }
        if mip_levels == 0 {
            errors.validation("mipLevelCount must be greater than zero");
            invalid = true;
        }
        if mip_levels > size.max_mips(dimension) {
            errors.validation("mipLevelCount is too large");
            invalid = true;
        }
        if usage.contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT)
            && (mip_levels != 1 || size.depth_or_array_layers != 1)
        {
            errors.validation("transient attachments require mipLevelCount and array layers of 1");
            invalid = true;
        }
        if usage.contains(wgpu::TextureUsages::STORAGE_BINDING)
            && !format_features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY)
            && !format_features
                .allowed_usages
                .contains(wgpu::TextureUsages::STORAGE_BINDING)
        {
            errors.validation("format does not support STORAGE_BINDING");
            invalid = true;
        }
        if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            if dimension == wgpu::TextureDimension::D1 {
                errors.validation("1d textures cannot be render attachments");
                invalid = true;
            }
            if !format_features
                .allowed_usages
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
            {
                errors.validation("format does not support RENDER_ATTACHMENT");
                invalid = true;
            }
        }
        let msaa_ok = samples == 4
            && dimension == wgpu::TextureDimension::D2
            && mip_levels == 1
            && size.depth_or_array_layers == 1
            && usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
            && !usage.contains(wgpu::TextureUsages::STORAGE_BINDING)
            && format_features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4);
        if samples != 1 && !msaa_ok {
            errors.validation("sampleCount is invalid for this texture descriptor");
            invalid = true;
        }
        if view_formats.iter().any(|view| {
            *view != tex_format && view.remove_srgb_suffix() != tex_format.remove_srgb_suffix()
        }) {
            errors.validation("viewFormats contains an incompatible format");
            invalid = true;
        }
        let extent_for_bytes = if zero_size {
            dummy_extent(tex_format, dimension)
        } else {
            size
        };
        let too_large = !invalid
            && texture_byte_size(
                extent_for_bytes,
                dimension,
                tex_format,
                mip_levels.max(1),
                samples.max(1),
            ) > MAX_HOST_ALLOCATION;
        if too_large {
            errors.out_of_memory("texture allocation exceeds the host budget");
        }
        // Invalid and over-budget descriptors become a 1x1 dummy. Valid MSAA
        // (sampleCount 4, 2d, mip 1, array 1, RENDER, no STORAGE) is created
        // for real so bind-group sample-count checks match the JS getter.
        let use_dummy = invalid || too_large;
        let (inner, is_dummy) = if use_dummy {
            (fallbacks.sampled.clone(), true)
        } else {
            let mut native_view_formats = view_formats;
            // wgpu rejects a non-empty viewFormats list on
            // TRANSIENT_ATTACHMENT.
            if !usage.contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT)
                && !native_view_formats.contains(&tex_format)
            {
                native_view_formats.push(tex_format);
            }
            crate::catch_gpu(errors, || {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: (!label.is_empty()).then_some(label.as_str()),
                    size,
                    mip_level_count: mip_levels,
                    sample_count: if msaa_ok { 4 } else { 1 },
                    dimension,
                    format: tex_format,
                    usage,
                    view_formats: &native_view_formats,
                })
            })
            .map_or_else(|| (fallbacks.sampled.clone(), true), |inner| (inner, false))
        };
        Ok(Self {
            destroyed: Rc::new(Cell::new(false)),
            errors: errors.clone(),
            fallbacks: fallbacks.clone(),
            inner,
            is_dummy,
            label: Rc::new(RefCell::new(label)),
            format: tex_format,
            size,
            dimension,
            mip_levels,
            samples,
            usage,
            device_id,
            features: device.features(),
        })
    }

    pub(crate) fn binding_view(&self) -> wgpu::TextureView {
        if self.is_dummy {
            self.fallbacks
                .view(1, wgpu::TextureUsages::TEXTURE_BINDING, self.format)
        } else {
            self.inner
                .create_view(&wgpu::TextureViewDescriptor::default())
        }
    }

    pub(crate) fn default_view(&self) -> wgpu::TextureView {
        if self.is_dummy {
            self.fallbacks
                .view(self.samples.max(1), self.usage, self.format)
        } else {
            self.inner
                .create_view(&wgpu::TextureViewDescriptor::default())
        }
    }
}

pub fn create_sampler<'js>(
    device: &wgpu::Device, descriptor: Opt<Option<Object<'js>>>, ctx: &Ctx<'js>, errors: &ErrorSink,
) -> Result<GPUSampler> {
    let descriptor = descriptor.0.flatten();
    let label = descriptor
        .as_ref()
        .map(crate::label)
        .transpose()?
        .unwrap_or_default();
    let address = |name: &str| -> Result<wgpu::AddressMode> {
        match descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>(name))
            .transpose()?
            .flatten()
            .as_deref()
        {
            None | Some("clamp-to-edge") => Ok(wgpu::AddressMode::ClampToEdge),
            Some("repeat") => Ok(wgpu::AddressMode::Repeat),
            Some("mirror-repeat") => Ok(wgpu::AddressMode::MirrorRepeat),
            Some(value) => Err(type_error(ctx, format!("invalid GPUAddressMode {value}"))),
        }
    };
    let filter = |name: &str, default: wgpu::FilterMode| -> Result<wgpu::FilterMode> {
        match descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>(name))
            .transpose()?
            .flatten()
            .as_deref()
        {
            None => Ok(default),
            Some("nearest") => Ok(wgpu::FilterMode::Nearest),
            Some("linear") => Ok(wgpu::FilterMode::Linear),
            Some(value) => Err(type_error(ctx, format!("invalid GPUFilterMode {value}"))),
        }
    };
    let lod_min_clamp = descriptor
        .as_ref()
        .map(|object| object.get::<_, Option<f64>>("lodMinClamp"))
        .transpose()?
        .flatten()
        .unwrap_or(0.0) as f32;
    let lod_max_clamp = descriptor
        .as_ref()
        .map(|object| object.get::<_, Option<f64>>("lodMaxClamp"))
        .transpose()?
        .flatten()
        .unwrap_or(32.0) as f32;
    let mag_filter = filter("magFilter", wgpu::FilterMode::Nearest)?;
    let min_filter = filter("minFilter", wgpu::FilterMode::Nearest)?;
    let mipmap_filter = match descriptor
        .as_ref()
        .map(|object| object.get::<_, Option<String>>("mipmapFilter"))
        .transpose()?
        .flatten()
        .as_deref()
    {
        None | Some("nearest") => wgpu::MipmapFilterMode::Nearest,
        Some("linear") => wgpu::MipmapFilterMode::Linear,
        Some(value) => {
            return Err(type_error(
                ctx,
                format!("invalid GPUMipmapFilterMode {value}"),
            ));
        }
    };
    let compare = match descriptor
        .as_ref()
        .map(|object| object.get::<_, Option<String>>("compare"))
        .transpose()?
        .flatten()
        .as_deref()
    {
        None => None,
        Some("never") => Some(wgpu::CompareFunction::Never),
        Some("less") => Some(wgpu::CompareFunction::Less),
        Some("equal") => Some(wgpu::CompareFunction::Equal),
        Some("less-equal") => Some(wgpu::CompareFunction::LessEqual),
        Some("greater") => Some(wgpu::CompareFunction::Greater),
        Some("not-equal") => Some(wgpu::CompareFunction::NotEqual),
        Some("greater-equal") => Some(wgpu::CompareFunction::GreaterEqual),
        Some("always") => Some(wgpu::CompareFunction::Always),
        Some(value) => {
            return Err(type_error(
                ctx,
                format!("invalid GPUCompareFunction {value}"),
            ));
        }
    };
    let anisotropy_requested = descriptor
        .as_ref()
        .map(|object| object.get::<_, Option<f64>>("maxAnisotropy"))
        .transpose()?
        .flatten()
        .unwrap_or(1.0);
    let filters_linear = mag_filter == wgpu::FilterMode::Linear
        && min_filter == wgpu::FilterMode::Linear
        && mipmap_filter == wgpu::MipmapFilterMode::Linear;
    let invalid = {
        let lod_min_invalid = lod_min_clamp < 0.0;
        if lod_min_invalid {
            errors.validation("lodMinClamp must be >= 0");
        }
        let lod_range_invalid = lod_max_clamp < lod_min_clamp;
        if lod_range_invalid {
            errors.validation("lodMaxClamp must be >= lodMinClamp");
        }
        let anisotropy_range_invalid = anisotropy_requested < 1.0;
        if anisotropy_range_invalid {
            errors.validation("maxAnisotropy must be >= 1");
        }
        let anisotropy_filter_invalid = anisotropy_requested > 1.0 && !filters_linear;
        if anisotropy_filter_invalid {
            errors.validation("maxAnisotropy > 1 requires linear mag, min, and mipmap filters");
        }
        lod_min_invalid
            || lod_range_invalid
            || anisotropy_range_invalid
            || anisotropy_filter_invalid
    };
    let anisotropy_clamp = if anisotropy_requested < 1.0 {
        1
    } else {
        u16::try_from(anisotropy_requested as u32)
            .unwrap_or(u16::MAX)
            .max(1)
    };
    let sampler_descriptor = if invalid {
        wgpu::SamplerDescriptor::default()
    } else {
        wgpu::SamplerDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            address_mode_u: address("addressModeU")?,
            address_mode_v: address("addressModeV")?,
            address_mode_w: address("addressModeW")?,
            mag_filter,
            min_filter,
            mipmap_filter,
            lod_min_clamp,
            lod_max_clamp,
            compare,
            anisotropy_clamp,
            border_color: None,
        }
    };
    let inner = crate::catch_gpu(errors, || device.create_sampler(&sampler_descriptor))
        .unwrap_or_else(|| device.create_sampler(&wgpu::SamplerDescriptor::default()));
    Ok(GPUSampler {
        inner,
        invalid,
        label: Rc::new(RefCell::new(label)),
    })
}

pub fn texel_copy_texture<'js>(
    object: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(
    Class<'js, GPUTexture>,
    u32,
    wgpu::Origin3d,
    wgpu::TextureAspect,
)> {
    let texture = crate::class_value::<GPUTexture>(&object, "texture", ctx)?;
    let mip_level = object
        .get::<_, Option<JsU32>>("mipLevel")?
        .map_or(0, |value| value.0);
    let origin = format::origin3d(object.get("origin")?, ctx)?;
    let aspect = format::aspect(object.get("aspect")?);
    Ok((texture, mip_level, origin, aspect))
}

pub fn texel_copy_layout(object: &Object<'_>) -> Result<wgpu::TexelCopyBufferLayout> {
    Ok(wgpu::TexelCopyBufferLayout {
        offset:         object
            .get::<_, Option<JsU64>>("offset")?
            .map_or(0, |value| value.0),
        bytes_per_row:  object
            .get::<_, Option<JsU32>>("bytesPerRow")?
            .map(|value| value.0),
        rows_per_image: object
            .get::<_, Option<JsU32>>("rowsPerImage")?
            .map(|value| value.0),
    })
}

pub fn texel_copy_buffer<'js>(
    object: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(Class<'js, crate::GPUBuffer>, wgpu::TexelCopyBufferLayout)> {
    let buffer = crate::class_value::<crate::GPUBuffer>(&object, "buffer", ctx)?;
    let layout = texel_copy_layout(&object)?;
    Ok((buffer, layout))
}
