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

pub fn vertex_format(name: &str, ctx: &Ctx<'_>) -> Result<wgpu::VertexFormat> {
    // wgpu 30 has Unorm10_10_10_2 but not Snorm10_10_10_2. Same packed 4-byte
    // layout; CTS validation only cares about size/alignment/scalar class.
    if name == "snorm10-10-10-2" {
        return Ok(wgpu::VertexFormat::Unorm10_10_10_2);
    }
    kebab!(wgpu::VertexFormat, name, ctx, "GPUVertexFormat")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderStage {
    Compute,
    Vertex,
    Fragment,
}

impl ShaderStage {
    const fn attr(self) -> &'static str {
        match self {
            Self::Compute => "@compute",
            Self::Vertex => "@vertex",
            Self::Fragment => "@fragment",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideType {
    Bool,
    U32,
    I32,
    F32,
    F16,
}

#[derive(Clone, Debug)]
pub struct Override {
    pub key:         String,
    pub name:        String,
    pub has_default: bool,
    pub default:     Option<f64>,
    pub ty:          OverrideType,
}

pub fn uses_external_texture(code: &str) -> bool { code.contains("texture_external") }

pub fn writes_frag_depth(code: &str, entry_point: Option<&str>) -> bool {
    fragment_return_has_builtin(code, entry_point, "@builtin(frag_depth)")
}

pub fn writes_sample_mask(code: &str, entry_point: Option<&str>) -> bool {
    fragment_return_has_builtin(code, entry_point, "@builtin(sample_mask)")
}

fn fragment_return_has_builtin(code: &str, entry_point: Option<&str>, builtin: &str) -> bool {
    let Some((_params, ret)) = stage_entry_sig(code, ShaderStage::Fragment, entry_point) else {
        return false;
    };
    type_body(code, ret).contains(builtin)
}

pub fn writes_blend_src(code: &str) -> bool { code.contains("@blend_src") }

fn clip_distance_slots(code: &str) -> u32 {
    let Some((_, after)) = code.split_once("@builtin(clip_distances)") else {
        return 0;
    };
    let Some((_, after_array)) = after.split_once("array<") else {
        return 0;
    };
    let Some((inner, _)) = after_array.split_once('>') else {
        return 0;
    };
    let count = inner
        .split(',')
        .nth(1)
        .and_then(|token| token.trim().parse::<u32>().ok())
        .unwrap_or(0);
    count.div_ceil(4)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterpTy {
    Perspective,
    Linear,
    Flat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterpSampling {
    Center,
    Centroid,
    Sample,
    First,
    Either,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterpSource {
    Implicit,
    TypeOnly,
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interp {
    ty:       InterpTy,
    sampling: InterpSampling,
    source:   InterpSource,
}

impl Default for Interp {
    fn default() -> Self {
        Self {
            ty:       InterpTy::Perspective,
            sampling: InterpSampling::Center,
            source:   InterpSource::Implicit,
        }
    }
}

fn interp_semantic_eq(left: Interp, right: Interp) -> bool {
    left.ty == right.ty && left.sampling == right.sampling
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocationVar {
    location: u32,
    ty:       String,
    interp:   Interp,
}

pub fn vertex_inputs_invalid(code: &str, attributes: &[(u32, wgpu::VertexFormat)]) -> bool {
    stage_locations(code, ShaderStage::Vertex, StageIo::Params, None)
        .iter()
        .any(|var| {
            attributes
                .iter()
                .find(|(location, _format)| *location == var.location)
                .is_none_or(|(_location, format)| !vertex_type_matches(*format, &var.ty))
        })
}

pub fn inter_stage_invalid(
    vertex: &str, fragment: Option<&str>, max_vars: u32, point_list: bool,
    fragment_entry: Option<&str>,
) -> bool {
    let outputs = stage_locations(vertex, ShaderStage::Vertex, StageIo::Return, None);
    let clip_slots = clip_distance_slots(vertex);
    let output_limit = if point_list {
        max_vars.saturating_sub(1)
    } else {
        max_vars
    };
    if outputs.len() as u32 + clip_slots > output_limit
        || outputs.iter().any(|var| var.location >= max_vars)
    {
        return true;
    }
    let Some(fragment) = fragment else {
        return false;
    };
    let inputs = stage_locations(
        fragment,
        ShaderStage::Fragment,
        StageIo::Params,
        fragment_entry,
    );
    let fragment_used =
        (inputs.len() as u32).saturating_add(fragment_builtin_slots(fragment, fragment_entry));
    fragment_used > max_vars
        || inputs.iter().any(|input| {
            input.location >= max_vars
                || outputs
                    .iter()
                    .find(|output| output.location == input.location)
                    .is_none_or(|output| {
                        output.ty != input.ty || !interp_semantic_eq(output.interp, input.interp)
                    })
        })
}

fn fragment_builtin_slots(code: &str, entry_point: Option<&str>) -> u32 {
    let Some((params, _)) = stage_entry_sig(code, ShaderStage::Fragment, entry_point) else {
        return 0;
    };
    let body = type_body(code, first_param_type(params));
    u32::from(body.contains("@builtin(front_facing)"))
        + u32::from(body.contains("@builtin(sample_mask)"))
        + u32::from(body.contains("@builtin(sample_index)"))
        + u32::from(body.contains("@builtin(primitive_index)"))
}

pub fn inter_stage_naga_skip(
    vertex: &str, fragment: Option<&str>, fragment_entry: Option<&str>,
) -> bool {
    let Some(fragment) = fragment else {
        return false;
    };
    let outputs = stage_locations(vertex, ShaderStage::Vertex, StageIo::Return, None);
    let inputs = stage_locations(
        fragment,
        ShaderStage::Fragment,
        StageIo::Params,
        fragment_entry,
    );
    inputs.iter().any(|input| {
        outputs.iter().any(|output| {
            output.location == input.location
                && interp_semantic_eq(output.interp, input.interp)
                && output.interp.source != input.interp.source
        })
    })
}

pub fn fragment_has_no_color_outputs(code: &str, entry_point: Option<&str>) -> bool {
    stage_locations(code, ShaderStage::Fragment, StageIo::Return, entry_point).is_empty()
}

pub fn fragment_color_io_invalid(
    code: &str, targets: &[Option<wgpu::ColorTargetState>], features: wgpu::Features,
    entry_point: Option<&str>,
) -> bool {
    let outputs = stage_locations(code, ShaderStage::Fragment, StageIo::Return, entry_point);
    targets.iter().enumerate().any(|(index, target)| {
        let Some(state) = target else {
            return false;
        };
        let location = index as u32;
        outputs
            .iter()
            .find(|output| output.location == location)
            .map_or_else(
                || !state.write_mask.is_empty(),
                |output| {
                    let expected = color_format_scalar(state.format, features);
                    let components = shader_component_count(&output.ty);
                    shader_scalar(&output.ty) != expected
                        || components < color_component_count(state.format)
                        || blend_requires_vec4(state) && components != 4
                },
            )
    })
}

#[derive(Clone, Copy)]
enum StageIo {
    Params,
    Return,
}

fn stage_locations(
    code: &str, stage: ShaderStage, io: StageIo, entry_point: Option<&str>,
) -> Vec<LocationVar> {
    let Some((params, ret)) = stage_entry_sig(code, stage, entry_point) else {
        return Vec::new();
    };
    let ty = match io {
        StageIo::Params => first_param_type(params),
        StageIo::Return => ret,
    };
    parse_location_fields(type_body(code, ty))
}

fn stage_entry_sig<'a>(
    code: &'a str, stage: ShaderStage, entry_point: Option<&str>,
) -> Option<(&'a str, &'a str)> {
    let attr = stage.attr();
    let mut remaining = code;
    while let Some((_, after_attr)) = remaining.split_once(attr) {
        remaining = after_attr;
        let Some((_, after_fn)) = after_attr.split_once("fn ") else {
            continue;
        };
        let Some(name) = ident_prefix(after_fn) else {
            continue;
        };
        if entry_point.is_some_and(|target| target != name) {
            continue;
        }
        let after_name = after_fn.get(name.len()..)?.trim_start().strip_prefix('(')?;
        let (params, after_params) = split_matching(after_name, '(', ')')?;
        let after_params = after_params.trim_start();
        let ret = if let Some(after_arrow) = after_params.strip_prefix("->") {
            let after_arrow = after_arrow.trim_start();
            let end = after_arrow.find('{')?;
            after_arrow.get(..end)?.trim()
        } else {
            ""
        };
        return Some((params.trim(), ret));
    }
    None
}

fn first_param_type(params: &str) -> &str {
    params.split_once(':').map_or("", |(_name, ty)| ty.trim())
}

fn type_body<'a>(code: &'a str, ty: &'a str) -> &'a str {
    let ty = ty.trim();
    ident_prefix(ty)
        .filter(|name| *name == ty)
        .and_then(|name| struct_body(code, name))
        .unwrap_or(ty)
}

fn struct_body<'a>(code: &'a str, name: &str) -> Option<&'a str> {
    let mut remaining = code;
    while let Some((_, after)) = remaining.split_once("struct") {
        remaining = after;
        let after = after.trim_start();
        let Some(ident) = ident_prefix(after) else {
            continue;
        };
        if ident != name {
            continue;
        }
        let after_name = after.get(ident.len()..)?.trim_start().strip_prefix('{')?;
        let (body, _) = split_matching(after_name, '{', '}')?;
        return Some(body);
    }
    None
}

fn split_matching(input: &str, open: char, close: char) -> Option<(&str, &str)> {
    let mut depth = 1_i32;
    for (index, ch) in input.char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some((input.get(..index)?, input.get(index + close.len_utf8()..)?));
            }
        }
    }
    None
}

fn parse_location_fields(body: &str) -> Vec<LocationVar> {
    let mut fields = Vec::new();
    let mut remaining = body;
    while let Some((before, after)) = remaining.split_once("@location(") {
        remaining = after;
        let Some((num, rest)) = after.split_once(')') else {
            continue;
        };
        let Ok(location) = num.trim().parse::<u32>() else {
            continue;
        };
        let field = field_rest(rest);
        let prefix = before
            .rsplit_once('\n')
            .map_or(before, |(_head, line)| line);
        let interp = interpolate_in(prefix)
            .or_else(|| interpolate_in(field))
            .unwrap_or_default();
        let ty = field
            .split_once(':')
            .map_or_else(|| normalize_type(field), |(_name, ty)| normalize_type(ty));
        fields.push(LocationVar {
            location,
            ty,
            interp,
        });
    }
    fields
}

fn field_rest(after_location: &str) -> &str {
    let mut depth = 0_i32;
    for (index, ch) in after_location.char_indices() {
        match ch {
            '(' | '<' | '{' => depth += 1,
            ')' | '>' | '}' => depth -= 1,
            ',' | ';' | '\n' if depth == 0 => {
                return after_location.get(..index).unwrap_or(after_location);
            }
            _ => {}
        }
    }
    after_location
}

fn interpolate_in(text: &str) -> Option<Interp> {
    let (_, after) = text.split_once("@interpolate(")?;
    let (args, _) = after.split_once(')')?;
    let mut parts = args.split(',').map(str::trim);
    let ty = match parts.next()? {
        "perspective" => InterpTy::Perspective,
        "linear" => InterpTy::Linear,
        "flat" => InterpTy::Flat,
        _ => return None,
    };
    let sampling_arg = parts.next();
    let sampling = match sampling_arg {
        None | Some("") => {
            match ty {
                InterpTy::Flat => InterpSampling::First,
                _ => InterpSampling::Center,
            }
        }
        Some("center") => InterpSampling::Center,
        Some("centroid") => InterpSampling::Centroid,
        Some("sample") => InterpSampling::Sample,
        Some("first") => InterpSampling::First,
        Some("either") => InterpSampling::Either,
        _ => return None,
    };
    let source = match sampling_arg {
        None | Some("") => InterpSource::TypeOnly,
        _ => InterpSource::Explicit,
    };
    Some(Interp {
        ty,
        sampling,
        source,
    })
}

fn blend_requires_vec4(state: &wgpu::ColorTargetState) -> bool {
    state.blend.as_ref().is_some_and(|blend| {
        reads_src_alpha(blend.color.src_factor) || reads_src_alpha(blend.color.dst_factor)
    })
}

const fn reads_src_alpha(factor: wgpu::BlendFactor) -> bool {
    matches!(
        factor,
        wgpu::BlendFactor::SrcAlpha
            | wgpu::BlendFactor::OneMinusSrcAlpha
            | wgpu::BlendFactor::SrcAlphaSaturated
            | wgpu::BlendFactor::Src1Alpha
            | wgpu::BlendFactor::OneMinusSrc1Alpha
    )
}

fn normalize_type(ty: &str) -> String {
    ty.trim()
        .trim_end_matches([',', ';'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("")
}

fn vertex_type_matches(format: wgpu::VertexFormat, ty: &str) -> bool {
    ty.contains(vertex_format_scalar(format))
}

fn vertex_format_scalar(format: wgpu::VertexFormat) -> &'static str {
    use wgpu::VertexFormat::{
        Sint8, Sint8x2, Sint8x4, Sint16, Sint16x2, Sint16x4, Sint32, Sint32x2, Sint32x3, Sint32x4,
        Uint8, Uint8x2, Uint8x4, Uint16, Uint16x2, Uint16x4, Uint32, Uint32x2, Uint32x3, Uint32x4,
    };
    if matches!(
        format,
        Uint8
            | Uint8x2
            | Uint8x4
            | Uint16
            | Uint16x2
            | Uint16x4
            | Uint32
            | Uint32x2
            | Uint32x3
            | Uint32x4
    ) {
        "u32"
    } else if matches!(
        format,
        Sint8
            | Sint8x2
            | Sint8x4
            | Sint16
            | Sint16x2
            | Sint16x4
            | Sint32
            | Sint32x2
            | Sint32x3
            | Sint32x4
    ) {
        "i32"
    } else {
        "f32"
    }
}

fn shader_scalar(ty: &str) -> &'static str {
    if ty.contains("u32") {
        "u32"
    } else if ty.contains("i32") {
        "i32"
    } else {
        "f32"
    }
}

fn shader_component_count(ty: &str) -> u32 {
    if ty.contains("vec4") {
        4
    } else if ty.contains("vec3") {
        3
    } else if ty.contains("vec2") {
        2
    } else {
        1
    }
}

fn color_format_scalar(format: wgpu::TextureFormat, features: wgpu::Features) -> &'static str {
    match format.sample_type(None, Some(features)) {
        Some(wgpu::TextureSampleType::Uint) => "u32",
        Some(wgpu::TextureSampleType::Sint) => "i32",
        _ => "f32",
    }
}

fn color_component_count(format: wgpu::TextureFormat) -> u32 {
    let channels = format.channels();
    u32::from(channels.contains(wgpu::TextureChannel::RED))
        + u32::from(channels.contains(wgpu::TextureChannel::GREEN))
        + u32::from(channels.contains(wgpu::TextureChannel::BLUE))
        + u32::from(channels.contains(wgpu::TextureChannel::ALPHA))
}

pub fn compute_shader_invalid(
    code: &str, entry_point: Option<&str>, constants: &[(String, f64)], limits: &wgpu::Limits,
) -> bool {
    let overrides = parse_overrides(code);
    entry_point_invalid(code, ShaderStage::Compute, entry_point)
        || unknown_or_missing_overrides(
            &overrides,
            constants,
            code,
            ShaderStage::Compute,
            entry_point,
        )
        || override_values_out_of_range(&overrides, constants)
        || complex_override_used(code, &overrides, entry_point)
        || workgroup_size_invalid(code, limits, &overrides, constants)
        || workgroup_storage_invalid(code, limits, &overrides, constants)
}

pub fn render_stage_invalid(
    code: &str, stage: ShaderStage, entry_point: Option<&str>, constants: &[(String, f64)],
) -> bool {
    let overrides = parse_overrides(code);
    entry_point_invalid(code, stage, entry_point)
        || unknown_or_missing_overrides(&overrides, constants, code, stage, entry_point)
        || override_values_out_of_range(&overrides, constants)
}

pub fn entry_point_invalid(code: &str, stage: ShaderStage, entry_point: Option<&str>) -> bool {
    let names = stage_entry_names(code, stage);
    entry_point.map_or(names.len() != 1, |name| {
        !names.iter().any(|entry| entry == name)
    })
}

pub fn stage_entry_names(code: &str, stage: ShaderStage) -> Vec<String> {
    let attr = stage.attr();
    let mut names = Vec::new();
    let mut remaining = code;
    while let Some((_, after_attr)) = remaining.split_once(attr) {
        remaining = after_attr;
        let Some((_, after_fn)) = after_attr.split_once("fn ") else {
            continue;
        };
        if let Some(name) = ident_prefix(after_fn) {
            names.push(name.to_owned());
        }
    }
    names
}

pub fn parse_overrides(code: &str) -> Vec<Override> {
    let mut overrides = Vec::new();
    let mut remaining = code;
    while let Some((before, after)) = remaining.split_once("override") {
        remaining = after;
        if before.chars().next_back().is_some_and(is_ident_char) {
            continue;
        }
        let after_kw = after.trim_start();
        let Some(name) = ident_prefix(after_kw) else {
            continue;
        };
        let Some(after_name) = after_kw.get(name.len()..) else {
            continue;
        };
        let Some(after_colon) = after_name.trim_start().strip_prefix(':') else {
            continue;
        };
        let after_colon = after_colon.trim_start();
        let Some(ty_name) = ident_prefix(after_colon) else {
            continue;
        };
        let Some(ty) = parse_override_type(ty_name) else {
            continue;
        };
        let Some(after_ty) = after_colon.get(ty_name.len()..) else {
            continue;
        };
        let after_ty = after_ty.trim_start();
        let has_default = after_ty.starts_with('=');
        let default = has_default
            .then(|| after_ty.strip_prefix('=').unwrap_or(after_ty))
            .map(str::trim)
            .map(|expr| expr.split_once(';').map_or(expr, |(literal, _)| literal))
            .map(str::trim)
            .and_then(parse_literal);
        overrides.push(Override {
            key: id_attribute(before).unwrap_or_else(|| name.to_owned()),
            name: name.to_owned(),
            has_default,
            default,
            ty,
        });
    }
    overrides
}

fn complex_override_used(code: &str, overrides: &[Override], entry_point: Option<&str>) -> bool {
    let names = stage_entry_names(code, ShaderStage::Compute);
    let entry = match entry_point {
        Some(name) => name,
        None => {
            match names.as_slice() {
                [name] => name.as_str(),
                _ => return false,
            }
        }
    };
    let Some(body) = entry_body(code, entry) else {
        return false;
    };
    overrides
        .iter()
        .any(|decl| decl.has_default && decl.default.is_none() && ident_in(body, &decl.name))
}

fn entry_body<'a>(code: &'a str, entry: &str) -> Option<&'a str> {
    let mut remaining = code;
    while let Some((_, after_fn)) = remaining.split_once("fn ") {
        remaining = after_fn;
        let Some(name) = ident_prefix(after_fn) else {
            continue;
        };
        if name != entry {
            continue;
        }
        let after_name = after_fn.get(name.len()..)?;
        return Some(
            after_name
                .split_once("fn ")
                .map_or(after_name, |(body, _)| body),
        );
    }
    None
}

fn ident_in(body: &str, ident: &str) -> bool {
    body.split(|ch: char| !is_ident_char(ch))
        .any(|token| token == ident)
}

fn unknown_or_missing_overrides(
    overrides: &[Override], constants: &[(String, f64)], code: &str, stage: ShaderStage,
    entry_point: Option<&str>,
) -> bool {
    let unknown = constants
        .iter()
        .any(|(key, _value)| key.contains('\0') || !overrides.iter().any(|decl| decl.key == *key));
    if unknown {
        return true;
    }
    let names = stage_entry_names(code, stage);
    let entry = match entry_point {
        Some(name) => name,
        None => {
            match names.as_slice() {
                [name] => name.as_str(),
                _ => {
                    return overrides.iter().any(|decl| {
                        !decl.has_default && !constants.iter().any(|(key, _value)| key == &decl.key)
                    });
                }
            }
        }
    };
    overrides.iter().any(|decl| {
        !decl.has_default
            && entry_uses_ident(code, entry, &decl.name)
            && !constants.iter().any(|(key, _value)| key == &decl.key)
    })
}

fn entry_uses_ident(code: &str, entry: &str, ident: &str) -> bool {
    if entry_body(code, entry).is_some_and(|body| ident_in(body, ident)) {
        return true;
    }
    let Some(fn_at) = fn_index(code, entry) else {
        return false;
    };
    let prefix = code.get(..fn_at).unwrap_or("");
    let from = prefix.rfind('@').unwrap_or(fn_at);
    ident_in(code.get(from..fn_at).unwrap_or(""), ident)
}

fn fn_index(code: &str, entry: &str) -> Option<usize> {
    let mut remaining = code;
    let mut offset = 0_usize;
    while let Some(idx) = remaining.find("fn ") {
        let at = offset + idx;
        let after = code.get(at + 3..)?;
        if ident_prefix(after) == Some(entry) {
            return Some(at);
        }
        offset = at + 3;
        remaining = code.get(offset..)?;
    }
    None
}

fn override_values_out_of_range(overrides: &[Override], constants: &[(String, f64)]) -> bool {
    constants.iter().any(|(key, value)| {
        overrides
            .iter()
            .find(|decl| decl.key == *key)
            .is_some_and(|decl| value_invalid_for(decl.ty, *value))
    })
}

fn workgroup_size_invalid(
    code: &str, limits: &wgpu::Limits, overrides: &[Override], constants: &[(String, f64)],
) -> bool {
    let Some((_, after)) = code.split_once("@workgroup_size(") else {
        return false;
    };
    let Some((nums, _)) = after.split_once(')') else {
        return false;
    };
    let mut parts = nums.split(',').map(|part| {
        let token = part.trim();
        token
            .trim_end_matches('u')
            .trim_end_matches('i')
            .parse::<f64>()
            .ok()
            .or_else(|| resolved_override(overrides, constants, token))
            .unwrap_or(1.0)
    });
    let size_x = parts.next().unwrap_or(1.0);
    let size_y = parts.next().unwrap_or(1.0);
    let size_z = parts.next().unwrap_or(1.0);
    if size_x < 1.0 || size_y < 1.0 || size_z < 1.0 {
        return true;
    }
    if size_x.fract() != 0.0 || size_y.fract() != 0.0 || size_z.fract() != 0.0 {
        return true;
    }
    let size_x = size_x as u32;
    let size_y = size_y as u32;
    let size_z = size_z as u32;
    let invocations = size_x.saturating_mul(size_y).saturating_mul(size_z);
    size_x > limits.max_compute_workgroup_size_x
        || size_y > limits.max_compute_workgroup_size_y
        || size_z > limits.max_compute_workgroup_size_z
        || invocations > limits.max_compute_invocations_per_workgroup
}

fn workgroup_storage_invalid(
    code: &str, limits: &wgpu::Limits, overrides: &[Override], constants: &[(String, f64)],
) -> bool {
    workgroup_storage_bytes(code, overrides, constants)
        > u64::from(limits.max_compute_workgroup_storage_size)
}

fn workgroup_storage_bytes(code: &str, overrides: &[Override], constants: &[(String, f64)]) -> u64 {
    let mut total = 0_u64;
    let mut remaining = code;
    while let Some((_, after_workgroup)) = remaining.split_once("var<workgroup>") {
        remaining = after_workgroup;
        let decl = after_workgroup
            .split_once(';')
            .map_or(after_workgroup, |(body, _)| body);
        let Some((_, after_array)) = decl.split_once("array<") else {
            continue;
        };
        let Some((payload, _)) = after_array.rsplit_once('>') else {
            continue;
        };
        let Some((_, count_token)) = payload.rsplit_once(',') else {
            continue;
        };
        let count_token = count_token.trim();
        let count = count_token.parse::<u64>().ok().or_else(|| {
            let value = resolved_override(overrides, constants, count_token)?;
            (value >= 0.0 && value.fract() == 0.0).then_some(value as u64)
        });
        let Some(count) = count else {
            continue;
        };
        let stride = if payload.contains("mat4x4") {
            64
        } else if payload.contains("vec4") {
            16
        } else {
            4
        };
        total = total.saturating_add(count.saturating_mul(stride));
    }
    total
}

fn resolved_override(
    overrides: &[Override], constants: &[(String, f64)], ident: &str,
) -> Option<f64> {
    let decl = overrides.iter().find(|decl| decl.name == ident)?;
    constants
        .iter()
        .find(|(key, _value)| key == &decl.key)
        .map(|(_key, value)| *value)
        .or(decl.default)
}

fn value_invalid_for(ty: OverrideType, value: f64) -> bool {
    match ty {
        OverrideType::Bool => false,
        OverrideType::U32 => value < 0.0 || value > f64::from(u32::MAX) || value.fract() != 0.0,
        OverrideType::I32 => {
            value < f64::from(i32::MIN) || value > f64::from(i32::MAX) || value.fract() != 0.0
        }
        OverrideType::F32 => value.abs() >= f64::from(f32::MAX) / 2.0 + 2.0_f64.powi(127),
        OverrideType::F16 => {
            const F16_MAX: f64 = 65504.0;
            value.abs() >= F16_MAX / 2.0 + 32_768.0
        }
    }
}

fn id_attribute(before: &str) -> Option<String> {
    let prefix = before.trim_end().strip_suffix(')')?;
    let (_, num) = prefix.rsplit_once("@id(")?;
    Some(num.trim().to_owned())
}

fn parse_override_type(name: &str) -> Option<OverrideType> {
    match name {
        "bool" => Some(OverrideType::Bool),
        "u32" => Some(OverrideType::U32),
        "i32" => Some(OverrideType::I32),
        "f32" => Some(OverrideType::F32),
        "f16" => Some(OverrideType::F16),
        _ => None,
    }
}

fn parse_literal(expr: &str) -> Option<f64> {
    match expr {
        "true" => return Some(1.0),
        "false" => return Some(0.0),
        _ => {}
    }
    let expr = expr
        .strip_suffix('h')
        .or_else(|| expr.strip_suffix(['u', 'i']))
        .unwrap_or(expr);
    expr.parse().ok()
}

fn ident_prefix(input: &str) -> Option<&str> {
    let input = input.trim_start();
    let end = input
        .find(|ch: char| {
            ch == '('
                || ch == ':'
                || ch == '='
                || ch == ';'
                || ch == ','
                || ch == '>'
                || ch.is_whitespace()
        })
        .unwrap_or(input.len());
    let name = input.get(..end)?;
    (!name.is_empty()).then_some(name)
}

fn is_ident_char(ch: char) -> bool { ch.is_alphanumeric() || ch == '_' }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderBindingClass {
    Uniform,
    Storage,
    ReadOnlyStorage,
    Sampler,
    ComparisonSampler,
    Texture {
        depth:        bool,
        multisampled: bool,
        dimension:    wgpu::TextureViewDimension,
        scalar:       TextureScalar,
    },
    StorageTexture {
        access:    wgpu::StorageTextureAccess,
        format:    wgpu::TextureFormat,
        dimension: wgpu::TextureViewDimension,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureScalar {
    Float,
    Sint,
    Uint,
    Depth,
}

pub struct ShaderBinding {
    pub group:   u32,
    pub binding: u32,
    pub class:   ShaderBindingClass,
}

pub fn parse_shader_bindings(code: &str) -> Vec<ShaderBinding> {
    let mut bindings = Vec::new();
    let mut remaining = code;
    while let Some((_, after_group)) = remaining.split_once("@group(") {
        remaining = after_group;
        let Some((group_str, after_group_num)) = after_group.split_once(')') else {
            continue;
        };
        let Ok(group) = group_str.trim().parse::<u32>() else {
            continue;
        };
        let Some((_, after_binding)) = after_group_num.split_once("@binding(") else {
            continue;
        };
        let Some((binding_str, after_binding_num)) = after_binding.split_once(')') else {
            continue;
        };
        let Ok(binding) = binding_str.trim().parse::<u32>() else {
            continue;
        };
        let decl = after_binding_num
            .split_once(';')
            .map_or(after_binding_num, |(body, _)| body);
        let Some(class) = parse_binding_class(decl) else {
            continue;
        };
        bindings.push(ShaderBinding {
            group,
            binding,
            class,
        });
    }
    bindings
}

/// Bindings statically used by entry points of `stage`. `None` means analysis
/// failed, so callers must treat every declared binding as used.
pub fn statically_used_bindings(
    code: &str, stage: wgpu::naga::ShaderStage,
) -> Option<Vec<(u32, u32)>> {
    let module = wgpu::naga::front::wgsl::parse_str(code).ok()?;
    let mut validator = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    );
    let info = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validator.validate(&module).ok()
    }))
    .ok()
    .flatten()?;
    let mut used = Vec::new();
    for (index, entry) in module.entry_points.iter().enumerate() {
        if entry.stage != stage {
            continue;
        }
        let function = info.get_entry_point(index);
        for (handle, var) in module.global_variables.iter() {
            let Some(binding) = var.binding.as_ref() else {
                continue;
            };
            if function[handle].is_empty() {
                continue;
            }
            used.push((binding.group, binding.binding));
        }
    }
    Some(used)
}

pub fn storage_texture_access_unsupported(code: &str, features: wgpu::Features) -> bool {
    let parsed = parse_shader_bindings(code).iter().any(|binding| {
        let ShaderBindingClass::StorageTexture { access, format, .. } = binding.class else {
            return false;
        };
        let flags = format.guaranteed_format_features(features).flags;
        match access {
            wgpu::StorageTextureAccess::WriteOnly => {
                !flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY)
            }
            wgpu::StorageTextureAccess::ReadOnly => {
                !flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_ONLY)
            }
            wgpu::StorageTextureAccess::ReadWrite => {
                !flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_WRITE)
            }
            wgpu::StorageTextureAccess::Atomic => {
                !flags.contains(wgpu::TextureFormatFeatureFlags::STORAGE_ATOMIC)
            }
        }
    });
    if parsed {
        return true;
    }
    // Parser miss: still skip wgpu for read_write storage of non-r32 formats.
    code.contains("texture_storage_")
        && code.contains("read_write")
        && !code.contains("r32uint")
        && !code.contains("r32sint")
        && !code.contains("r32float")
}

pub fn binding_type(class: ShaderBindingClass) -> wgpu::BindingType {
    match class {
        ShaderBindingClass::Uniform => {
            wgpu::BindingType::Buffer {
                ty:                 wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size:   None,
            }
        }
        ShaderBindingClass::Storage => {
            wgpu::BindingType::Buffer {
                ty:                 wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size:   None,
            }
        }
        ShaderBindingClass::ReadOnlyStorage => {
            wgpu::BindingType::Buffer {
                ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size:   None,
            }
        }
        ShaderBindingClass::Sampler => {
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
        }
        ShaderBindingClass::ComparisonSampler => {
            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
        }
        ShaderBindingClass::Texture {
            depth,
            multisampled,
            dimension,
            scalar,
        } => {
            wgpu::BindingType::Texture {
                sample_type: match (depth, scalar) {
                    (true, _) | (_, TextureScalar::Depth) => wgpu::TextureSampleType::Depth,
                    (_, TextureScalar::Sint) => wgpu::TextureSampleType::Sint,
                    (_, TextureScalar::Uint) => wgpu::TextureSampleType::Uint,
                    (_, TextureScalar::Float) => {
                        wgpu::TextureSampleType::Float {
                            filterable: !multisampled,
                        }
                    }
                },
                view_dimension: dimension,
                multisampled,
            }
        }
        ShaderBindingClass::StorageTexture {
            access,
            format,
            dimension,
        } => {
            wgpu::BindingType::StorageTexture {
                access,
                format,
                view_dimension: dimension,
            }
        }
    }
}

pub fn shader_binding_matches(binding: &ShaderBinding, ty: wgpu::BindingType) -> bool {
    match (binding.class, ty) {
        (ShaderBindingClass::Uniform, wgpu::BindingType::Buffer { ty, .. }) => {
            ty == wgpu::BufferBindingType::Uniform
        }
        (ShaderBindingClass::Storage, wgpu::BindingType::Buffer { ty, .. }) => {
            ty == wgpu::BufferBindingType::Storage { read_only: false }
        }
        (ShaderBindingClass::ReadOnlyStorage, wgpu::BindingType::Buffer { ty, .. }) => {
            ty == wgpu::BufferBindingType::Storage { read_only: true }
        }
        (ShaderBindingClass::Sampler, wgpu::BindingType::Sampler(sampler)) => {
            sampler != wgpu::SamplerBindingType::Comparison
        }
        (ShaderBindingClass::ComparisonSampler, wgpu::BindingType::Sampler(sampler)) => {
            sampler == wgpu::SamplerBindingType::Comparison
        }
        (
            ShaderBindingClass::Texture {
                depth,
                multisampled,
                dimension,
                scalar,
            },
            wgpu::BindingType::Texture {
                sample_type,
                view_dimension,
                multisampled: layout_ms,
            },
        ) => {
            multisampled == layout_ms
                && dimension == view_dimension
                && texture_scalar_matches(scalar, depth, sample_type)
        }
        (
            ShaderBindingClass::StorageTexture {
                access,
                format,
                dimension,
            },
            wgpu::BindingType::StorageTexture {
                access: layout_access,
                format: layout_format,
                view_dimension,
            },
        ) => {
            storage_access_matches(access, layout_access)
                && format == layout_format
                && dimension == view_dimension
        }
        _ => false,
    }
}

fn texture_scalar_matches(
    scalar: TextureScalar, depth: bool, sample_type: wgpu::TextureSampleType,
) -> bool {
    match (scalar, sample_type) {
        (TextureScalar::Float, wgpu::TextureSampleType::Float { .. }) => !depth,
        (TextureScalar::Sint, wgpu::TextureSampleType::Sint)
        | (TextureScalar::Uint, wgpu::TextureSampleType::Uint)
        | (TextureScalar::Depth, wgpu::TextureSampleType::Depth) => true,
        _ => false,
    }
}

fn storage_access_matches(
    shader: wgpu::StorageTextureAccess, layout: wgpu::StorageTextureAccess,
) -> bool {
    shader == layout
        || matches!(
            (layout, shader),
            (
                wgpu::StorageTextureAccess::ReadWrite,
                wgpu::StorageTextureAccess::WriteOnly
            )
        )
}

fn parse_binding_class(decl: &str) -> Option<ShaderBindingClass> {
    if decl.contains("var<uniform>") {
        return Some(ShaderBindingClass::Uniform);
    }
    if decl.contains("var<storage, read_write>") {
        return Some(ShaderBindingClass::Storage);
    }
    if decl.contains("var<storage") {
        return Some(ShaderBindingClass::ReadOnlyStorage);
    }
    if decl.contains("sampler_comparison") {
        return Some(ShaderBindingClass::ComparisonSampler);
    }
    if decl.contains("sampler") {
        return Some(ShaderBindingClass::Sampler);
    }
    if let Some(storage) = parse_storage_texture(decl) {
        return Some(storage);
    }
    parse_sampled_texture(decl)
}

fn parse_sampled_texture(decl: &str) -> Option<ShaderBindingClass> {
    let (_, after) = decl.split_once("texture_")?;
    let depth = after.starts_with("depth");
    let after = after.strip_prefix("depth_").unwrap_or(after);
    let multisampled = after.starts_with("multisampled_");
    let after = after.strip_prefix("multisampled_").unwrap_or(after);
    let (dim_name, rest) = after.split_once('<').unwrap_or((after, ""));
    let dimension = wgsl_view_dimension(dim_name.trim())?;
    let scalar = if depth {
        TextureScalar::Depth
    } else if rest.contains("i32") {
        TextureScalar::Sint
    } else if rest.contains("u32") {
        TextureScalar::Uint
    } else {
        TextureScalar::Float
    };
    Some(ShaderBindingClass::Texture {
        depth,
        multisampled,
        dimension,
        scalar,
    })
}

fn parse_storage_texture(decl: &str) -> Option<ShaderBindingClass> {
    let (_, after) = decl.split_once("texture_storage_")?;
    let (dim_name, rest) = after.split_once('<')?;
    let dimension = wgsl_view_dimension(dim_name.trim())?;
    let inner = rest.split_once('>').map_or(rest, |(inner, _)| inner);
    let (format_name, access_name) = inner.split_once(',')?;
    let format =
        serde_json::from_str::<wgpu::TextureFormat>(&format!("\"{}\"", format_name.trim())).ok()?;
    let access = match access_name.trim() {
        "write" | "write_only" => wgpu::StorageTextureAccess::WriteOnly,
        "read" | "read_only" => wgpu::StorageTextureAccess::ReadOnly,
        "read_write" => wgpu::StorageTextureAccess::ReadWrite,
        _ => return None,
    };
    Some(ShaderBindingClass::StorageTexture {
        access,
        format,
        dimension,
    })
}

fn wgsl_view_dimension(name: &str) -> Option<wgpu::TextureViewDimension> {
    match name {
        "1d" => Some(wgpu::TextureViewDimension::D1),
        "2d" => Some(wgpu::TextureViewDimension::D2),
        "2d_array" => Some(wgpu::TextureViewDimension::D2Array),
        "3d" => Some(wgpu::TextureViewDimension::D3),
        "cube" => Some(wgpu::TextureViewDimension::Cube),
        "cube_array" => Some(wgpu::TextureViewDimension::CubeArray),
        _ => None,
    }
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

pub const NAGA_IMMEDIATE_SLOT_BYTES: u32 = 256;

pub fn immediate_byte_size(code: &str) -> u32 {
    let Some((_, after)) = code.split_once("var<immediate") else {
        return 0;
    };
    if let Some(body) = struct_immediate_body(code, after) {
        let fields = body.matches(": u32").count()
            + body.matches(": i32").count()
            + body.matches(": f32").count();
        return u32::try_from(fields).unwrap_or(u32::MAX).saturating_mul(4);
    }
    0
}

fn struct_immediate_body<'a>(code: &'a str, after_var: &'a str) -> Option<&'a str> {
    let ty = after_var.split_once(':')?.1.trim();
    let name = ident_prefix(ty)?;
    struct_body(code, name)
}

pub fn naga_immediate_unusable(code: &str) -> bool {
    immediate_byte_size(code) > NAGA_IMMEDIATE_SLOT_BYTES
}

pub fn immediate_slots_mask(bytes: u32) -> u64 {
    let slots = (bytes >> 2).min(64);
    (0..slots).fold(0_u64, |mask, index| mask | (1_u64 << index))
}

pub fn immediate_slots_used(code: &str, entry_point: Option<&str>, stage: ShaderStage) -> u64 {
    let Ok(module) = wgpu::naga::front::wgsl::parse_str(code) else {
        return 0;
    };
    let mut validator = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::IMMEDIATES,
    );
    let Ok(Some(info)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validator.validate(&module).ok()
    })) else {
        return 0;
    };
    let want_stage = match stage {
        ShaderStage::Compute => wgpu::naga::ShaderStage::Compute,
        ShaderStage::Vertex => wgpu::naga::ShaderStage::Vertex,
        ShaderStage::Fragment => wgpu::naga::ShaderStage::Fragment,
    };
    let gctx = module.to_ctx();
    module
        .entry_points
        .iter()
        .enumerate()
        .filter(|(_index, entry)| {
            entry.stage == want_stage && entry_point.is_none_or(|name| entry.name == name)
        })
        .fold(0, |bits, (index, _entry)| {
            let function = info.get_entry_point(index);
            bits | module
                .global_variables
                .iter()
                .filter(|(_handle, var)| var.space == wgpu::naga::AddressSpace::Immediate)
                .filter(|(handle, _var)| !function[*handle].is_empty())
                .fold(0, |used, (_handle, var)| {
                    used | wgpu::naga::valid::ImmediateSlots::from_type(
                        &module.types[var.ty].inner,
                        &module.types,
                        gctx,
                    )
                    .map_or(0, immediate_slots_bits)
                })
        })
}

pub fn shader_buffer_min_sizes(code: &str) -> Vec<(u32, u32, u64)> {
    let Ok(module) = wgpu::naga::front::wgsl::parse_str(code) else {
        return Vec::new();
    };
    module
        .global_variables
        .iter()
        .filter_map(|(_handle, var)| {
            let binding = var.binding.as_ref()?;
            let min = match var.space {
                wgpu::naga::AddressSpace::Uniform | wgpu::naga::AddressSpace::Storage { .. } => {
                    type_min_bytes(&module.types[var.ty].inner)
                }
                _ => return None,
            };
            (min > 0).then_some((binding.group, binding.binding, min))
        })
        .collect()
}

fn type_min_bytes(inner: &wgpu::naga::TypeInner) -> u64 {
    match *inner {
        wgpu::naga::TypeInner::Scalar(scalar) | wgpu::naga::TypeInner::Atomic(scalar) => {
            u64::from(scalar.width)
        }
        wgpu::naga::TypeInner::Vector { size, scalar } => {
            vector_components(size) * u64::from(scalar.width)
        }
        wgpu::naga::TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => vector_components(columns) * vector_components(rows) * u64::from(scalar.width),
        wgpu::naga::TypeInner::Array {
            size: wgpu::naga::ArraySize::Constant(count),
            stride,
            ..
        } => u64::from(count.get()) * u64::from(stride),
        wgpu::naga::TypeInner::Array { stride, .. } => u64::from(stride),
        wgpu::naga::TypeInner::Struct { span, .. } => u64::from(span),
        _ => 0,
    }
}

const fn vector_components(size: wgpu::naga::VectorSize) -> u64 {
    match size {
        wgpu::naga::VectorSize::Bi => 2,
        wgpu::naga::VectorSize::Tri => 3,
        wgpu::naga::VectorSize::Quad => 4,
    }
}

fn immediate_slots_bits(slots: wgpu::naga::valid::ImmediateSlots) -> u64 {
    (0..64).fold(0_u64, |bits, index| {
        let one = wgpu::naga::valid::ImmediateSlots::from_raw(1_u64 << index);
        if slots.contains(one) {
            bits | (1_u64 << index)
        } else {
            bits
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        OverrideType, ShaderBindingClass, ShaderStage, parse_overrides, parse_shader_bindings,
        render_stage_invalid, stage_entry_names, storage_texture_access_unsupported,
    };

    #[test]
    fn parse_overrides_keys_id_over_name() {
        let code = "
            override c0: bool = true;
            override c1: u32 = 0u;
            override 数: u32 = 0u;
            override séquençage: u32 = 0u;
            @id(1000) override c2: u32 = 10u;
            @id(1) override c3: u32 = 11u;
            @compute @workgroup_size(1) fn main () {}
        ";
        let overrides = parse_overrides(code);
        assert_eq!(
            overrides
                .iter()
                .map(|decl| {
                    (
                        decl.key.as_str(),
                        decl.name.as_str(),
                        decl.ty,
                        decl.has_default,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("c0", "c0", OverrideType::Bool, true),
                ("c1", "c1", OverrideType::U32, true),
                ("数", "数", OverrideType::U32, true),
                ("séquençage", "séquençage", OverrideType::U32, true),
                ("1000", "c2", OverrideType::U32, true),
                ("1", "c3", OverrideType::U32, true),
            ]
        );
    }

    #[test]
    fn parse_overrides_uninitialized_has_no_default() {
        let code = "override c0: bool;\noverride c1: bool = false;";
        let overrides = parse_overrides(code);
        let first = overrides.first().expect("c0");
        let second = overrides.get(1).expect("c1");
        assert!(!first.has_default);
        assert!(second.has_default);
        assert_eq!(second.default, Some(0.0));
    }

    #[test]
    fn immediate_byte_size_counts_u32_fields() {
        let code = "
            struct Immediates { m0: u32, m1: u32, m2: u32, m3: u32 }
            var<immediate> data: Immediates;
            fn use_data() { _ = data.m0; }
            @compute @workgroup_size(1) fn main_compute() { use_data(); }
        ";
        assert_eq!(super::immediate_byte_size(code), 16);
        assert!(!super::naga_immediate_unusable(code));
        let fields = (0..80)
            .map(|index| format!("m{index}: u32"))
            .collect::<Vec<_>>()
            .join(", ");
        let large = format!(
            "struct Immediates {{ {fields} }}\nvar<immediate> data: Immediates;\nfn use_data() {{ \
             _ = data.m0; }}\n@compute @workgroup_size(1) fn main_compute() {{ use_data(); }}"
        );
        assert!(super::immediate_byte_size(&large) > 256);
        assert!(super::naga_immediate_unusable(&large));
    }

    #[test]
    fn unused_override_without_default_is_not_required() {
        let code = "
            override A: f32 = 3.0;
            override C: f32;
            @vertex fn vertexMain() -> @builtin(position) vec4<f32> {
                return vec4<f32>(A, 0.0, 0.0, 1.0);
            }
            @fragment fn fragmentMain() -> @location(0) vec4<f32> {
                return vec4<f32>(C, 0.0, 0.0, 1.0);
            }
        ";
        assert!(!render_stage_invalid(
            code,
            ShaderStage::Vertex,
            Some("vertexMain"),
            &[]
        ));
        assert!(render_stage_invalid(
            code,
            ShaderStage::Fragment,
            Some("fragmentMain"),
            &[]
        ));
        assert!(!render_stage_invalid(
            code,
            ShaderStage::Fragment,
            Some("fragmentMain"),
            &[("C".into(), 0.8)]
        ));
    }

    #[test]
    fn stage_entry_names_collects_compute_fns() {
        let code =
            "@compute @workgroup_size(1) fn main() {}\n@compute @workgroup_size(1) fn extra() {}";
        assert_eq!(stage_entry_names(code, ShaderStage::Compute), [
            "main", "extra"
        ]);
        assert!(stage_entry_names(code, ShaderStage::Vertex).is_empty());
    }

    #[test]
    fn complex_override_default_used_by_entry_is_invalid() {
        let code = "
            override cu: u32 = 0u;
            override cx: u32 = 1u/cu;
            @compute @workgroup_size(1) fn main_success () { _ = cu; }
            @compute @workgroup_size(1) fn main_pipe_error () { _ = cx; }
        ";
        let limits = wgpu::Limits::default();
        assert!(!super::compute_shader_invalid(
            code,
            Some("main_success"),
            &[],
            &limits,
        ));
        assert!(super::compute_shader_invalid(
            code,
            Some("main_pipe_error"),
            &[],
            &limits,
        ));
    }

    #[test]
    fn concatenated_compute_attribute_is_valid() {
        let code = "@group(0) @binding(0) var<storage, read_write> res : \
                    array<vec4u>;\n@compute@workgroup_size(1)\nfn main() {\n  _ = res[0];\n}\n";
        assert_eq!(stage_entry_names(code, ShaderStage::Compute), ["main"]);
        assert!(!super::compute_shader_invalid(
            code,
            Some("main"),
            &[],
            &wgpu::Limits::default(),
        ));
    }

    #[test]
    fn read_write_rgba8unorm_storage_is_unsupported() {
        let code = "
      @group(0) @binding(0) var tex: texture_storage_2d<rgba8unorm, read_write>;
      @compute @workgroup_size(1) fn main() {
        _ = tex;
      }
    ";
        let bindings = parse_shader_bindings(code);
        assert_eq!(bindings.len(), 1);
        assert!(matches!(
            bindings.first().map(|binding| binding.class),
            Some(ShaderBindingClass::StorageTexture {
                access: wgpu::StorageTextureAccess::ReadWrite,
                format: wgpu::TextureFormat::Rgba8Unorm,
                ..
            })
        ));
        assert!(storage_texture_access_unsupported(
            code,
            wgpu::Features::empty()
        ));
        assert!(storage_texture_access_unsupported(
            code,
            wgpu::Features::all()
        ));
    }

    #[test]
    fn inter_stage_requires_vertex_superset() {
        let vertex = "
            struct A {
                @location(0) vout0: f32,
                @builtin(position) pos: vec4<f32>,
            }
            @vertex fn main() -> A {
                var vertexOut: A;
                vertexOut.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
                return vertexOut;
            }
        ";
        let fragment_ok = "
            struct B {
                @location(0) fin0: f32,
            }
            @fragment fn main(fragmentIn: B) -> @location(0) vec4<f32> {
                return vec4<f32>(1.0, 1.0, 1.0, 1.0);
            }
        ";
        let fragment_missing = "
            struct B {
                @location(0) fin0: f32,
                @location(1) fin1: f32,
            }
            @fragment fn main(fragmentIn: B) -> @location(0) vec4<f32> {
                return vec4<f32>(1.0, 1.0, 1.0, 1.0);
            }
        ";
        assert!(!super::inter_stage_invalid(
            vertex,
            Some(fragment_ok),
            16,
            false,
            None
        ));
        assert!(super::inter_stage_invalid(
            vertex,
            Some(fragment_missing),
            16,
            false,
            None
        ));
        assert!(super::writes_frag_depth(
            "@fragment fn main() -> @builtin(frag_depth) f32 { return 0.5; }",
            None
        ));
        let multi_entry = "
            struct FragmentOutput1 {
              @builtin(sample_mask) mask : u32,
              @location(0) color : vec4<f32>,
            }
            @fragment fn fmain__fragment_output_mask__flat() -> FragmentOutput1 {
              return FragmentOutput1(0u, vec4<f32>(1.0));
            }
            struct FragmentOutput2 {
              @location(0) color0 : vec4<f32>,
              @location(1) color1 : vec4<f32>,
            }
            @fragment fn fmain__alpha_to_coverage_mask__flat() -> FragmentOutput2 {
              return FragmentOutput2(vec4<f32>(1.0), vec4<f32>(1.0));
            }
        ";
        assert!(super::writes_sample_mask(
            multi_entry,
            Some("fmain__fragment_output_mask__flat")
        ));
        assert!(!super::writes_sample_mask(
            multi_entry,
            Some("fmain__alpha_to_coverage_mask__flat")
        ));
        let a2c_targets = [
            Some(wgpu::ColorTargetState {
                format:     wgpu::TextureFormat::Rgba8Unorm,
                blend:      None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format:     wgpu::TextureFormat::Rgba8Unorm,
                blend:      None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];
        assert!(super::fragment_color_io_invalid(
            multi_entry,
            &a2c_targets,
            wgpu::Features::empty(),
            Some("fmain__fragment_output_mask__flat")
        ));
        assert!(!super::fragment_color_io_invalid(
            multi_entry,
            &a2c_targets,
            wgpu::Features::empty(),
            Some("fmain__alpha_to_coverage_mask__flat")
        ));
    }

    #[test]
    fn fragment_inline_location_matches_rgba8() {
        let code = "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 1.0, \
                    0.0, 1.0); }";
        let targets = [Some(wgpu::ColorTargetState {
            format:     wgpu::TextureFormat::Rgba8Unorm,
            blend:      None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        assert!(!super::fragment_color_io_invalid(
            code,
            &targets,
            wgpu::Features::empty(),
            None
        ));
    }
}
