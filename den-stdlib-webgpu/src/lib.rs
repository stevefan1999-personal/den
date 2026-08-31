//! A native, headless WebGPU slice for `den:webgpu`.
//!
//! The public `wgpu` layer owns validation and backend dispatch. This crate is
//! only the rquickjs boundary: branded handles, descriptor conversion, promise
//! bridging, and safe copy-in/copy-out mapped buffers.
//!
//! Canvas, surfaces, and Deno's BYOW window-handle bridge are deliberately
//! absent: den has no host-owned surface, and copying that bridge would punch a
//! capability hole. `DENO_WEBGPU_BACKEND` selects wgpu backends; `noop` is the
//! hermetic test path.
#![recursion_limit = "256"]

mod format;
mod query;
mod render;
mod texture;

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    mem::MaybeUninit,
    num::NonZeroU64,
    ops::Range,
    rc::Rc,
    sync::{Arc, Mutex},
};

use den_util::BufferSource;
use rquickjs::{
    Array, ArrayBuffer, Class, Coerced, Constructor, Ctx, Error, Exception, FromJs, Function,
    IntoJs as _, JsLifetime, Object, Persistent, Promise, Result, Value,
    class::{Trace, Tracer},
    function::Opt,
};

const MAP_READ: u32 = 1;
const MAP_WRITE: u32 = 2;
const WEBGPU_BUFFER_USAGE_MASK: u32 = 0x03ff;
const JS_MAX_SAFE_INTEGER: u64 = 0x001F_FFFF_FFFF_FFFF;

pub(crate) fn illegal_constructor<T>(ctx: &Ctx<'_>) -> Result<T> {
    Err(Exception::throw_type(ctx, "Illegal constructor"))
}

pub(crate) fn type_error(ctx: &Ctx<'_>, message: impl AsRef<str>) -> rquickjs::Error {
    Exception::throw_type(ctx, message.as_ref())
}

pub(crate) fn operation_error(ctx: &Ctx<'_>, message: impl AsRef<str>) -> rquickjs::Error {
    den_util::throw_dom_exception(ctx, "OperationError", message.as_ref())
}

pub(crate) fn invalid_state(ctx: &Ctx<'_>, message: impl AsRef<str>) -> rquickjs::Error {
    den_util::throw_dom_exception(ctx, "InvalidStateError", message.as_ref())
}

#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct JsU32(pub(crate) u32);

impl<'js> FromJs<'js> for JsU32 {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let Coerced(value) = Coerced::<u64>::from_js(ctx, value)?;
        u32::try_from(value)
            .map(Self)
            .map_err(|_error| Exception::throw_range(ctx, "unsigned integer is out of range"))
    }
}

#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct JsU64(pub(crate) u64);

impl<'js> FromJs<'js> for JsU64 {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let Coerced(value) = Coerced::<u64>::from_js(ctx, value)?;
        Ok(Self(value))
    }
}

pub(crate) fn label(object: &Object<'_>) -> Result<String> {
    Ok(object
        .get::<_, Option<String>>("label")?
        .unwrap_or_default())
}

pub(crate) fn class_value<'js, T>(
    object: &Object<'js>, key: &str, ctx: &Ctx<'js>,
) -> Result<Class<'js, T>>
where
    T: rquickjs::class::JsClass<'js>,
{
    object
        .get(key)
        .map_err(|_error| type_error(ctx, format!("{key} must be a WebGPU object")))
}

fn array_value<'js>(object: &Object<'js>, key: &str, ctx: &Ctx<'js>) -> Result<Array<'js>> {
    object
        .get(key)
        .map_err(|_error| type_error(ctx, format!("{key} must be an array")))
}

fn parse_backends() -> wgpu::Backends {
    std::env::var("DEN_WEBGPU_BACKEND")
        .or_else(|_| std::env::var("DENO_WEBGPU_BACKEND"))
        .map_or_else(
            |_error| wgpu::Backends::all(),
            |value| wgpu::Backends::from_comma_list(&value),
        )
}

struct GpuPoll;

impl GpuPoll {
    fn until<T: Send>(
        device: wgpu::Device, receiver: std::sync::mpsc::Receiver<T>,
    ) -> std::result::Result<T, String> {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())
    }
}

fn make_instance() -> wgpu::Instance {
    let backends = parse_backends();
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    if backends.contains(wgpu::Backends::NOOP) {
        descriptor.backend_options.noop = wgpu::NoopBackendOptions::enabled();
    }
    wgpu::Instance::new(descriptor)
}

fn string_set<'js>(ctx: &Ctx<'js>, values: Vec<String>) -> Result<Object<'js>> {
    let constructor: Constructor<'js> = ctx.globals().get("Set")?;
    constructor.construct((values,))
}

fn feature_set<'js>(ctx: &Ctx<'js>, features: wgpu::Features) -> Result<Object<'js>> {
    string_set(
        ctx,
        (features & wgpu::Features::all_webgpu_mask())
            .iter()
            .filter_map(|feature| feature.as_str().map(str::to_owned))
            .collect(),
    )
}

fn limits_object<'js>(ctx: &Ctx<'js>, limits: &wgpu::Limits) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    macro_rules! set {
        ($js:literal, $rust:ident) => {
            object.set($js, limits.$rust)?;
        };
    }
    set!("maxTextureDimension1D", max_texture_dimension_1d);
    set!("maxTextureDimension2D", max_texture_dimension_2d);
    set!("maxTextureDimension3D", max_texture_dimension_3d);
    set!("maxTextureArrayLayers", max_texture_array_layers);
    set!("maxBindGroups", max_bind_groups);
    set!(
        "maxBindGroupsPlusVertexBuffers",
        max_bind_groups_plus_vertex_buffers
    );
    set!("maxBindingsPerBindGroup", max_bindings_per_bind_group);
    set!(
        "maxDynamicUniformBuffersPerPipelineLayout",
        max_dynamic_uniform_buffers_per_pipeline_layout
    );
    set!(
        "maxDynamicStorageBuffersPerPipelineLayout",
        max_dynamic_storage_buffers_per_pipeline_layout
    );
    set!(
        "maxSampledTexturesPerShaderStage",
        max_sampled_textures_per_shader_stage
    );
    set!("maxSamplersPerShaderStage", max_samplers_per_shader_stage);
    set!(
        "maxStorageBuffersPerShaderStage",
        max_storage_buffers_per_shader_stage
    );
    object.set(
        "maxStorageBuffersInVertexStage",
        limits.max_storage_buffers_per_shader_stage,
    )?;
    object.set(
        "maxStorageBuffersInFragmentStage",
        limits.max_storage_buffers_per_shader_stage,
    )?;
    set!(
        "maxStorageTexturesPerShaderStage",
        max_storage_textures_per_shader_stage
    );
    set!(
        "maxUniformBuffersPerShaderStage",
        max_uniform_buffers_per_shader_stage
    );
    object.set(
        "maxUniformBufferBindingSize",
        limits
            .max_uniform_buffer_binding_size
            .min(JS_MAX_SAFE_INTEGER),
    )?;
    object.set(
        "maxStorageBufferBindingSize",
        limits
            .max_storage_buffer_binding_size
            .min(JS_MAX_SAFE_INTEGER),
    )?;
    set!(
        "minUniformBufferOffsetAlignment",
        min_uniform_buffer_offset_alignment
    );
    set!(
        "minStorageBufferOffsetAlignment",
        min_storage_buffer_offset_alignment
    );
    set!("maxVertexBuffers", max_vertex_buffers);
    object.set(
        "maxBufferSize",
        limits.max_buffer_size.min(JS_MAX_SAFE_INTEGER),
    )?;
    set!("maxVertexAttributes", max_vertex_attributes);
    set!("maxVertexBufferArrayStride", max_vertex_buffer_array_stride);
    set!(
        "maxInterStageShaderVariables",
        max_inter_stage_shader_variables
    );
    set!("maxColorAttachments", max_color_attachments);
    set!(
        "maxColorAttachmentBytesPerSample",
        max_color_attachment_bytes_per_sample
    );
    set!(
        "maxComputeWorkgroupStorageSize",
        max_compute_workgroup_storage_size
    );
    set!(
        "maxComputeInvocationsPerWorkgroup",
        max_compute_invocations_per_workgroup
    );
    set!("maxComputeWorkgroupSizeX", max_compute_workgroup_size_x);
    set!("maxComputeWorkgroupSizeY", max_compute_workgroup_size_y);
    set!("maxComputeWorkgroupSizeZ", max_compute_workgroup_size_z);
    set!(
        "maxComputeWorkgroupsPerDimension",
        max_compute_workgroups_per_dimension
    );
    set!("maxImmediateSize", max_immediate_size);
    Ok(object)
}

fn apply_required_limits<'js>(
    ctx: &Ctx<'js>, object: Option<Object<'js>>, limits: &mut wgpu::Limits,
) -> Result<()> {
    let Some(object) = object else {
        return Ok(());
    };
    for property in object.props::<String, Value>() {
        let (name, value) = property?;
        if value.is_null() || value.is_undefined() {
            continue;
        }
        let value = JsU64::from_js(ctx, value)?.0;
        macro_rules! u32_limit {
            ($field:ident) => {
                limits.$field = value.try_into().map_err(|_| {
                    Exception::throw_range(ctx, &format!("required limit {name} is too large"))
                })?
            };
        }
        match name.as_str() {
            "maxTextureDimension1D" => u32_limit!(max_texture_dimension_1d),
            "maxTextureDimension2D" => u32_limit!(max_texture_dimension_2d),
            "maxTextureDimension3D" => u32_limit!(max_texture_dimension_3d),
            "maxTextureArrayLayers" => u32_limit!(max_texture_array_layers),
            "maxBindGroups" => u32_limit!(max_bind_groups),
            "maxBindGroupsPlusVertexBuffers" => u32_limit!(max_bind_groups_plus_vertex_buffers),
            "maxBindingsPerBindGroup" => u32_limit!(max_bindings_per_bind_group),
            "maxDynamicUniformBuffersPerPipelineLayout" => {
                u32_limit!(max_dynamic_uniform_buffers_per_pipeline_layout)
            }
            "maxDynamicStorageBuffersPerPipelineLayout" => {
                u32_limit!(max_dynamic_storage_buffers_per_pipeline_layout)
            }
            "maxSampledTexturesPerShaderStage" => {
                u32_limit!(max_sampled_textures_per_shader_stage)
            }
            "maxSamplersPerShaderStage" => u32_limit!(max_samplers_per_shader_stage),
            "maxStorageBuffersPerShaderStage"
            | "maxStorageBuffersInVertexStage"
            | "maxStorageBuffersInFragmentStage" => {
                u32_limit!(max_storage_buffers_per_shader_stage)
            }
            "maxStorageTexturesPerShaderStage"
            | "maxStorageTexturesInVertexStage"
            | "maxStorageTexturesInFragmentStage" => {
                u32_limit!(max_storage_textures_per_shader_stage)
            }
            "maxUniformBuffersPerShaderStage" => u32_limit!(max_uniform_buffers_per_shader_stage),
            "maxUniformBufferBindingSize" => {
                limits.max_uniform_buffer_binding_size = value.min(JS_MAX_SAFE_INTEGER)
            }
            "maxStorageBufferBindingSize" => limits.max_storage_buffer_binding_size = value,
            "minUniformBufferOffsetAlignment" => {
                u32_limit!(min_uniform_buffer_offset_alignment)
            }
            "minStorageBufferOffsetAlignment" => {
                u32_limit!(min_storage_buffer_offset_alignment)
            }
            "maxVertexBuffers" => u32_limit!(max_vertex_buffers),
            "maxBufferSize" => limits.max_buffer_size = value,
            "maxVertexAttributes" => u32_limit!(max_vertex_attributes),
            "maxVertexBufferArrayStride" => u32_limit!(max_vertex_buffer_array_stride),
            "maxInterStageShaderVariables" => u32_limit!(max_inter_stage_shader_variables),
            "maxColorAttachments" => u32_limit!(max_color_attachments),
            "maxColorAttachmentBytesPerSample" => {
                u32_limit!(max_color_attachment_bytes_per_sample)
            }
            "maxComputeWorkgroupStorageSize" => u32_limit!(max_compute_workgroup_storage_size),
            "maxComputeInvocationsPerWorkgroup" => {
                u32_limit!(max_compute_invocations_per_workgroup)
            }
            "maxComputeWorkgroupSizeX" => u32_limit!(max_compute_workgroup_size_x),
            "maxComputeWorkgroupSizeY" => u32_limit!(max_compute_workgroup_size_y),
            "maxComputeWorkgroupSizeZ" => u32_limit!(max_compute_workgroup_size_z),
            "maxComputeWorkgroupsPerDimension" => {
                u32_limit!(max_compute_workgroups_per_dimension)
            }
            "maxImmediateSize" => u32_limit!(max_immediate_size),
            _ => return Err(type_error(ctx, format!("unknown required limit {name}"))),
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GPUErrorKind {
    Internal,
    OutOfMemory,
    Validation,
}

struct GPUErrorData {
    kind:    GPUErrorKind,
    message: String,
}

struct ErrorScopeState {
    error:  Option<GPUErrorData>,
    filter: GPUErrorKind,
}

#[derive(Default)]
struct ErrorRouter {
    scopes: Vec<ErrorScopeState>,
}

impl ErrorRouter {
    fn capture(&mut self, error: GPUErrorData) -> Option<GPUErrorData> {
        if let Some(scope) = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.filter == error.kind)
        {
            if scope.error.is_none() {
                scope.error = Some(error);
            }
            None
        } else {
            Some(error)
        }
    }
}

fn gpu_error_data(error: wgpu::Error) -> GPUErrorData {
    match error {
        wgpu::Error::Validation { description, .. } => {
            GPUErrorData {
                kind:    GPUErrorKind::Validation,
                message: description,
            }
        }
        wgpu::Error::OutOfMemory { source } => {
            GPUErrorData {
                kind:    GPUErrorKind::OutOfMemory,
                message: source.to_string(),
            }
        }
        wgpu::Error::Internal { description, .. } => {
            GPUErrorData {
                kind:    GPUErrorKind::Internal,
                message: description,
            }
        }
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPU")]
pub struct GPU {
    #[qjs(skip_trace)]
    instance: wgpu::Instance,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPU {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    pub async fn request_adapter<'js>(
        self, options: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        let options = options.0.flatten();
        let power_preference = match options
            .as_ref()
            .map(|value| value.get::<_, Option<String>>("powerPreference"))
            .transpose()?
            .flatten()
            .as_deref()
        {
            None => wgpu::PowerPreference::None,
            Some("low-power") => wgpu::PowerPreference::LowPower,
            Some("high-performance") => wgpu::PowerPreference::HighPerformance,
            Some(value) => {
                return Err(type_error(
                    &ctx,
                    format!("invalid GPU powerPreference {value}"),
                ));
            }
        };
        if let Some(feature_level) = options
            .as_ref()
            .map(|value| value.get::<_, Option<String>>("featureLevel"))
            .transpose()?
            .flatten()
            && feature_level != "core"
            && feature_level != "compatibility"
        {
            return Ok(Value::new_null(ctx));
        }
        let force_fallback_adapter = options
            .as_ref()
            .map(|value| value.get::<_, Option<bool>>("forceFallbackAdapter"))
            .transpose()?
            .flatten()
            .unwrap_or_default();
        match self
            .instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                force_fallback_adapter,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .ok()
        {
            Some(adapter)
                if force_fallback_adapter
                    && adapter.get_info().device_type == wgpu::DeviceType::Cpu =>
            {
                Ok(Value::new_null(ctx))
            }
            Some(adapter) => {
                Class::instance(ctx.clone(), GPUAdapter::from_inner(adapter))?.into_js(&ctx)
            }
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn get_preferred_canvas_format(&self) -> &'static str {
        if cfg!(target_os = "android") {
            "rgba8unorm"
        } else {
            "bgra8unorm"
        }
    }

    #[qjs(get)]
    pub fn wgsl_language_features<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let supported = self.instance.wgsl_language_features();
        let mut names = Vec::new();
        for (feature, name) in [
            (
                wgpu::WgslLanguageFeatures::ReadOnlyAndReadWriteStorageTextures,
                "readonly_and_readwrite_storage_textures",
            ),
            (
                wgpu::WgslLanguageFeatures::Packed4x8IntegerDotProduct,
                "packed_4x8_integer_dot_product",
            ),
            (
                wgpu::WgslLanguageFeatures::UnrestrictedPointerParameters,
                "unrestricted_pointer_parameters",
            ),
            (
                wgpu::WgslLanguageFeatures::PointerCompositeAccess,
                "pointer_composite_access",
            ),
        ] {
            if supported.contains(feature) {
                names.push(name.to_owned());
            }
        }
        string_set(&ctx, names)
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUAdapter")]
pub struct GPUAdapter {
    #[qjs(skip_trace)]
    inner: wgpu::Adapter,
}

impl GPUAdapter {
    fn from_inner(inner: wgpu::Adapter) -> Self { Self { inner } }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUAdapter {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn features<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        feature_set(&ctx, self.inner.features())
    }

    #[qjs(get)]
    pub fn limits<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        limits_object(&ctx, &self.inner.limits())
    }

    #[qjs(get)]
    pub fn info(&self) -> GPUAdapterInfo {
        GPUAdapterInfo {
            inner: self.inner.get_info(),
        }
    }

    pub async fn request_device<'js>(
        self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUDevice<'js>> {
        let descriptor = descriptor.0.flatten();
        let label = descriptor
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let queue_label = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Object>>("defaultQueue"))
            .transpose()?
            .flatten()
            .map(|queue| crate::label(&queue))
            .transpose()?
            .unwrap_or_default();
        let mut required_features = wgpu::Features::empty();
        if let Some(features) = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Array>>("requiredFeatures"))
            .transpose()?
            .flatten()
        {
            for name in features.iter::<String>() {
                let name = name?;
                required_features |= name
                    .parse::<wgpu::Features>()
                    .map_err(|()| type_error(&ctx, format!("unknown WebGPU feature {name}")))?;
            }
        }
        if !required_features
            .difference(wgpu::Features::all_webgpu_mask())
            .is_empty()
        {
            return Err(type_error(
                &ctx,
                "requiredFeatures contains a native-only wgpu feature",
            ));
        }
        if !self.inner.features().contains(required_features) {
            return Err(type_error(
                &ctx,
                "requiredFeatures must be a subset of the adapter features",
            ));
        }
        let mut required_limits = wgpu::Limits::default();
        let limits = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Object>>("requiredLimits"))
            .transpose()?
            .flatten();
        apply_required_limits(&ctx, limits, &mut required_limits)?;
        let request = wgpu::DeviceDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            required_features,
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
        };
        let (device, queue) = self
            .inner
            .request_device(&request)
            .await
            .map_err(|error| operation_error(&ctx, error.to_string()))?;
        let errors = Arc::new(Mutex::new(ErrorRouter::default()));
        let error_sink = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error| {
            let error = gpu_error_data(error);
            let uncaptured = error_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .capture(error);
            if let Some(error) = uncaptured {
                log::error!("uncaptured WebGPU error: {}", error.message);
            }
        }));
        let queue = Class::instance(ctx.clone(), GPUQueue {
            device: Some(device.clone()),
            inner:  queue,
            label:  Rc::new(RefCell::new(queue_label)),
        })?;
        let (lost, resolve, _reject) = ctx.promise()?;
        Ok(GPUDevice {
            adapter_info: GPUAdapterInfo {
                inner: self.inner.get_info(),
            },
            destroyed: Rc::new(Cell::new(false)),
            device,
            label: Rc::new(RefCell::new(label)),
            errors,
            lost,
            lost_resolve: Rc::new(RefCell::new(Some(resolve))),
            queue,
        })
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUAdapterInfo")]
pub struct GPUAdapterInfo {
    #[qjs(skip_trace)]
    inner: wgpu::AdapterInfo,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUAdapterInfo {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn vendor(&self) -> String { self.inner.vendor.to_string() }

    #[qjs(get)]
    pub const fn architecture(&self) -> &'static str { "" }

    #[qjs(get)]
    pub fn device(&self) -> String { self.inner.device.to_string() }

    #[qjs(get)]
    pub fn description(&self) -> String { self.inner.name.clone() }

    #[qjs(get)]
    pub const fn subgroup_min_size(&self) -> u32 { self.inner.subgroup_min_size }

    #[qjs(get)]
    pub const fn subgroup_max_size(&self) -> u32 { self.inner.subgroup_max_size }

    #[qjs(get)]
    pub fn is_fallback_adapter(&self) -> bool { self.inner.device_type == wgpu::DeviceType::Cpu }
}

#[derive(Clone, JsLifetime)]
#[rquickjs::class(rename = "GPUDevice")]
pub struct GPUDevice<'js> {
    adapter_info: GPUAdapterInfo,
    destroyed:    Rc<Cell<bool>>,
    device:       wgpu::Device,
    errors:       Arc<Mutex<ErrorRouter>>,
    label:        Rc<RefCell<String>>,
    lost:         Promise<'js>,
    lost_resolve: Rc<RefCell<Option<Function<'js>>>>,
    queue:        Class<'js, GPUQueue>,
}

impl<'js> Trace<'js> for GPUDevice<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.lost.trace(tracer);
        self.queue.trace(tracer);
        if let Ok(resolve) = self.lost_resolve.try_borrow()
            && let Some(resolve) = resolve.as_ref()
        {
            resolve.trace(tracer);
        }
    }
}

enum OwnedBinding {
    Buffer {
        binding: u32,
        buffer:  wgpu::Buffer,
        offset:  u64,
        size:    Option<NonZeroU64>,
    },
    View {
        binding: u32,
        view:    wgpu::TextureView,
    },
    Sampler {
        binding: u32,
        sampler: wgpu::Sampler,
    },
}

impl GPUDevice<'_> {
    fn ensure_alive(&self, ctx: &Ctx<'_>) -> Result<()> {
        if self.destroyed.get() {
            Err(invalid_state(ctx, "GPUDevice is destroyed"))
        } else {
            Ok(())
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUDevice<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get)]
    pub fn features(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        feature_set(&ctx, self.device.features())
    }

    #[qjs(get)]
    pub fn limits(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        limits_object(&ctx, &self.device.limits())
    }

    #[qjs(get)]
    pub fn adapter_info(&self) -> GPUAdapterInfo { self.adapter_info.clone() }

    #[qjs(get)]
    pub fn queue(&self) -> Class<'js, GPUQueue> { self.queue.clone() }

    #[qjs(get)]
    pub fn lost(&self) -> Promise<'js> { self.lost.clone() }

    pub fn destroy(&self, ctx: Ctx<'js>) -> Result<()> {
        if !self.destroyed.replace(true) {
            self.device.destroy();
            if let Some(resolve) = self.lost_resolve.borrow_mut().take() {
                let info = Class::instance(ctx.clone(), GPUDeviceLostInfo {
                    reason:  "destroyed",
                    message: "GPUDevice.destroy() was called".into(),
                })?;
                resolve.call::<_, ()>((info,))?;
            }
        }
        Ok(())
    }

    pub fn create_texture(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<texture::GPUTexture> {
        self.ensure_alive(&ctx)?;
        texture::GPUTexture::from_descriptor(&self.device, descriptor, &ctx)
    }

    pub fn create_sampler(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<texture::GPUSampler> {
        self.ensure_alive(&ctx)?;
        texture::create_sampler(&self.device, descriptor, &ctx)
    }

    pub fn create_query_set(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<query::GPUQuerySet> {
        self.ensure_alive(&ctx)?;
        query::GPUQuerySet::from_descriptor(&self.device, descriptor, &ctx)
    }

    pub fn create_buffer(&self, descriptor: Object<'js>, ctx: Ctx<'js>) -> Result<GPUBuffer> {
        self.ensure_alive(&ctx)?;
        let label = label(&descriptor)?;
        let size = descriptor.get::<_, JsU64>("size")?.0;
        let usage_bits = descriptor.get::<_, JsU32>("usage")?.0;
        if usage_bits & !WEBGPU_BUFFER_USAGE_MASK != 0 {
            return Err(type_error(&ctx, "usage contains native-only wgpu flags"));
        }
        let usage = wgpu::BufferUsages::from_bits(usage_bits)
            .ok_or_else(|| type_error(&ctx, "usage is not a valid GPUBufferUsage mask"))?;
        let mapped_at_creation = descriptor
            .get::<_, Option<bool>>("mappedAtCreation")?
            .unwrap_or_default();
        if mapped_at_creation && !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            return Err(Exception::throw_range(
                &ctx,
                "mappedAtCreation buffer size must be a multiple of 4",
            ));
        }
        let inner = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            size,
            usage,
            mapped_at_creation,
        });
        Ok(GPUBuffer {
            device: self.device.clone(),
            inner,
            label: Rc::new(RefCell::new(label)),
            size,
            state: Rc::new(RefCell::new(if mapped_at_creation {
                BufferMapState::Mapped {
                    kind:  MappingKind::Write,
                    range: 0..size,
                    views: Vec::new(),
                }
            } else {
                BufferMapState::Unmapped
            })),
            usage,
        })
    }

    pub fn create_shader_module(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUShaderModule> {
        self.ensure_alive(&ctx)?;
        let label = label(&descriptor)?;
        let code: String = descriptor.get("code")?;
        Ok(GPUShaderModule {
            inner: self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label:  (!label.is_empty()).then_some(label.as_str()),
                    source: wgpu::ShaderSource::Wgsl(code.into()),
                }),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn create_bind_group_layout(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUBindGroupLayout> {
        self.ensure_alive(&ctx)?;
        let label = label(&descriptor)?;
        let entries = array_value(&descriptor, "entries", &ctx)?;
        let mut native = Vec::with_capacity(entries.len());
        for entry in entries.iter::<Object>() {
            let entry = entry?;
            let binding = entry.get::<_, JsU32>("binding")?.0;
            let visibility_bits = entry.get::<_, JsU32>("visibility")?.0;
            let visibility = wgpu::ShaderStages::from_bits(visibility_bits)
                .filter(|stages| {
                    stages
                        .difference(
                            wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                        )
                        .is_empty()
                })
                .ok_or_else(|| type_error(&ctx, "visibility is not a valid GPUShaderStage mask"))?;
            if entry.get::<_, Option<JsU32>>("count")?.is_some() {
                return Err(type_error(&ctx, "binding arrays are not implemented"));
            }
            let ty = bind_group_layout_type(&entry, &ctx)?;
            native.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty,
                count: None,
            });
        }
        Ok(GPUBindGroupLayout {
            inner: self
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label:   (!label.is_empty()).then_some(label.as_str()),
                    entries: &native,
                }),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn create_pipeline_layout(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUPipelineLayout> {
        self.ensure_alive(&ctx)?;
        let label = label(&descriptor)?;
        let layouts = array_value(&descriptor, "bindGroupLayouts", &ctx)?;
        let owned = layouts
            .iter::<Class<GPUBindGroupLayout>>()
            .map(|layout| Ok(layout?.borrow().inner.clone()))
            .collect::<Result<Vec<_>>>()?;
        let borrowed = owned.iter().map(Some).collect::<Vec<_>>();
        Ok(GPUPipelineLayout {
            inner: self
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label:              (!label.is_empty()).then_some(label.as_str()),
                    bind_group_layouts: &borrowed,
                    immediate_size:     0,
                }),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn create_bind_group(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUBindGroup> {
        self.ensure_alive(&ctx)?;
        let label = label(&descriptor)?;
        let layout = class_value::<GPUBindGroupLayout>(&descriptor, "layout", &ctx)?
            .borrow()
            .inner
            .clone();
        let entries = array_value(&descriptor, "entries", &ctx)?;
        let mut resources = Vec::with_capacity(entries.len());
        for entry in entries.iter::<Object>() {
            let entry = entry?;
            let binding = entry.get::<_, JsU32>("binding")?.0;
            let resource: Value = entry.get("resource")?;
            if let Ok(view) = Class::<texture::GPUTextureView>::from_js(&ctx, resource.clone()) {
                resources.push(OwnedBinding::View {
                    binding,
                    view: view.borrow().inner.clone(),
                });
                continue;
            }
            if let Ok(sampler) = Class::<texture::GPUSampler>::from_js(&ctx, resource.clone()) {
                resources.push(OwnedBinding::Sampler {
                    binding,
                    sampler: sampler.borrow().inner.clone(),
                });
                continue;
            }
            let resource = Object::from_js(&ctx, resource)
                .map_err(|_error| type_error(&ctx, "bind group resource is not a GPU binding"))?;
            let buffer = class_value::<GPUBuffer>(&resource, "buffer", &ctx)?;
            let buffer = buffer.borrow();
            buffer.ensure_usable(&ctx)?;
            let offset = resource
                .get::<_, Option<JsU64>>("offset")?
                .map_or(0, |value| value.0);
            let size = resource
                .get::<_, Option<JsU64>>("size")?
                .map(|value| {
                    NonZeroU64::new(value.0).ok_or_else(|| {
                        type_error(&ctx, "buffer binding size must be greater than zero")
                    })
                })
                .transpose()?;
            let end = size.map_or(buffer.size, |size| offset.saturating_add(size.get()));
            if offset > buffer.size || end > buffer.size {
                return Err(operation_error(&ctx, "buffer binding is out of bounds"));
            }
            resources.push(OwnedBinding::Buffer {
                binding,
                buffer: buffer.inner.clone(),
                offset,
                size,
            });
        }
        let native = resources
            .iter()
            .map(|resource| {
                match resource {
                    OwnedBinding::Buffer {
                        binding,
                        buffer,
                        offset,
                        size,
                    } => {
                        wgpu::BindGroupEntry {
                            binding:  *binding,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer,
                                offset: *offset,
                                size: *size,
                            }),
                        }
                    }
                    OwnedBinding::View { binding, view } => {
                        wgpu::BindGroupEntry {
                            binding:  *binding,
                            resource: wgpu::BindingResource::TextureView(view),
                        }
                    }
                    OwnedBinding::Sampler { binding, sampler } => {
                        wgpu::BindGroupEntry {
                            binding:  *binding,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        }
                    }
                }
            })
            .collect::<Vec<_>>();
        Ok(GPUBindGroup {
            inner: self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label:   (!label.is_empty()).then_some(label.as_str()),
                layout:  &layout,
                entries: &native,
            }),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn create_compute_pipeline(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUComputePipeline> {
        self.ensure_alive(&ctx)?;
        create_compute_pipeline(self, descriptor, &ctx)
    }

    #[expect(
        clippy::unused_async,
        reason = "WebGPU requires a Promise-returning variant"
    )]
    pub async fn create_compute_pipeline_async(
        self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<GPUComputePipeline> {
        self.create_compute_pipeline(descriptor, ctx)
    }

    pub fn create_render_pipeline(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderPipeline> {
        self.ensure_alive(&ctx)?;
        render::create_pipeline(&self.device, descriptor, &ctx)
    }

    #[expect(
        clippy::unused_async,
        reason = "WebGPU requires a Promise-returning variant"
    )]
    pub async fn create_render_pipeline_async(
        self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderPipeline> {
        self.create_render_pipeline(descriptor, ctx)
    }

    pub fn create_render_bundle_encoder(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderBundleEncoder> {
        self.ensure_alive(&ctx)?;
        render::new_bundle_encoder(&self.device, descriptor, &ctx)
    }

    pub fn create_command_encoder(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUCommandEncoder> {
        self.ensure_alive(&ctx)?;
        let label = descriptor
            .0
            .flatten()
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        Ok(GPUCommandEncoder {
            label: Rc::new(RefCell::new(label.clone())),
            state: Rc::new(RefCell::new(EncoderState {
                encoder:     Some(self.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: (!label.is_empty()).then_some(label.as_str()),
                    },
                )),
                open_passes: 0,
            })),
        })
    }

    pub fn push_error_scope(&self, filter: String, ctx: Ctx<'_>) -> Result<()> {
        self.ensure_alive(&ctx)?;
        let filter = match filter.as_str() {
            "validation" => GPUErrorKind::Validation,
            "out-of-memory" => GPUErrorKind::OutOfMemory,
            "internal" => GPUErrorKind::Internal,
            _ => return Err(type_error(&ctx, format!("invalid GPUErrorFilter {filter}"))),
        };
        self.errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scopes
            .push(ErrorScopeState {
                error: None,
                filter,
            });
        Ok(())
    }

    pub fn pop_error_scope(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let (promise, resolve, reject) = ctx.promise()?;
        if self.destroyed.get() {
            let error: Value = den_util::construct(
                &ctx,
                "DOMException",
                ("GPUDevice is destroyed", "InvalidStateError"),
            )?;
            reject.call::<_, ()>((error,))?;
            return Ok(promise);
        }
        let scope = self
            .errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scopes
            .pop();
        match scope {
            None => {
                let error: Value = den_util::construct(
                    &ctx,
                    "DOMException",
                    ("GPU error scope stack is empty", "OperationError"),
                )?;
                reject.call::<_, ()>((error,))?;
            }
            Some(ErrorScopeState { error: None, .. }) => {
                resolve.call::<_, ()>((Value::new_null(ctx.clone()),))?;
            }
            Some(ErrorScopeState {
                error: Some(error), ..
            }) => {
                resolve.call::<_, ()>((gpu_error_value(&ctx, error)?,))?;
            }
        }
        Ok(promise)
    }
}

fn create_compute_pipeline<'js>(
    device: &GPUDevice<'_>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<GPUComputePipeline> {
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
    let compute: Object = descriptor.get("compute")?;
    let module = class_value::<GPUShaderModule>(&compute, "module", ctx)?
        .borrow()
        .inner
        .clone();
    let entry_point = compute.get::<_, Option<String>>("entryPoint")?;
    let constants = format::pipeline_constants(compute.get("constants")?, ctx)?;
    let constant_pairs = constants
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect::<Vec<_>>();
    Ok(GPUComputePipeline {
        inner: device
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label:               (!label.is_empty()).then_some(label.as_str()),
                layout:              layout.as_ref(),
                module:              &module,
                entry_point:         entry_point.as_deref(),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants:                        &constant_pairs,
                    zero_initialize_workgroup_memory: true,
                },
                cache:               None,
            }),
        label: Rc::new(RefCell::new(label)),
    })
}

fn bind_group_layout_type(entry: &Object<'_>, ctx: &Ctx<'_>) -> Result<wgpu::BindingType> {
    if let Some(buffer) = entry.get::<_, Option<Object>>("buffer")? {
        let ty = match buffer
            .get::<_, Option<String>>("type")?
            .as_deref()
            .unwrap_or("uniform")
        {
            "uniform" => wgpu::BufferBindingType::Uniform,
            "storage" => wgpu::BufferBindingType::Storage { read_only: false },
            "read-only-storage" => wgpu::BufferBindingType::Storage { read_only: true },
            value => {
                return Err(type_error(
                    ctx,
                    format!("invalid GPUBufferBindingType {value}"),
                ));
            }
        };
        return Ok(wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: buffer
                .get::<_, Option<bool>>("hasDynamicOffset")?
                .unwrap_or_default(),
            min_binding_size: buffer
                .get::<_, Option<JsU64>>("minBindingSize")?
                .and_then(|value| NonZeroU64::new(value.0)),
        });
    }
    if let Some(sampler) = entry.get::<_, Option<Object>>("sampler")? {
        let name = sampler
            .get::<_, Option<String>>("type")?
            .unwrap_or_else(|| "filtering".into());
        return Ok(wgpu::BindingType::Sampler(format::sampler_binding_type(
            &name, ctx,
        )?));
    }
    if let Some(texture) = entry.get::<_, Option<Object>>("texture")? {
        return Ok(wgpu::BindingType::Texture {
            sample_type:    format::sample_type(
                texture.get::<_, Option<String>>("sampleType")?.as_deref(),
                ctx,
            )?,
            view_dimension: format::view_dimension_or(
                texture
                    .get::<_, Option<String>>("viewDimension")?
                    .as_deref(),
                wgpu::TextureViewDimension::D2,
                ctx,
            )?,
            multisampled:   texture
                .get::<_, Option<bool>>("multisampled")?
                .unwrap_or_default(),
        });
    }
    if let Some(storage) = entry.get::<_, Option<Object>>("storageTexture")? {
        return Ok(wgpu::BindingType::StorageTexture {
            access:         format::storage_access(
                storage.get::<_, Option<String>>("access")?.as_deref(),
                ctx,
            )?,
            format:         format::texture_format(&storage.get::<_, String>("format")?, ctx)?,
            view_dimension: format::view_dimension_or(
                storage
                    .get::<_, Option<String>>("viewDimension")?
                    .as_deref(),
                wgpu::TextureViewDimension::D2,
                ctx,
            )?,
        });
    }
    if entry.get::<_, Option<Object>>("externalTexture")?.is_some() {
        return Err(type_error(ctx, "GPUExternalTexture is not implemented"));
    }
    Err(type_error(
        ctx,
        "bind group layout entry must specify buffer, sampler, texture, or storageTexture",
    ))
}

pub(crate) fn dynamic_offsets<'js>(
    ctx: &Ctx<'js>, offsets: Opt<Value<'js>>, start: Opt<Option<JsU64>>, length: Opt<Option<JsU64>>,
) -> Result<Vec<u32>> {
    let Some(value) = offsets.0 else {
        return Ok(Vec::new());
    };
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    if let Ok(array) = Array::from_js(ctx, value.clone()) {
        return array
            .iter::<JsU32>()
            .map(|value| Ok(value?.0))
            .collect::<Result<Vec<_>>>();
    }
    let bytes = BufferSource::from_js(ctx, value)?.into_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(type_error(
            ctx,
            "dynamic offsets must be a multiple of 4 bytes",
        ));
    }
    let values = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    let start = usize::try_from(start.0.flatten().map_or(0, |value| value.0))
        .map_err(|_error| type_error(ctx, "dynamicOffsetsDataStart is too large"))?;
    let count = match length.0.flatten() {
        Some(value) => {
            usize::try_from(value.0)
                .map_err(|_error| type_error(ctx, "dynamicOffsetsDataLength is too large"))?
        }
        None => values.len().saturating_sub(start),
    };
    values
        .get(start..start.saturating_add(count))
        .map(<[u32]>::to_vec)
        .ok_or_else(|| type_error(ctx, "dynamic offset window is out of bounds"))
}

#[derive(Clone, Copy)]
enum MappingKind {
    Read,
    Write,
}

struct MappedView {
    buffer: Persistent<ArrayBuffer<'static>>,
    range:  Range<u64>,
}

enum BufferMapState {
    Unmapped,
    Pending,
    Mapped {
        kind:  MappingKind,
        range: Range<u64>,
        views: Vec<MappedView>,
    },
    Destroyed,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUBuffer")]
pub struct GPUBuffer {
    #[qjs(skip_trace)]
    device:           wgpu::Device,
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::Buffer,
    #[qjs(skip_trace)]
    label:            Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) size:  u64,
    #[qjs(skip_trace)]
    state:            Rc<RefCell<BufferMapState>>,
    #[qjs(skip_trace)]
    usage:            wgpu::BufferUsages,
}

impl GPUBuffer {
    fn ensure_usable(&self, ctx: &Ctx<'_>) -> Result<()> {
        if matches!(*self.state.borrow(), BufferMapState::Destroyed) {
            Err(invalid_state(ctx, "GPUBuffer is destroyed"))
        } else {
            Ok(())
        }
    }

    fn ensure_unmapped(&self, ctx: &Ctx<'_>) -> Result<()> {
        self.ensure_usable(ctx)?;
        if matches!(*self.state.borrow(), BufferMapState::Unmapped) {
            Ok(())
        } else {
            Err(invalid_state(ctx, "GPUBuffer is mapped"))
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUBuffer {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get)]
    pub const fn size(&self) -> u64 { self.size }

    #[qjs(get)]
    pub const fn usage(&self) -> u32 { self.usage.bits() }

    #[qjs(get)]
    pub fn map_state(&self) -> &'static str {
        match *self.state.borrow() {
            BufferMapState::Unmapped | BufferMapState::Destroyed => "unmapped",
            BufferMapState::Pending => "pending",
            BufferMapState::Mapped { .. } => "mapped",
        }
    }

    pub async fn map_async(
        self, mode: JsU32, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        self.ensure_unmapped(&ctx)?;
        let kind = match mode.0 {
            MAP_READ if self.usage.contains(wgpu::BufferUsages::MAP_READ) => MappingKind::Read,
            MAP_WRITE if self.usage.contains(wgpu::BufferUsages::MAP_WRITE) => MappingKind::Write,
            MAP_READ | MAP_WRITE => {
                return Err(operation_error(
                    &ctx,
                    "buffer usage does not permit this map mode",
                ));
            }
            _ => {
                return Err(type_error(
                    &ctx,
                    "mode must be GPUMapMode.READ or GPUMapMode.WRITE",
                ));
            }
        };
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size
            .0
            .flatten()
            .map_or_else(|| self.size.saturating_sub(offset), |value| value.0);
        let end = offset
            .checked_add(size)
            .ok_or_else(|| operation_error(&ctx, "mapped range overflows"))?;
        if !offset.is_multiple_of(wgpu::MAP_ALIGNMENT)
            || size == 0
            || !size.is_multiple_of(4)
            || end > self.size
        {
            return Err(operation_error(
                &ctx,
                "mapped range must be in bounds, non-empty, 8-byte aligned, and a multiple of 4",
            ));
        }
        *self.state.borrow_mut() = BufferMapState::Pending;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.inner.map_async(
            match kind {
                MappingKind::Read => wgpu::MapMode::Read,
                MappingKind::Write => wgpu::MapMode::Write,
            },
            offset..end,
            move |result| {
                let _ = sender.send(result);
            },
        );
        let device = self.device.clone();
        let outcome =
            match tokio::task::spawn_blocking(move || GpuPoll::until(device, receiver)).await {
                Err(join) => Err(join.to_string()),
                Ok(Err(wait)) => Err(wait),
                Ok(Ok(Err(mapped))) => Err(mapped.to_string()),
                Ok(Ok(Ok(()))) => Ok(()),
            };
        if !matches!(*self.state.borrow(), BufferMapState::Pending) {
            return Err(operation_error(&ctx, "buffer mapping was cancelled"));
        }
        match outcome {
            Ok(()) => {
                *self.state.borrow_mut() = BufferMapState::Mapped {
                    kind,
                    range: offset..end,
                    views: Vec::new(),
                };
                Ok(())
            }
            Err(message) => {
                *self.state.borrow_mut() = BufferMapState::Unmapped;
                Err(operation_error(&ctx, message))
            }
        }
    }

    pub fn get_mapped_range<'js>(
        &self, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<ArrayBuffer<'js>> {
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let mut state = self.state.borrow_mut();
        let BufferMapState::Mapped {
            range: mapped,
            views,
            ..
        } = &mut *state
        else {
            return Err(invalid_state(&ctx, "GPUBuffer is not mapped"));
        };
        let size = size
            .0
            .flatten()
            .map_or_else(|| mapped.end.saturating_sub(offset), |value| value.0);
        let end = offset
            .checked_add(size)
            .ok_or_else(|| operation_error(&ctx, "mapped range overflows"))?;
        let range = offset..end;
        if size == 0
            || !offset.is_multiple_of(wgpu::MAP_ALIGNMENT)
            || !size.is_multiple_of(4)
            || range.start < mapped.start
            || range.end > mapped.end
        {
            return Err(operation_error(&ctx, "requested mapped range is invalid"));
        }
        if views
            .iter()
            .any(|view| range.start < view.range.end && view.range.start < range.end)
        {
            return Err(operation_error(
                &ctx,
                "requested mapped range overlaps an existing range",
            ));
        }
        let native = self
            .inner
            .get_mapped_range(range.clone())
            .map_err(|error| operation_error(&ctx, error.to_string()))?;
        let buffer = ArrayBuffer::new_copy(ctx.clone(), native.as_ref())?;
        drop(native);
        views.push(MappedView {
            buffer: Persistent::save(&ctx, buffer.clone()),
            range,
        });
        Ok(buffer)
    }

    pub fn unmap(&self, ctx: Ctx<'_>) -> Result<()> {
        let state = std::mem::replace(&mut *self.state.borrow_mut(), BufferMapState::Unmapped);
        match state {
            BufferMapState::Mapped { kind, views, .. } => {
                for view in views {
                    let mut array = view.buffer.restore(&ctx)?;
                    if matches!(kind, MappingKind::Write)
                        && let Some(bytes) = array.as_bytes().map(<[u8]>::to_vec)
                    {
                        self.inner
                            .get_mapped_range_mut(view.range)
                            .map_err(|error| operation_error(&ctx, error.to_string()))?
                            .copy_from_slice(&bytes);
                    }
                    array.detach();
                }
                self.inner.unmap();
                Ok(())
            }
            BufferMapState::Pending => {
                self.inner.unmap();
                Ok(())
            }
            BufferMapState::Unmapped => Err(invalid_state(&ctx, "GPUBuffer is not mapped")),
            BufferMapState::Destroyed => {
                *self.state.borrow_mut() = BufferMapState::Destroyed;
                Ok(())
            }
        }
    }

    pub fn destroy(&self, ctx: Ctx<'_>) -> Result<()> {
        if !matches!(*self.state.borrow(), BufferMapState::Destroyed) {
            if !matches!(*self.state.borrow(), BufferMapState::Unmapped) {
                self.unmap(ctx)?;
            }
            self.inner.destroy();
            *self.state.borrow_mut() = BufferMapState::Destroyed;
        }
        Ok(())
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUQueue")]
pub struct GPUQueue {
    #[qjs(skip_trace)]
    device: Option<wgpu::Device>,
    #[qjs(skip_trace)]
    inner:  wgpu::Queue,
    #[qjs(skip_trace)]
    label:  Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUQueue {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn write_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, buffer_offset: JsU64, data: Value<'js>,
        data_offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        buffer.ensure_unmapped(&ctx)?;
        if !buffer.usage.contains(wgpu::BufferUsages::COPY_DST) {
            return Err(operation_error(&ctx, "buffer does not have COPY_DST usage"));
        }
        if !buffer_offset.0.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            return Err(operation_error(
                &ctx,
                "bufferOffset must be a multiple of 4",
            ));
        }
        let element_size = typed_array_element_size(&ctx, &data)?;
        let bytes = BufferSource::from_js(&ctx, data)?.into_bytes();
        let start = data_offset
            .0
            .flatten()
            .map_or(0, |value| value.0)
            .checked_mul(element_size)
            .ok_or_else(|| operation_error(&ctx, "dataOffset overflows"))?;
        let length = match size.0.flatten() {
            Some(value) => {
                value
                    .0
                    .checked_mul(element_size)
                    .ok_or_else(|| operation_error(&ctx, "size overflows"))?
            }
            None => (bytes.len() as u64).saturating_sub(start),
        };
        let end = start
            .checked_add(length)
            .ok_or_else(|| operation_error(&ctx, "data range overflows"))?;
        if end > bytes.len() as u64 || !length.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            return Err(operation_error(
                &ctx,
                "data range must be in bounds and a multiple of 4 bytes",
            ));
        }
        if buffer_offset
            .0
            .checked_add(length)
            .is_none_or(|end| end > buffer.size)
        {
            return Err(operation_error(
                &ctx,
                "destination buffer range is out of bounds",
            ));
        }
        let data = bytes
            .get(start as usize..end as usize)
            .ok_or_else(|| operation_error(&ctx, "data range is out of bounds"))?;
        self.inner
            .write_buffer(&buffer.inner, buffer_offset.0, data);
        Ok(())
    }

    pub fn write_texture<'js>(
        &self, destination: Object<'js>, data: Value<'js>, data_layout: Object<'js>,
        size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(destination, &ctx)?;
        let layout = texture::texel_copy_layout(&data_layout)?;
        let size = format::extent3d(size, &ctx)?;
        let bytes = BufferSource::from_js(&ctx, data)?.into_bytes();
        let texture = texture.borrow();
        self.inner.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.inner,
                mip_level,
                origin,
                aspect,
            },
            &bytes,
            layout,
            size,
        );
        Ok(())
    }

    pub fn submit<'js>(&self, command_buffers: Array<'js>, ctx: Ctx<'js>) -> Result<()> {
        let buffers = command_buffers
            .iter::<Class<GPUCommandBuffer>>()
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::with_capacity(buffers.len());
        let mut native = Vec::with_capacity(buffers.len());
        for buffer in &buffers {
            let state = buffer.borrow().inner.clone();
            if !seen.insert(Rc::as_ptr(&state)) {
                return Err(invalid_state(
                    &ctx,
                    "GPUCommandBuffer was already submitted",
                ));
            }
            let command = state
                .borrow_mut()
                .take()
                .ok_or_else(|| invalid_state(&ctx, "GPUCommandBuffer was already submitted"))?;
            native.push(command);
        }
        self.inner.submit(native);
        Ok(())
    }

    pub async fn on_submitted_work_done(self, ctx: Ctx<'_>) -> Result<()> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.inner.on_submitted_work_done(move || {
            let _ = sender.send(());
        });
        let device = self
            .device
            .clone()
            .ok_or_else(|| invalid_state(&ctx, "GPUQueue has no device"))?;
        tokio::task::spawn_blocking(move || GpuPoll::until(device, receiver))
            .await
            .map_err(|error| operation_error(&ctx, error.to_string()))?
            .map_err(|error| operation_error(&ctx, error))?;
        Ok(())
    }
}

fn typed_array_element_size(ctx: &Ctx<'_>, value: &Value<'_>) -> Result<u64> {
    if unsafe { rquickjs::qjs::JS_GetTypedArrayType(value.as_raw()) } < 0 {
        return Ok(1);
    }
    let mut offset = MaybeUninit::<rquickjs::qjs::size_t>::uninit();
    let mut length = MaybeUninit::<rquickjs::qjs::size_t>::uninit();
    let mut element_size = MaybeUninit::<rquickjs::qjs::size_t>::uninit();
    // SAFETY: the native typed-array brand was checked above; QuickJS
    // initializes all three metadata outputs and returns one owned reference.
    let raw = unsafe {
        rquickjs::qjs::JS_GetTypedArrayBuffer(
            ctx.as_raw().as_ptr(),
            value.as_raw(),
            offset.as_mut_ptr(),
            length.as_mut_ptr(),
            element_size.as_mut_ptr(),
        )
    };
    if unsafe { rquickjs::qjs::JS_IsException(raw) } {
        return Err(Error::Exception);
    }
    // SAFETY: JS_GetTypedArrayBuffer transferred one owned JSValue reference.
    drop(unsafe { Value::from_raw(ctx.clone(), raw) });
    // `size_t` is `u64` on 64-bit and `u32` on 32-bit; the conversion is a
    // no-op only on the hosts we currently ship.
    #[expect(
        clippy::useless_conversion,
        reason = "qjs size_t width is pointer-sized"
    )]
    u64::try_from(unsafe { element_size.assume_init() })
        .map_err(|_error| Exception::throw_range(ctx, "typed-array element size is too large"))
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUShaderModule")]
pub struct GPUShaderModule {
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::ShaderModule,
    #[qjs(skip_trace)]
    label:            Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUShaderModule {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub async fn get_compilation_info(self, ctx: Ctx<'_>) -> Result<Object<'_>> {
        let info = self.inner.get_compilation_info().await;
        let messages = Array::new(ctx.clone())?;
        for (index, message) in info.messages.into_iter().enumerate() {
            let object = Object::new(ctx.clone())?;
            object.set("message", message.message)?;
            object.set("type", match message.message_type {
                wgpu::CompilationMessageType::Error => "error",
                wgpu::CompilationMessageType::Warning => "warning",
                wgpu::CompilationMessageType::Info => "info",
            })?;
            if let Some(location) = message.location {
                object.set("lineNum", location.line_number)?;
                object.set("linePos", location.line_position)?;
                object.set("offset", location.offset)?;
                object.set("length", location.length)?;
            } else {
                object.set("lineNum", 0)?;
                object.set("linePos", 0)?;
                object.set("offset", 0)?;
                object.set("length", 0)?;
            }
            messages.set(index, object)?;
        }
        let object = Object::new(ctx)?;
        object.set("messages", messages)?;
        Ok(object)
    }
}

macro_rules! labeled_resource {
    ($name:ident, $js_name:literal, $inner:ty) => {
        #[derive(Clone, Trace, JsLifetime)]
        #[rquickjs::class(rename = $js_name)]
        pub struct $name {
            #[qjs(skip_trace)]
            pub(crate) inner: $inner,
            #[qjs(skip_trace)]
            pub(crate) label: Rc<RefCell<String>>,
        }

        #[rquickjs::methods]
        impl $name {
            #[qjs(constructor)]
            pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

            #[qjs(get)]
            pub fn label(&self) -> String { self.label.borrow().clone() }

            #[qjs(set, rename = "label")]
            pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
        }
    };
}

labeled_resource!(
    GPUBindGroupLayout,
    "GPUBindGroupLayout",
    wgpu::BindGroupLayout
);
labeled_resource!(GPUPipelineLayout, "GPUPipelineLayout", wgpu::PipelineLayout);
labeled_resource!(GPUBindGroup, "GPUBindGroup", wgpu::BindGroup);

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePipeline")]
pub struct GPUComputePipeline {
    #[qjs(skip_trace)]
    inner: wgpu::ComputePipeline,
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUComputePipeline {
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

pub(crate) struct EncoderState {
    pub(crate) encoder:     Option<wgpu::CommandEncoder>,
    pub(crate) open_passes: usize,
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUCommandEncoder")]
pub struct GPUCommandEncoder {
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state: Rc<RefCell<EncoderState>>,
}

impl GPUCommandEncoder {
    fn with_encoder<T>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::CommandEncoder) -> T,
    ) -> Result<T> {
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            return Err(invalid_state(ctx, "GPUCommandEncoder has an open pass"));
        }
        state
            .encoder
            .as_mut()
            .map(operation)
            .ok_or_else(|| invalid_state(ctx, "GPUCommandEncoder is already finished"))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUCommandEncoder {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn begin_compute_pass<'js>(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUComputePassEncoder> {
        let descriptor = descriptor.0.flatten();
        let label = descriptor
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            return Err(invalid_state(
                &ctx,
                "GPUCommandEncoder already has an open pass",
            ));
        }
        let encoder = state
            .encoder
            .as_mut()
            .ok_or_else(|| invalid_state(&ctx, "GPUCommandEncoder is already finished"))?;
        let timestamp_query;
        let timestamp_writes = match descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Object>>("timestampWrites"))
            .transpose()?
            .flatten()
        {
            Some(writes) => {
                timestamp_query = class_value::<query::GPUQuerySet>(&writes, "querySet", &ctx)?
                    .borrow()
                    .inner
                    .clone();
                Some(wgpu::ComputePassTimestampWrites {
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
        let pass = encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
                timestamp_writes,
            })
            .forget_lifetime();
        state.open_passes = 1;
        drop(state);
        Ok(GPUComputePassEncoder {
            label: Rc::new(RefCell::new(label)),
            state: Rc::new(RefCell::new(ComputePassState {
                parent: self.state.clone(),
                pass:   Some(pass),
            })),
        })
    }

    pub fn begin_render_pass<'js>(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderPassEncoder> {
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            return Err(invalid_state(
                &ctx,
                "GPUCommandEncoder already has an open pass",
            ));
        }
        let encoder = state
            .encoder
            .as_mut()
            .ok_or_else(|| invalid_state(&ctx, "GPUCommandEncoder is already finished"))?;
        let (label, pass) = render::begin_render_pass(encoder, descriptor, &ctx)?;
        state.open_passes = 1;
        drop(state);
        Ok(render::GPURenderPassEncoder::new(
            label,
            self.state.clone(),
            pass,
        ))
    }

    pub fn copy_buffer_to_buffer<'js>(
        &self, source: Class<'js, GPUBuffer>, source_offset_or_destination: Value<'js>,
        destination_or_size: Opt<Value<'js>>, destination_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (source_offset, destination, destination_offset, size) = if let Ok(destination) =
            Class::<GPUBuffer>::from_js(&ctx, source_offset_or_destination.clone())
        {
            let size = destination_or_size
                .0
                .filter(|value| !value.type_of().is_void())
                .map(|value| JsU64::from_js(&ctx, value))
                .transpose()?;
            (0, destination, 0, size)
        } else {
            let source_offset = JsU64::from_js(&ctx, source_offset_or_destination)?;
            let destination = destination_or_size
                .0
                .ok_or_else(|| type_error(&ctx, "copyBufferToBuffer destination is required"))?;
            let destination = Class::<GPUBuffer>::from_js(&ctx, destination)?;
            (
                source_offset.0,
                destination,
                destination_offset.0.flatten().map_or(0, |value| value.0),
                size.0.flatten(),
            )
        };
        let source = source.borrow();
        let destination = destination.borrow();
        source.ensure_unmapped(&ctx)?;
        destination.ensure_unmapped(&ctx)?;
        if !source.usage.contains(wgpu::BufferUsages::COPY_SRC)
            || !destination.usage.contains(wgpu::BufferUsages::COPY_DST)
        {
            return Err(operation_error(
                &ctx,
                "copy buffers have incompatible usage",
            ));
        }
        let size = size.map(|value| value.0);
        if !source_offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || !destination_offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || size.is_some_and(|size| !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT))
        {
            return Err(operation_error(
                &ctx,
                "copy offsets and size must be multiples of 4",
            ));
        }
        if let Some(size) = size
            && (source_offset
                .checked_add(size)
                .is_none_or(|end| end > source.size)
                || destination_offset
                    .checked_add(size)
                    .is_none_or(|end| end > destination.size))
        {
            return Err(operation_error(&ctx, "copy range is out of bounds"));
        }
        self.with_encoder(&ctx, |encoder| {
            encoder.copy_buffer_to_buffer(
                &source.inner,
                source_offset,
                &destination.inner,
                destination_offset,
                size,
            );
        })
    }

    pub fn copy_buffer_to_texture<'js>(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (buffer, layout) = texture::texel_copy_buffer(source, &ctx)?;
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let buffer = buffer.borrow();
        let texture = texture.borrow();
        buffer.ensure_unmapped(&ctx)?;
        self.with_encoder(&ctx, |encoder| {
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer.inner,
                    layout,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &texture.inner,
                    mip_level,
                    origin,
                    aspect,
                },
                size,
            );
        })
    }

    pub fn copy_texture_to_buffer<'js>(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(source, &ctx)?;
        let (buffer, layout) = texture::texel_copy_buffer(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let buffer = buffer.borrow();
        let texture = texture.borrow();
        buffer.ensure_unmapped(&ctx)?;
        self.with_encoder(&ctx, |encoder| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture.inner,
                    mip_level,
                    origin,
                    aspect,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer.inner,
                    layout,
                },
                size,
            );
        })
    }

    pub fn copy_texture_to_texture<'js>(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (source_texture, source_mip, source_origin, source_aspect) =
            texture::texel_copy_texture(source, &ctx)?;
        let (destination_texture, destination_mip, destination_origin, destination_aspect) =
            texture::texel_copy_texture(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let source_texture = source_texture.borrow();
        let destination_texture = destination_texture.borrow();
        self.with_encoder(&ctx, |encoder| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture:   &source_texture.inner,
                    mip_level: source_mip,
                    origin:    source_origin,
                    aspect:    source_aspect,
                },
                wgpu::TexelCopyTextureInfo {
                    texture:   &destination_texture.inner,
                    mip_level: destination_mip,
                    origin:    destination_origin,
                    aspect:    destination_aspect,
                },
                size,
            );
        })
    }

    pub fn resolve_query_set<'js>(
        &self, query_set: Class<'js, query::GPUQuerySet>, first_query: JsU32, query_count: JsU32,
        destination: Class<'js, GPUBuffer>, destination_offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let query_set = query_set.borrow();
        let destination = destination.borrow();
        destination.ensure_unmapped(&ctx)?;
        self.with_encoder(&ctx, |encoder| {
            encoder.resolve_query_set(
                &query_set.inner,
                first_query.0..first_query.0 + query_count.0,
                &destination.inner,
                destination_offset.0,
            );
        })
    }

    pub fn clear_buffer<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        buffer.ensure_unmapped(&ctx)?;
        if !buffer.usage.contains(wgpu::BufferUsages::COPY_DST) {
            return Err(operation_error(&ctx, "buffer does not have COPY_DST usage"));
        }
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        if !offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || size.is_some_and(|size| !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT))
            || size
                .unwrap_or_else(|| buffer.size.saturating_sub(offset))
                .checked_add(offset)
                .is_none_or(|end| end > buffer.size)
        {
            return Err(operation_error(&ctx, "clear range is invalid"));
        }
        self.with_encoder(&ctx, |encoder| {
            encoder.clear_buffer(&buffer.inner, offset, size)
        })
    }

    pub fn finish<'js>(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUCommandBuffer> {
        let label = descriptor
            .0
            .flatten()
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            return Err(invalid_state(&ctx, "GPUCommandEncoder has an open pass"));
        }
        let encoder = state
            .encoder
            .take()
            .ok_or_else(|| invalid_state(&ctx, "GPUCommandEncoder is already finished"))?;
        Ok(GPUCommandBuffer {
            inner: Rc::new(RefCell::new(Some(encoder.finish()))),
            label: Rc::new(RefCell::new(label)),
        })
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_encoder(&ctx, |encoder| encoder.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'_>) -> Result<()> {
        self.with_encoder(&ctx, wgpu::CommandEncoder::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_encoder(&ctx, |encoder| encoder.insert_debug_marker(&value))
    }
}

struct ComputePassState {
    parent: Rc<RefCell<EncoderState>>,
    pass:   Option<wgpu::ComputePass<'static>>,
}

impl ComputePassState {
    fn end(&mut self) -> bool {
        let Some(pass) = self.pass.take() else {
            return false;
        };
        drop(pass);
        self.parent.borrow_mut().open_passes -= 1;
        true
    }
}

impl Drop for ComputePassState {
    fn drop(&mut self) { self.end(); }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePassEncoder")]
pub struct GPUComputePassEncoder {
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state: Rc<RefCell<ComputePassState>>,
}

impl GPUComputePassEncoder {
    fn with_pass<T>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::ComputePass<'static>) -> T,
    ) -> Result<T> {
        self.state
            .borrow_mut()
            .pass
            .as_mut()
            .map(operation)
            .ok_or_else(|| invalid_state(ctx, "GPUComputePassEncoder is already ended"))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUComputePassEncoder {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn set_pipeline<'js>(
        &self, pipeline: Class<'js, GPUComputePipeline>, ctx: Ctx<'js>,
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

    pub fn dispatch_workgroups(
        &self, x: JsU32, y: Opt<Option<JsU32>>, z: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        self.with_pass(&ctx, |pass| {
            pass.dispatch_workgroups(
                x.0,
                y.0.flatten().map_or(1, |value| value.0),
                z.0.flatten().map_or(1, |value| value.0),
            );
        })
    }

    pub fn dispatch_workgroups_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        self.with_pass(&ctx, |pass| {
            pass.dispatch_workgroups_indirect(&buffer.inner, offset.0)
        })
    }

    pub fn end(&self, ctx: Ctx<'_>) -> Result<()> {
        if self.state.borrow_mut().end() {
            Ok(())
        } else {
            Err(invalid_state(
                &ctx,
                "GPUComputePassEncoder is already ended",
            ))
        }
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, wgpu::ComputePass::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'_>) -> Result<()> {
        self.with_pass(&ctx, |pass| pass.insert_debug_marker(&value))
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUCommandBuffer")]
pub struct GPUCommandBuffer {
    #[qjs(skip_trace)]
    inner: Rc<RefCell<Option<wgpu::CommandBuffer>>>,
    #[qjs(skip_trace)]
    label: Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPUCommandBuffer {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUError")]
pub struct GPUError {
    #[qjs(get, enumerable, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl GPUError {
    #[qjs(constructor)]
    pub const fn new(message: String) -> Self { Self { message } }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUDeviceLostInfo")]
pub struct GPUDeviceLostInfo {
    #[qjs(get, enumerable, skip_trace)]
    reason:  &'static str,
    #[qjs(get, enumerable, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl GPUDeviceLostInfo {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUValidationError")]
pub struct GPUValidationError {
    #[qjs(get, enumerable, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl GPUValidationError {
    #[qjs(constructor)]
    pub const fn new(message: String) -> Self { Self { message } }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUOutOfMemoryError")]
pub struct GPUOutOfMemoryError {
    #[qjs(get, enumerable, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl GPUOutOfMemoryError {
    #[qjs(constructor)]
    pub const fn new(message: String) -> Self { Self { message } }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUInternalError")]
pub struct GPUInternalError {
    #[qjs(get, enumerable, skip_trace)]
    message: String,
}

#[rquickjs::methods]
impl GPUInternalError {
    #[qjs(constructor)]
    pub const fn new(message: String) -> Self { Self { message } }
}

fn gpu_error_value<'js>(ctx: &Ctx<'js>, error: GPUErrorData) -> Result<Value<'js>> {
    match error.kind {
        GPUErrorKind::Validation => {
            Class::instance(ctx.clone(), GPUValidationError {
                message: error.message,
            })?
            .into_js(ctx)
        }
        GPUErrorKind::OutOfMemory => {
            Class::instance(ctx.clone(), GPUOutOfMemoryError {
                message: error.message,
            })?
            .into_js(ctx)
        }
        GPUErrorKind::Internal => {
            Class::instance(ctx.clone(), GPUInternalError {
                message: error.message,
            })?
            .into_js(ctx)
        }
    }
}

fn constants<'js>(ctx: &Ctx<'js>, values: &[(&str, u32)]) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    for (name, value) in values {
        object.set(*name, *value)?;
    }
    Ok(object)
}

fn install_classes(globals: &Object<'_>) -> Result<()> {
    use den_util::ConstructorInstaller as _;
    if globals.get::<_, Value>("GPU")?.is_function() {
        return Ok(());
    }
    globals.install_constructor::<GPU>(0)?;
    globals.install_constructor::<GPUAdapter>(0)?;
    globals.install_constructor::<GPUAdapterInfo>(0)?;
    globals.install_constructor::<GPUBindGroup>(0)?;
    globals.install_constructor::<GPUBindGroupLayout>(0)?;
    globals.install_constructor::<GPUBuffer>(0)?;
    globals.install_constructor::<GPUCommandBuffer>(0)?;
    globals.install_constructor::<GPUCommandEncoder>(0)?;
    globals.install_constructor::<GPUComputePassEncoder>(0)?;
    globals.install_constructor::<GPUComputePipeline>(0)?;
    globals.install_constructor::<GPUDevice>(0)?;
    globals.install_constructor::<GPUDeviceLostInfo>(0)?;
    globals.install_constructor::<GPUError>(1)?;
    globals.install_constructor::<GPUInternalError>(1)?;
    globals.install_constructor::<GPUOutOfMemoryError>(1)?;
    globals.install_constructor::<GPUPipelineLayout>(0)?;
    globals.install_constructor::<query::GPUQuerySet>(0)?;
    globals.install_constructor::<GPUQueue>(0)?;
    globals.install_constructor::<render::GPURenderBundle>(0)?;
    globals.install_constructor::<render::GPURenderBundleEncoder>(0)?;
    globals.install_constructor::<render::GPURenderPassEncoder>(0)?;
    globals.install_constructor::<render::GPURenderPipeline>(0)?;
    globals.install_constructor::<texture::GPUSampler>(0)?;
    globals.install_constructor::<GPUShaderModule>(0)?;
    globals.install_constructor::<texture::GPUTexture>(0)?;
    globals.install_constructor::<texture::GPUTextureView>(0)?;
    globals.install_constructor::<GPUValidationError>(1)?;
    den_util::inherit::<GPUValidationError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUOutOfMemoryError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUInternalError, GPUError>(globals.ctx())?;
    inherit_global_prototype::<GPUDevice<'_>>(globals.ctx(), "EventTarget")?;
    Ok(())
}

fn inherit_global_prototype<'js, T: rquickjs::class::JsClass<'js>>(
    ctx: &Ctx<'js>, name: &str,
) -> Result<()> {
    let Ok(constructor) = ctx.globals().get::<_, Function>(name) else {
        return Ok(());
    };
    let Ok(super_proto) = constructor.get::<_, Object>("prototype") else {
        return Ok(());
    };
    if let Some(sub) = Class::<T>::prototype(ctx)? {
        sub.set_prototype(Some(&super_proto))?;
    }
    Ok(())
}

const GLOBAL_CLASSES: [&str; 27] = [
    "GPU",
    "GPUAdapter",
    "GPUAdapterInfo",
    "GPUBindGroup",
    "GPUBindGroupLayout",
    "GPUBuffer",
    "GPUCommandBuffer",
    "GPUCommandEncoder",
    "GPUComputePassEncoder",
    "GPUComputePipeline",
    "GPUDevice",
    "GPUDeviceLostInfo",
    "GPUError",
    "GPUInternalError",
    "GPUOutOfMemoryError",
    "GPUPipelineLayout",
    "GPUQuerySet",
    "GPUQueue",
    "GPURenderBundle",
    "GPURenderBundleEncoder",
    "GPURenderPassEncoder",
    "GPURenderPipeline",
    "GPUSampler",
    "GPUShaderModule",
    "GPUTexture",
    "GPUTextureView",
    "GPUValidationError",
];

#[rquickjs::module(rename_vars = "camelCase", rename_types = "PascalCase")]
pub mod webgpu {
    use rquickjs::{
        Class, Ctx, Object, Result, Value,
        module::{Declarations, Exports},
    };

    use super::{GLOBAL_CLASSES, MAP_READ, MAP_WRITE, constants, install_classes, make_instance};
    pub use super::{
        GPU, GPUAdapter, GPUAdapterInfo, GPUBindGroup, GPUBindGroupLayout, GPUBuffer,
        GPUCommandBuffer, GPUCommandEncoder, GPUComputePassEncoder, GPUComputePipeline, GPUDevice,
        GPUDeviceLostInfo, GPUError, GPUInternalError, GPUOutOfMemoryError, GPUPipelineLayout,
        GPUQueue, GPUShaderModule, GPUValidationError,
        query::GPUQuerySet,
        render::{
            GPURenderBundle, GPURenderBundleEncoder, GPURenderPassEncoder, GPURenderPipeline,
        },
        texture::{GPUSampler, GPUTexture, GPUTextureView},
    };

    #[qjs(declare)]
    pub fn declare(declarations: &Declarations) -> Result<()> {
        declarations.declare("gpu")?;
        declarations.declare("GPUBufferUsage")?;
        declarations.declare("GPUMapMode")?;
        declarations.declare("GPUShaderStage")?;
        declarations.declare("GPUTextureUsage")?;
        declarations.declare("GPUColorWrite")?;
        for name in super::GLOBAL_CLASSES {
            declarations.declare(name)?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let globals = ctx.globals();
        let gpu = globals
            .get::<_, Object>("navigator")
            .ok()
            .and_then(|navigator| navigator.get::<_, Class<GPU>>("gpu").ok())
            .map_or_else(
                || {
                    Class::instance(ctx.clone(), GPU {
                        instance: make_instance(),
                    })
                },
                Ok,
            )?;
        let buffer_usage = constants(ctx, &[
            ("MAP_READ", wgpu::BufferUsages::MAP_READ.bits()),
            ("MAP_WRITE", wgpu::BufferUsages::MAP_WRITE.bits()),
            ("COPY_SRC", wgpu::BufferUsages::COPY_SRC.bits()),
            ("COPY_DST", wgpu::BufferUsages::COPY_DST.bits()),
            ("INDEX", wgpu::BufferUsages::INDEX.bits()),
            ("VERTEX", wgpu::BufferUsages::VERTEX.bits()),
            ("UNIFORM", wgpu::BufferUsages::UNIFORM.bits()),
            ("STORAGE", wgpu::BufferUsages::STORAGE.bits()),
            ("INDIRECT", wgpu::BufferUsages::INDIRECT.bits()),
            ("QUERY_RESOLVE", wgpu::BufferUsages::QUERY_RESOLVE.bits()),
        ])?;
        let map_mode = constants(ctx, &[("READ", MAP_READ), ("WRITE", MAP_WRITE)])?;
        let shader_stage = constants(ctx, &[
            ("VERTEX", wgpu::ShaderStages::VERTEX.bits()),
            ("FRAGMENT", wgpu::ShaderStages::FRAGMENT.bits()),
            ("COMPUTE", wgpu::ShaderStages::COMPUTE.bits()),
        ])?;
        exports.export("gpu", gpu.clone())?;
        exports.export("GPUBufferUsage", buffer_usage.clone())?;
        exports.export("GPUMapMode", map_mode.clone())?;
        exports.export("GPUShaderStage", shader_stage.clone())?;

        install_classes(&globals)?;
        for name in GLOBAL_CLASSES {
            exports.export(name, globals.get::<_, Value>(name)?)?;
        }
        let texture_usage = constants(ctx, &[
            ("COPY_SRC", wgpu::TextureUsages::COPY_SRC.bits()),
            ("COPY_DST", wgpu::TextureUsages::COPY_DST.bits()),
            (
                "TEXTURE_BINDING",
                wgpu::TextureUsages::TEXTURE_BINDING.bits(),
            ),
            (
                "STORAGE_BINDING",
                wgpu::TextureUsages::STORAGE_BINDING.bits(),
            ),
            (
                "RENDER_ATTACHMENT",
                wgpu::TextureUsages::RENDER_ATTACHMENT.bits(),
            ),
        ])?;
        let color_write = constants(ctx, &[
            ("RED", 0x1),
            ("GREEN", 0x2),
            ("BLUE", 0x4),
            ("ALPHA", 0x8),
            ("ALL", 0xf),
        ])?;
        exports.export("GPUTextureUsage", texture_usage.clone())?;
        exports.export("GPUColorWrite", color_write.clone())?;
        globals.set("GPUBufferUsage", buffer_usage)?;
        globals.set("GPUMapMode", map_mode)?;
        globals.set("GPUShaderStage", shader_stage)?;
        globals.set("GPUTextureUsage", texture_usage)?;
        globals.set("GPUColorWrite", color_write)?;
        if let Ok(navigator) = globals.get::<_, Object>("navigator") {
            navigator.set("gpu", gpu)?;
        } else {
            let navigator = Object::new(ctx.clone())?;
            navigator.set("gpu", gpu)?;
            globals.set("navigator", navigator)?;
        }
        Ok(())
    }
}
