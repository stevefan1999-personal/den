use rquickjs::{Array, Coerced, Ctx, FromJs as _, Object, Result, Value};

use crate::{JsU32, type_error};

macro_rules! kebab {
    ($ty:ty, $name:expr, $ctx:expr, $kind:expr) => {
        serde_json::from_str::<$ty>(&format!("\"{}\"", $name))
            .map_err(|_error| type_error($ctx, format!("unknown {} {}", $kind, $name)))
    };
}

pub fn texture_format(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::TextureFormat> {
    kebab!(wgpu::TextureFormat, name, ctx, "GPUTextureFormat")
}

/// The spec validates "texture format required features" on the content
/// timeline, so a feature-gated format is a `TypeError` rather than a
/// validation error.
pub fn texture_format_with_features(
    name: &str, features: wgpu::Features, ctx: &Ctx<'_>,
) -> Result<wgpu::TextureFormat> {
    let format = texture_format(name, ctx)?;
    if !features.contains(format.required_features()) {
        return Err(type_error(
            ctx,
            format!("texture format {name} requires missing features"),
        ));
    }
    Ok(format)
}

pub fn vertex_format(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::VertexFormat> {
    kebab!(wgpu::VertexFormat, name, ctx, "GPUVertexFormat")
}

/// `GPUTextureComponentSwizzle` is four of `rgba01`; the syntax is a
/// content-timeline `TypeError`, and so is the missing feature.
pub fn component_swizzle(
    swizzle: &str, features: wgpu::Features, ctx: &Ctx<'_>,
) -> Result<wgpu::TextureComponentSwizzle> {
    let component = |letter: Option<char>| {
        match letter {
            Some('0') => Ok(wgpu::ComponentSwizzle::Zero),
            Some('1') => Ok(wgpu::ComponentSwizzle::One),
            Some('r') => Ok(wgpu::ComponentSwizzle::R),
            Some('g') => Ok(wgpu::ComponentSwizzle::G),
            Some('b') => Ok(wgpu::ComponentSwizzle::B),
            Some('a') => Ok(wgpu::ComponentSwizzle::A),
            _ => {
                Err(type_error(
                    ctx,
                    "swizzle must be exactly four characters from rgba01",
                ))
            }
        }
    };
    if swizzle.chars().count() != 4 {
        return Err(type_error(
            ctx,
            "swizzle must be exactly four characters from rgba01",
        ));
    }
    let mut letters = swizzle.chars();
    let parsed = wgpu::TextureComponentSwizzle {
        r: component(letters.next())?,
        g: component(letters.next())?,
        b: component(letters.next())?,
        a: component(letters.next())?,
    };
    if parsed != wgpu::TextureComponentSwizzle::default()
        && !features.contains(wgpu::Features::TEXTURE_COMPONENT_SWIZZLE)
    {
        return Err(type_error(
            ctx,
            "texture-component-swizzle feature is not enabled",
        ));
    }
    Ok(parsed)
}

pub fn compare_function(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::CompareFunction> {
    kebab!(wgpu::CompareFunction, name, ctx, "GPUCompareFunction")
}

pub fn blend_factor(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::BlendFactor> {
    kebab!(wgpu::BlendFactor, name, ctx, "GPUBlendFactor")
}

pub fn blend_operation(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::BlendOperation> {
    kebab!(wgpu::BlendOperation, name, ctx, "GPUBlendOperation")
}

pub fn primitive_topology(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::PrimitiveTopology> {
    kebab!(wgpu::PrimitiveTopology, name, ctx, "GPUPrimitiveTopology")
}

pub fn index_format(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::IndexFormat> {
    kebab!(wgpu::IndexFormat, name, ctx, "GPUIndexFormat")
}

pub fn stencil_operation(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::StencilOperation> {
    kebab!(wgpu::StencilOperation, name, ctx, "GPUStencilOperation")
}

pub fn sampler_binding_type(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::SamplerBindingType> {
    kebab!(wgpu::SamplerBindingType, name, ctx, "GPUSamplerBindingType")
}

pub fn extent3d<'js>(value: Value<'js>, ctx: &Ctx<'js>) -> Result<wgpu::Extent3d> {
    if value.as_number().is_some() {
        return Ok(wgpu::Extent3d {
            width:                 JsU32::from_js(ctx, value)?.0,
            height:                1,
            depth_or_array_layers: 1,
        });
    }
    if let Ok(array) = Array::from_js(ctx, value.clone()) {
        let width = array.get::<JsU32>(0)?.0;
        let height = array.get::<Option<JsU32>>(1)?.map_or(1, |value| value.0);
        let depth = array.get::<Option<JsU32>>(2)?.map_or(1, |value| value.0);
        return Ok(wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: depth,
        });
    }
    let object = Object::from_js(ctx, value)
        .map_err(|_error| type_error(ctx, "GPUExtent3D must be a number, array, or dictionary"))?;
    Ok(wgpu::Extent3d {
        width:                 object.get::<_, JsU32>("width")?.0,
        height:                object
            .get::<_, Option<JsU32>>("height")?
            .map_or(1, |value| value.0),
        depth_or_array_layers: object
            .get::<_, Option<JsU32>>("depthOrArrayLayers")?
            .map_or(1, |value| value.0),
    })
}

pub fn origin3d<'js>(value: Option<Value<'js>>, ctx: &Ctx<'js>) -> Result<wgpu::Origin3d> {
    let Some(value) = value else {
        return Ok(wgpu::Origin3d::ZERO);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(wgpu::Origin3d::ZERO);
    }
    if let Ok(array) = Array::from_js(ctx, value.clone()) {
        return Ok(wgpu::Origin3d {
            x: array.get::<Option<JsU32>>(0)?.map_or(0, |value| value.0),
            y: array.get::<Option<JsU32>>(1)?.map_or(0, |value| value.0),
            z: array.get::<Option<JsU32>>(2)?.map_or(0, |value| value.0),
        });
    }
    let object = Object::from_js(ctx, value)
        .map_err(|_error| type_error(ctx, "GPUOrigin3D must be an array or dictionary"))?;
    Ok(wgpu::Origin3d {
        x: object
            .get::<_, Option<JsU32>>("x")?
            .map_or(0, |value| value.0),
        y: object
            .get::<_, Option<JsU32>>("y")?
            .map_or(0, |value| value.0),
        z: object
            .get::<_, Option<JsU32>>("z")?
            .map_or(0, |value| value.0),
    })
}

pub fn color<'js>(value: Option<Value<'js>>, ctx: &Ctx<'js>) -> Result<wgpu::Color> {
    let Some(value) = value else {
        return Ok(wgpu::Color::TRANSPARENT);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(wgpu::Color::TRANSPARENT);
    }
    if let Ok(array) = Array::from_js(ctx, value.clone()) {
        return Ok(wgpu::Color {
            r: array.get::<Option<f64>>(0)?.unwrap_or(0.0),
            g: array.get::<Option<f64>>(1)?.unwrap_or(0.0),
            b: array.get::<Option<f64>>(2)?.unwrap_or(0.0),
            a: array.get::<Option<f64>>(3)?.unwrap_or(1.0),
        });
    }
    let object = Object::from_js(ctx, value)
        .map_err(|_error| type_error(ctx, "GPUColor must be an array or dictionary"))?;
    Ok(wgpu::Color {
        r: object.get::<_, Option<f64>>("r")?.unwrap_or(0.0),
        g: object.get::<_, Option<f64>>("g")?.unwrap_or(0.0),
        b: object.get::<_, Option<f64>>("b")?.unwrap_or(0.0),
        a: object.get::<_, Option<f64>>("a")?.unwrap_or(1.0),
    })
}

pub fn dimension(value: Option<String>) -> wgpu::TextureDimension {
    match value.as_deref() {
        Some("1d") => wgpu::TextureDimension::D1,
        Some("3d") => wgpu::TextureDimension::D3,
        _ => wgpu::TextureDimension::D2,
    }
}

pub fn view_dimension(
    value: Option<&str>, ctx: &Ctx<'_>,
) -> Result<Option<wgpu::TextureViewDimension>> {
    match value {
        Some("1d") => Ok(Some(wgpu::TextureViewDimension::D1)),
        Some("2d") => Ok(Some(wgpu::TextureViewDimension::D2)),
        Some("2d-array") => Ok(Some(wgpu::TextureViewDimension::D2Array)),
        Some("cube") => Ok(Some(wgpu::TextureViewDimension::Cube)),
        Some("cube-array") => Ok(Some(wgpu::TextureViewDimension::CubeArray)),
        Some("3d") => Ok(Some(wgpu::TextureViewDimension::D3)),
        Some(name) => {
            Err(type_error(
                ctx,
                format!("invalid GPUTextureViewDimension {name}"),
            ))
        }
        None => Ok(None),
    }
}

pub fn view_dimension_or(
    value: Option<&str>, default: wgpu::TextureViewDimension, ctx: &Ctx<'_>,
) -> Result<wgpu::TextureViewDimension> {
    Ok(view_dimension(value, ctx)?.unwrap_or(default))
}

pub fn aspect(value: Option<String>) -> wgpu::TextureAspect {
    match value.as_deref() {
        Some("stencil-only") => wgpu::TextureAspect::StencilOnly,
        Some("depth-only") => wgpu::TextureAspect::DepthOnly,
        _ => wgpu::TextureAspect::All,
    }
}

pub fn sample_type(name: Option<&str>, ctx: &Ctx<'_>) -> Result<wgpu::TextureSampleType> {
    match name.unwrap_or("float") {
        "float" => Ok(wgpu::TextureSampleType::Float { filterable: true }),
        "unfilterable-float" => Ok(wgpu::TextureSampleType::Float { filterable: false }),
        "depth" => Ok(wgpu::TextureSampleType::Depth),
        "sint" => Ok(wgpu::TextureSampleType::Sint),
        "uint" => Ok(wgpu::TextureSampleType::Uint),
        value => {
            Err(type_error(
                ctx,
                format!("invalid GPUTextureSampleType {value}"),
            ))
        }
    }
}

pub fn storage_access(name: Option<&str>, ctx: &Ctx<'_>) -> Result<wgpu::StorageTextureAccess> {
    match name.unwrap_or("write-only") {
        "write-only" => Ok(wgpu::StorageTextureAccess::WriteOnly),
        "read-only" => Ok(wgpu::StorageTextureAccess::ReadOnly),
        "read-write" => Ok(wgpu::StorageTextureAccess::ReadWrite),
        value => {
            Err(type_error(
                ctx,
                format!("invalid GPUStorageTextureAccess {value}"),
            ))
        }
    }
}

pub fn store_op(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::StoreOp> {
    match name {
        "store" => Ok(wgpu::StoreOp::Store),
        "discard" => Ok(wgpu::StoreOp::Discard),
        value => Err(type_error(ctx, format!("invalid GPUStoreOp {value}"))),
    }
}

pub fn pipeline_constants<'js>(
    object: Option<Object<'js>>, ctx: &Ctx<'js>,
) -> Result<Vec<(String, f64)>> {
    let Some(object) = object else {
        return Ok(Vec::new());
    };
    let mut constants = Vec::new();
    // Atom→StdString truncates at NUL (`JS_AtomToCStringLen` + `CStr`). JS
    // String→UTF-8 keeps embedded NULs so `'c0\0'` stays distinct from `c0`.
    for property in object.props::<rquickjs::String<'_>, Value<'_>>() {
        let (js_name, value) = property?;
        let name = js_name.to_string()?;
        let number = if let Some(number) = value.as_number() {
            number
        } else {
            match Coerced::<f64>::from_js(ctx, value) {
                Ok(Coerced(number)) => number,
                Err(_error) => {
                    return Err(type_error(ctx, "pipeline constant must be a number"));
                }
            }
        };
        if !number.is_finite() {
            return Err(type_error(ctx, "pipeline constant must be a finite number"));
        }
        constants.push((name, number));
    }
    Ok(constants)
}

pub fn signed_i32(value: Option<Value<'_>>, ctx: &Ctx<'_>, default: i32) -> Result<i32> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_undefined() || value.is_null() {
        return Ok(default);
    }
    let Some(number) = value.as_number() else {
        return Err(type_error(ctx, "expected a signed integer"));
    };
    if !number.is_finite()
        || number.fract() != 0.0
        || !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&number)
    {
        return Err(rquickjs::Exception::throw_range(
            ctx,
            "signed integer is out of range",
        ));
    }
    Ok(number as i32)
}
