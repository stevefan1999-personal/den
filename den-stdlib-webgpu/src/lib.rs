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
pub mod query;
pub mod render;
pub mod supported;
pub mod texture;

use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    ops::Range,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use den_stdlib_worker::events::define_event_handler;
use den_util::BufferSource;
use rquickjs::{
    Array, ArrayBuffer, Class, Coerced, Constructor, Ctx, Exception, FromJs, Function, IntoJs as _,
    JsLifetime, Object, Persistent, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Args, Opt, This},
};

use crate::supported::{
    GPUExternalTexture, GPUSupportedFeatures, GPUSupportedLimits, GPUSupportedWGSLLanguageFeatures,
};

const MAP_READ: u32 = 1;
const MAP_WRITE: u32 = 2;
pub(crate) const JS_MAX_SAFE_INTEGER: u64 = 0x001F_FFFF_FFFF_FFFF;

pub(crate) fn illegal_constructor<T>(ctx: &Ctx<'_>) -> Result<T> {
    Err(Exception::throw_type(ctx, "Illegal constructor"))
}

pub(crate) fn type_error(ctx: &Ctx<'_>, message: impl AsRef<str>) -> rquickjs::Error {
    Exception::throw_type(ctx, message.as_ref())
}

pub(crate) fn operation_error(ctx: &Ctx<'_>, message: impl AsRef<str>) -> rquickjs::Error {
    den_util::throw_dom_exception(ctx, "OperationError", message.as_ref())
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

/// WebIDL `GPUSize64`: rquickjs's own `JS_ToIndex` coercion, nothing added.
pub(crate) type JsU64 = Coerced<u64>;

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

/// A wgpu handle that JS can only label and hand back. The Rust name is the
/// WebGPU interface name, so rquickjs needs no rename.
macro_rules! opaque_handle {
    ($($name:ident($handle:ty)),+ $(,)?) => { $(
        #[derive(Clone, Trace, JsLifetime)]
        #[rquickjs::class]
        pub struct $name {
            #[qjs(skip_trace)]
            pub(crate) inner: $handle,
            #[qjs(skip_trace)]
            pub(crate) label: Rc<RefCell<String>>,
        }

        #[rquickjs::methods]
        impl $name {
            #[qjs(constructor)]
            pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

            #[qjs(get, configurable)]
            pub fn label(&self) -> String { self.label.borrow().clone() }

            #[qjs(set, rename = "label", configurable)]
            pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }
        }
    )+ };
}
pub(crate) use opaque_handle;

/// A GPUError and its subclasses: one `message` and nothing else. The Rust name
/// is the WebGPU interface name, so rquickjs needs no rename.
macro_rules! error_class {
    ($($name:ident),+ $(,)?) => { $(
        #[derive(Clone, Trace, JsLifetime)]
        #[rquickjs::class]
        pub struct $name {
            #[qjs(get, enumerable, skip_trace)]
            message: String,
        }

        #[rquickjs::methods]
        impl $name {
            #[qjs(constructor)]
            pub const fn new(message: String) -> Self { Self { message } }
        }
    )+ };
}

opaque_handle!(
    GPUBindGroup(wgpu::BindGroup),
    GPUBindGroupLayout(wgpu::BindGroupLayout),
    GPUPipelineLayout(wgpu::PipelineLayout),
);

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

/// wgpu's `BufferSlice` panics on out-of-range and empty ranges, both of
/// which WebGPU reports as validation errors, so the range is resolved here:
/// `Ok(None)` is an empty binding, `Err` carries the validation message.
pub(crate) fn buffer_slice(
    buffer: &wgpu::Buffer, offset: u64, size: Option<u64>,
) -> std::result::Result<Option<wgpu::BufferSlice<'_>>, String> {
    let end = size.map_or_else(|| Some(buffer.size()), |size| offset.checked_add(size));
    match end {
        // ponytail: a legal zero-size binding is reported as "no buffer".
        // `wgpu::RenderPass::set_vertex_buffer` calls `size_expect_nonzero`
        // and panics on an empty slice (gfx-rs/wgpu#3170), and the safe API
        // exposes no other way to record one; deno_webgpu can only bind it
        // because it drives wgpu-core directly. Bind it for real once wgpu
        // accepts empty slices.
        Some(end) if offset <= end && end <= buffer.size() => {
            Ok((offset < end).then(|| buffer.slice(offset..end)))
        }
        _ => {
            Err(format!(
                "buffer binding {offset}..{size:?} is outside the {} byte buffer",
                buffer.size()
            ))
        }
    }
}

enum RecorderState<T> {
    Open(T),
    Ended,
    /// Created from an already finished parent; that error was reported at
    /// creation, so later operations stay silent like wgpu's invalid passes.
    Invalid,
}

/// A wgpu recorder (command encoder, pass, bundle encoder) that `finish`/`end`
/// consumes. Operations after that are the one content-timeline validation
/// error the safe `wgpu` crate cannot produce itself.
pub(crate) struct Recorder<T> {
    what:     &'static str,
    state:    RefCell<RecorderState<T>>,
    /// Messages wgpu would only report when the recorder ends (its pass
    /// validation runs at `end`), so den defers them to the same point.
    deferred: RefCell<Option<String>>,
}

// SAFETY: a recorder never holds a `'js` value; `'static` wgpu handles do
// not change when the JS lifetime is rebound.
unsafe impl<T: 'static> JsLifetime<'_> for Recorder<T> {
    type Changed<'to> = Self;
}

impl<T> Recorder<T> {
    pub(crate) fn open(what: &'static str, inner: T) -> Self {
        Self {
            what,
            state: RefCell::new(RecorderState::Open(inner)),
            deferred: RefCell::new(None),
        }
    }

    pub(crate) fn invalid(what: &'static str) -> Self {
        Self {
            what,
            state: RefCell::new(RecorderState::Invalid),
            deferred: RefCell::new(None),
        }
    }

    pub(crate) fn map<U>(
        &self, errors: &ErrorSink, operation: impl FnOnce(&mut T) -> U,
    ) -> Option<U> {
        match &mut *self.state.borrow_mut() {
            RecorderState::Open(inner) => Some(operation(inner)),
            RecorderState::Ended => {
                errors.validation(format!("{} is already ended", self.what));
                None
            }
            RecorderState::Invalid => None,
        }
    }

    pub(crate) fn record(&self, errors: &ErrorSink, operation: impl FnOnce(&mut T)) {
        self.map(errors, operation);
    }

    pub(crate) fn defer(&self, message: String) {
        self.deferred.borrow_mut().get_or_insert(message);
    }

    /// Ends the recorder: `finish` consumes the wgpu value (dropping a pass
    /// ends it), then any deferred message is reported after wgpu's own. A
    /// deferred message means the recorder was invalid, so what it produced is
    /// dropped the way wgpu-core drops the output of a failed `finish`.
    pub(crate) fn end<U>(&self, errors: &ErrorSink, finish: impl FnOnce(T) -> U) -> Option<U> {
        let state = std::mem::replace(&mut *self.state.borrow_mut(), RecorderState::Ended);
        let finished = match state {
            RecorderState::Open(inner) => Some(finish(inner)),
            RecorderState::Ended => {
                errors.validation(format!("{} is already ended", self.what));
                None
            }
            RecorderState::Invalid => None,
        };
        self.deferred
            .borrow_mut()
            .take()
            .map_or(finished, |message| {
                errors.validation(message);
                None
            })
    }
}

/// `writeBuffer` and `setImmediates` both take `(data, dataOffset, size)`
/// counted in typed-array elements, and both owe JS an `OperationError` when
/// that window leaves the source (deno_webgpu's `get_data_slice`). Everything
/// about the destination is wgpu's to validate.
pub(crate) fn data_window<'js>(
    data: Value<'js>, data_offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: &Ctx<'js>,
) -> Result<Vec<u8>> {
    let element_size = typed_array_element_size(&data);
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
    // Both destinations copy in 4-byte units (`COPY_BUFFER_ALIGNMENT` and
    // `IMMEDIATE_DATA_ALIGNMENT`), and the spec makes that a content-timeline
    // check rather than a validation error.
    if !length.is_multiple_of(4) {
        return Err(operation_error(
            ctx,
            "data size in bytes is not a multiple of 4",
        ));
    }
    let end = start
        .checked_add(length)
        .ok_or_else(|| operation_error(ctx, "data range overflows"))?;
    usize::try_from(start)
        .ok()
        .zip(usize::try_from(end).ok())
        .and_then(|(start, end)| bytes.get(start..end))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| operation_error(ctx, "data range is out of bounds"))
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

struct ErrorScope {
    filter:      GPUErrorKind,
    first_error: Option<GPUErrorData>,
}

/// den's own error-scope stack. wgpu's scopes are never pushed: with none
/// active, every wgpu error reaches `on_uncaptured_error` synchronously on
/// the calling thread, so routing it here keeps the WebGPU semantics exact.
#[derive(Default)]
struct ErrorRouter {
    scopes:     Vec<ErrorScope>,
    uncaptured: Vec<GPUErrorData>,
}

impl ErrorRouter {
    fn capture(&mut self, error: GPUErrorData) {
        let scope = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.filter == error.kind);
        match scope {
            Some(scope) => {
                scope.first_error.get_or_insert(error);
            }
            None => self.uncaptured.push(error),
        }
    }
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

    /// Content-timeline validation errors the safe `wgpu` crate cannot
    /// produce (double finish, ops after end) take the same route as wgpu's.
    pub(crate) fn validation(&self, message: impl Into<String>) {
        self.capture(GPUErrorData {
            kind:    GPUErrorKind::Validation,
            message: message.into(),
        });
    }

    pub(crate) fn capture(&self, error: GPUErrorData) { self.lock().capture(error); }

    fn take_uncaptured(&self) -> Vec<GPUErrorData> { std::mem::take(&mut self.lock().uncaptured) }

    pub(crate) fn push(&self, filter: GPUErrorKind) {
        self.lock().scopes.push(ErrorScope {
            filter,
            first_error: None,
        });
    }

    pub(crate) fn pop(&self) -> std::result::Result<Option<GPUErrorData>, ScopeStackEmpty> {
        self.lock()
            .scopes
            .pop()
            .map(|scope| scope.first_error)
            .ok_or(ScopeStackEmpty)
    }
}

pub(crate) struct ScopeStackEmpty;

fn flush_uncaptured<'js>(device: &Class<'js, GPUDevice<'js>>, ctx: &Ctx<'js>) -> Result<()> {
    let pending = device.borrow().errors.take_uncaptured();
    pending
        .into_iter()
        .try_for_each(|error| dispatch_uncaptured_error(device, ctx, error))
}

fn flushed<'js, T>(
    this: &This<Class<'js, GPUDevice<'js>>>, ctx: &Ctx<'js>, result: Result<T>,
) -> Result<T> {
    // Flushed on both paths, but an operation's own error outranks a flush one.
    let flush = flush_uncaptured(&this.0, ctx);
    let value = result?;
    flush.map(|()| value)
}

fn dispatch_uncaptured_error<'js>(
    device: &Class<'js, GPUDevice<'js>>, ctx: &Ctx<'js>, error: GPUErrorData,
) -> Result<()> {
    let error_js = gpu_error_value(ctx, error)?;
    let init = Object::new(ctx.clone())?;
    init.set("error", error_js)?;
    // Installed by `install_classes` before any device can exist.
    let ctor: Constructor = ctx.globals().get("GPUUncapturedErrorEvent")?;
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
            limits: GPUSupportedLimits::from_limits(ctx, inner.limits())?,
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
        GPUSupportedLimits::apply(&ctx, limits, &mut required_limits)?;
        // WebGPU ignores a request below the spec default for a maximum limit;
        // whether the adapter can meet the rest is wgpu's answer, not den's.
        let required_limits = required_limits.or_better_values_from(&wgpu::Limits::default());
        let request = wgpu::DeviceDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            required_features,
            required_limits,
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
            device:    device.clone(),
            device_js: device_js.clone(),
            errors:    errors.clone(),
            inner:     queue,
            label:     Rc::new(RefCell::new(queue_label)),
        })?;
        let (lost, resolve, _reject) = ctx.promise()?;
        let features = GPUSupportedFeatures::from_features(&ctx, device.features())?;
        let limits = GPUSupportedLimits::from_limits(&ctx, device.limits())?;
        let gpu_device = Class::instance(ctx.clone(), GPUDevice {
            adapter_info: self.info.clone(),
            buffer_states: Rc::new(RefCell::new(Vec::new())),
            destroyed: Rc::new(Cell::new(false)),
            device,
            errors,
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
    adapter_info:      Class<'js, GPUAdapterInfo>,
    /// Mapping state of every buffer created here, weakly so that a collected
    /// `GPUBuffer` drops out; `destroy` walks it to detach mapped ranges.
    buffer_states:     Rc<RefCell<Vec<std::rc::Weak<RefCell<BufferMapState>>>>>,
    destroyed:         Rc<Cell<bool>>,
    pub(crate) device: wgpu::Device,
    pub(crate) errors: ErrorSink,
    features:          Class<'js, GPUSupportedFeatures>,
    label:             Rc<RefCell<String>>,
    limits:            Class<'js, GPUSupportedLimits>,
    lost:              Promise<'js>,
    lost_resolve:      Rc<RefCell<Option<Function<'js>>>>,
    queue:             Class<'js, GPUQueue<'js>>,
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

    fn create_buffer_inner(
        &self, device: &Class<'js, Self>, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUBuffer<'js>> {
        let label = label(&descriptor)?;
        let size = descriptor.get::<_, JsU64>("size")?.0;
        // Unknown usage bits reach wgpu untruncated so that it, not den,
        // reports them; it validates the size, the map/copy combinations and
        // the alignment too.
        let usage = wgpu::BufferUsages::from_bits_retain(descriptor.get::<_, JsU32>("usage")?.0);
        let mapped_at_creation = descriptor
            .get::<_, Option<bool>>("mappedAtCreation")?
            .unwrap_or_default();
        if mapped_at_creation && !size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) {
            return Err(Exception::throw_range(
                ctx,
                "mappedAtCreation buffer size must be a multiple of 4",
            ));
        }
        let inner = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: (!label.is_empty()).then_some(label.as_str()),
            size,
            usage,
            mapped_at_creation,
        });
        // A buffer whose creation failed is still mapped on the content
        // timeline, but wgpu holds no memory for it. Probing here (the getter
        // reports no error to the device) records which kind this is.
        let native_map = mapped_at_creation && inner.get_mapped_range(0..size).is_ok();
        let state = Rc::new(RefCell::new(if mapped_at_creation {
            BufferMapState::Mapped {
                kind:  MappingKind::Write,
                range: 0..size,
                views: Vec::new(),
            }
        } else {
            BufferMapState::Unmapped
        }));
        self.buffer_states.borrow_mut().push(Rc::downgrade(&state));
        Ok(GPUBuffer {
            device: device.clone(),
            inner,
            label: Rc::new(RefCell::new(label)),
            map_gen: Rc::new(Cell::new(0)),
            native_map: Rc::new(Cell::new(native_map)),
            state,
        })
    }

    fn create_shader_module_inner(&self, descriptor: Object<'js>) -> Result<GPUShaderModule> {
        let label = label(&descriptor)?;
        let code: String = descriptor.get("code")?;
        let inner = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label:  (!label.is_empty()).then_some(label.as_str()),
                source: wgpu::ShaderSource::Wgsl(code.into()),
            });
        Ok(GPUShaderModule {
            inner,
            label: Rc::new(RefCell::new(label)),
        })
    }

    fn create_bind_group_layout_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUBindGroupLayout> {
        let label = label(&descriptor)?;
        let entries = array_value(&descriptor, "entries", ctx)?
            .iter::<Object>()
            .map(|entry| {
                let entry = entry?;
                // `GPUShaderStageFlags` is a WebIDL typedef; unknown bits are a
                // conversion failure, like deno's converter.
                let visibility =
                    wgpu::ShaderStages::from_bits(entry.get::<_, JsU32>("visibility")?.0)
                        .ok_or_else(|| type_error(ctx, "shader stage is not valid"))?;
                Ok(wgpu::BindGroupLayoutEntry {
                    binding: entry.get::<_, JsU32>("binding")?.0,
                    visibility,
                    ty: bind_group_layout_type(&entry, ctx)?,
                    count: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let inner = self
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label:   (!label.is_empty()).then_some(label.as_str()),
                entries: &entries,
            });
        Ok(GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(label)),
        })
    }

    fn create_pipeline_layout_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUPipelineLayout> {
        let label = label(&descriptor)?;
        let layouts = array_value(&descriptor, "bindGroupLayouts", ctx)?
            .iter::<Option<Class<GPUBindGroupLayout>>>()
            .map(|layout| Ok(layout?.map(|layout| layout.borrow().inner.clone())))
            .collect::<Result<Vec<_>>>()?;
        let borrowed = layouts.iter().map(Option::as_ref).collect::<Vec<_>>();
        let inner = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label:              (!label.is_empty()).then_some(label.as_str()),
                bind_group_layouts: &borrowed,
                immediate_size:     descriptor
                    .get::<_, Option<JsU32>>("immediateSize")?
                    .map_or(0, |value| value.0),
            });
        Ok(GPUPipelineLayout {
            inner,
            label: Rc::new(RefCell::new(label)),
        })
    }

    fn create_bind_group_inner(
        &self, descriptor: Object<'js>, ctx: &Ctx<'js>,
    ) -> Result<GPUBindGroup> {
        let label = label(&descriptor)?;
        let layout = class_value::<GPUBindGroupLayout>(&descriptor, "layout", ctx)?
            .borrow()
            .inner
            .clone();
        let entries = array_value(&descriptor, "entries", ctx)?;
        let mut resources = Vec::with_capacity(entries.len());
        for entry in entries.iter::<Object>() {
            let entry = entry?;
            let binding = entry.get::<_, JsU32>("binding")?.0;
            let resource: Value = entry.get("resource")?;
            if Class::<GPUExternalTexture>::from_js(ctx, resource.clone()).is_ok() {
                // den has no external texture backing; the missing entry then
                // also fails wgpu's entry-count check, which is the intent.
                self.errors
                    .validation("GPUExternalTexture bindings are not supported");
                continue;
            }
            if let Ok(buffer) = Class::<GPUBuffer<'js>>::from_js(ctx, resource.clone()) {
                resources.push(OwnedBinding::Buffer {
                    binding,
                    buffer: buffer.borrow().inner.clone(),
                    offset: 0,
                    size: None,
                });
                continue;
            }
            if let Ok(texture) = Class::<texture::GPUTexture>::from_js(ctx, resource.clone()) {
                resources.push(OwnedBinding::View {
                    binding,
                    view: texture.borrow().default_view(),
                });
                continue;
            }
            if let Ok(view) = Class::<texture::GPUTextureView>::from_js(ctx, resource.clone()) {
                resources.push(OwnedBinding::View {
                    binding,
                    view: view.borrow().inner.clone(),
                });
                continue;
            }
            if let Ok(sampler) = Class::<texture::GPUSampler>::from_js(ctx, resource.clone()) {
                resources.push(OwnedBinding::Sampler {
                    binding,
                    sampler: sampler.borrow().inner.clone(),
                });
                continue;
            }
            let resource = Object::from_js(ctx, resource)
                .map_err(|_error| type_error(ctx, "bind group resource is not a GPU binding"))?;
            let buffer = class_value::<GPUBuffer<'js>>(&resource, "buffer", ctx)?
                .borrow()
                .inner
                .clone();
            let offset = resource
                .get::<_, Option<JsU64>>("offset")?
                .map_or(0, |value| value.0);
            // wgpu's binding size is `NonZeroU64`; the spec rejects zero on
            // the device timeline, so report it and bind to the end instead.
            let size = resource.get::<_, Option<JsU64>>("size")?.and_then(|value| {
                NonZeroU64::new(value.0).or_else(|| {
                    self.errors
                        .validation("buffer binding size must be greater than zero");
                    None
                })
            });
            resources.push(OwnedBinding::Buffer {
                binding,
                buffer,
                offset,
                size,
            });
        }
        let native = resources
            .iter()
            .map(OwnedBinding::entry)
            .collect::<Vec<_>>();
        let inner = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   (!label.is_empty()).then_some(label.as_str()),
            layout:  &layout,
            entries: &native,
        });
        Ok(GPUBindGroup {
            inner,
            label: Rc::new(RefCell::new(label)),
        })
    }
}

impl OwnedBinding {
    fn entry(&self) -> wgpu::BindGroupEntry<'_> {
        match self {
            Self::Buffer {
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
            Self::View { binding, view } => {
                wgpu::BindGroupEntry {
                    binding:  *binding,
                    resource: wgpu::BindingResource::TextureView(view),
                }
            }
            Self::Sampler { binding, sampler } => {
                wgpu::BindGroupEntry {
                    binding:  *binding,
                    resource: wgpu::BindingResource::Sampler(sampler),
                }
            }
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
            // Destroying the device unmaps every buffer on it, so the
            // ArrayBuffers handed to JS have to be detached here too.
            for state in self.buffer_states.borrow_mut().drain(..) {
                let Some(state) = state.upgrade() else {
                    continue;
                };
                if let BufferMapState::Mapped { views, .. } =
                    std::mem::replace(&mut *state.borrow_mut(), BufferMapState::Destroyed)
                {
                    for view in views {
                        view.buffer.restore(&ctx)?.detach();
                    }
                }
            }
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
    ) -> Result<texture::GPUTexture<'js>> {
        flushed(
            &this,
            &ctx,
            texture::GPUTexture::from_descriptor(&this.0, descriptor, &ctx),
        )
    }

    pub fn create_sampler(
        &self, descriptor: Opt<Option<Object<'js>>>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<texture::GPUSampler> {
        flushed(
            &this,
            &ctx,
            texture::create_sampler(&self.device, descriptor, &ctx),
        )
    }

    pub fn create_query_set(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<query::GPUQuerySet> {
        flushed(
            &this,
            &ctx,
            query::GPUQuerySet::from_descriptor(&self.device, descriptor, &ctx),
        )
    }

    pub fn create_buffer(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<GPUBuffer<'js>> {
        flushed(
            &this,
            &ctx,
            self.create_buffer_inner(&this, descriptor, &ctx),
        )
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
        flushed(&this, &ctx, self.create_shader_module_inner(descriptor))
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
    ) -> Result<GPUComputePipeline<'js>> {
        flushed(
            &this,
            &ctx,
            create_compute_pipeline(&this.0, descriptor, &ctx),
        )
    }

    pub fn create_compute_pipeline_async(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<Promise<'js>> {
        Self::settle_pipeline(&this, &ctx, |device| {
            create_compute_pipeline(device, descriptor, &ctx)
        })
    }

    pub fn create_render_pipeline(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<render::GPURenderPipeline<'js>> {
        flushed(
            &this,
            &ctx,
            render::create_pipeline(&this.0, descriptor, &ctx),
        )
    }

    pub fn create_render_pipeline_async(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<Promise<'js>> {
        Self::settle_pipeline(&this, &ctx, |device| {
            render::create_pipeline(device, descriptor, &ctx)
        })
    }

    pub fn create_render_bundle_encoder(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<render::GPURenderBundleEncoder<'js>> {
        flushed(
            &this,
            &ctx,
            render::new_bundle_encoder(&this.0, descriptor, &ctx),
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
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: (!label.is_empty()).then_some(label.as_str()),
            });
        flushed(
            &this,
            &ctx,
            Ok(GPUCommandEncoder {
                device:  this.0.clone(),
                label:   RefCell::new(label),
                encoder: Rc::new(Recorder::open("GPUCommandEncoder", encoder)),
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
        self.errors.push(filter);
        Ok(())
    }

    pub fn pop_error_scope(
        &self, ctx: Ctx<'js>, this: This<Class<'js, Self>>,
    ) -> Result<Promise<'js>> {
        let (promise, resolve, reject) = ctx.promise()?;
        if self.is_destroyed() {
            resolve.call::<_, ()>((Value::new_null(ctx.clone()),))?;
            return Ok(promise);
        }
        match self.errors.pop() {
            Err(ScopeStackEmpty) => {
                let error: Value = den_util::construct(
                    &ctx,
                    "DOMException",
                    ("GPU error scope stack is empty", "OperationError"),
                )?;
                reject.call::<_, ()>((error,))?;
            }
            Ok(None) => resolve.call::<_, ()>((Value::new_null(ctx.clone()),))?,
            Ok(Some(error)) => resolve.call::<_, ()>((gpu_error_value(&ctx, error)?,))?,
        }
        flush_uncaptured(&this.0, &ctx)?;
        Ok(promise)
    }
}

impl<'js> GPUDevice<'js> {
    /// Runs `create` under a private validation scope and settles `promise`
    /// like deno's `*_or_error` path: a captured error becomes the
    /// `GPUPipelineError` rejection and never reaches script-visible scopes.
    fn settle_pipeline<T: rquickjs::class::JsClass<'js> + 'js>(
        this: &This<Class<'js, Self>>, ctx: &Ctx<'js>,
        create: impl FnOnce(&Class<'js, Self>) -> Result<T>,
    ) -> Result<Promise<'js>> {
        let errors = this.0.borrow().errors.clone();
        errors.push(GPUErrorKind::Validation);
        let created = create(&this.0);
        let captured = errors.pop().unwrap_or_default();
        let pipeline = created?;
        let (promise, resolve, reject) = ctx.promise()?;
        match captured {
            Some(error) => reject.call::<_, ()>((pipeline_error(ctx, &error.message)?,))?,
            None => resolve.call::<_, ()>((Class::instance(ctx.clone(), pipeline)?,))?,
        }
        flush_uncaptured(&this.0, ctx)?;
        Ok(promise)
    }
}

/// `layout` undefined or `'auto'` is wgpu's `None`; wgpu derives the layout
/// from the shader and validates everything else.
pub(crate) fn pipeline_layout<'js>(
    descriptor: &Object<'js>, ctx: &Ctx<'js>,
) -> Result<Option<wgpu::PipelineLayout>> {
    let layout: Value = descriptor.get("layout")?;
    let auto = layout
        .as_string()
        .map(rquickjs::String::to_string)
        .transpose()?
        .as_deref()
        == Some("auto");
    if layout.is_undefined() || auto {
        return Ok(None);
    }
    Ok(Some(
        Class::<GPUPipelineLayout>::from_js(ctx, layout)?
            .borrow()
            .inner
            .clone(),
    ))
}

/// `GPUProgrammableStage`: the module handle plus the owned strings wgpu's
/// borrowed descriptors point into.
pub(crate) struct ProgrammableStage {
    pub(crate) module:      wgpu::ShaderModule,
    pub(crate) entry_point: Option<String>,
    constants:              Vec<(String, f64)>,
}

impl ProgrammableStage {
    pub(crate) fn read<'js>(object: &Object<'js>, ctx: &Ctx<'js>) -> Result<Self> {
        Ok(Self {
            module:      class_value::<GPUShaderModule>(object, "module", ctx)?
                .borrow()
                .inner
                .clone(),
            entry_point: object.get("entryPoint")?,
            constants:   format::pipeline_constants(object.get("constants")?, ctx)?,
        })
    }

    pub(crate) fn constant_pairs(&self) -> Vec<(&str, f64)> {
        self.constants
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect()
    }

    pub(crate) fn compilation_options<'a>(
        constants: &'a [(&'a str, f64)],
    ) -> wgpu::PipelineCompilationOptions<'a> {
        wgpu::PipelineCompilationOptions {
            constants,
            zero_initialize_workgroup_memory: true,
        }
    }
}

fn create_compute_pipeline<'js>(
    device_class: &Class<'js, GPUDevice<'js>>, descriptor: Object<'js>, ctx: &Ctx<'js>,
) -> Result<GPUComputePipeline<'js>> {
    let label = label(&descriptor)?;
    let layout = pipeline_layout(&descriptor, ctx)?;
    let stage = ProgrammableStage::read(&descriptor.get("compute")?, ctx)?;
    let constants = stage.constant_pairs();
    let inner =
        device_class
            .borrow()
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label:               (!label.is_empty()).then_some(label.as_str()),
                layout:              layout.as_ref(),
                module:              &stage.module,
                entry_point:         stage.entry_point.as_deref(),
                compilation_options: ProgrammableStage::compilation_options(&constants),
                cache:               None,
            });
    Ok(GPUComputePipeline {
        device: device_class.clone(),
        inner,
        label: Rc::new(RefCell::new(label)),
    })
}

fn bind_group_layout_type(entry: &Object<'_>, ctx: &Ctx<'_>) -> Result<wgpu::BindingType> {
    let buffer = entry.get::<_, Option<Object>>("buffer")?;
    let sampler = entry.get::<_, Option<Object>>("sampler")?;
    let texture = entry.get::<_, Option<Object>>("texture")?;
    let storage = entry.get::<_, Option<Object>>("storageTexture")?;
    let external = entry.get::<_, Option<Object>>("externalTexture")?;
    let kinds = [
        buffer.is_some(),
        sampler.is_some(),
        texture.is_some(),
        storage.is_some(),
        external.is_some(),
    ];
    if kinds.iter().filter(|present| **present).count() != 1 {
        return Err(type_error(
            ctx,
            "exactly one of buffer, sampler, texture, storageTexture or externalTexture must be \
             specified",
        ));
    }
    if let Some(buffer) = buffer {
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
    if let Some(sampler) = sampler {
        let name = sampler
            .get::<_, Option<String>>("type")?
            .unwrap_or_else(|| "filtering".into());
        return Ok(wgpu::BindingType::Sampler(format::sampler_binding_type(
            &name, ctx,
        )?));
    }
    if let Some(texture) = texture {
        return Ok(wgpu::BindingType::Texture {
            sample_type:    format::sample_type(
                texture.get::<_, Option<String>>("sampleType")?.as_deref(),
                ctx,
            )?,
            view_dimension: format::view_dimension(
                texture
                    .get::<_, Option<String>>("viewDimension")?
                    .as_deref(),
                ctx,
            )?
            .unwrap_or(wgpu::TextureViewDimension::D2),
            multisampled:   texture
                .get::<_, Option<bool>>("multisampled")?
                .unwrap_or_default(),
        });
    }
    if let Some(storage) = storage {
        return Ok(wgpu::BindingType::StorageTexture {
            access:         format::storage_access(
                storage.get::<_, Option<String>>("access")?.as_deref(),
                ctx,
            )?,
            format:         format::texture_format(&storage.get::<_, String>("format")?, ctx)?,
            view_dimension: format::view_dimension(
                storage
                    .get::<_, Option<String>>("viewDimension")?
                    .as_deref(),
                ctx,
            )?
            .unwrap_or(wgpu::TextureViewDimension::D2),
        });
    }
    Ok(wgpu::BindingType::ExternalTexture)
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
pub struct GPUBuffer<'js> {
    /// The owning device, so that every buffer method can dispatch the
    /// uncaptured errors its wgpu call produced instead of leaving them to be
    /// misattributed to a later, unrelated call.
    device:           Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    pub(crate) inner: wgpu::Buffer,
    #[qjs(skip_trace)]
    label:            Rc<RefCell<String>>,
    /// Bumped by every `mapAsync`, `unmap` and `destroy` so that a waiter for
    /// a superseded map rejects with `AbortError` instead of resolving over
    /// the state a later call installed.
    #[qjs(skip_trace)]
    map_gen:          Rc<Cell<u64>>,
    /// Whether wgpu, and not just the content timeline, holds a mapping. They
    /// disagree for a buffer whose creation failed with `mappedAtCreation`.
    #[qjs(skip_trace)]
    native_map:       Rc<Cell<bool>>,
    #[qjs(skip_trace)]
    pub(crate) state: Rc<RefCell<BufferMapState>>,
}

impl<'js> GPUBuffer<'js> {
    fn errors(&self) -> ErrorSink { self.device.borrow().errors.clone() }

    fn flush(&self, ctx: &Ctx<'js>) -> Result<()> { flush_uncaptured(&self.device, ctx) }

    /// The `[[mapping]]`-state failures WebGPU rejects before `mapAsync`
    /// returns (`earlyRejection: true` in the CTS mapping suite).
    fn reject_now(&self, ctx: &Ctx<'js>, message: &str) -> Result<Promise<'js>> {
        self.errors().validation(message);
        let (promise, _resolve, reject) = ctx.promise()?;
        let error: Value = den_util::construct(ctx, "DOMException", (message, "OperationError"))?;
        reject.call::<_, ()>((error,))?;
        self.flush(ctx)?;
        Ok(promise)
    }

    /// Every other `mapAsync` failure belongs to the device timeline: the
    /// validation error is raised now but the promise stays pending until a
    /// later microtask, which is what `earlyRejection: false` asserts.
    fn reject_later(&self, ctx: &Ctx<'js>, message: &'static str) -> Result<Promise<'js>> {
        self.errors().validation(message);
        let (promise, _resolve, reject) = ctx.promise()?;
        let spawn_ctx = ctx.clone();
        ctx.spawn(async move {
            tokio::task::yield_now().await;
            if let Ok(error) = den_util::construct::<_, Value>(
                &spawn_ctx,
                "DOMException",
                (message, "OperationError"),
            ) {
                let _ = reject.call::<_, ()>((error,));
            }
        });
        self.flush(ctx)?;
        Ok(promise)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUBuffer<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get, configurable)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label", configurable)]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    #[qjs(get, configurable)]
    pub fn size(&self) -> u64 { self.inner.size() }

    #[qjs(get, configurable)]
    pub fn usage(&self) -> u32 { self.inner.usage().bits() }

    #[qjs(get, configurable)]
    pub fn map_state(&self) -> &'static str {
        match *self.state.borrow() {
            BufferMapState::Unmapped | BufferMapState::Destroyed => "unmapped",
            BufferMapState::Pending => "pending",
            BufferMapState::Mapped { .. } => "mapped",
        }
    }

    pub fn map_async(
        self, mode: JsU32, offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<Promise<'js>> {
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
            return self.reject_now(&ctx, "GPUBuffer is not unmapped");
        }
        if destroyed {
            return self.reject_later(&ctx, "GPUBuffer is destroyed");
        }
        // Whether the usage permits the mode is wgpu's call; the mode itself is
        // a content-timeline check, exactly as in deno_webgpu.
        let kind = match mode.0 {
            MAP_READ => MappingKind::Read,
            MAP_WRITE => MappingKind::Write,
            _ => {
                return self.reject_later(&ctx, "mode must be GPUMapMode.READ or GPUMapMode.WRITE");
            }
        };
        let buffer_size = self.inner.size();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size
            .0
            .flatten()
            .map_or_else(|| buffer_size.saturating_sub(offset), |value| value.0);
        let Some(end) = offset.checked_add(size) else {
            return self.reject_later(&ctx, "mapped range overflows");
        };
        // `Buffer::map_async` panics on an out-of-range slice, so the bounds
        // have to be settled here before wgpu sees them.
        if !offset.is_multiple_of(wgpu::MAP_ALIGNMENT)
            || !size.is_multiple_of(4)
            || end > buffer_size
        {
            return self.reject_later(
                &ctx,
                "mapped range must be in bounds, 8-byte aligned, and a multiple of 4",
            );
        }
        let (promise, resolve, reject) = ctx.promise()?;
        *self.state.borrow_mut() = BufferMapState::Pending;
        let generation = self.map_gen.get().wrapping_add(1);
        self.map_gen.set(generation);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let refused = Arc::new(AtomicBool::new(false));
        let refused_callback = refused.clone();
        self.inner.map_async(
            match kind {
                MappingKind::Read => wgpu::MapMode::Read,
                MappingKind::Write => wgpu::MapMode::Write,
            },
            offset..end,
            move |result| {
                refused_callback.store(result.is_err(), Ordering::Relaxed);
                let _ = sender.send(result);
            },
        );
        // wgpu runs the callback inline only when it refuses the map outright;
        // then nothing is mapped and `unmap` must not ask it to undo one.
        self.native_map.set(!refused.load(Ordering::Relaxed));
        self.flush(&ctx)?;
        let device = self.device.borrow().device.clone();
        let device_js = self.device.clone();
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
            let _ = flush_uncaptured(&device_js, &spawn_ctx);
        });
        Ok(promise)
    }

    pub fn get_mapped_range(
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
            .map_or_else(|| self.inner.size().saturating_sub(offset), |value| value.0);
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
        // Never alias wgpu's memory into JS: the view is copied out here and
        // copied back in `unmap`. Without a wgpu mapping the content timeline
        // still owes JS a zeroed range of the requested size.
        let buffer = if self.native_map.get() {
            let native = self
                .inner
                .get_mapped_range(range.clone())
                .map_err(|error| operation_error(&ctx, error.to_string()))?;
            let buffer = ArrayBuffer::new_copy(ctx.clone(), native.as_ref())?;
            drop(native);
            buffer
        } else {
            // A buffer whose creation failed still reports its requested size,
            // so the length here is script-chosen: reserve fallibly rather
            // than let `alloc_zeroed` abort the process.
            let length = usize::try_from(size)
                .map_err(|_error| operation_error(&ctx, "mapped range is too large"))?;
            let mut zeroed = Vec::new();
            zeroed
                .try_reserve_exact(length)
                .map_err(|_error| operation_error(&ctx, "mapped range is too large"))?;
            zeroed.resize(length, 0_u8);
            ArrayBuffer::new_copy(ctx.clone(), zeroed)?
        };
        views.push(MappedView {
            buffer: Persistent::save(&ctx, buffer.clone()),
            range,
        });
        drop(state);
        self.flush(&ctx)?;
        Ok(buffer)
    }

    pub fn unmap(&self, ctx: Ctx<'js>) -> Result<()> {
        let destroyed = matches!(*self.state.borrow(), BufferMapState::Destroyed);
        let previous = std::mem::replace(&mut *self.state.borrow_mut(), BufferMapState::Unmapped);
        if let BufferMapState::Mapped { kind, views, .. } = previous {
            for view in views {
                let mut array = view.buffer.restore(&ctx)?;
                let written = (matches!(kind, MappingKind::Write) && self.native_map.get())
                    .then(|| array.as_bytes().map(<[u8]>::to_vec))
                    .flatten()
                    .filter(|bytes| !bytes.is_empty());
                if let Some(bytes) = written {
                    self.inner
                        .get_mapped_range_mut(view.range)
                        .map_err(|error| operation_error(&ctx, error.to_string()))?
                        .copy_from_slice(&bytes);
                }
                array.detach();
            }
        }
        // A pending map is cancelled by the generation bump.
        self.map_gen.set(self.map_gen.get().wrapping_add(1));
        if self.native_map.replace(false) {
            self.inner.unmap();
        }
        if destroyed {
            *self.state.borrow_mut() = BufferMapState::Destroyed;
        }
        self.flush(&ctx)
    }

    pub fn destroy(&self, ctx: Ctx<'js>) -> Result<()> {
        let previous = std::mem::replace(&mut *self.state.borrow_mut(), BufferMapState::Destroyed);
        if let BufferMapState::Mapped { views, .. } = previous {
            for view in views {
                view.buffer.restore(&ctx)?.detach();
            }
        }
        self.map_gen.set(self.map_gen.get().wrapping_add(1));
        self.native_map.set(false);
        self.inner.destroy();
        self.flush(&ctx)
    }
}

#[derive(Clone, JsLifetime)]
#[rquickjs::class(rename = "GPUQueue")]
pub struct GPUQueue<'js> {
    #[qjs(skip_trace)]
    device:    wgpu::Device,
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
        &self, buffer: Class<'js, GPUBuffer<'js>>, buffer_offset: JsU64, data: Value<'js>,
        data_offset: Opt<Option<JsU64>>, size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let data = data_window(data, data_offset, size, &ctx)?;
        self.inner
            .write_buffer(&buffer.borrow().inner, buffer_offset.0, &data);
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
        self.inner.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.borrow().inner,
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
        let mut native = Vec::with_capacity(command_buffers.len());
        for buffer in command_buffers.iter::<Class<GPUCommandBuffer>>() {
            // A buffer from a failed finish, or one already submitted, has no
            // wgpu handle; wgpu would report the same validation error.
            match buffer?.borrow().inner.borrow_mut().take() {
                Some(command_buffer) => native.push(command_buffer),
                None => {
                    self.errors
                        .validation("GPUCommandBuffer is invalid or was already submitted");
                }
            }
        }
        self.inner.submit(native);
        self.flush(&ctx)
    }

    pub async fn on_submitted_work_done(self, ctx: Ctx<'_>) -> Result<()> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.inner.on_submitted_work_done(move || {
            let _ = sender.send(());
        });
        let device = self.device.clone();
        tokio::task::spawn_blocking(move || GpuPoll::until(device, receiver))
            .await
            .map_err(|error| operation_error(&ctx, error.to_string()))?
            .map_err(|error| operation_error(&ctx, error))?;
        Ok(())
    }
}

/// The stride `writeBuffer`'s `dataOffset`/`size` are counted in. A value that
/// is not a typed array — a DataView or a bare ArrayBuffer — counts bytes.
fn typed_array_element_size(value: &Value<'_>) -> u64 {
    use rquickjs::qjs::{
        JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_INT64 as BIG_INT64,
        JSTypedArrayEnum_JS_TYPED_ARRAY_BIG_UINT64 as BIG_UINT64,
        JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT16 as FLOAT16,
        JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT32 as FLOAT32,
        JSTypedArrayEnum_JS_TYPED_ARRAY_FLOAT64 as FLOAT64,
        JSTypedArrayEnum_JS_TYPED_ARRAY_INT16 as INT16,
        JSTypedArrayEnum_JS_TYPED_ARRAY_INT32 as INT32,
        JSTypedArrayEnum_JS_TYPED_ARRAY_UINT16 as UINT16,
        JSTypedArrayEnum_JS_TYPED_ARRAY_UINT32 as UINT32,
    };
    // SAFETY: reads the value's typed-array class tag only; anything else
    // answers -1.
    match u32::try_from(unsafe { rquickjs::qjs::JS_GetTypedArrayType(value.as_raw()) }) {
        Ok(INT16 | UINT16 | FLOAT16) => 2,
        Ok(INT32 | UINT32 | FLOAT32) => 4,
        Ok(BIG_INT64 | BIG_UINT64 | FLOAT64) => 8,
        _ => 1,
    }
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

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePipeline")]
pub struct GPUComputePipeline<'js> {
    device: Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    inner:  wgpu::ComputePipeline,
    #[qjs(skip_trace)]
    label:  Rc<RefCell<String>>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUComputePipeline<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn get_bind_group_layout(&self, index: JsU32, ctx: Ctx<'js>) -> Result<GPUBindGroupLayout> {
        let inner = self.inner.get_bind_group_layout(index.0);
        flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUBindGroupLayout {
            inner,
            label: Rc::new(RefCell::new(String::new())),
        })
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUCommandEncoder")]
pub struct GPUCommandEncoder<'js> {
    device:  Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    label:   RefCell<String>,
    /// Shared with the render passes it opens: a pass command the safe API
    /// cannot forward invalidates the encoder, and WebGPU reports that at
    /// `finish`.
    #[qjs(skip_trace)]
    encoder: Rc<Recorder<wgpu::CommandEncoder>>,
}

impl<'js> GPUCommandEncoder<'js> {
    fn errors(&self) -> ErrorSink { self.device.borrow().errors.clone() }

    fn record(
        &self, ctx: &Ctx<'js>, operation: impl FnOnce(&mut wgpu::CommandEncoder),
    ) -> Result<()> {
        self.encoder.record(&self.errors(), operation);
        flush_uncaptured(&self.device, ctx)
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
    ) -> Result<GPUComputePassEncoder<'js>> {
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
            .map(|writes| query::timestamp_writes_from(&writes, &ctx))
            .transpose()?;
        let timestamp_writes = timestamp.as_ref().map(|(query_set, beginning, end)| {
            wgpu::ComputePassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: *beginning,
                end_of_pass_write_index: *end,
            }
        });
        let pass = self.encoder.map(&self.errors(), |encoder| {
            encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: (!label.is_empty()).then_some(label.as_str()),
                    timestamp_writes,
                })
                .forget_lifetime()
        });
        flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUComputePassEncoder {
            device: self.device.clone(),
            label:  RefCell::new(label),
            pass:   pass.map_or_else(
                || Recorder::invalid("GPUComputePassEncoder"),
                |pass| Recorder::open("GPUComputePassEncoder", pass),
            ),
        })
    }

    pub fn begin_render_pass(
        &self, descriptor: Object<'js>, ctx: Ctx<'js>,
    ) -> Result<render::GPURenderPassEncoder<'js>> {
        // The descriptor is read before the encoder is borrowed: its getters
        // are script-visible and may re-enter this encoder.
        let mut plan = render::read_render_pass(&descriptor, &ctx)?;
        let label = plan.label.clone();
        let begun = plan.fault.take().map_or_else(
            || {
                self.encoder
                    .map(&self.errors(), |encoder| plan.begin(encoder))
            },
            |message| {
                self.encoder.defer(message);
                None
            },
        );
        flush_uncaptured(&self.device, &ctx)?;
        Ok(render::GPURenderPassEncoder::new(
            self.device.clone(),
            label,
            begun.map_or_else(
                || Recorder::invalid("GPURenderPassEncoder"),
                |pass| Recorder::open("GPURenderPassEncoder", pass),
            ),
            self.encoder.clone(),
        ))
    }

    pub fn copy_buffer_to_buffer(
        &self, source: Class<'js, GPUBuffer<'js>>, source_offset_or_destination: Value<'js>,
        destination_or_size: Opt<Value<'js>>, destination_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        // WebGPU overloads: (source, destination, size?) and
        // (source, sourceOffset, destination, destinationOffset, size?).
        let (source_offset, destination, destination_offset, size) = if let Ok(destination) =
            Class::<GPUBuffer<'js>>::from_js(&ctx, source_offset_or_destination.clone())
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
            let destination = Class::<GPUBuffer<'js>>::from_js(&ctx, destination)?;
            (
                source_offset.0,
                destination,
                destination_offset.0.flatten().map_or(0, |value| value.0),
                size.0.flatten(),
            )
        };
        let source = source.borrow().inner.clone();
        let destination = destination.borrow().inner.clone();
        self.record(&ctx, |encoder| {
            encoder.copy_buffer_to_buffer(
                &source,
                source_offset,
                &destination,
                destination_offset,
                size.map(|value| value.0),
            );
        })
    }

    pub fn copy_buffer_to_texture(
        &self, source: Object<'js>, destination: Object<'js>, copy_size: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let (buffer, layout) = texture::texel_copy_buffer(source, &ctx)?;
        let (texture, mip_level, origin, aspect) = texture::texel_copy_texture(destination, &ctx)?;
        let size = format::extent3d(copy_size, &ctx)?;
        let buffer = buffer.borrow().inner.clone();
        let texture = texture.borrow().inner.clone();
        self.record(&ctx, |encoder| {
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
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
        let buffer = buffer.borrow().inner.clone();
        let texture = texture.borrow().inner.clone();
        self.record(&ctx, |encoder| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level,
                    origin,
                    aspect,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
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
        let source_texture = source_texture.borrow().inner.clone();
        let destination_texture = destination_texture.borrow().inner.clone();
        self.record(&ctx, |encoder| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture:   &source_texture,
                    mip_level: source_mip,
                    origin:    source_origin,
                    aspect:    source_aspect,
                },
                wgpu::TexelCopyTextureInfo {
                    texture:   &destination_texture,
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
        destination: Class<'js, GPUBuffer<'js>>, destination_offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let query_set = query_set.borrow().inner.clone();
        let destination = destination.borrow().inner.clone();
        let Some(query_end) = first_query.0.checked_add(query_count.0) else {
            self.errors()
                .validation("resolveQuerySet query range overflows");
            return flush_uncaptured(&self.device, &ctx);
        };
        self.record(&ctx, |encoder| {
            encoder.resolve_query_set(
                &query_set,
                first_query.0..query_end,
                &destination,
                destination_offset.0,
            );
        })
    }

    pub fn clear_buffer(
        &self, buffer: Class<'js, GPUBuffer<'js>>, offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        let offset = offset.0.flatten().map_or(0, |value| value.0);
        let size = size.0.flatten().map(|value| value.0);
        self.record(&ctx, |encoder| encoder.clear_buffer(&buffer, offset, size))
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
        let inner = self
            .encoder
            .end(&self.errors(), wgpu::CommandEncoder::finish);
        flush_uncaptured(&self.device, &ctx)?;
        Ok(GPUCommandBuffer {
            inner: RefCell::new(inner),
            label: RefCell::new(label),
        })
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |encoder| encoder.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, wgpu::CommandEncoder::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |encoder| encoder.insert_debug_marker(&value))
    }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUComputePassEncoder")]
pub struct GPUComputePassEncoder<'js> {
    device: Class<'js, GPUDevice<'js>>,
    #[qjs(skip_trace)]
    label:  RefCell<String>,
    #[qjs(skip_trace)]
    pass:   Recorder<wgpu::ComputePass<'static>>,
}

impl<'js> GPUComputePassEncoder<'js> {
    fn errors(&self) -> ErrorSink { self.device.borrow().errors.clone() }

    fn record(
        &self, ctx: &Ctx<'js>, operation: impl FnOnce(&mut wgpu::ComputePass<'static>),
    ) -> Result<()> {
        self.pass.record(&self.errors(), operation);
        flush_uncaptured(&self.device, ctx)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> GPUComputePassEncoder<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> { illegal_constructor(&ctx) }

    #[qjs(get)]
    pub fn label(&self) -> String { self.label.borrow().clone() }

    #[qjs(set, rename = "label")]
    pub fn set_label(&self, value: String) { *self.label.borrow_mut() = value; }

    pub fn set_pipeline(
        &self, pipeline: Class<'js, GPUComputePipeline<'js>>, ctx: Ctx<'js>,
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

    pub fn dispatch_workgroups(
        &self, x: JsU32, y: Opt<Option<JsU32>>, z: Opt<Option<JsU32>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let y = y.0.flatten().map_or(1, |value| value.0);
        let z = z.0.flatten().map_or(1, |value| value.0);
        self.record(&ctx, |pass| pass.dispatch_workgroups(x.0, y, z))
    }

    pub fn dispatch_workgroups_indirect(
        &self, buffer: Class<'js, GPUBuffer<'js>>, offset: JsU64, ctx: Ctx<'js>,
    ) -> Result<()> {
        let buffer = buffer.borrow().inner.clone();
        self.record(&ctx, |pass| {
            pass.dispatch_workgroups_indirect(&buffer, offset.0);
        })
    }

    pub fn set_immediates(
        &self, offset: JsU32, data: Value<'js>, data_offset: Opt<Option<JsU64>>,
        size: Opt<Option<JsU64>>, ctx: Ctx<'js>,
    ) -> Result<()> {
        let bytes = data_window(data, data_offset, size, &ctx)?;
        self.record(&ctx, |pass| pass.set_immediates(offset.0, &bytes))
    }

    pub fn end(&self, ctx: Ctx<'js>) -> Result<()> {
        // Dropping the pass ends it; wgpu validates the recorded commands
        // here and reports through the sink.
        self.pass.end(&self.errors(), drop);
        flush_uncaptured(&self.device, &ctx)
    }

    pub fn push_debug_group(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.push_debug_group(&value))
    }

    pub fn pop_debug_group(&self, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, wgpu::ComputePass::pop_debug_group)
    }

    pub fn insert_debug_marker(&self, value: String, ctx: Ctx<'js>) -> Result<()> {
        self.record(&ctx, |pass| pass.insert_debug_marker(&value))
    }
}

/// `inner` is `None` after submit or when `finish` failed; submitting it then
/// reports the validation error wgpu would have.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "GPUCommandBuffer")]
pub struct GPUCommandBuffer {
    #[qjs(skip_trace)]
    inner: RefCell<Option<wgpu::CommandBuffer>>,
    #[qjs(skip_trace)]
    label: RefCell<String>,
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

error_class!(
    GPUError,
    GPUValidationError,
    GPUOutOfMemoryError,
    GPUInternalError,
);

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
        // DOMException is a QuickJS intrinsic, present in every realm den
        // builds.
        let exception: Object =
            den_util::construct(&ctx, "DOMException", (message.as_str(), "GPUPipelineError"))?;
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

fn pipeline_error<'js>(ctx: &Ctx<'js>, message: &str) -> Result<Value<'js>> {
    let ctor: Constructor = ctx.globals().get("GPUPipelineError")?;
    let init = Object::new(ctx.clone())?;
    init.set("reason", "validation")?;
    ctor.construct((message, init))
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
    if globals.get::<_, Value>("GPU")?.is_function() {
        return Ok(());
    }
    install_constructors(globals)?;
    den_util::inherit::<GPUValidationError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUOutOfMemoryError, GPUError>(globals.ctx())?;
    den_util::inherit::<GPUInternalError, GPUError>(globals.ctx())?;
    inherit_global_prototype::<GPUDevice<'_>>(globals.ctx(), "EventTarget")?;
    inherit_global_prototype::<GPUUncapturedErrorEvent>(globals.ctx(), "Event")?;
    inherit_global_prototype::<GPUPipelineError>(globals.ctx(), "DOMException")?;
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

/// Every WebGPU interface den installs, with its constructor arity. One list
/// feeds the globals, the module declarations and the module exports, so they
/// cannot drift apart.
macro_rules! webgpu_interfaces {
    ($($name:literal => $ty:ty, $arity:literal);+ $(;)?) => {
        const GLOBAL_CLASSES: [&str; [$($name),+].len()] = [$($name),+];

        fn install_constructors(globals: &Object<'_>) -> Result<()> {
            use den_util::ConstructorInstaller as _;
            $(globals.install_constructor::<$ty>($arity)?;)+
            Ok(())
        }
    };
}

webgpu_interfaces! {
    "GPU" => GPU, 0;
    "GPUAdapter" => GPUAdapter, 0;
    "GPUAdapterInfo" => GPUAdapterInfo, 0;
    "GPUBindGroup" => GPUBindGroup, 0;
    "GPUBindGroupLayout" => GPUBindGroupLayout, 0;
    "GPUBuffer" => GPUBuffer, 0;
    "GPUCommandBuffer" => GPUCommandBuffer, 0;
    "GPUCommandEncoder" => GPUCommandEncoder, 0;
    "GPUComputePassEncoder" => GPUComputePassEncoder, 0;
    "GPUComputePipeline" => GPUComputePipeline, 0;
    "GPUDevice" => GPUDevice, 0;
    "GPUDeviceLostInfo" => GPUDeviceLostInfo, 0;
    "GPUError" => GPUError, 1;
    "GPUExternalTexture" => GPUExternalTexture, 0;
    "GPUInternalError" => GPUInternalError, 1;
    "GPUOutOfMemoryError" => GPUOutOfMemoryError, 1;
    "GPUPipelineError" => GPUPipelineError, 2;
    "GPUPipelineLayout" => GPUPipelineLayout, 0;
    "GPUQuerySet" => query::GPUQuerySet, 0;
    "GPUQueue" => GPUQueue, 0;
    "GPURenderBundle" => render::GPURenderBundle, 0;
    "GPURenderBundleEncoder" => render::GPURenderBundleEncoder, 0;
    "GPURenderPassEncoder" => render::GPURenderPassEncoder, 0;
    "GPURenderPipeline" => render::GPURenderPipeline, 0;
    "GPUSampler" => texture::GPUSampler, 0;
    "GPUShaderModule" => GPUShaderModule, 0;
    "GPUSupportedFeatures" => GPUSupportedFeatures, 0;
    "GPUSupportedLimits" => GPUSupportedLimits, 0;
    "GPUSupportedWGSLLanguageFeatures" => GPUSupportedWGSLLanguageFeatures, 0;
    "GPUTexture" => texture::GPUTexture, 0;
    "GPUTextureView" => texture::GPUTextureView, 0;
    "GPUUncapturedErrorEvent" => GPUUncapturedErrorEvent, 2;
    "GPUValidationError" => GPUValidationError, 1;
}

#[rquickjs::module(rename_vars = "camelCase", rename_types = "PascalCase")]
pub mod webgpu {
    use rquickjs::{
        Class, Ctx, Object, Result, Value,
        module::{Declarations, Exports},
    };

    use super::{
        GLOBAL_CLASSES, GPU, MAP_READ, MAP_WRITE, constants, install_classes, make_instance,
        supported::GPUSupportedWGSLLanguageFeatures,
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
