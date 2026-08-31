use std::{cell::RefCell, rc::Rc};

use rquickjs::{Class, Ctx, JsLifetime, Object, Result, class::Trace, function::Opt};

use crate::{JsU32, JsU64, format, illegal_constructor, label, type_error};

const TEXTURE_USAGE_MASK: u32 = 0x001f;

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUTexture")]
pub struct GPUTexture {
    #[qjs(skip_trace)]
    pub(crate) inner:      wgpu::Texture,
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
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUTexture {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get)]
    pub const fn width(&self) -> u32 { self.size.width }

    #[qjs(get)]
    pub const fn height(&self) -> u32 { self.size.height }

    #[qjs(get)]
    pub const fn depth_or_array_layers(&self) -> u32 { self.size.depth_or_array_layers }

    #[qjs(get)]
    pub const fn mip_level_count(&self) -> u32 { self.mip_levels }

    #[qjs(get)]
    pub const fn sample_count(&self) -> u32 { self.samples }

    #[qjs(get)]
    pub fn dimension(&self) -> &'static str {
        match self.dimension {
            wgpu::TextureDimension::D1 => "1d",
            wgpu::TextureDimension::D3 => "3d",
            wgpu::TextureDimension::D2 => "2d",
        }
    }

    #[qjs(get)]
    pub fn format(&self) -> String {
        serde_json::to_string(&self.format)
            .unwrap_or_else(|_error| "\"unknown\"".into())
            .trim_matches('"')
            .to_owned()
    }

    #[qjs(get)]
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
        let format = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>("format"))
            .transpose()?
            .flatten()
            .map(|name| format::texture_format(&name, &ctx))
            .transpose()?
            .unwrap_or(self.format);
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
        let view_dimension = format::view_dimension(dimension.as_deref(), &ctx)?;
        Ok(GPUTextureView {
            inner: self.inner.create_view(&wgpu::TextureViewDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                format: Some(format),
                dimension: view_dimension,
                usage: None,
                aspect: format::aspect(aspect),
                base_mip_level,
                mip_level_count,
                base_array_layer,
                array_layer_count,
            }),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn destroy(&self) { self.inner.destroy(); }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUTextureView")]
pub struct GPUTextureView {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::TextureView,
    #[qjs(skip_trace)]
    pub(crate) label: Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPUTextureView {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUSampler")]
pub struct GPUSampler {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::Sampler,
    #[qjs(skip_trace)]
    pub(crate) label: Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPUSampler {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

impl GPUTexture {
    pub fn from_descriptor<'js>(
        device: &wgpu::Device, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<Self> {
        let label = label(&descriptor)?;
        let size = format::extent3d(descriptor.get("size")?, ctx)?;
        let mip_levels = descriptor
            .get::<_, Option<JsU32>>("mipLevelCount")?
            .map_or(1, |value| value.0.max(1));
        let samples = descriptor
            .get::<_, Option<JsU32>>("sampleCount")?
            .map_or(1, |value| value.0.max(1));
        let dimension = format::dimension(descriptor.get("dimension")?);
        let format_name: String = descriptor.get("format")?;
        let tex_format = format::texture_format(&format_name, ctx)?;
        let usage_bits = descriptor.get::<_, JsU32>("usage")?.0;
        if usage_bits & !TEXTURE_USAGE_MASK != 0 {
            return Err(type_error(
                ctx,
                "usage contains unknown GPUTextureUsage flags",
            ));
        }
        let usage = wgpu::TextureUsages::from_bits(usage_bits)
            .ok_or_else(|| type_error(ctx, "usage is not a valid GPUTextureUsage mask"))?;
        let view_formats = descriptor
            .get::<_, Option<Vec<String>>>("viewFormats")?
            .unwrap_or_default();
        let view_formats = view_formats
            .iter()
            .map(|name| format::texture_format(name, ctx))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            inner: device.create_texture(&wgpu::TextureDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                size,
                mip_level_count: mip_levels,
                sample_count: samples,
                dimension,
                format: tex_format,
                usage,
                view_formats: &view_formats,
            }),
            label: Rc::new(RefCell::new(label)),
            format: tex_format,
            size,
            dimension,
            mip_levels,
            samples,
            usage,
        })
    }
}

pub fn create_sampler<'js>(
    device: &wgpu::Device, descriptor: Opt<Option<Object<'js>>>, ctx: &Ctx<'js>,
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
    Ok(GPUSampler {
        inner: device.create_sampler(&wgpu::SamplerDescriptor {
            label:            (!label.is_empty()).then_some(label.as_str()),
            address_mode_u:   address("addressModeU")?,
            address_mode_v:   address("addressModeV")?,
            address_mode_w:   address("addressModeW")?,
            mag_filter:       filter("magFilter", wgpu::FilterMode::Nearest)?,
            min_filter:       filter("minFilter", wgpu::FilterMode::Nearest)?,
            mipmap_filter:    match descriptor
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
            },
            lod_min_clamp:    descriptor
                .as_ref()
                .map(|object| object.get::<_, Option<f64>>("lodMinClamp"))
                .transpose()?
                .flatten()
                .unwrap_or(0.0) as f32,
            lod_max_clamp:    descriptor
                .as_ref()
                .map(|object| object.get::<_, Option<f64>>("lodMaxClamp"))
                .transpose()?
                .flatten()
                .unwrap_or(32.0) as f32,
            compare:          match descriptor
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
            },
            anisotropy_clamp: descriptor
                .as_ref()
                .map(|object| object.get::<_, Option<JsU32>>("maxAnisotropy"))
                .transpose()?
                .flatten()
                .map_or(1, |value| u16::try_from(value.0.max(1)).unwrap_or(1)),
            border_color:     None,
        }),
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
