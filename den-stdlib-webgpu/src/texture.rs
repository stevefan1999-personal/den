use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
};

use rquickjs::{Class, Coerced, Ctx, JsLifetime, Object, Result, class::Trace, function::Opt};

use crate::{GPUDevice, JsU32, JsU64, format, illegal_constructor, label, type_error};

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUTexture")]
pub struct GPUTexture<'js> {
    device:           Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::Texture,
    /// Attachments and bindings accept a bare `GPUTexture`; one shared view
    /// keeps wgpu's per-view state consistent across those uses.
    #[qjs(skip_trace)]
    default_view:     OnceCell<wgpu::TextureView>,
    #[qjs(skip_trace)]
    label:            RefCell<String>,
}

impl<'js> GPUTexture<'js> {
    pub(crate) fn default_view(&self) -> wgpu::TextureView {
        self.default_view
            .get_or_init(|| {
                self.inner
                    .create_view(&wgpu::TextureViewDescriptor::default())
            })
            .clone()
    }

    pub(crate) fn from_descriptor(
        device: &Class<'js, GPUDevice<'js>>, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<Self> {
        let wgpu_device = device.borrow().device.clone();
        let features = wgpu_device.features();
        let label = label(&descriptor)?;
        let size = format::extent3d(descriptor.get("size")?, ctx)?;
        let mip_level_count = descriptor
            .get::<_, Option<JsU32>>("mipLevelCount")?
            .map_or(1, |value| value.0);
        let sample_count = descriptor
            .get::<_, Option<JsU32>>("sampleCount")?
            .map_or(1, |value| value.0);
        let dimension = format::dimension(descriptor.get("dimension")?);
        let format = format::texture_format_with_features(
            &descriptor.get::<_, String>("format")?,
            features,
            ctx,
        )?;
        // Unknown usage bits are a device-timeline error; wgpu rejects the
        // empty set they collapse to.
        let usage = wgpu::TextureUsages::from_bits(descriptor.get::<_, JsU32>("usage")?.0)
            .unwrap_or_else(wgpu::TextureUsages::empty);
        let view_formats = descriptor
            .get::<_, Option<Vec<String>>>("viewFormats")?
            .unwrap_or_default()
            .iter()
            .map(|name| format::texture_format_with_features(name, features, ctx))
            .collect::<Result<Vec<_>>>()?;
        let inner = wgpu_device.create_texture(&wgpu::TextureDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            size,
            mip_level_count,
            sample_count,
            dimension,
            format,
            usage,
            view_formats: &view_formats,
        });
        Ok(Self {
            device: device.clone(),
            inner,
            default_view: OnceCell::new(),
            label: RefCell::new(label),
        })
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUTexture<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable)]
    pub fn width(&self) -> u32 { self.inner.width() }

    #[qjs(get, configurable)]
    pub fn height(&self) -> u32 { self.inner.height() }

    #[qjs(get, configurable)]
    pub fn depth_or_array_layers(&self) -> u32 { self.inner.depth_or_array_layers() }

    #[qjs(get, configurable)]
    pub fn mip_level_count(&self) -> u32 { self.inner.mip_level_count() }

    #[qjs(get, configurable)]
    pub fn sample_count(&self) -> u32 { self.inner.sample_count() }

    #[qjs(get, configurable)]
    pub fn dimension(&self) -> &'static str {
        match self.inner.dimension() {
            wgpu::TextureDimension::D1 => "1d",
            wgpu::TextureDimension::D3 => "3d",
            wgpu::TextureDimension::D2 => "2d",
        }
    }

    #[qjs(get, configurable)]
    pub fn format(&self) -> String {
        serde_json::to_string(&self.inner.format())
            .unwrap_or_else(|_error| "\"unknown\"".into())
            .trim_matches('"')
            .to_owned()
    }

    #[qjs(get, configurable)]
    pub fn usage(&self) -> u32 { self.inner.usage().bits() }

    pub fn create_view(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUTextureView> {
        let descriptor = descriptor.0.flatten();
        let get = |key: &str| -> Result<Option<String>> {
            Ok(descriptor
                .as_ref()
                .map(|object| object.get::<_, Option<Coerced<String>>>(key))
                .transpose()?
                .flatten()
                .map(|value| value.0))
        };
        let get_u32 = |key: &str| -> Result<Option<u32>> {
            Ok(descriptor
                .as_ref()
                .map(|object| object.get::<_, Option<JsU32>>(key))
                .transpose()?
                .flatten()
                .map(|value| value.0))
        };
        let label = descriptor
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let features = self.device.borrow().device.features();
        let format = get("format")?
            .map(|name| format::texture_format_with_features(&name, features, &ctx))
            .transpose()?;
        let dimension = format::view_dimension(get("dimension")?.as_deref(), &ctx)?;
        let swizzle = get("swizzle")?
            .map(|swizzle| format::component_swizzle(&swizzle, features, &ctx))
            .transpose()?
            .unwrap_or_default();
        // Zero means "inherit"; unknown bits collapse to the empty set, which
        // wgpu rejects on the device timeline.
        let usage = get_u32("usage")?.filter(|bits| *bits != 0).map(|bits| {
            wgpu::TextureUsages::from_bits(bits).unwrap_or_else(wgpu::TextureUsages::empty)
        });
        let inner = self.inner.create_view(&wgpu::TextureViewDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            format,
            dimension,
            usage,
            aspect: format::aspect(get("aspect")?),
            base_mip_level: get_u32("baseMipLevel")?.unwrap_or(0),
            mip_level_count: get_u32("mipLevelCount")?,
            base_array_layer: get_u32("baseArrayLayer")?.unwrap_or(0),
            array_layer_count: get_u32("arrayLayerCount")?,
            swizzle,
        });
        crate::flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUTextureView {
            inner,
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

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
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

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
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
    let get = |key: &str| -> Result<Option<String>> {
        Ok(descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<String>>(key))
            .transpose()?
            .flatten())
    };
    let get_f64 = |key: &str| -> Result<Option<f64>> {
        Ok(descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<f64>>(key))
            .transpose()?
            .flatten())
    };
    let address = |key: &str| -> Result<wgpu::AddressMode> {
        match get(key)?.as_deref() {
            None | Some("clamp-to-edge") => Ok(wgpu::AddressMode::ClampToEdge),
            Some("repeat") => Ok(wgpu::AddressMode::Repeat),
            Some("mirror-repeat") => Ok(wgpu::AddressMode::MirrorRepeat),
            Some(value) => Err(type_error(ctx, format!("invalid GPUAddressMode {value}"))),
        }
    };
    let filter = |key: &str| -> Result<wgpu::FilterMode> {
        match get(key)?.as_deref() {
            None | Some("nearest") => Ok(wgpu::FilterMode::Nearest),
            Some("linear") => Ok(wgpu::FilterMode::Linear),
            Some(value) => Err(type_error(ctx, format!("invalid GPUFilterMode {value}"))),
        }
    };
    let mipmap_filter = match get("mipmapFilter")?.as_deref() {
        None | Some("nearest") => wgpu::MipmapFilterMode::Nearest,
        Some("linear") => wgpu::MipmapFilterMode::Linear,
        Some(value) => {
            return Err(type_error(
                ctx,
                format!("invalid GPUMipmapFilterMode {value}"),
            ));
        }
    };
    let compare = get("compare")?
        .map(|name| format::compare_function(&name, ctx))
        .transpose()?;
    // WebIDL `[Clamp] unsigned short`.
    let anisotropy_clamp =
        get_f64("maxAnisotropy")?.map_or(1, |value| value.clamp(0.0, f64::from(u16::MAX)) as u16);
    let inner = device.create_sampler(&wgpu::SamplerDescriptor {
        label: (!label.is_empty()).then_some(label.as_str()),
        address_mode_u: address("addressModeU")?,
        address_mode_v: address("addressModeV")?,
        address_mode_w: address("addressModeW")?,
        mag_filter: filter("magFilter")?,
        min_filter: filter("minFilter")?,
        mipmap_filter,
        lod_min_clamp: get_f64("lodMinClamp")?.unwrap_or(0.0) as f32,
        lod_max_clamp: get_f64("lodMaxClamp")?.unwrap_or(32.0) as f32,
        compare,
        anisotropy_clamp,
        border_color: None,
    });
    Ok(GPUSampler {
        inner,
        label: Rc::new(RefCell::new(label)),
    })
}

pub fn texel_copy_texture<'js>(
    object: Object<'js>, ctx: &Ctx<'js>,
) -> Result<(
    Class<'js, GPUTexture<'js>>,
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
