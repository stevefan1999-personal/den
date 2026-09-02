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
mod supported;
mod texture;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    mem::MaybeUninit,
    num::NonZeroU64,
    ops::Range,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use den_stdlib_worker::events::define_event_handler;
use den_util::BufferSource;
use rquickjs::{
    Array, ArrayBuffer, Class, Coerced, Constructor, Ctx, Error, Exception, FromJs, Function,
    IntoJs as _, JsLifetime, Object, Persistent, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Args, Opt, This},
    object::Accessor,
};

use crate::supported::{
    GPUExternalTexture, GPUSupportedFeatures, GPUSupportedLimits, GPUSupportedWGSLLanguageFeatures,
};

static DEVICE_IDS: AtomicU64 = AtomicU64::new(1);
static RESOURCE_IDS: AtomicU64 = AtomicU64::new(1);

type DynamicWindow = (u32, u64, u64, u64);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextureUseKind {
    Attachment,
    Sampled,
    Storage,
}

#[derive(Clone, Copy)]
pub(crate) struct TextureUse {
    id:          usize,
    write:       bool,
    kind:        TextureUseKind,
    aspect:      wgpu::TextureAspect,
    mip_base:    u32,
    mip_count:   u32,
    layer_base:  u32,
    layer_count: u32,
}

#[derive(Clone)]
struct BindGroupExtras {
    windows:  Rc<[DynamicWindow]>,
    textures: Rc<[TextureUse]>,
}
type DynamicWindowTable = HashMap<u64, BindGroupExtras>;

thread_local! {
    static BIND_GROUP_WINDOWS: RefCell<DynamicWindowTable> = RefCell::new(HashMap::new());
}

pub(crate) fn next_resource_id() -> u64 { RESOURCE_IDS.fetch_add(1, Ordering::Relaxed) }

const MAP_READ: u32 = 1;
const MAP_WRITE: u32 = 2;
const WEBGPU_BUFFER_USAGE_MASK: u32 = 0x03ff;
pub(crate) const MAX_HOST_ALLOCATION: u64 = 0x8000_0000;
pub(crate) const JS_MAX_SAFE_INTEGER: u64 = 0x001F_FFFF_FFFF_FFFF;
const INDIRECT_DISPATCH_BYTES: u64 = 12;

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
        let Coerced(number) = Coerced::<f64>::from_js(ctx, value)?;
        if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) {
            return Err(type_error(ctx, "unsigned integer is out of range"));
        }
        Ok(Self(number as u32))
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
    std::env::var("DEN_WEBGPU_BACKEND").map_or_else(
        |_error| wgpu::Backends::all(),
        |value| wgpu::Backends::from_comma_list(&value),
    )
}

struct GpuPoll;

impl GpuPoll {
    fn until<T: Send>(
        device: wgpu::Device, receiver: std::sync::mpsc::Receiver<T>,
    ) -> std::result::Result<T, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            device.poll(wgpu::PollType::wait_indefinitely())
        })) {
            Ok(Ok(_poll)) => {}
            Ok(Err(error)) => return Err(error.to_string()),
            Err(payload) => return Err(panic_message(payload.as_ref())),
        }
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

fn iterable_strings<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<String>> {
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    if let Ok(features) = Class::<GPUSupportedFeatures>::from_js(ctx, value.clone()) {
        return Ok(features.borrow().names().to_vec());
    }
    if let Ok(features) = Class::<GPUSupportedWGSLLanguageFeatures>::from_js(ctx, value.clone()) {
        return Ok(features.borrow().names().to_vec());
    }
    if let Ok(array) = Array::from_js(ctx, value.clone()) {
        return array.iter::<String>().collect();
    }
    let object = Object::from_js(ctx, value.clone())?;
    let iterator_fn: Function = object
        .get(PredefinedAtom::SymbolIterator)
        .or_else(|_error| object.get("values"))?;
    let iterator: Object = iterator_fn.call((This(value.clone()),))?;
    let next: Function = iterator.get("next")?;
    let mut names = Vec::new();
    loop {
        let step: Object = next.call((This(iterator.clone()),))?;
        if step.get::<_, Coerced<bool>>("done")?.0 {
            break;
        }
        names.push(step.get::<_, Coerced<String>>("value")?.0);
    }
    Ok(names)
}

pub(crate) fn buffer_end(size: u64, offset: u64, length: Option<u64>) -> Option<u64> {
    let end = match length {
        Some(length) => offset.checked_add(length)?,
        None => size,
    };
    (offset <= size && end <= size).then_some(end)
}

pub(crate) struct BufferBind {
    pub invalid:   bool,
    #[expect(dead_code, reason = "callers now always track buffer state")]
    pub destroyed: bool,
    pub slice:     Option<(u64, u64)>,
}

pub(crate) fn buffer_bind(
    buffer: &GPUBuffer, offset: u64, size: Option<u64>, required: wgpu::BufferUsages, align: u64,
    device_id: u64, extra_invalid: bool,
) -> BufferBind {
    let destroyed = matches!(*buffer.state.borrow(), BufferMapState::Destroyed);
    let range = buffer_end(buffer.size, offset, size);
    let empty = range == Some(offset);
    let invalid = extra_invalid
        || buffer.invalid
        || !buffer.usage.contains(required)
        || buffer.device_id != device_id
        || (align != 0 && !offset.is_multiple_of(align))
        || range.is_none();
    let slice = match (invalid, destroyed, empty, range) {
        (false, false, false, Some(end)) => Some((offset, end)),
        _ => None,
    };
    BufferBind {
        invalid,
        destroyed,
        slice,
    }
}

pub(crate) fn immediates_bytes<'js>(
    data: Value<'js>, data_offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: &Ctx<'js>,
) -> Result<Vec<u8>> {
    let element_size = typed_array_element_size(ctx, &data)?;
    let bytes = BufferSource::from_js(ctx, data)?.into_bytes();
    let start = data_offset
        .0
        .flatten()
        .map_or(0, |value| value.0)
        .checked_mul(element_size)
        .ok_or_else(|| operation_error(ctx, "dataOffset overflows"))?;
    let length = match size.0.flatten() {
        Some(value) => {
            value
                .0
                .checked_mul(element_size)
                .ok_or_else(|| operation_error(ctx, "size overflows"))?
        }
        None => (bytes.len() as u64).saturating_sub(start),
    };
    let end = start
        .checked_add(length)
        .ok_or_else(|| operation_error(ctx, "data range overflows"))?;
    if end > bytes.len() as u64 || !length.is_multiple_of(4) {
        return Err(operation_error(
            ctx,
            "immediate data range must be in bounds and a multiple of 4 bytes",
        ));
    }
    Ok(bytes
        .get(start as usize..end as usize)
        .unwrap_or(&[])
        .to_vec())
}

pub(crate) type PipelineLayoutGroups = Rc<[Rc<[wgpu::BindGroupLayoutEntry]>]>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindGroupLayoutKind {
    UniformBuffer,
    StorageBuffer,
    ReadOnlyStorageBuffer,
    Sampler,
    ComparisonSampler,
    Texture,
    StorageTexture,
    ExternalTexture,
}

#[derive(Clone)]
pub(crate) struct BindGroupSlot {
    pub invalid:        bool,
    pub auto:           bool,
    pub auto_layout_id: u64,
    pub entries:        Rc<[(u32, u32, u64)]>,
    pub buffer_sizes:   Rc<[(u32, u64)]>,
    pub textures:       Rc<[TextureUse]>,
}

#[derive(Clone, Default)]
pub(crate) struct BindGroupSet {
    pub slots: Vec<Option<BindGroupSlot>>,
}

pub(crate) fn empty_layout_groups() -> PipelineLayoutGroups { Rc::from(Vec::new()) }

fn naga_stage_for_visibility(visibility: wgpu::ShaderStages) -> Option<wgpu::naga::ShaderStage> {
    if visibility == wgpu::ShaderStages::COMPUTE {
        Some(wgpu::naga::ShaderStage::Compute)
    } else if visibility == wgpu::ShaderStages::VERTEX {
        Some(wgpu::naga::ShaderStage::Vertex)
    } else if visibility == wgpu::ShaderStages::FRAGMENT {
        Some(wgpu::naga::ShaderStage::Fragment)
    } else {
        None
    }
}

pub(crate) fn auto_layout_groups(stages: &[(wgpu::ShaderStages, &str)]) -> PipelineLayoutGroups {
    let mut groups: Vec<Vec<wgpu::BindGroupLayoutEntry>> = Vec::new();
    for &(visibility, code) in stages {
        let used = naga_stage_for_visibility(visibility)
            .and_then(|stage| format::statically_used_bindings(code, stage));
        let mins = format::shader_buffer_min_sizes(code);
        for binding in format::parse_shader_bindings(code) {
            if used.as_ref().is_some_and(|used| {
                !used
                    .iter()
                    .any(|(group, slot)| *group == binding.group && *slot == binding.binding)
            }) {
                continue;
            }
            let group = binding.group as usize;
            if groups.len() <= group {
                groups.resize_with(group + 1, Vec::new);
            }
            let mut ty = format::binding_type(binding.class);
            if let wgpu::BindingType::Buffer {
                min_binding_size, ..
            } = &mut ty
            {
                let min = mins
                    .iter()
                    .find(|(shader_group, shader_binding, _min)| {
                        *shader_group == binding.group && *shader_binding == binding.binding
                    })
                    .map_or(0, |(_group, _binding, min)| *min);
                *min_binding_size = NonZeroU64::new(min);
            }
            let Some(group_entries) = groups.get_mut(group) else {
                continue;
            };
            if let Some(existing) = group_entries
                .iter_mut()
                .find(|entry| entry.binding == binding.binding)
            {
                existing.visibility |= visibility;
            } else {
                group_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: binding.binding,
                    visibility,
                    ty,
                    count: None,
                });
            }
        }
    }
    Rc::from(
        groups
            .into_iter()
            .map(Rc::<[wgpu::BindGroupLayoutEntry]>::from)
            .collect::<Vec<_>>(),
    )
}

fn layout_entry_fingerprint(ty: wgpu::BindingType) -> u64 {
    use std::hash::Hasher as _;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(format!("{ty:?}").as_bytes());
    hasher.finish()
}

fn bind_group_entries_equivalent(left: &[(u32, u32, u64)], right: &[(u32, u32, u64)]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_by_key(|entry| entry.0);
    right.sort_by_key(|entry| entry.0);
    left == right
}

pub(crate) fn pipeline_bind_groups_mismatch(
    groups: &[Rc<[wgpu::BindGroupLayoutEntry]>], auto: bool, auto_id: u64, bound: &BindGroupSet,
) -> bool {
    groups.iter().enumerate().any(|(index, entries)| {
        if entries.is_empty() {
            return false;
        }
        let Some(slot) = bound.slots.get(index).and_then(Option::as_ref) else {
            return true;
        };
        if slot.invalid {
            return true;
        }
        if auto != slot.auto {
            return true;
        }
        let fingerprints = entries
            .iter()
            .map(|entry| {
                (
                    entry.binding,
                    entry.visibility.bits(),
                    layout_entry_fingerprint(entry.ty),
                )
            })
            .collect::<Vec<_>>();
        if auto {
            auto_id != slot.auto_layout_id
                || !bind_group_entries_equivalent(&fingerprints, &slot.entries)
        } else {
            !bind_group_entries_equivalent(&fingerprints, &slot.entries)
        }
    })
}

#[expect(
    clippy::integer_division,
    reason = "immediate slots are 4-byte words; integer division is the slot index"
)]
pub(crate) fn immediate_slots_from_range(offset: u32, length: usize) -> u64 {
    let Ok(size) = u32::try_from(length) else {
        return u64::MAX;
    };
    if size == 0 {
        return 0;
    }
    let start = offset / 4;
    if start >= 64 {
        return 0;
    }
    let end = offset.saturating_add(size).div_ceil(4).min(64);
    (u64::MAX << start) & (u64::MAX >> (64 - end))
}

pub(crate) fn immediates_unfilled(required: u64, filled: u64) -> bool { required & !filled != 0 }

pub(crate) fn immediates_range_invalid(offset: u32, length: usize, max: u32) -> bool {
    if !offset.is_multiple_of(4) {
        return true;
    }
    let Some(end) = (offset as u64).checked_add(length as u64) else {
        return true;
    };
    if end > u64::from(u32::MAX) {
        return true;
    }
    max != 0 && end > u64::from(max)
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
pub(crate) enum GPUErrorKind {
    Internal,
    OutOfMemory,
    Validation,
}

pub(crate) struct GPUErrorData {
    pub(crate) kind:    GPUErrorKind,
    pub(crate) message: String,
}

struct ErrorScopeState {
    error:  Option<GPUErrorData>,
    filter: GPUErrorKind,
}

#[derive(Default)]
struct ErrorRouter {
    scopes:     Vec<ErrorScopeState>,
    uncaptured: Vec<GPUErrorData>,
    suppress:   bool,
}

impl ErrorRouter {
    fn capture(&mut self, error: GPUErrorData) {
        if let Some(scope) = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.filter == error.kind)
        {
            if scope.error.is_none() {
                scope.error = Some(error);
            }
        } else if !self.suppress {
            self.uncaptured.push(error);
        }
    }

    fn take_uncaptured(&mut self) -> Vec<GPUErrorData> { std::mem::take(&mut self.uncaptured) }
}

#[derive(Clone, Default)]
pub(crate) struct ErrorSink {
    inner: Arc<Mutex<ErrorRouter>>,
}

impl ErrorSink {
    fn lock(&self) -> std::sync::MutexGuard<'_, ErrorRouter> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn validation(&self, message: impl Into<String>) {
        self.capture(GPUErrorData {
            kind:    GPUErrorKind::Validation,
            message: message.into(),
        });
    }

    pub(crate) fn out_of_memory(&self, message: impl Into<String>) {
        self.capture(GPUErrorData {
            kind:    GPUErrorKind::OutOfMemory,
            message: message.into(),
        });
    }

    pub(crate) fn capture(&self, error: GPUErrorData) { self.lock().capture(error); }

    fn take_uncaptured(&self) -> Vec<GPUErrorData> { self.lock().take_uncaptured() }

    pub(crate) fn push(&self, filter: GPUErrorKind) {
        self.lock().scopes.push(ErrorScopeState {
            error: None,
            filter,
        });
    }

    pub(crate) fn pop(&self) -> Option<GPUErrorData> { self.lock().scopes.pop()?.error }

    fn set_suppress(&self, suppress: bool) { self.lock().suppress = suppress; }

    pub(crate) fn scope_has_error(&self) -> bool {
        self.lock()
            .scopes
            .last()
            .is_some_and(|scope| scope.error.is_some())
    }
}

fn flush_uncaptured<'js>(device: &Class<'js, GPUDevice<'js>>, ctx: &Ctx<'js>) -> Result<()> {
    let pending = {
        let errors = device.borrow().errors.clone();
        let pending = errors.take_uncaptured();
        let mut router = errors.lock();
        if router.scopes.is_empty() {
            pending
        } else {
            for error in pending {
                router.capture(error);
            }
            router.take_uncaptured()
        }
    };
    for error in pending {
        dispatch_uncaptured_error(device, ctx, error)?;
    }
    Ok(())
}

fn flushed<'js, T>(
    this: &This<Class<'js, GPUDevice<'js>>>, ctx: &Ctx<'js>, result: Result<T>,
) -> Result<T> {
    match result {
        Ok(value) => {
            flush_uncaptured(&this.0, ctx)?;
            Ok(value)
        }
        Err(error) => {
            let _ = flush_uncaptured(&this.0, ctx);
            Err(error)
        }
    }
}

fn dispatch_uncaptured_error<'js>(
    device: &Class<'js, GPUDevice<'js>>, ctx: &Ctx<'js>, error: GPUErrorData,
) -> Result<()> {
    let error_js = gpu_error_value(ctx, error)?;
    let init = Object::new(ctx.clone())?;
    init.set("error", error_js)?;
    let ctor: Constructor = match ctx.globals().get("GPUUncapturedErrorEvent") {
        Ok(ctor) => ctor,
        Err(_error) => return Ok(()),
    };
    let event: Value = ctor.construct(("uncapturederror", init))?;
    let Ok(dispatch) = device.as_inner().get::<_, Function>("dispatchEvent") else {
        return Ok(());
    };
    let mut args = Args::new(ctx.clone(), 1);
    args.this(device.clone())?;
    args.push_arg(event)?;
    dispatch.call_arg::<Value>(args)?;
    Ok(())
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
pub struct GPU<'js> {
    #[qjs(skip_trace)]
    instance:               wgpu::Instance,
    wgsl_language_features: Class<'js, GPUSupportedWGSLLanguageFeatures>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPU<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    pub async fn request_adapter(
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
                Class::instance(ctx.clone(), GPUAdapter::from_inner(&ctx, adapter)?)?.into_js(&ctx)
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

    #[qjs(get, configurable)]
    pub fn wgsl_language_features(&self) -> Class<'js, GPUSupportedWGSLLanguageFeatures> {
        self.wgsl_language_features.clone()
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUAdapter")]
pub struct GPUAdapter<'js> {
    #[qjs(skip_trace)]
    inner:    wgpu::Adapter,
    features: Class<'js, GPUSupportedFeatures>,
    limits:   Class<'js, GPUSupportedLimits>,
    info:     Class<'js, GPUAdapterInfo>,
}

impl<'js> GPUAdapter<'js> {
    fn from_inner(ctx: &Ctx<'js>, inner: wgpu::Adapter) -> Result<Self> {
        Ok(Self {
            features: GPUSupportedFeatures::from_features(ctx, inner.features())?,
            limits: GPUSupportedLimits::from_limits(
                ctx,
                GPUSupportedLimits::grant_defaults(inner.limits()),
            )?,
            info: Class::instance(ctx.clone(), GPUAdapterInfo {
                inner: inner.get_info(),
            })?,
            inner,
        })
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUAdapter<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn features(&self) -> Class<'js, GPUSupportedFeatures> { self.features.clone() }

    #[qjs(get, configurable)]
    pub fn limits(&self) -> Class<'js, GPUSupportedLimits> { self.limits.clone() }

    #[qjs(get, configurable)]
    pub fn info(&self) -> Class<'js, GPUAdapterInfo> { self.info.clone() }

    pub async fn request_device(
        self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Class<'js, GPUDevice<'js>>> {
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
            .map(|object| object.get::<_, Option<Value>>("requiredFeatures"))
            .transpose()?
            .flatten()
        {
            for name in iterable_strings(&ctx, features)? {
                if GPUSupportedFeatures::is_host_feature(&name) {
                    continue;
                }
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
        if self.inner.features().contains(wgpu::Features::IMMEDIATES) {
            required_features |= wgpu::Features::IMMEDIATES;
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
        if required_features.contains(wgpu::Features::IMMEDIATES)
            && required_limits.max_immediate_size == 0
        {
            required_limits.max_immediate_size = self.inner.limits().max_immediate_size.max(16);
        }
        let adapter_native = self.inner.limits();
        let adapter_reported = GPUSupportedLimits::grant_defaults(adapter_native.clone());
        if !required_limits.check_limits(&adapter_reported) {
            return Err(operation_error(
                &ctx,
                "requiredLimits exceed the adapter's supported limits",
            ));
        }
        let granted = GPUSupportedLimits::grant_defaults(required_limits);
        let wgpu_limits = granted.clone().or_worse_values_from(&adapter_native);
        let request = wgpu::DeviceDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            required_features,
            required_limits: wgpu_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
            default_queue: wgpu::QueueDescriptor { label: None },
        };
        let (device, queue) = self
            .inner
            .request_device(&request)
            .await
            .map_err(|error| operation_error(&ctx, error.to_string()))?;
        let errors = ErrorSink::default();
        let error_sink = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            error_sink.capture(gpu_error_data(error));
        }));
        let device_js = Rc::new(RefCell::new(None));
        let queue = Class::instance(ctx.clone(), GPUQueue {
            device:    Some(device.clone()),
            device_js: device_js.clone(),
            errors:    errors.clone(),
            inner:     queue,
            label:     Rc::new(RefCell::new(queue_label)),
        })?;
        let (lost, resolve, _reject) = ctx.promise()?;
        let features = GPUSupportedFeatures::from_features(&ctx, device.features())?;
        let limits = GPUSupportedLimits::from_limits(&ctx, granted)?;
        let fallbacks = Rc::new(texture::FallbackResources::new(&device));
        let gpu_device = Class::instance(ctx.clone(), GPUDevice {
            adapter_info: self.info.clone(),
            destroyed: Rc::new(Cell::new(false)),
            device,
            id: DEVICE_IDS.fetch_add(1, Ordering::Relaxed),
            errors,
            fallbacks,
            features,
            label: Rc::new(RefCell::new(label)),
            limits,
            lost,
            lost_resolve: Rc::new(RefCell::new(Some(resolve))),
            queue,
        })?;
        *device_js.borrow_mut() = Some(gpu_device.clone());
        Ok(gpu_device)
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

    #[qjs(get, configurable)]
    pub fn vendor(&self) -> String { self.inner.vendor.to_string() }

    #[qjs(get, configurable)]
    pub const fn architecture(&self) -> &'static str { "" }

    #[qjs(get, configurable)]
    pub fn device(&self) -> String { self.inner.device.to_string() }

    #[qjs(get, configurable)]
    pub fn description(&self) -> String { self.inner.name.clone() }

    #[qjs(get, configurable)]
    pub const fn subgroup_min_size(&self) -> u32 { self.inner.subgroup_min_size }

    #[qjs(get, configurable)]
    pub const fn subgroup_max_size(&self) -> u32 { self.inner.subgroup_max_size }

    #[qjs(get, configurable)]
    pub fn is_fallback_adapter(&self) -> bool { self.inner.device_type == wgpu::DeviceType::Cpu }
}

#[derive(Clone, JsLifetime)]
#[rquickjs::class(rename = "GPUDevice")]
pub struct GPUDevice<'js> {
    adapter_info:         Class<'js, GPUAdapterInfo>,
    destroyed:            Rc<Cell<bool>>,
    pub(crate) device:    wgpu::Device,
    pub(crate) id:        u64,
    pub(crate) errors:    ErrorSink,
    pub(crate) fallbacks: Rc<texture::FallbackResources>,
    features:             Class<'js, GPUSupportedFeatures>,
    label:                Rc<RefCell<String>>,
    limits:               Class<'js, GPUSupportedLimits>,
    lost:                 Promise<'js>,
    lost_resolve:         Rc<RefCell<Option<Function<'js>>>>,
    queue:                Class<'js, GPUQueue<'js>>,
}

impl<'js> Trace<'js> for GPUDevice<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.adapter_info.trace(tracer);
        self.features.trace(tracer);
        self.limits.trace(tracer);
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

impl<'js> GPUDevice<'js> {
    fn is_destroyed(&self) -> bool { self.destroyed.get() }

    pub(crate) fn reported_limits(&self) -> wgpu::Limits { self.limits.borrow().inner().clone() }

    fn create_buffer_inner(&self, descriptor: Object<'js>, ctx: &Ctx<'js>) -> Result<GPUBuffer> {
        let label = label(&descriptor)?;
        let size = descriptor.get::<_, JsU64>("size")?.0;
        let usage_bits = descriptor.get::<_, JsU32>("usage")?.0;
        let extra_usage = usage_bits & !WEBGPU_BUFFER_USAGE_MASK != 0;
        let usage = wgpu::BufferUsages::from_bits_truncate(usage_bits & WEBGPU_BUFFER_USAGE_MASK);
        let mapped_at_creation = descriptor
            .get::<_, Option<bool>>("mappedAtCreation")?
            .unwrap_or_default();
        if mapped_at_creation && !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            return Err(Exception::throw_range(
                ctx,
                "mappedAtCreation buffer size must be a multiple of 4",
            ));
        }
        let map_read_ok = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;
        let map_write_ok = wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC;
        let mut invalid = extra_usage || usage.is_empty();
        if extra_usage {
            self.errors
                .validation("usage contains flags outside GPUBufferUsage");
        }
        if usage.is_empty() && !extra_usage {
            self.errors.validation("usage must not be zero");
        }
        if usage.contains(wgpu::BufferUsages::MAP_READ) && !map_read_ok.contains(usage) {
            self.errors
                .validation("MAP_READ may only be combined with COPY_DST");
            invalid = true;
        }
        if usage.contains(wgpu::BufferUsages::MAP_WRITE) && !map_write_ok.contains(usage) {
            self.errors
                .validation("MAP_WRITE may only be combined with COPY_SRC");
            invalid = true;
        }
        if size > self.device.limits().max_buffer_size {
            self.errors.validation("buffer size exceeds maxBufferSize");
            invalid = true;
        }
        let over_budget = size > MAX_HOST_ALLOCATION;
        if !invalid && over_budget {
            self.errors
                .out_of_memory("buffer allocation exceeds the host budget");
        }
        let gpu_size = if invalid || over_budget { 4 } else { size };
        let gpu_usage = if invalid {
            wgpu::BufferUsages::COPY_DST
        } else {
            usage
        };
        let gpu_mapped = mapped_at_creation && !invalid && !over_budget && size > 0;
        // Invalid mappedAtCreation buffers are still mapped in JS; the content
        // process must not wait for GPU validation.
        let js_mapped = mapped_at_creation && !over_budget;
        let host_map =
            (js_mapped && !gpu_mapped).then(|| vec![0_u8; usize::try_from(size).unwrap_or(0)]);
        let descriptor = wgpu::BufferDescriptor {
            label:              (!label.is_empty()).then_some(label.as_str()),
            size:               gpu_size,
            usage:              gpu_usage,
            mapped_at_creation: gpu_mapped,
        };
        let inner = catch_gpu(&self.errors, || self.device.create_buffer(&descriptor))
            .unwrap_or_else(|| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label:              None,
                    size:               4,
                    usage:              wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
        Ok(GPUBuffer {
            device: self.device.clone(),
            errors: self.errors.clone(),
            fallbacks: self.fallbacks.clone(),
            host_map: Rc::new(RefCell::new(host_map)),
            inner,
            invalid,
            device_id: self.id,
            label: Rc::new(RefCell::new(label)),
            map_abort: Rc::new(Cell::new(false)),
            map_gen: Rc::new(Cell::new(0)),
            native_mapped: Rc::new(Cell::new(gpu_mapped)),
            size,
            state: Rc::new(RefCell::new(if js_mapped {
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

    fn create_shader_module_inner(
        &self, descriptor: Object<'js>, _ctx: &Ctx<'js>,
    ) -> Result<GPUShaderModule> {
        let label = label(&descriptor)?;
        let code: String = descriptor.get("code")?;
        let code = Rc::<str>::from(code);
        if format::uses_external_texture(&code) {
            return Ok(GPUShaderModule {
                inner: self.fallbacks.shader_module.clone(),
                invalid: false,
                code,
                device_id: self.id,
                label: Rc::new(RefCell::new(label)),
            });
        }
        if code.contains("var<immediate") {
            let over = format::naga_immediate_unusable(&code)
                || format::immediate_byte_size(&code) > self.device.limits().max_immediate_size;
            return Ok(GPUShaderModule {
                inner: self.fallbacks.shader_module.clone(),
                invalid: over,
                code,
                device_id: self.id,
                label: Rc::new(RefCell::new(label)),
            });
        }
        let module_descriptor = wgpu::ShaderModuleDescriptor {
            label:  (!label.is_empty()).then_some(label.as_str()),
            source: wgpu::ShaderSource::Wgsl(code.to_string().into()),
        };
        self.errors.push(GPUErrorKind::Validation);
        let (inner, gpu_failed) = catch_gpu(&self.errors, || {
            self.device.create_shader_module(module_descriptor)
        })
        .map_or_else(
            || (self.fallbacks.shader_module.clone(), true),
            |inner| (inner, false),
        );
        let scoped = self.errors.pop();
        let invalid = gpu_failed || scoped.is_some();
        if let Some(error) = scoped {
            self.errors.capture(error);
        }
        Ok(GPUShaderModule {
            inner,
            invalid,
            code,
            device_id: self.id,
            label: Rc::new(RefCell::new(label)),
        })
    }

    fn create_bind_group_layout_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUBindGroupLayout> {
        let label = label(&descriptor)?;
        let entries = array_value(&descriptor, "entries", ctx)?;
        let mut native = Vec::with_capacity(entries.len());
        let mut kinds = Vec::with_capacity(entries.len());
        for entry in entries.iter::<Object>() {
            let entry = entry?;
            let binding = entry.get::<_, JsU32>("binding")?.0;
            let visibility_bits = entry.get::<_, JsU32>("visibility")?.0;
            if visibility_bits
                & !(wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE).bits()
                != 0
            {
                self.errors
                    .validation("visibility contains flags outside GPUShaderStage");
            }
            let visibility = wgpu::ShaderStages::from_bits_truncate(
                visibility_bits
                    & (wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE).bits(),
            );
            if entry.get::<_, Option<JsU32>>("count")?.is_some() {
                self.errors.validation("binding arrays are not implemented");
            }
            let (ty, kind) = bind_group_layout_type(&entry, ctx)?;
            if let wgpu::BindingType::StorageTexture { format, .. } = ty
                && !self.device.features().contains(format.required_features())
            {
                return Err(type_error(
                    ctx,
                    "storage texture format requires missing features",
                ));
            }
            kinds.push((binding, kind));
            native.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty,
                count: None,
            });
        }
        let skip_bgl = skip_native_bgl(&native, self.device.features());
        if skip_bgl {
            self.errors
                .validation("bind group layout is invalid for this device");
        }
        let inner = if skip_bgl {
            self.fallbacks.bind_group_layout.clone()
        } else {
            catch_gpu(&self.errors, || {
                self.device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label:   (!label.is_empty()).then_some(label.as_str()),
                        entries: &native,
                    })
            })
            .unwrap_or_else(|| self.fallbacks.bind_group_layout.clone())
        };
        Ok(GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(label)),
            kinds: Rc::from(kinds),
            entries: Rc::from(native),
            auto: false,
            auto_layout_id: 0,
        })
    }

    fn create_pipeline_layout_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUPipelineLayout> {
        let label = label(&descriptor)?;
        let layouts = array_value(&descriptor, "bindGroupLayouts", ctx)?;
        let mut inners = Vec::new();
        let mut groups = Vec::new();
        for layout in layouts.iter::<Option<Class<GPUBindGroupLayout>>>() {
            if let Some(layout) = layout? {
                let layout = layout.borrow();
                inners.push(Some(layout.inner.clone()));
                groups.push(layout.entries.clone());
            } else {
                inners.push(None);
                groups.push(Rc::from(Vec::new()));
            }
        }
        let borrowed = inners
            .iter()
            .map(|layout| layout.as_ref())
            .collect::<Vec<_>>();
        let immediate_size = descriptor
            .get::<_, Option<JsU32>>("immediateSize")?
            .map_or(0, |value| value.0);
        let max_immediate = self.device.limits().max_immediate_size;
        let invalid = {
            let unaligned = !immediate_size.is_multiple_of(4);
            if unaligned {
                self.errors
                    .validation("immediateSize must be a multiple of 4");
            }
            let too_large = immediate_size > max_immediate;
            if too_large {
                self.errors
                    .validation("immediateSize exceeds maxImmediateSize");
            }
            unaligned || too_large
        };
        let gpu_immediate = if invalid { 0 } else { immediate_size };
        let inner = catch_gpu(&self.errors, || {
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label:              (!label.is_empty()).then_some(label.as_str()),
                    bind_group_layouts: &borrowed,
                    immediate_size:     gpu_immediate,
                })
        })
        .unwrap_or_else(|| {
            self.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label:              None,
                    bind_group_layouts: &[],
                    immediate_size:     0,
                })
        });
        Ok(GPUPipelineLayout {
            inner,
            device_id: self.id,
            groups: Rc::from(groups),
            immediate_size,
            label: Rc::new(RefCell::new(label)),
        })
    }

    fn create_bind_group_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUBindGroup> {
        let label = label(&descriptor)?;
        let layout_js = class_value::<GPUBindGroupLayout>(&descriptor, "layout", ctx)?;
        let layout = layout_js.borrow().inner.clone();
        let kinds = layout_js.borrow().kinds.clone();
        let layout_entries = layout_js.borrow().entries.clone();
        let auto = layout_js.borrow().auto;
        let auto_layout_id = layout_js.borrow().auto_layout_id;
        let entries = array_value(&descriptor, "entries", ctx)?;
        let mut resources = Vec::with_capacity(entries.len());
        let mut used_destroyed = Vec::new();
        let mut used_mapped = Vec::new();
        let mut binding_buffers = Vec::new();
        let mut texture_uses = Vec::new();
        let mut invalid = false;
        for entry in entries.iter::<Object>() {
            let entry = entry?;
            let binding = entry.get::<_, JsU32>("binding")?.0;
            let kind = kinds
                .iter()
                .find(|(id, _kind)| *id == binding)
                .map(|(_id, kind)| *kind);
            let resource: Value = entry.get("resource")?;
            if Class::<GPUExternalTexture>::from_js(ctx, resource.clone()).is_ok() {
                invalid = true;
                self.errors
                    .validation("GPUExternalTexture bindings are not supported");
                continue;
            }
            if let Ok(buffer) = Class::<GPUBuffer>::from_js(ctx, resource.clone()) {
                if matches!(kind, Some(BindGroupLayoutKind::ExternalTexture)) {
                    invalid = true;
                    self.errors
                        .validation("externalTexture binding requires a texture view");
                }
                let buffer = buffer.borrow();
                // Destroyed buffers are still valid JS objects; only invalid
                // ones generate GPUValidationError at bind-group creation.
                if buffer.invalid {
                    invalid = true;
                    self.errors.validation("GPUBuffer is invalid");
                }
                used_mapped.push(buffer.state.clone());
                let min_size = layout_entries
                    .iter()
                    .find(|entry| entry.binding == binding)
                    .and_then(|entry| {
                        match entry.ty {
                            wgpu::BindingType::Buffer {
                                min_binding_size, ..
                            } => min_binding_size.map(NonZeroU64::get),
                            _ => None,
                        }
                    })
                    .unwrap_or(0);
                if min_size != 0 && buffer.size < min_size {
                    invalid = true;
                    self.errors
                        .validation("buffer binding is smaller than minBindingSize");
                }
                binding_buffers.push((binding, 0, buffer.size, buffer.size));
                resources.push(OwnedBinding::Buffer {
                    binding,
                    buffer: buffer.binding_buffer(&self.device),
                    offset: 0,
                    size: None,
                });
                continue;
            }
            if let Ok(texture) = Class::<texture::GPUTexture>::from_js(ctx, resource.clone()) {
                if matches!(kind, Some(BindGroupLayoutKind::ExternalTexture)) {
                    invalid = true;
                    self.errors
                        .validation("externalTexture binding requires a texture view");
                }
                let texture = texture.borrow();
                if texture.is_dummy {
                    invalid = true;
                    self.errors.validation("GPUTexture is invalid");
                }
                used_destroyed.push(texture.destroyed.clone());
                if let Some((write, kind)) = texture_binding_use(layout_entries.as_ref(), binding) {
                    texture_uses.push(TextureUse {
                        id: Rc::as_ptr(&texture.destroyed) as usize,
                        write,
                        kind,
                        aspect: wgpu::TextureAspect::All,
                        mip_base: 0,
                        mip_count: texture.mip_levels,
                        layer_base: 0,
                        layer_count: texture.size.depth_or_array_layers.max(1),
                    });
                }
                resources.push(OwnedBinding::View {
                    binding,
                    view: texture.binding_view(),
                });
                continue;
            }
            if let Ok(view) = Class::<texture::GPUTextureView>::from_js(ctx, resource.clone()) {
                let view = view.borrow();
                if view.is_dummy {
                    invalid = true;
                    self.errors.validation("GPUTextureView is invalid");
                }
                used_destroyed.push(view.destroyed.clone());
                if let Some((write, kind)) = texture_binding_use(layout_entries.as_ref(), binding) {
                    texture_uses.push(TextureUse {
                        id: Rc::as_ptr(&view.destroyed) as usize,
                        write,
                        kind,
                        aspect: view.aspect(),
                        mip_base: view.base_mip,
                        mip_count: view.mip_count,
                        layer_base: view.base_layer,
                        layer_count: view.layer_count,
                    });
                }
                if matches!(kind, Some(BindGroupLayoutKind::ExternalTexture))
                    && !view.is_valid_external_texture()
                {
                    invalid = true;
                    self.errors
                        .validation("texture view is not valid as GPUExternalTexture");
                }
                resources.push(OwnedBinding::View {
                    binding,
                    view: view.inner.clone(),
                });
                continue;
            }
            if let Ok(sampler) = Class::<texture::GPUSampler>::from_js(ctx, resource.clone()) {
                if matches!(kind, Some(BindGroupLayoutKind::ExternalTexture)) {
                    invalid = true;
                    self.errors
                        .validation("externalTexture binding requires a texture view");
                }
                let sampler = sampler.borrow();
                if sampler.invalid {
                    invalid = true;
                    self.errors.validation("GPUSampler is invalid");
                }
                resources.push(OwnedBinding::Sampler {
                    binding,
                    sampler: sampler.inner.clone(),
                });
                continue;
            }
            let resource = Object::from_js(ctx, resource)
                .map_err(|_error| type_error(ctx, "bind group resource is not a GPU binding"))?;
            let buffer = class_value::<GPUBuffer>(&resource, "buffer", ctx)?;
            let buffer = buffer.borrow();
            if buffer.invalid {
                invalid = true;
                self.errors.validation("GPUBuffer is invalid");
            }
            used_mapped.push(buffer.state.clone());
            if matches!(kind, Some(BindGroupLayoutKind::ExternalTexture)) {
                invalid = true;
                self.errors
                    .validation("externalTexture binding requires a texture view");
            }
            let offset = resource
                .get::<_, Option<JsU64>>("offset")?
                .map_or(0, |value| value.0);
            let size = resource.get::<_, Option<JsU64>>("size")?.and_then(|value| {
                NonZeroU64::new(value.0).or_else(|| {
                    invalid = true;
                    self.errors
                        .validation("buffer binding size must be greater than zero");
                    None
                })
            });
            let end = size.map_or(buffer.size, |size| offset.saturating_add(size.get()));
            if offset > buffer.size || end > buffer.size {
                invalid = true;
                self.errors.validation("buffer binding is out of bounds");
            }
            let binding_size =
                size.map_or_else(|| buffer.size.saturating_sub(offset), NonZeroU64::get);
            let min_size = layout_entries
                .iter()
                .find(|entry| entry.binding == binding)
                .and_then(|entry| {
                    match entry.ty {
                        wgpu::BindingType::Buffer {
                            min_binding_size, ..
                        } => min_binding_size.map(NonZeroU64::get),
                        _ => None,
                    }
                })
                .unwrap_or(0);
            if min_size != 0 && binding_size < min_size {
                invalid = true;
                self.errors
                    .validation("buffer binding is smaller than minBindingSize");
            }
            binding_buffers.push((binding, offset, binding_size, buffer.size));
            resources.push(OwnedBinding::Buffer {
                binding,
                buffer: buffer.binding_buffer(&self.device),
                offset,
                size,
            });
        }
        let mut dynamic_count = 0_u32;
        let mut windows = Vec::new();
        let mut ordered = layout_entries.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|entry| entry.binding);
        for entry in ordered {
            let wgpu::BindingType::Buffer {
                has_dynamic_offset: true,
                ty,
                ..
            } = entry.ty
            else {
                continue;
            };
            let align = match ty {
                wgpu::BufferBindingType::Uniform => {
                    self.device.limits().min_uniform_buffer_offset_alignment
                }
                wgpu::BufferBindingType::Storage { .. } => {
                    self.device.limits().min_storage_buffer_offset_alignment
                }
            };
            let (bind_offset, binding_size, buffer_size) = binding_buffers
                .iter()
                .find(|(binding, _, _, _)| *binding == entry.binding)
                .map_or((0, 0, 0), |(_, bind_offset, size, buffer_size)| {
                    (*bind_offset, *size, *buffer_size)
                });
            windows.push((align, bind_offset, binding_size, buffer_size));
            dynamic_count += 1;
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
        if layout_entries.len() != native.len() {
            invalid = true;
            self.errors
                .validation("bind group entry count must match the layout");
        }
        let inner = if skip_native_bgl(&layout_entries, self.device.features())
            || layout_entries.len() != native.len()
        {
            self.fallbacks.bind_group.clone()
        } else {
            catch_gpu(&self.errors, || {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label:   (!label.is_empty()).then_some(label.as_str()),
                    layout:  &layout,
                    entries: &native,
                })
            })
            .unwrap_or_else(|| {
                invalid = true;
                self.fallbacks.bind_group.clone()
            })
        };
        let delayed = self.errors.take_uncaptured();
        if !delayed.is_empty() {
            invalid = true;
            for error in delayed {
                self.errors.capture(error);
            }
        }
        invalid |= self.errors.scope_has_error();
        let fingerprints = layout_entries
            .iter()
            .map(|entry| {
                (
                    entry.binding,
                    entry.visibility.bits(),
                    layout_entry_fingerprint(entry.ty),
                )
            })
            .collect::<Vec<_>>();
        let window_key = fingerprints.as_ptr() as u64;
        if !windows.is_empty() || !texture_uses.is_empty() {
            BIND_GROUP_WINDOWS.with(|table| {
                table.borrow_mut().insert(window_key, BindGroupExtras {
                    windows:  Rc::from(windows),
                    textures: Rc::from(texture_uses),
                });
            });
        }
        Ok(GPUBindGroup {
            inner,
            label: Rc::new(RefCell::new(label)),
            invalid,
            auto,
            auto_layout_id,
            device_id: self.id,
            dynamic_count,
            used_destroyed,
            used_mapped,
            fingerprints,
            buffer_sizes: Rc::from(
                binding_buffers
                    .iter()
                    .map(|(binding, _offset, binding_size, _buffer_size)| (*binding, *binding_size))
                    .collect::<Vec<_>>(),
            ),
        })
    }
}

fn attachment_destroyed<'js>(
    object: &Object<'js>, key: &str, ctx: &Ctx<'js>,
) -> Result<Rc<Cell<bool>>> {
    let value = object
        .get::<_, Value>(key)
        .map_err(|_error| type_error(ctx, format!("{key} must be a WebGPU object")))?;
    if let Ok(view) = Class::<texture::GPUTextureView>::from_js(ctx, value.clone()) {
        return Ok(view.borrow().destroyed.clone());
    }
    if let Ok(texture) = Class::<texture::GPUTexture>::from_js(ctx, value) {
        return Ok(texture.borrow().destroyed.clone());
    }
    Err(type_error(
        ctx,
        format!("{key} must be a GPUTexture or GPUTextureView"),
    ))
}

fn record_attachment_liveness<'js>(
    descriptor: &Object<'js>, used: &mut Vec<Rc<Cell<bool>>>, ctx: &Ctx<'js>,
) -> Result<()> {
    let colors = descriptor.get::<_, Array>("colorAttachments")?;
    for attachment in colors.iter::<Option<Object>>() {
        let Some(attachment) = attachment? else {
            continue;
        };
        used.push(attachment_destroyed(&attachment, "view", ctx)?);
        match attachment.get::<_, Option<Value>>("resolveTarget")? {
            Some(value) if !value.is_null() && !value.is_undefined() => {
                if let Ok(view) = Class::<texture::GPUTextureView>::from_js(ctx, value.clone()) {
                    used.push(view.borrow().destroyed.clone());
                } else if let Ok(texture) = Class::<texture::GPUTexture>::from_js(ctx, value) {
                    used.push(texture.borrow().destroyed.clone());
                } else {
                    return Err(type_error(
                        ctx,
                        "resolveTarget must be a GPUTexture or GPUTextureView",
                    ));
                }
            }
            _ => {}
        }
    }
    if let Some(depth) = descriptor.get::<_, Option<Object>>("depthStencilAttachment")? {
        used.push(attachment_destroyed(&depth, "view", ctx)?);
    }
    Ok(())
}

pub(crate) fn catch_gpu<T>(sink: &ErrorSink, operation: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => Some(value),
        Err(payload) => {
            sink.validation(panic_message(payload.as_ref()));
            None
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|value| (*value).to_owned())
        })
        .unwrap_or_else(|| "wgpu panic".into())
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUDevice<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable)]
    pub fn features(&self) -> Class<'js, GPUSupportedFeatures> { self.features.clone() }

    #[qjs(get, configurable)]
    pub fn limits(&self) -> Class<'js, GPUSupportedLimits> { self.limits.clone() }

    #[qjs(get, configurable)]
    pub fn adapter_info(&self) -> Class<'js, GPUAdapterInfo> { self.adapter_info.clone() }

    #[qjs(get, configurable)]
    pub fn queue(&self) -> Class<'js, GPUQueue<'js>> { self.queue.clone() }

    #[qjs(get, configurable)]
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
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<texture::GPUTexture> {
        flushed(
            &this,
            &ctx,
            texture::GPUTexture::from_descriptor(
                &self.device,
                &self.fallbacks,
                descriptor,
                &ctx,
                &self.errors,
                self.id,
            ),
        )
    }

    pub fn create_sampler(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<texture::GPUSampler> {
        flushed(
            &this,
            &ctx,
            texture::create_sampler(&self.device, descriptor, &ctx, &self.errors),
        )
    }

    pub fn create_query_set(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<query::GPUQuerySet> {
        flushed(
            &this,
            &ctx,
            query::GPUQuerySet::from_descriptor(
                &self.device,
                descriptor,
                &ctx,
                &self.errors,
                self.id,
            ),
        )
    }

    pub fn create_buffer(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUBuffer> {
        flushed(&this, &ctx, self.create_buffer_inner(descriptor, &ctx))
    }

    pub fn import_external_texture(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUExternalTexture> {
        let label = label(&descriptor)?;
        self.errors
            .validation("GPUExternalTexture is not supported in this host");
        flushed(&this, &ctx, Ok(GPUExternalTexture::with_label(label)))
    }

    pub fn create_shader_module(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUShaderModule> {
        flushed(
            &this,
            &ctx,
            self.create_shader_module_inner(descriptor, &ctx),
        )
    }

    pub fn create_bind_group_layout(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUBindGroupLayout> {
        flushed(
            &this,
            &ctx,
            self.create_bind_group_layout_inner(descriptor, &ctx),
        )
    }

    pub fn create_pipeline_layout(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUPipelineLayout> {
        flushed(
            &this,
            &ctx,
            self.create_pipeline_layout_inner(descriptor, &ctx),
        )
    }

    pub fn create_bind_group(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUBindGroup> {
        flushed(&this, &ctx, self.create_bind_group_inner(descriptor, &ctx))
    }

    pub fn create_compute_pipeline(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUComputePipeline> {
        self.errors.set_suppress(false);
        flushed(&this, &ctx, {
            let (pipeline, mut invalid) = create_compute_pipeline(self, descriptor, &ctx)?;
            let delayed = self.errors.take_uncaptured();
            if !delayed.is_empty() {
                invalid = true;
                for error in delayed {
                    self.errors.capture(error);
                }
            }
            if invalid {
                self.errors.validation("invalid compute pipeline");
            }
            Ok(pipeline)
        })
    }

    pub async fn create_compute_pipeline_async(
        self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUComputePipeline> {
        self.errors.set_suppress(true);
        let (pipeline, mut invalid) = create_compute_pipeline(&self, descriptor, &ctx)?;
        let delayed = self.errors.take_uncaptured();
        if !delayed.is_empty() {
            invalid = true;
        }
        if invalid {
            if !self.is_destroyed() {
                tokio::task::yield_now().await;
            }
            self.errors.set_suppress(false);
            let _ = self.errors.take_uncaptured();
            if self.is_destroyed() {
                return flushed(&this, &ctx, Ok(pipeline));
            }
            return Err(pipeline_error(&ctx, "invalid compute pipeline"));
        }
        self.errors.set_suppress(false);
        flushed(&this, &ctx, Ok(pipeline))
    }

    pub fn create_render_pipeline(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<render::GPURenderPipeline> {
        self.errors.set_suppress(false);
        flushed(&this, &ctx, {
            let (pipeline, mut invalid) = render::create_pipeline(self, descriptor, &ctx)?;
            let delayed = self.errors.take_uncaptured();
            if invalid {
                for error in delayed {
                    self.errors.capture(error);
                }
                self.errors.validation("invalid render pipeline");
            } else if !pipeline.dummy() && !delayed.is_empty() {
                invalid = true;
                for error in delayed {
                    self.errors.capture(error);
                }
                self.errors.validation("invalid render pipeline");
            }
            let _ = invalid;
            Ok(pipeline)
        })
    }

    pub async fn create_render_pipeline_async(
        self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<render::GPURenderPipeline> {
        self.errors.set_suppress(true);
        let (pipeline, mut invalid) = render::create_pipeline(&self, descriptor, &ctx)?;
        let delayed = self.errors.take_uncaptured();
        if invalid {
            // already invalid
        } else if !pipeline.dummy() && !delayed.is_empty() {
            invalid = true;
        }
        if invalid {
            if !self.is_destroyed() {
                tokio::task::yield_now().await;
            }
            self.errors.set_suppress(false);
            let _ = self.errors.take_uncaptured();
            if self.is_destroyed() {
                return flushed(&this, &ctx, Ok(pipeline));
            }
            return Err(pipeline_error(&ctx, "invalid render pipeline"));
        }
        self.errors.set_suppress(false);
        flushed(&this, &ctx, Ok(pipeline))
    }

    pub fn create_render_bundle_encoder(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<render::GPURenderBundleEncoder> {
        flushed(
            &this,
            &ctx,
            render::new_bundle_encoder(
                &self.device,
                descriptor,
                &ctx,
                self.errors.clone(),
                self.id,
                self.device.limits().max_vertex_buffers,
                self.device.limits().max_immediate_size,
                self.device.limits().max_bind_groups,
            ),
        )
    }

    pub fn create_command_encoder(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUCommandEncoder<'js>> {
        let label = descriptor
            .0
            .flatten()
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        flushed(
            &this,
            &ctx,
            Ok(GPUCommandEncoder {
                device_js: this.0.clone(),
                errors:    self.errors.clone(),
                label:     Rc::new(RefCell::new(label.clone())),
                state:     Rc::new(RefCell::new(EncoderState {
                    encoder:                Some(self.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: (!label.is_empty()).then_some(label.as_str()),
                        },
                    )),
                    open_passes:            0,
                    native_passes:          0,
                    used_destroyed:         Vec::new(),
                    used_mapped:            Vec::new(),
                    invalid:                false,
                    device_id:              self.id,
                    max_compute_workgroups: self
                        .device
                        .limits()
                        .max_compute_workgroups_per_dimension,
                    max_vertex_buffers:     self.device.limits().max_vertex_buffers,
                    max_immediate_size:     self.device.limits().max_immediate_size,
                    max_bind_groups:        self.device.limits().max_bind_groups,
                })),
            }),
        )
    }

    pub fn push_error_scope(&self, filter: String, ctx: Ctx<'_>) -> Result<()> {
        if self.is_destroyed() {
            return Ok(());
        }
        let filter = match filter.as_str() {
            "validation" => GPUErrorKind::Validation,
            "out-of-memory" => GPUErrorKind::OutOfMemory,
            "internal" => GPUErrorKind::Internal,
            _ => return Err(type_error(&ctx, format!("invalid GPUErrorFilter {filter}"))),
        };
        self.errors.lock().scopes.push(ErrorScopeState {
            error: None,
            filter,
        });
        Ok(())
    }

    pub fn pop_error_scope(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let (promise, resolve, reject) = ctx.promise()?;
        if self.is_destroyed() {
            resolve.call::<_, ()>((Value::new_null(ctx.clone()),))?;
            return Ok(promise);
        }
        let scope = self.errors.lock().scopes.pop();
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
) -> Result<(GPUComputePipeline, bool)> {
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
    let compute: Object = descriptor.get("compute")?;
    let module_js = class_value::<GPUShaderModule>(&compute, "module", ctx)?;
    let module_invalid = module_js.borrow().invalid;
    let code = module_js.borrow().code.clone();
    let module_id = module_js.borrow().device_id;
    let limits = device.reported_limits();
    let mismatched = module_id != device.id || layout_id != device.id;
    let module = module_js.borrow().inner.clone();
    let entry_point = compute.get::<_, Option<String>>("entryPoint")?;
    let constants = format::pipeline_constants(compute.get("constants")?, ctx)?;
    let constant_pairs = constants
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect::<Vec<_>>();
    let integer_filter = layout_groups
        .as_ref()
        .is_some_and(|groups| layout_filtering_nonfilterable(groups));
    let shader_invalid =
        format::compute_shader_invalid(&code, entry_point.as_deref(), &constants, &limits)
            || format::storage_texture_access_unsupported(&code, device.device.features())
            || format::naga_immediate_unusable(&code)
            || format::immediate_byte_size(&code) > layout_immediate
            || integer_filter;
    let layout_mismatch = layout_groups
        .as_ref()
        .is_some_and(|groups| layout_shader_mismatch(&code, groups, wgpu::ShaderStages::COMPUTE));
    let invalid = module_invalid || mismatched || shader_invalid || layout_mismatch;
    let auto_layout = layout.is_none();
    let auto_layout_id = if auto_layout { next_resource_id() } else { 0 };
    let stored_groups = layout_groups
        .unwrap_or_else(|| auto_layout_groups(&[(wgpu::ShaderStages::COMPUTE, code.as_ref())]));
    let empty_bgl = device.fallbacks.bind_group_layout.clone();
    let skip_immediate = code.contains("var<immediate");
    let immediate_bytes = format::immediate_byte_size(&code);
    let immediate_slots = if immediate_bytes > format::NAGA_IMMEDIATE_SLOT_BYTES {
        format::immediate_slots_mask(immediate_bytes)
    } else {
        format::immediate_slots_used(&code, entry_point.as_deref(), format::ShaderStage::Compute)
    };
    let skip_storage = code.contains("texture_storage_");
    let shader_mins = Rc::from(format::shader_buffer_min_sizes(&code));
    if invalid || format::uses_external_texture(&code) || skip_storage || skip_immediate {
        let bgls = bind_group_layouts_from_groups(&device.device, &stored_groups, &empty_bgl);
        return Ok((
            GPUComputePipeline {
                inner: device.fallbacks.compute_pipeline.clone(),
                invalid,
                device_id: device.id,
                layout_groups: stored_groups,
                auto_layout,
                auto_layout_id,
                empty_bgl,
                bgls,
                errors: device.errors.clone(),
                max_bind_groups: device.reported_limits().max_bind_groups,
                immediate_slots,
                shader_mins,
                label: Rc::new(RefCell::new(label)),
            },
            invalid,
        ));
    }
    let bgls = bind_group_layouts_from_groups(&device.device, &stored_groups, &empty_bgl);
    device.errors.push(GPUErrorKind::Validation);
    let (inner, gpu_failed) = catch_gpu(&device.errors, || {
        let pipeline = device
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
            });
        let _ = device.device.poll(wgpu::PollType::wait_indefinitely());
        pipeline
    })
    .map_or_else(
        || (device.fallbacks.compute_pipeline.clone(), true),
        |inner| (inner, false),
    );
    let scoped = device.errors.pop();
    if let Some(error) = scoped.as_ref() {
        device.errors.capture(GPUErrorData {
            kind:    GPUErrorKind::Validation,
            message: error.message.clone(),
        });
    }
    let invalid = gpu_failed || scoped.is_some();
    Ok((
        GPUComputePipeline {
            inner,
            invalid,
            device_id: device.id,
            layout_groups: stored_groups,
            auto_layout,
            auto_layout_id,
            empty_bgl,
            bgls,
            errors: device.errors.clone(),
            max_bind_groups: device.reported_limits().max_bind_groups,
            immediate_slots,
            shader_mins,
            label: Rc::new(RefCell::new(label)),
        },
        invalid,
    ))
}

pub(crate) fn layout_shader_mismatch(
    code: &str, groups: &[Rc<[wgpu::BindGroupLayoutEntry]>], stage: wgpu::ShaderStages,
) -> bool {
    let naga_stage = if stage == wgpu::ShaderStages::COMPUTE {
        Some(wgpu::naga::ShaderStage::Compute)
    } else if stage == wgpu::ShaderStages::VERTEX {
        Some(wgpu::naga::ShaderStage::Vertex)
    } else if stage == wgpu::ShaderStages::FRAGMENT {
        Some(wgpu::naga::ShaderStage::Fragment)
    } else {
        None
    };
    let used = naga_stage.and_then(|stage| format::statically_used_bindings(code, stage));
    format::parse_shader_bindings(code).iter().any(|binding| {
        if used.as_ref().is_some_and(|used| {
            !used
                .iter()
                .any(|(group, slot)| *group == binding.group && *slot == binding.binding)
        }) {
            return false;
        }
        let Some(group) = groups.get(binding.group as usize) else {
            return true;
        };
        let Some(entry) = group.iter().find(|entry| entry.binding == binding.binding) else {
            return true;
        };
        !entry.visibility.contains(stage) || !format::shader_binding_matches(binding, entry.ty)
    })
}

fn shader_buffer_too_small(mins: &[(u32, u32, u64)], bound: &BindGroupSet) -> bool {
    mins.iter().any(|&(group, binding, min)| {
        let size = bound
            .slots
            .get(group as usize)
            .and_then(Option::as_ref)
            .and_then(|slot| {
                slot.buffer_sizes
                    .iter()
                    .find(|(slot_binding, _size)| *slot_binding == binding)
                    .map(|(_binding, size)| *size)
            })
            .unwrap_or(0);
        size < min
    })
}

pub(crate) fn layout_filtering_nonfilterable(groups: &[Rc<[wgpu::BindGroupLayoutEntry]>]) -> bool {
    let filtering = groups.iter().any(|group| {
        group.iter().any(|entry| {
            matches!(
                entry.ty,
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
            )
        })
    });
    let nonfilterable = groups.iter().any(|group| {
        group.iter().any(|entry| {
            matches!(entry.ty, wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false }
                    | wgpu::TextureSampleType::Sint
                    | wgpu::TextureSampleType::Uint
                    | wgpu::TextureSampleType::Depth,
                ..
            })
        })
    });
    filtering && nonfilterable
}

fn skip_native_bgl(entries: &[wgpu::BindGroupLayoutEntry], features: wgpu::Features) -> bool {
    entries.iter().any(|entry| {
        let vertex = entry.visibility.contains(wgpu::ShaderStages::VERTEX);
        match entry.ty {
            wgpu::BindingType::StorageTexture { access, format, .. } => {
                let flags = format.guaranteed_format_features(features).flags;
                let unsupported = match access {
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
                };
                let writable = !matches!(access, wgpu::StorageTextureAccess::ReadOnly);
                let vertex_writable = writable
                    && vertex
                    && !features.contains(wgpu::Features::VERTEX_WRITABLE_STORAGE);
                unsupported || vertex_writable
            }
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                multisampled: true,
                ..
            } => true,
            _ => false,
        }
    })
}

fn bind_group_layout_type(
    entry: &Object<'_>, ctx: &Ctx<'_>,
) -> Result<(wgpu::BindingType, BindGroupLayoutKind)> {
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
        return Ok((
            wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: buffer
                    .get::<_, Option<bool>>("hasDynamicOffset")?
                    .unwrap_or_default(),
                min_binding_size: buffer
                    .get::<_, Option<JsU64>>("minBindingSize")?
                    .and_then(|value| NonZeroU64::new(value.0)),
            },
            match ty {
                wgpu::BufferBindingType::Uniform => BindGroupLayoutKind::UniformBuffer,
                wgpu::BufferBindingType::Storage { read_only: false } => {
                    BindGroupLayoutKind::StorageBuffer
                }
                wgpu::BufferBindingType::Storage { read_only: true } => {
                    BindGroupLayoutKind::ReadOnlyStorageBuffer
                }
            },
        ));
    }
    if let Some(sampler) = entry.get::<_, Option<Object>>("sampler")? {
        let name = sampler
            .get::<_, Option<String>>("type")?
            .unwrap_or_else(|| "filtering".into());
        return Ok((
            wgpu::BindingType::Sampler(format::sampler_binding_type(&name, ctx)?),
            if name == "comparison" {
                BindGroupLayoutKind::ComparisonSampler
            } else {
                BindGroupLayoutKind::Sampler
            },
        ));
    }
    if let Some(texture) = entry.get::<_, Option<Object>>("texture")? {
        return Ok((
            wgpu::BindingType::Texture {
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
            },
            BindGroupLayoutKind::Texture,
        ));
    }
    if let Some(storage) = entry.get::<_, Option<Object>>("storageTexture")? {
        return Ok((
            wgpu::BindingType::StorageTexture {
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
            },
            BindGroupLayoutKind::StorageTexture,
        ));
    }
    if entry.get::<_, Option<Object>>("externalTexture")?.is_some() {
        return Ok((
            wgpu::BindingType::Texture {
                sample_type:    wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled:   false,
            },
            BindGroupLayoutKind::ExternalTexture,
        ));
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
        .ok_or_else(|| Exception::throw_range(ctx, "dynamic offset window is out of bounds"))
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

pub(crate) enum BufferMapState {
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
    device:               wgpu::Device,
    #[qjs(skip_trace)]
    errors:               ErrorSink,
    #[qjs(skip_trace)]
    pub(crate) fallbacks: Rc<texture::FallbackResources>,
    #[qjs(skip_trace)]
    host_map:             Rc<RefCell<Option<Vec<u8>>>>,
    #[qjs(skip_trace)]
    pub(crate) inner:     wgpu::Buffer,
    #[qjs(skip_trace)]
    invalid:              bool,
    #[qjs(skip_trace)]
    device_id:            u64,
    #[qjs(skip_trace)]
    label:                Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    map_abort:            Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    map_gen:              Rc<Cell<u64>>,
    #[qjs(skip_trace)]
    native_mapped:        Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    pub(crate) size:      u64,
    #[qjs(skip_trace)]
    pub(crate) state:     Rc<RefCell<BufferMapState>>,
    #[qjs(skip_trace)]
    usage:                wgpu::BufferUsages,
}

impl GPUBuffer {
    fn binding_buffer(&self, _device: &wgpu::Device) -> wgpu::Buffer {
        if self.invalid || matches!(*self.state.borrow(), BufferMapState::Destroyed) {
            self.fallbacks.buffer.clone()
        } else {
            self.inner.clone()
        }
    }

    fn reject_map<'js>(
        &self, ctx: &Ctx<'js>, message: &str, early: bool, name: &str,
    ) -> Result<Promise<'js>> {
        self.errors.validation(message);
        let (promise, _resolve, reject) = ctx.promise()?;
        let error: Value = den_util::construct(ctx, "DOMException", (message, name))?;
        if early {
            reject.call::<_, ()>((error,))?;
        } else {
            ctx.spawn(async move {
                let _ = reject.call::<_, ()>((error,));
            });
        }
        Ok(promise)
    }

    fn reject_map_pending<'js>(&self, ctx: &Ctx<'js>, message: &str) -> Result<Promise<'js>> {
        self.errors.validation(message);
        self.map_abort.set(false);
        let generation = self.map_gen.get().wrapping_add(1);
        self.map_gen.set(generation);
        *self.state.borrow_mut() = BufferMapState::Pending;
        let (promise, _resolve, reject) = ctx.promise()?;
        let abort = self.map_abort.clone();
        let map_gen = self.map_gen.clone();
        let state = self.state.clone();
        let message = message.to_owned();
        let spawn_ctx = ctx.clone();
        ctx.spawn(async move {
            tokio::task::yield_now().await;
            let superseded = map_gen.get() != generation;
            let aborted = abort.get() || superseded;
            if !superseded && matches!(*state.borrow(), BufferMapState::Pending) {
                *state.borrow_mut() = BufferMapState::Unmapped;
            }
            let name = if aborted {
                "AbortError"
            } else {
                "OperationError"
            };
            if let Ok(error) =
                den_util::construct::<_, Value>(&spawn_ctx, "DOMException", (message, name))
            {
                let _ = reject.call::<_, ()>((error,));
            }
        });
        Ok(promise)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl GPUBuffer {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable)]
    pub const fn size(&self) -> u64 { self.size }

    #[qjs(get, configurable)]
    pub const fn usage(&self) -> u32 { self.usage.bits() }

    #[qjs(get, configurable)]
    pub fn map_state(&self) -> &'static str {
        match *self.state.borrow() {
            BufferMapState::Unmapped | BufferMapState::Destroyed => "unmapped",
            BufferMapState::Pending => "pending",
            BufferMapState::Mapped { .. } => "mapped",
        }
    }

    pub fn map_async(
        self, mode: JsU32, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'_>,
    ) -> Result<Promise<'_>> {
        let (mapped_or_pending, destroyed) = {
            let state = self.state.borrow();
            (
                matches!(
                    *state,
                    BufferMapState::Mapped { .. } | BufferMapState::Pending
                ),
                matches!(*state, BufferMapState::Destroyed),
            )
        };
        if mapped_or_pending {
            return self.reject_map(&ctx, "GPUBuffer is not unmapped", true, "OperationError");
        }
        if destroyed {
            return self.reject_map(&ctx, "GPUBuffer is destroyed", false, "OperationError");
        }
        if self.invalid {
            return self.reject_map_pending(&ctx, "GPUBuffer is invalid");
        }
        let kind = match mode.0 {
            MAP_READ if self.usage.contains(wgpu::BufferUsages::MAP_READ) => MappingKind::Read,
            MAP_WRITE if self.usage.contains(wgpu::BufferUsages::MAP_WRITE) => MappingKind::Write,
            MAP_READ | MAP_WRITE => {
                return self.reject_map_pending(&ctx, "buffer usage does not permit this map mode");
            }
            _ => {
                return self
                    .reject_map_pending(&ctx, "mode must be GPUMapMode.READ or GPUMapMode.WRITE");
            }
        };
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size
            .0
            .flatten()
            .map_or_else(|| self.size.saturating_sub(offset), |value| value.0);
        let Some(end) = offset.checked_add(size) else {
            return self.reject_map(&ctx, "mapped range overflows", false, "OperationError");
        };
        if !offset.is_multiple_of(wgpu::MAP_ALIGNMENT) || !size.is_multiple_of(4) || end > self.size
        {
            return self.reject_map(
                &ctx,
                "mapped range must be in bounds, 8-byte aligned, and a multiple of 4",
                false,
                "OperationError",
            );
        }
        let (promise, resolve, reject) = ctx.promise()?;
        if size == 0 {
            *self.state.borrow_mut() = BufferMapState::Mapped {
                kind,
                range: offset..end,
                views: Vec::new(),
            };
            resolve.call::<_, ()>(())?;
            return Ok(promise);
        }
        *self.state.borrow_mut() = BufferMapState::Pending;
        let generation = self.map_gen.get().wrapping_add(1);
        self.map_gen.set(generation);
        self.native_mapped.set(true);
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
        let map_gen = self.map_gen.clone();
        let state = self.state;
        let spawn_ctx = ctx.clone();
        ctx.spawn(async move {
            tokio::task::yield_now().await;
            let outcome =
                match tokio::task::spawn_blocking(move || GpuPoll::until(device, receiver)).await {
                    Err(join) => Err(join.to_string()),
                    Ok(Err(wait)) => Err(wait),
                    Ok(Ok(Err(mapped))) => Err(mapped.to_string()),
                    Ok(Ok(Ok(()))) => Ok(()),
                };
            // A later mapAsync/unmap/destroy bumps map_gen. The old waiter
            // must not rewrite the new map's state or steal its promise.
            if map_gen.get() != generation || !matches!(*state.borrow(), BufferMapState::Pending) {
                let error: Result<Value> = den_util::construct(
                    &spawn_ctx,
                    "DOMException",
                    ("buffer mapping was cancelled", "AbortError"),
                );
                if let Ok(error) = error {
                    let _ = reject.call::<_, ()>((error,));
                }
                return;
            }
            match outcome {
                Ok(()) => {
                    *state.borrow_mut() = BufferMapState::Mapped {
                        kind,
                        range: offset..end,
                        views: Vec::new(),
                    };
                    let _ = resolve.call::<_, ()>(());
                }
                Err(message) => {
                    *state.borrow_mut() = BufferMapState::Unmapped;
                    if let Ok(error) = den_util::construct::<_, Value>(
                        &spawn_ctx,
                        "DOMException",
                        (message, "OperationError"),
                    ) {
                        let _ = reject.call::<_, ()>((error,));
                    }
                }
            }
        });
        Ok(promise)
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
            return Err(operation_error(&ctx, "GPUBuffer is not mapped"));
        };
        let size = size
            .0
            .flatten()
            .map_or_else(|| self.size.saturating_sub(offset), |value| value.0);
        let end = offset
            .checked_add(size)
            .ok_or_else(|| operation_error(&ctx, "mapped range overflows"))?;
        let range = offset..end;
        if !offset.is_multiple_of(wgpu::MAP_ALIGNMENT)
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
        if size == 0 {
            let buffer = ArrayBuffer::new_copy(ctx.clone(), &[] as &[u8])?;
            views.push(MappedView {
                buffer: Persistent::save(&ctx, buffer.clone()),
                range,
            });
            return Ok(buffer);
        }
        let buffer = if let Some(host) = self.host_map.borrow().as_ref() {
            let start = usize::try_from(range.start)
                .map_err(|_error| operation_error(&ctx, "mapped range is too large"))?;
            let end = usize::try_from(range.end)
                .map_err(|_error| operation_error(&ctx, "mapped range is too large"))?;
            let bytes = host
                .get(start..end)
                .ok_or_else(|| operation_error(&ctx, "mapped range is out of bounds"))?;
            ArrayBuffer::new_copy(ctx.clone(), bytes)?
        } else {
            let native = self
                .inner
                .get_mapped_range(range.clone())
                .map_err(|error| operation_error(&ctx, error.to_string()))?;
            ArrayBuffer::new_copy(ctx.clone(), native.as_ref())?
        };
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
                let host = self.host_map.borrow_mut().take();
                for view in views {
                    let mut array = view.buffer.restore(&ctx)?;
                    if host.is_none()
                        && matches!(kind, MappingKind::Write)
                        && let Some(bytes) = array.as_bytes().map(<[u8]>::to_vec)
                    {
                        self.inner
                            .get_mapped_range_mut(view.range)
                            .map_err(|error| operation_error(&ctx, error.to_string()))?
                            .copy_from_slice(&bytes);
                    }
                    array.detach();
                }
                if host.is_none() {
                    self.inner.unmap();
                    self.native_mapped.set(false);
                }
                Ok(())
            }
            BufferMapState::Pending => {
                self.map_abort.set(true);
                self.map_gen.set(self.map_gen.get().wrapping_add(1));
                if self.native_mapped.get() {
                    self.inner.unmap();
                    self.native_mapped.set(false);
                }
                Ok(())
            }
            BufferMapState::Unmapped => Ok(()),
            BufferMapState::Destroyed => {
                *self.state.borrow_mut() = BufferMapState::Destroyed;
                Ok(())
            }
        }
    }

    pub fn destroy(&self, ctx: Ctx<'_>) -> Result<()> {
        if matches!(*self.state.borrow(), BufferMapState::Destroyed) {
            return Ok(());
        }
        let state = std::mem::replace(&mut *self.state.borrow_mut(), BufferMapState::Destroyed);
        if matches!(state, BufferMapState::Pending) {
            self.map_abort.set(true);
            self.map_gen.set(self.map_gen.get().wrapping_add(1));
        }
        if let BufferMapState::Mapped { views, .. } = state {
            for view in views {
                let mut array = view.buffer.restore(&ctx)?;
                array.detach();
            }
        }
        self.host_map.borrow_mut().take();
        self.inner.destroy();
        Ok(())
    }
}

#[derive(Clone, JsLifetime)]
#[rquickjs::class(rename = "GPUQueue")]
pub struct GPUQueue<'js> {
    #[qjs(skip_trace)]
    device:    Option<wgpu::Device>,
    device_js: Rc<RefCell<Option<Class<'js, GPUDevice<'js>>>>>,
    #[qjs(skip_trace)]
    errors:    ErrorSink,
    #[qjs(skip_trace)]
    inner:     wgpu::Queue,
    #[qjs(skip_trace)]
    label:     Rc<RefCell<String>>,
}

impl<'js> Trace<'js> for GPUQueue<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(device) = self.device_js.try_borrow()
            && let Some(device) = device.as_ref()
        {
            device.trace(tracer);
        }
    }
}

impl<'js> GPUQueue<'js> {
    fn flush(&self, ctx: &Ctx<'js>) -> Result<()> {
        if let Some(device) = self.device_js.borrow().as_ref() {
            flush_uncaptured(device, ctx)?;
        }
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUQueue<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn write_buffer(
        &self, buffer: Class<'js, GPUBuffer>, buffer_offset: JsU64, data: Value<'js>,
        data_offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        if matches!(*buffer.state.borrow(), BufferMapState::Destroyed) {
            self.errors.validation("GPUBuffer is destroyed");
            return self.flush(&ctx);
        }
        if !matches!(*buffer.state.borrow(), BufferMapState::Unmapped) {
            self.errors.validation("GPUBuffer is mapped");
        }
        if !buffer.usage.contains(wgpu::BufferUsages::COPY_DST) {
            self.errors
                .validation("buffer does not have COPY_DST usage");
        }
        if !buffer_offset.0.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            self.errors
                .validation("bufferOffset must be a multiple of 4");
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
            self.errors
                .validation("destination buffer range is out of bounds");
            return self.flush(&ctx);
        }
        let Some(data) = bytes.get(start as usize..end as usize) else {
            self.errors.validation("data range is out of bounds");
            return self.flush(&ctx);
        };
        self.inner
            .write_buffer(&buffer.inner, buffer_offset.0, data);
        self.flush(&ctx)
    }

    pub fn write_texture(
        &self, destination: Object<'js>, data: Value<'js>, data_layout: Object<'js>,
        size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(destination, &ctx)?;
        let layout = texture::texel_copy_layout(&data_layout)?;
        let size = format::extent3d(size, &ctx)?;
        let bytes = BufferSource::from_js(&ctx, data)?.into_bytes();
        let texture = texture.borrow();
        if texture.destroyed.get() || texture.is_dummy {
            self.errors
                .validation("writeTexture destination texture is invalid or destroyed");
            return self.flush(&ctx);
        }
        if !texture.usage.contains(wgpu::TextureUsages::COPY_DST) {
            self.errors
                .validation("writeTexture destination must have COPY_DST");
            return self.flush(&ctx);
        }
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
        self.flush(&ctx)
    }

    pub fn submit(&self, command_buffers: Array<'js>, ctx: Ctx<'js>) -> Result<()> {
        let buffers = command_buffers
            .iter::<Class<GPUCommandBuffer>>()
            .collect::<Result<Vec<_>>>()?;
        let mut seen = HashSet::with_capacity(buffers.len());
        let mut native = Vec::with_capacity(buffers.len());
        for buffer in &buffers {
            let state = buffer.borrow().inner.clone();
            if !seen.insert(Rc::as_ptr(&state)) {
                self.errors
                    .validation("command buffer listed more than once");
                continue;
            }
            let Some(command) = state.borrow_mut().take() else {
                self.errors
                    .validation("command buffer was already submitted");
                continue;
            };
            if buffer
                .borrow()
                .used_destroyed
                .iter()
                .any(|destroyed| destroyed.get())
            {
                self.errors
                    .validation("destroyed texture used in submitted commands");
            }
            if buffer
                .borrow()
                .used_mapped
                .iter()
                .any(|mapped| matches!(*mapped.borrow(), BufferMapState::Destroyed))
            {
                self.errors
                    .validation("destroyed buffer used in submitted commands");
            }
            if buffer.borrow().used_mapped.iter().any(|mapped| {
                !matches!(
                    *mapped.borrow(),
                    BufferMapState::Unmapped | BufferMapState::Destroyed
                )
            }) {
                self.errors.validation("copy buffers must be unmapped");
            }
            native.push(command);
        }
        self.inner.submit(native);
        self.flush(&ctx)
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
    pub(crate) inner:     wgpu::ShaderModule,
    #[qjs(skip_trace)]
    pub(crate) invalid:   bool,
    #[qjs(skip_trace)]
    pub(crate) code:      Rc<str>,
    #[qjs(skip_trace)]
    pub(crate) device_id: u64,
    #[qjs(skip_trace)]
    label:                Rc<RefCell<String>>,
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

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUBindGroupLayout")]
pub struct GPUBindGroupLayout {
    #[qjs(skip_trace)]
    pub(crate) inner:          wgpu::BindGroupLayout,
    #[qjs(skip_trace)]
    pub(crate) label:          Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) kinds:          Rc<[(u32, BindGroupLayoutKind)]>,
    #[qjs(skip_trace)]
    pub(crate) entries:        Rc<[wgpu::BindGroupLayoutEntry]>,
    #[qjs(skip_trace)]
    pub(crate) auto:           bool,
    #[qjs(skip_trace)]
    pub(crate) auto_layout_id: u64,
}

#[rquickjs::methods]
impl GPUBindGroupLayout {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUPipelineLayout")]
pub struct GPUPipelineLayout {
    #[qjs(skip_trace)]
    pub(crate) inner:          wgpu::PipelineLayout,
    #[qjs(skip_trace)]
    pub(crate) device_id:      u64,
    #[qjs(skip_trace)]
    pub(crate) groups:         PipelineLayoutGroups,
    #[qjs(skip_trace)]
    pub(crate) immediate_size: u32,
    #[qjs(skip_trace)]
    pub(crate) label:          Rc<RefCell<String>>,
}

#[rquickjs::methods]
impl GPUPipelineLayout {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}
#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUBindGroup")]
pub struct GPUBindGroup {
    #[qjs(skip_trace)]
    pub(crate) inner:          wgpu::BindGroup,
    #[qjs(skip_trace)]
    label:                     Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    pub(crate) invalid:        bool,
    #[qjs(skip_trace)]
    pub(crate) auto:           bool,
    #[qjs(skip_trace)]
    pub(crate) auto_layout_id: u64,
    #[qjs(skip_trace)]
    pub(crate) device_id:      u64,
    #[qjs(skip_trace)]
    dynamic_count:             u32,
    #[qjs(skip_trace)]
    used_destroyed:            Vec<Rc<Cell<bool>>>,
    #[qjs(skip_trace)]
    used_mapped:               Vec<Rc<RefCell<BufferMapState>>>,
    #[qjs(skip_trace)]
    fingerprints:              Vec<(u32, u32, u64)>,
    #[qjs(skip_trace)]
    buffer_sizes:              Rc<[(u32, u64)]>,
}

impl Drop for GPUBindGroup {
    fn drop(&mut self) {
        BIND_GROUP_WINDOWS.with(|table| {
            table
                .borrow_mut()
                .remove(&(self.fingerprints.as_ptr() as u64));
        });
    }
}

#[rquickjs::methods]
impl GPUBindGroup {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
}

fn kind_from_binding_type(ty: wgpu::BindingType) -> BindGroupLayoutKind {
    match ty {
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            ..
        } => BindGroupLayoutKind::UniformBuffer,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            ..
        } => BindGroupLayoutKind::StorageBuffer,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            ..
        } => BindGroupLayoutKind::ReadOnlyStorageBuffer,
        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison) => {
            BindGroupLayoutKind::ComparisonSampler
        }
        wgpu::BindingType::Sampler(_) => BindGroupLayoutKind::Sampler,
        wgpu::BindingType::Texture { .. } | wgpu::BindingType::AccelerationStructure { .. } => {
            BindGroupLayoutKind::Texture
        }
        wgpu::BindingType::StorageTexture { .. } => BindGroupLayoutKind::StorageTexture,
        wgpu::BindingType::ExternalTexture => BindGroupLayoutKind::ExternalTexture,
    }
}

pub(crate) fn bind_group_layouts_from_groups(
    device: &wgpu::Device, groups: &PipelineLayoutGroups, empty: &wgpu::BindGroupLayout,
) -> Rc<[wgpu::BindGroupLayout]> {
    let features = device.features();
    Rc::from(
        groups
            .iter()
            .map(|entries| {
                if entries.is_empty() || skip_native_bgl(entries, features) {
                    empty.clone()
                } else {
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: None,
                        entries,
                    })
                }
            })
            .collect::<Vec<_>>(),
    )
}

fn texture_binding_use(
    entries: &[wgpu::BindGroupLayoutEntry], binding: u32,
) -> Option<(bool, TextureUseKind)> {
    entries
        .iter()
        .find(|entry| entry.binding == binding)
        .and_then(|entry| {
            match entry.ty {
                wgpu::BindingType::Texture { .. } => Some((false, TextureUseKind::Sampled)),
                wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::ReadOnly,
                    ..
                } => Some((false, TextureUseKind::Storage)),
                wgpu::BindingType::StorageTexture { .. } => Some((true, TextureUseKind::Storage)),
                _ => None,
            }
        })
}

fn kinds_from_entries(entries: &[wgpu::BindGroupLayoutEntry]) -> Rc<[(u32, BindGroupLayoutKind)]> {
    Rc::from(
        entries
            .iter()
            .map(|entry| (entry.binding, kind_from_binding_type(entry.ty)))
            .collect::<Vec<_>>(),
    )
}

fn bind_group_extras(group: &GPUBindGroup) -> BindGroupExtras {
    let key = group.fingerprints.as_ptr() as u64;
    BIND_GROUP_WINDOWS.with(|table| {
        table.borrow().get(&key).cloned().unwrap_or_else(|| {
            BindGroupExtras {
                windows:  Rc::from(Vec::new()),
                textures: Rc::from(Vec::new()),
            }
        })
    })
}

fn dynamic_windows(group: &GPUBindGroup) -> Rc<[DynamicWindow]> { bind_group_extras(group).windows }

pub(crate) fn bind_group_texture_uses(group: &GPUBindGroup) -> Rc<[TextureUse]> {
    bind_group_extras(group).textures
}

fn dynamic_offsets_invalid(group: &GPUBindGroup, offsets: &[u32]) -> bool {
    if offsets.len() as u32 != group.dynamic_count {
        return true;
    }
    if group.dynamic_count == 0 {
        return false;
    }
    let windows = dynamic_windows(group);
    if windows.len() != offsets.len() {
        return true;
    }
    offsets.iter().zip(windows.iter()).any(
        |(&offset, &(align, bind_offset, binding_size, buffer_size))| {
            let offset = u64::from(offset);
            if align != 0 && !offset.is_multiple_of(u64::from(align)) {
                return true;
            }
            let Some(end) = bind_offset
                .checked_add(offset)
                .and_then(|start| start.checked_add(binding_size))
            else {
                return true;
            };
            end > buffer_size
        },
    )
}

pub(crate) struct BindGroupApply {
    pub invalid:        bool,
    pub used_destroyed: Vec<Rc<Cell<bool>>>,
    pub used_mapped:    Vec<Rc<RefCell<BufferMapState>>>,
}

impl BindGroupSet {
    pub(crate) fn apply(
        &mut self, index: u32, group: Option<&GPUBindGroup>, offsets: &[u32], device_id: u64,
        max_bind_groups: u32,
    ) -> BindGroupApply {
        if index >= max_bind_groups {
            return BindGroupApply {
                invalid:        true,
                used_destroyed: Vec::new(),
                used_mapped:    Vec::new(),
            };
        }
        let slot = index as usize;
        if self.slots.len() <= slot {
            self.slots.resize_with(slot + 1, || None);
        }
        let Some(group) = group else {
            if let Some(entry) = self.slots.get_mut(slot) {
                *entry = None;
            }
            return BindGroupApply {
                invalid:        false,
                used_destroyed: Vec::new(),
                used_mapped:    Vec::new(),
            };
        };
        let invalid = group.invalid
            || group.device_id != device_id
            || dynamic_offsets_invalid(group, offsets);
        if let Some(entry) = self.slots.get_mut(slot) {
            *entry = Some(BindGroupSlot {
                invalid,
                auto: group.auto,
                auto_layout_id: group.auto_layout_id,
                entries: Rc::from(group.fingerprints.clone()),
                buffer_sizes: group.buffer_sizes.clone(),
                textures: bind_group_extras(group).textures,
            });
        }
        BindGroupApply {
            invalid,
            used_destroyed: group.used_destroyed.clone(),
            used_mapped: group.used_mapped.clone(),
        }
    }

    pub(crate) fn texture_usage_conflict(
        &self, pipeline_groups: Option<&PipelineLayoutGroups>, storage_alias: bool,
    ) -> bool {
        let uses = self
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _slot)| {
                pipeline_groups.is_none_or(|groups| {
                    groups
                        .get(*index)
                        .is_some_and(|entries| !entries.is_empty())
                })
            })
            .filter_map(|(_index, slot)| slot.as_ref())
            .flat_map(|slot| slot.textures.iter().copied())
            .collect::<Vec<_>>();
        texture_uses_conflict(&uses, storage_alias)
    }
}

pub(crate) fn texture_uses_conflict(uses: &[TextureUse], storage_alias: bool) -> bool {
    uses.iter().enumerate().any(|(index, left)| {
        uses.get(index + 1..)
            .into_iter()
            .flatten()
            .any(|right| left.conflicts(*right, storage_alias))
    })
}

impl TextureUse {
    pub(crate) fn ranges_overlap(self, other: Self) -> bool {
        self.id == other.id
            && self.mip_base < other.mip_base.saturating_add(other.mip_count)
            && other.mip_base < self.mip_base.saturating_add(self.mip_count)
            && self.layer_base < other.layer_base.saturating_add(other.layer_count)
            && other.layer_base < self.layer_base.saturating_add(self.layer_count)
    }

    pub(crate) fn aspects_overlap(self, other: Self) -> bool {
        matches!(self.aspect, wgpu::TextureAspect::All)
            || matches!(other.aspect, wgpu::TextureAspect::All)
            || self.aspect == other.aspect
    }

    pub(crate) fn with_aspect(mut self, aspect: wgpu::TextureAspect) -> Self {
        self.aspect = aspect;
        self
    }

    // wgpu 30 records DEPTH_STENCIL_WRITE for a DS attachment unless both
    // aspects are read-only, so a sampled bind group leftovers even when the
    // sampled aspect is spec-compatible with a read-only aspect.
    pub(crate) fn native_resource_leftover(self, binding: Self) -> bool {
        self.kind == TextureUseKind::Attachment
            && self.write
            && binding.kind != TextureUseKind::Attachment
            && !binding.write
            && self.ranges_overlap(binding)
    }

    pub(crate) fn conflicts(self, other: Self, storage_alias: bool) -> bool {
        if !self.ranges_overlap(other) || !self.aspects_overlap(other) {
            return false;
        }
        if self.kind == other.kind {
            return match self.kind {
                TextureUseKind::Sampled => false,
                TextureUseKind::Storage => {
                    self.write != other.write || storage_alias && self.write && other.write
                }
                TextureUseKind::Attachment => true,
            };
        }
        let attachment_storage = matches!(
            (self.kind, other.kind),
            (TextureUseKind::Attachment, TextureUseKind::Storage)
                | (TextureUseKind::Storage, TextureUseKind::Attachment)
        );
        if attachment_storage {
            true
        } else {
            self.write || other.write
        }
    }

    pub(crate) fn from_view(
        view: &crate::texture::GPUTextureView, write: bool, kind: TextureUseKind,
    ) -> Self {
        Self {
            id: Rc::as_ptr(&view.destroyed) as usize,
            write,
            kind,
            aspect: view.aspect(),
            mip_base: view.base_mip,
            mip_count: view.mip_count,
            layer_base: view.base_layer,
            layer_count: view.layer_count,
        }
    }

    pub(crate) fn from_texture(
        texture: &crate::texture::GPUTexture, write: bool, kind: TextureUseKind,
    ) -> Self {
        Self {
            id: Rc::as_ptr(&texture.destroyed) as usize,
            write,
            kind,
            aspect: wgpu::TextureAspect::All,
            mip_base: 0,
            mip_count: texture.mip_levels,
            layer_base: 0,
            layer_count: texture.size.depth_or_array_layers.max(1),
        }
    }
}

pub(crate) fn skip_native_texture_bind(attachments: &[TextureUse], extras: &[TextureUse]) -> bool {
    extras.iter().any(|used| {
        attachments.iter().any(|attachment| {
            used.conflicts(*attachment, false) || attachment.native_resource_leftover(*used)
        })
    })
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePipeline")]
pub struct GPUComputePipeline {
    #[qjs(skip_trace)]
    inner:           wgpu::ComputePipeline,
    #[qjs(skip_trace)]
    invalid:         bool,
    #[qjs(skip_trace)]
    device_id:       u64,
    #[qjs(skip_trace)]
    layout_groups:   PipelineLayoutGroups,
    #[qjs(skip_trace)]
    auto_layout:     bool,
    #[qjs(skip_trace)]
    auto_layout_id:  u64,
    #[qjs(skip_trace)]
    empty_bgl:       wgpu::BindGroupLayout,
    #[qjs(skip_trace)]
    bgls:            Rc<[wgpu::BindGroupLayout]>,
    #[qjs(skip_trace)]
    errors:          ErrorSink,
    #[qjs(skip_trace)]
    max_bind_groups: u32,
    #[qjs(skip_trace)]
    immediate_slots: u64,
    #[qjs(skip_trace)]
    shader_mins:     Rc<[(u32, u32, u64)]>,
    #[qjs(skip_trace)]
    label:           Rc<RefCell<String>>,
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
        if index.0 >= self.max_bind_groups {
            self.errors
                .validation("bind group layout index is out of range");
        }
        let entries = self
            .layout_groups
            .get(index.0 as usize)
            .cloned()
            .unwrap_or_else(|| Rc::from(Vec::new()));
        let inner = if !self.invalid && !entries.is_empty() {
            self.inner.get_bind_group_layout(index.0)
        } else {
            self.bgls
                .get(index.0 as usize)
                .cloned()
                .unwrap_or_else(|| self.empty_bgl.clone())
        };
        GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(String::new())),
            kinds: kinds_from_entries(&entries),
            entries,
            auto: self.auto_layout,
            auto_layout_id: self.auto_layout_id,
        }
    }
}

pub(crate) struct EncoderState {
    pub(crate) encoder:                Option<wgpu::CommandEncoder>,
    pub(crate) open_passes:            usize,
    pub(crate) native_passes:          u32,
    pub(crate) used_destroyed:         Vec<Rc<Cell<bool>>>,
    pub(crate) used_mapped:            Vec<Rc<RefCell<BufferMapState>>>,
    pub(crate) invalid:                bool,
    pub(crate) device_id:              u64,
    pub(crate) max_compute_workgroups: u32,
    pub(crate) max_vertex_buffers:     u32,
    pub(crate) max_immediate_size:     u32,
    pub(crate) max_bind_groups:        u32,
}

/// radv rejects a command stream with tens of thousands of timestamp passes.
const MAX_NATIVE_PASSES: u32 = 256;

#[derive(Clone, JsLifetime)]
#[rquickjs::class(rename = "GPUCommandEncoder")]
pub struct GPUCommandEncoder<'js> {
    device_js: Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    errors:    ErrorSink,
    #[qjs(skip_trace)]
    label:     Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state:     Rc<RefCell<EncoderState>>,
}

impl<'js> Trace<'js> for GPUCommandEncoder<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.device_js.trace(tracer); }
}

impl<'js> GPUCommandEncoder<'js> {
    fn flush(&self, ctx: &Ctx<'js>) -> Result<()> { flush_uncaptured(&self.device_js, ctx) }

    fn with_encoder<T: Default>(
        &self, ctx: &Ctx<'js>, operation: impl FnOnce(&mut wgpu::CommandEncoder) -> T,
    ) -> Result<T> {
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            self.errors.validation("GPUCommandEncoder has an open pass");
            self.flush(ctx)?;
            return Ok(T::default());
        }
        if let Some(encoder) = state.encoder.as_mut() {
            let value = operation(encoder);
            drop(state);
            self.flush(ctx)?;
            Ok(value)
        } else {
            self.errors
                .validation("GPUCommandEncoder is already finished");
            drop(state);
            self.flush(ctx)?;
            Ok(T::default())
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUCommandEncoder<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn begin_compute_pass(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>,
    ) -> Result<GPUComputePassEncoder> {
        let descriptor = descriptor.0.flatten();
        let label = descriptor
            .as_ref()
            .map(crate::label)
            .transpose()?
            .unwrap_or_default();
        let timestamp = descriptor
            .as_ref()
            .map(|object| object.get::<_, Option<Object>>("timestampWrites"))
            .transpose()?
            .flatten()
            .map(|writes| query::timestamp_writes_from(&writes, &ctx, self.device_js.borrow().id))
            .transpose()?;
        let mut state = self.state.borrow_mut();
        if timestamp
            .as_ref()
            .is_some_and(|(_query, _beginning, _end, invalid)| *invalid)
        {
            state.invalid = true;
        }
        if state.open_passes != 0 {
            state.invalid = true;
            drop(state);
            return Ok(GPUComputePassEncoder {
                errors: self.errors.clone(),
                label:  Rc::new(RefCell::new(label)),
                state:  Rc::new(RefCell::new(ComputePassState::new(
                    self.state.clone(),
                    None,
                    false,
                ))),
            });
        }
        let native = state.native_passes < MAX_NATIVE_PASSES;
        if native {
            state.native_passes += 1;
        }
        let Some(encoder) = state.encoder.as_mut() else {
            self.errors
                .validation("GPUCommandEncoder is already finished");
            drop(state);
            self.flush(&ctx)?;
            return Ok(GPUComputePassEncoder {
                errors: self.errors.clone(),
                label:  Rc::new(RefCell::new(label)),
                state:  Rc::new(RefCell::new(ComputePassState::new(
                    self.state.clone(),
                    None,
                    false,
                ))),
            });
        };
        let timestamp_query;
        let timestamp_writes = match timestamp {
            Some((query, beginning, end, false)) => {
                timestamp_query = query;
                Some(wgpu::ComputePassTimestampWrites {
                    query_set:                     &timestamp_query,
                    beginning_of_pass_write_index: beginning,
                    end_of_pass_write_index:       end,
                })
            }
            Some(_) | None => None,
        };
        let pass = native.then(|| {
            encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: (!label.is_empty()).then_some(label.as_str()),
                    timestamp_writes,
                })
                .forget_lifetime()
        });
        state.open_passes = 1;
        drop(state);
        Ok(GPUComputePassEncoder {
            errors: self.errors.clone(),
            label:  Rc::new(RefCell::new(label)),
            state:  Rc::new(RefCell::new(ComputePassState::new(
                self.state.clone(),
                pass,
                true,
            ))),
        })
    }

    pub fn begin_render_pass(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderPassEncoder> {
        let mut attachments = Vec::new();
        record_attachment_liveness(&descriptor, &mut attachments, &ctx)?;
        let mut state = self.state.borrow_mut();
        if state.open_passes != 0 {
            state.invalid = true;
            drop(state);
            return Ok(render::GPURenderPassEncoder::finished(
                crate::label(&descriptor)?,
                self.state.clone(),
                self.errors.clone(),
            ));
        }
        state.used_destroyed.extend(attachments);
        let native = state.native_passes < MAX_NATIVE_PASSES;
        if native {
            state.native_passes += 1;
        }
        let Some(encoder) = state.encoder.as_mut() else {
            self.errors
                .validation("GPUCommandEncoder is already finished");
            drop(state);
            self.flush(&ctx)?;
            return Ok(render::GPURenderPassEncoder::finished(
                crate::label(&descriptor)?,
                self.state.clone(),
                self.errors.clone(),
            ));
        };
        let begun = render::begin_render_pass(
            encoder,
            descriptor,
            &ctx,
            self.device_js.borrow().id,
            native,
        )?;
        if begun.invalid {
            state.invalid = true;
        }
        if let Some(destroyed) = begun.occlusion_destroyed.clone() {
            state.used_destroyed.push(destroyed);
        }
        state.open_passes = 1;
        drop(state);
        Ok(render::GPURenderPassEncoder::new(
            self.state.clone(),
            self.errors.clone(),
            begun,
        ))
    }

    pub fn copy_buffer_to_buffer(
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
        self.state
            .borrow_mut()
            .used_mapped
            .push(source.state.clone());
        self.state
            .borrow_mut()
            .used_mapped
            .push(destination.state.clone());
        let size = size.map(|value| value.0);
        let usage_invalid = !source.usage.contains(wgpu::BufferUsages::COPY_SRC)
            || !destination.usage.contains(wgpu::BufferUsages::COPY_DST);
        let align_invalid = !source_offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || !destination_offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || size.is_some_and(|size| !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT));
        let range_invalid = size.is_some_and(|size| {
            source_offset
                .checked_add(size)
                .is_none_or(|end| end > source.size)
                || destination_offset
                    .checked_add(size)
                    .is_none_or(|end| end > destination.size)
        });
        let invalid = source.invalid
            || destination.invalid
            || usage_invalid
            || align_invalid
            || range_invalid;
        if invalid {
            self.state.borrow_mut().invalid = true;
            self.flush(&ctx)?;
            return Ok(());
        }
        if !matches!(*source.state.borrow(), BufferMapState::Unmapped)
            || !matches!(*destination.state.borrow(), BufferMapState::Unmapped)
        {
            self.flush(&ctx)?;
            return Ok(());
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

    pub fn copy_buffer_to_texture(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (buffer, layout) = texture::texel_copy_buffer(source, &ctx)?;
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let buffer = buffer.borrow();
        let texture = texture.borrow();
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        self.state
            .borrow_mut()
            .used_destroyed
            .push(texture.destroyed.clone());
        let buffer_mapped = matches!(
            *buffer.state.borrow(),
            BufferMapState::Mapped { .. } | BufferMapState::Pending
        );
        let buffer_destroyed = matches!(*buffer.state.borrow(), BufferMapState::Destroyed);
        if buffer_mapped || buffer.invalid || texture.is_dummy {
            self.state.borrow_mut().invalid = true;
        }
        if buffer_mapped
            || buffer_destroyed
            || buffer.invalid
            || texture.is_dummy
            || texture.destroyed.get()
        {
            self.flush(&ctx)?;
            return Ok(());
        }
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

    pub fn copy_texture_to_buffer(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(source, &ctx)?;
        let (buffer, layout) = texture::texel_copy_buffer(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let buffer = buffer.borrow();
        let texture = texture.borrow();
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        self.state
            .borrow_mut()
            .used_destroyed
            .push(texture.destroyed.clone());
        let buffer_mapped = matches!(
            *buffer.state.borrow(),
            BufferMapState::Mapped { .. } | BufferMapState::Pending
        );
        let buffer_destroyed = matches!(*buffer.state.borrow(), BufferMapState::Destroyed);
        if buffer_mapped || buffer.invalid || texture.is_dummy {
            self.state.borrow_mut().invalid = true;
        }
        if buffer_mapped
            || buffer_destroyed
            || buffer.invalid
            || texture.is_dummy
            || texture.destroyed.get()
        {
            self.flush(&ctx)?;
            return Ok(());
        }
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

    pub fn copy_texture_to_texture(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (source_texture, source_mip, source_origin, source_aspect) =
            texture::texel_copy_texture(source, &ctx)?;
        let (destination_texture, destination_mip, destination_origin, destination_aspect) =
            texture::texel_copy_texture(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let source_texture = source_texture.borrow();
        let destination_texture = destination_texture.borrow();
        self.state
            .borrow_mut()
            .used_destroyed
            .push(source_texture.destroyed.clone());
        self.state
            .borrow_mut()
            .used_destroyed
            .push(destination_texture.destroyed.clone());
        let device_id = self.state.borrow().device_id;
        let finish_invalid = source_texture.is_dummy
            || destination_texture.is_dummy
            || source_texture.device_id != device_id
            || destination_texture.device_id != device_id
            || !source_texture.usage.contains(wgpu::TextureUsages::COPY_SRC)
            || !destination_texture
                .usage
                .contains(wgpu::TextureUsages::COPY_DST);
        let destroyed = source_texture.destroyed.get() || destination_texture.destroyed.get();
        if finish_invalid {
            self.state.borrow_mut().invalid = true;
        }
        if finish_invalid || destroyed {
            self.flush(&ctx)?;
            return Ok(());
        }
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

    pub fn resolve_query_set(
        &self, query_set: Class<'js, query::GPUQuerySet>, first_query: JsU32, query_count: JsU32,
        destination: Class<'js, GPUBuffer>, destination_offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let query_set = query_set.borrow();
        let destination = destination.borrow();
        let dest_mapped = matches!(
            *destination.state.borrow(),
            BufferMapState::Mapped { .. } | BufferMapState::Pending
        );
        let dest_destroyed = matches!(*destination.state.borrow(), BufferMapState::Destroyed);
        let query_end = first_query.0.saturating_add(query_count.0);
        let dest_bytes = u64::from(query_count.0).saturating_mul(8);
        let dest_end = destination_offset.0.saturating_add(dest_bytes);
        let range_invalid = query_end > query_set.count
            || !destination_offset.0.is_multiple_of(256)
            || dest_end > destination.size;
        let encode_invalid = query_set.invalid
            || destination.invalid
            || dest_mapped
            || !destination
                .usage
                .contains(wgpu::BufferUsages::QUERY_RESOLVE)
            || range_invalid
            || query_set.device_id != self.state.borrow().device_id
            || destination.device_id != self.state.borrow().device_id;
        if dest_mapped {
            self.errors
                .validation("resolve destination must be unmapped");
        }
        if encode_invalid {
            self.state.borrow_mut().invalid = true;
        }
        if dest_destroyed {
            self.state
                .borrow_mut()
                .used_mapped
                .push(destination.state.clone());
        }
        if query_set.destroyed.get() {
            self.state
                .borrow_mut()
                .used_destroyed
                .push(query_set.destroyed.clone());
        }
        if encode_invalid || dest_destroyed || query_set.destroyed.get() {
            return Ok(());
        }
        self.with_encoder(&ctx, |encoder| {
            encoder.resolve_query_set(
                &query_set.inner,
                first_query.0..query_end,
                &destination.inner,
                destination_offset.0,
            );
        })
    }

    pub fn clear_buffer(
        &self, buffer: Class<'js, GPUBuffer>, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow();
        self.state
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        let range_invalid = !offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || size.is_some_and(|size| !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT))
            || size
                .unwrap_or_else(|| buffer.size.saturating_sub(offset))
                .checked_add(offset)
                .is_none_or(|end| end > buffer.size);
        let invalid =
            buffer.invalid || !buffer.usage.contains(wgpu::BufferUsages::COPY_DST) || range_invalid;
        if invalid {
            self.state.borrow_mut().invalid = true;
            self.flush(&ctx)?;
            return Ok(());
        }
        if !matches!(*buffer.state.borrow(), BufferMapState::Unmapped) {
            self.flush(&ctx)?;
            return Ok(());
        }
        self.with_encoder(&ctx, |encoder| {
            encoder.clear_buffer(&buffer.inner, offset, size)
        })
    }

    pub fn finish(
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
            self.errors.validation("GPUCommandEncoder has an open pass");
        }
        if state.invalid {
            self.errors.validation("GPUCommandEncoder is invalid");
        }
        let Some(encoder) = state.encoder.take() else {
            self.errors
                .validation("GPUCommandEncoder is already finished");
            drop(state);
            self.flush(&ctx)?;
            return Ok(GPUCommandBuffer {
                inner:          Rc::new(RefCell::new(None)),
                label:          Rc::new(RefCell::new(label)),
                used_destroyed: Rc::from(Vec::new()),
                used_mapped:    Rc::from(Vec::new()),
            });
        };
        let used_destroyed =
            Rc::<[Rc<Cell<bool>>]>::from(std::mem::take(&mut state.used_destroyed));
        let used_mapped =
            Rc::<[Rc<RefCell<BufferMapState>>]>::from(std::mem::take(&mut state.used_mapped));
        let buffer = GPUCommandBuffer {
            inner: Rc::new(RefCell::new(Some(encoder.finish()))),
            label: Rc::new(RefCell::new(label)),
            used_destroyed,
            used_mapped,
        };
        drop(state);
        self.flush(&ctx)?;
        Ok(buffer)
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.with_encoder(&ctx, |encoder| encoder.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'js>) -> Result<()> {
        self.with_encoder(&ctx, wgpu::CommandEncoder::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.with_encoder(&ctx, |encoder| encoder.insert_debug_marker(&value))
    }
}

struct ComputePassState {
    parent:             Rc<RefCell<EncoderState>>,
    pass:               Option<wgpu::ComputePass<'static>>,
    open:               bool,
    bound_groups:       BindGroupSet,
    pipeline_groups:    PipelineLayoutGroups,
    pipeline_auto:      bool,
    pipeline_auto_id:   u64,
    pipeline_bound:     bool,
    immediate_required: u64,
    immediate_filled:   u64,
    shader_mins:        Rc<[(u32, u32, u64)]>,
}

impl ComputePassState {
    fn new(
        parent: Rc<RefCell<EncoderState>>, pass: Option<wgpu::ComputePass<'static>>, open: bool,
    ) -> Self {
        Self {
            parent,
            pass,
            open,
            bound_groups: BindGroupSet::default(),
            pipeline_groups: empty_layout_groups(),
            pipeline_auto: false,
            pipeline_auto_id: 0,
            pipeline_bound: false,
            immediate_required: 0,
            immediate_filled: 0,
            shader_mins: Rc::from(Vec::new()),
        }
    }
}

impl ComputePassState {
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

impl Drop for ComputePassState {
    fn drop(&mut self) { self.end(); }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePassEncoder")]
pub struct GPUComputePassEncoder {
    #[qjs(skip_trace)]
    errors: ErrorSink,
    #[qjs(skip_trace)]
    label:  Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    state:  Rc<RefCell<ComputePassState>>,
}

impl GPUComputePassEncoder {
    fn mark_invalid(&self) { self.state.borrow_mut().parent.borrow_mut().invalid = true; }

    fn compute_bind_mismatch(&self) -> bool {
        let state = self.state.borrow();
        if !state.pipeline_bound {
            return true;
        }
        pipeline_bind_groups_mismatch(
            &state.pipeline_groups,
            state.pipeline_auto,
            state.pipeline_auto_id,
            &state.bound_groups,
        )
    }

    fn skip_native_dispatch(&self) -> bool {
        let missing = {
            let state = self.state.borrow();
            immediates_unfilled(state.immediate_required, state.immediate_filled)
        };
        let too_small = {
            let state = self.state.borrow();
            shader_buffer_too_small(&state.shader_mins, &state.bound_groups)
        };
        let usage_conflict = {
            let state = self.state.borrow();
            state
                .bound_groups
                .texture_usage_conflict(Some(&state.pipeline_groups), true)
        };
        if self.compute_bind_mismatch() || missing || too_small || usage_conflict {
            self.mark_invalid();
            return true;
        }
        let (skip_immediates, parent_invalid) = {
            let state = self.state.borrow();
            (state.immediate_required != 0, state.parent.borrow().invalid)
        };
        skip_immediates || parent_invalid
    }

    fn ensure_open(&self) -> bool {
        if self.state.borrow().open {
            true
        } else {
            self.errors
                .validation("GPUComputePassEncoder is already ended");
            false
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "JS methods return Result; keep one helper shape"
    )]
    fn with_pass<T: Default>(
        &self, ctx: &Ctx<'_>, operation: impl FnOnce(&mut wgpu::ComputePass<'static>) -> T,
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
        if !self.ensure_open() {
            return Ok(());
        }
        let pipeline = pipeline.borrow();
        let device_id = self.state.borrow().parent.borrow().device_id;
        if pipeline.invalid || pipeline.device_id != device_id {
            self.mark_invalid();
            return Ok(());
        }
        {
            let mut state = self.state.borrow_mut();
            state.pipeline_groups = pipeline.layout_groups.clone();
            state.pipeline_auto = pipeline.auto_layout;
            state.pipeline_auto_id = pipeline.auto_layout_id;
            state.pipeline_bound = true;
            state.immediate_required = pipeline.immediate_slots;
            state.shader_mins = pipeline.shader_mins.clone();
        }
        self.with_pass(&ctx, |pass| pass.set_pipeline(&pipeline.inner))
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
        drop(bind_group);
        self.with_pass(&ctx, |pass| {
            pass.set_bind_group(index.0, native.as_ref(), &offsets);
        })
    }

    pub fn dispatch_workgroups(
        &self, x: JsU32, y: Opt<Option<JsU32>>, z: Opt<Option<JsU32>>, ctx: Ctx<'_>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let y = y.0.flatten().map_or(1, |value| value.0);
        let z = z.0.flatten().map_or(1, |value| value.0);
        let max = self.state.borrow().parent.borrow().max_compute_workgroups;
        if x.0 > max || y > max || z > max {
            self.mark_invalid();
            return Ok(());
        }
        if self.skip_native_dispatch() {
            return Ok(());
        }
        self.with_pass(&ctx, |pass| {
            pass.dispatch_workgroups(x.0, y, z);
        })
    }

    pub fn dispatch_workgroups_indirect<'js>(
        &self, buffer: Class<'js, GPUBuffer>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let buffer = buffer.borrow();
        let offset = offset.0;
        let device_id = self.state.borrow().parent.borrow().device_id;
        let bind = buffer_bind(
            &buffer,
            offset,
            Some(INDIRECT_DISPATCH_BYTES),
            wgpu::BufferUsages::INDIRECT,
            4,
            device_id,
            false,
        );
        self.state
            .borrow_mut()
            .parent
            .borrow_mut()
            .used_mapped
            .push(buffer.state.clone());
        if bind.invalid {
            self.mark_invalid();
            return Ok(());
        }
        if self.skip_native_dispatch() {
            return Ok(());
        }
        let Some((_, _)) = bind.slice else {
            return Ok(());
        };
        self.with_pass(&ctx, |pass| {
            pass.dispatch_workgroups_indirect(&buffer.inner, offset)
        })
    }

    pub fn set_immediates<'js>(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        if !self.ensure_open() {
            return Ok(());
        }
        let bytes = immediates_bytes(data, data_offset, size, &ctx)?;
        let max = self.state.borrow().parent.borrow().max_immediate_size;
        if immediates_range_invalid(offset.0, bytes.len(), max) {
            self.mark_invalid();
        } else {
            let slots = immediate_slots_from_range(offset.0, bytes.len());
            self.state.borrow_mut().immediate_filled |= slots;
        }
        Ok(())
    }

    pub fn end(&self, ctx: Ctx<'_>) -> Result<()> {
        let _ = ctx;
        if !self.state.borrow_mut().end() {
            self.errors
                .validation("GPUComputePassEncoder is already ended");
        }
        Ok(())
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
    inner:          Rc<RefCell<Option<wgpu::CommandBuffer>>>,
    #[qjs(skip_trace)]
    label:          Rc<RefCell<String>>,
    #[qjs(skip_trace)]
    used_destroyed: Rc<[Rc<Cell<bool>>]>,
    #[qjs(skip_trace)]
    used_mapped:    Rc<[Rc<RefCell<BufferMapState>>]>,
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

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUUncapturedErrorEvent")]
pub struct GPUUncapturedErrorEvent {}

#[rquickjs::methods]
impl GPUUncapturedErrorEvent {
    #[qjs(constructor)]
    pub fn construct<'js>(ctx: Ctx<'js>, type_: String, init: Object<'js>) -> Result<Object<'js>> {
        let error: Value = init.get("error")?;
        if error.is_undefined() {
            return Err(type_error(
                &ctx,
                "Failed to construct 'GPUUncapturedErrorEvent': required member error is undefined",
            ));
        }
        let event = if let Ok(ctor) = ctx.globals().get::<_, Constructor>("Event") {
            ctor.construct::<_, Object>((type_.as_str(), init.clone()))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("type", type_.as_str())?;
            object
        };
        if let Some(proto) = Class::<Self>::prototype(&ctx)? {
            event.set_prototype(Some(&proto))?;
        }
        event.prop(
            "error",
            rquickjs::object::Property::from(error).enumerable(),
        )?;
        Ok(event)
    }
}

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUPipelineError")]
pub struct GPUPipelineError {}

#[rquickjs::methods]
impl GPUPipelineError {
    #[qjs(constructor)]
    pub fn construct<'js>(
        ctx: Ctx<'js>, message: Opt<Option<String>>, options: Object<'js>,
    ) -> Result<Object<'js>> {
        let message = message.0.flatten().unwrap_or_default();
        let reason: String = options.get("reason")?;
        if reason != "validation" && reason != "internal" {
            return Err(type_error(
                &ctx,
                format!("invalid GPUPipelineErrorReason {reason}"),
            ));
        }
        let exception = if let Ok(ctor) = ctx.globals().get::<_, Constructor>("DOMException") {
            ctor.construct::<_, Object>((message.as_str(), "GPUPipelineError"))?
        } else {
            let object = Object::new(ctx.clone())?;
            object.set("message", message)?;
            object.set("name", "GPUPipelineError")?;
            object
        };
        if let Some(proto) = Class::<Self>::prototype(&ctx)? {
            exception.set_prototype(Some(&proto))?;
        }
        exception.prop(
            "reason",
            rquickjs::object::Property::from(reason).enumerable(),
        )?;
        Ok(exception)
    }
}

fn pipeline_error(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
    let Ok(ctor) = ctx.globals().get::<_, Constructor>("GPUPipelineError") else {
        return type_error(ctx, message);
    };
    let Ok(init) = Object::new(ctx.clone()) else {
        return type_error(ctx, message);
    };
    if init.set("reason", "validation").is_err() {
        return type_error(ctx, message);
    }
    match ctor.construct::<_, Value>((message, init)) {
        Ok(error) => ctx.throw(error),
        Err(error) => error,
    }
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
    globals.install_constructor::<GPUExternalTexture>(0)?;
    globals.install_constructor::<GPUInternalError>(1)?;
    globals.install_constructor::<GPUOutOfMemoryError>(1)?;
    globals.install_constructor::<GPUPipelineError>(2)?;
    globals.install_constructor::<GPUPipelineLayout>(0)?;
    globals.install_constructor::<query::GPUQuerySet>(0)?;
    globals.install_constructor::<GPUQueue>(0)?;
    globals.install_constructor::<render::GPURenderBundle>(0)?;
    globals.install_constructor::<render::GPURenderBundleEncoder>(0)?;
    globals.install_constructor::<render::GPURenderPassEncoder>(0)?;
    globals.install_constructor::<render::GPURenderPipeline>(0)?;
    globals.install_constructor::<texture::GPUSampler>(0)?;
    globals.install_constructor::<GPUShaderModule>(0)?;
    globals.install_constructor::<GPUSupportedFeatures>(0)?;
    globals.install_constructor::<GPUSupportedLimits>(0)?;
    globals.install_constructor::<GPUSupportedWGSLLanguageFeatures>(0)?;
    globals.install_constructor::<texture::GPUTexture>(0)?;
    globals.install_constructor::<texture::GPUTextureView>(0)?;
    globals.install_constructor::<GPUUncapturedErrorEvent>(2)?;
    globals.install_constructor::<GPUValidationError>(1)?;
    den_util::inherit::<GPUValidationError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUOutOfMemoryError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUInternalError, GPUError>(globals.ctx())?;
    inherit_global_prototype::<GPUDevice<'_>>(globals.ctx(), "EventTarget")?;
    inherit_global_prototype::<GPUUncapturedErrorEvent>(globals.ctx(), "Event")?;
    inherit_global_prototype::<GPUPipelineError>(globals.ctx(), "DOMException")?;
    forward_event_target_methods(globals.ctx())?;
    if let Some(proto) = Class::<GPUDevice<'_>>::prototype(globals.ctx())? {
        define_event_handler(
            globals.ctx().clone(),
            proto,
            "onuncapturederror".into(),
            Opt(None),
        )?;
    }
    Ok(())
}

fn forward_event_target_methods<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let Some(proto) = Class::<GPUDevice<'_>>::prototype(ctx)? else {
        return Ok(());
    };
    if ctx.globals().get::<_, Function>("EventTarget").is_err() {
        return Ok(());
    }
    for name in ["addEventListener", "removeEventListener", "dispatchEvent"] {
        let get_name = name.to_owned();
        let set_name = name.to_owned();
        proto.prop(
            name,
            Accessor::new(
                move |ctx: Ctx<'js>| -> Result<Value<'js>> {
                    event_target_prototype(&ctx)?.get(get_name.as_str())
                },
                move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
                    event_target_prototype(&ctx)?.set(set_name.as_str(), value)
                },
            )
            .configurable(),
        )?;
    }
    Ok(())
}

fn event_target_prototype<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    ctx.globals()
        .get::<_, Function>("EventTarget")?
        .get("prototype")
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

const GLOBAL_CLASSES: [&str; 33] = [
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
    "GPUExternalTexture",
    "GPUInternalError",
    "GPUOutOfMemoryError",
    "GPUPipelineError",
    "GPUPipelineLayout",
    "GPUQuerySet",
    "GPUQueue",
    "GPURenderBundle",
    "GPURenderBundleEncoder",
    "GPURenderPassEncoder",
    "GPURenderPipeline",
    "GPUSampler",
    "GPUShaderModule",
    "GPUSupportedFeatures",
    "GPUSupportedLimits",
    "GPUSupportedWGSLLanguageFeatures",
    "GPUTexture",
    "GPUTextureView",
    "GPUUncapturedErrorEvent",
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
        GPUDeviceLostInfo, GPUError, GPUInternalError, GPUOutOfMemoryError, GPUPipelineError,
        GPUPipelineLayout, GPUQueue, GPUShaderModule, GPUUncapturedErrorEvent, GPUValidationError,
        query::GPUQuerySet,
        render::{
            GPURenderBundle, GPURenderBundleEncoder, GPURenderPassEncoder, GPURenderPipeline,
        },
        supported::{
            GPUExternalTexture, GPUSupportedFeatures, GPUSupportedLimits,
            GPUSupportedWGSLLanguageFeatures,
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
                    let instance = make_instance();
                    let names = crate::supported::wgsl_language_feature_names(&instance);
                    Class::instance(ctx.clone(), GPU {
                        instance,
                        wgsl_language_features: GPUSupportedWGSLLanguageFeatures::from_names(
                            ctx, names,
                        )?,
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
            (
                "TRANSIENT_ATTACHMENT",
                wgpu::TextureUsages::TRANSIENT_ATTACHMENT.bits(),
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
