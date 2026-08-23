//! DOM `Event` / `EventTarget` and the event subclasses every other API
//! dispatches, as `#[rquickjs::class]` types.
//!
//! Listener maps live in the [`EventTarget`] Rust struct. Methods take `this`
//! as a JS value so they still work on objects that merely inherit
//! `EventTarget.prototype` — JS `class FileReader extends EventTarget`, native
//! subclasses that reparent their prototype, and both the main realm and a
//! worker global, which become EventTargets by borrowing these methods rather
//! than being constructed as one. A hidden slot on such objects holds the
//! struct.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

use den_stdlib_core::report::{print_exception, report_exception, set_exception_sink};
use den_util::{coerce_string, inherit, throw_dom_exception};
use rquickjs::{
    Array, Class, Coerced, Ctx, Error, Exception, FromJs, Function, IntoJs, JsLifetime, Object,
    Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Args, Opt, This},
    object::{Accessor, Property},
    qjs,
};

/// Hidden own property holding an [`EventTarget`] for objects that inherit the
/// prototype without being an `EventTarget` instance. Non-enumerable, so
/// structured clone's `Object.keys` walk never sees it.
const TARGET_SLOT: &str = "\0den:EventTarget";

/// DOM §2.2 phase constants. Observable on both the constructor and the
/// prototype; dispatch itself is always [`AT_TARGET`].
const PHASE_NONE: u32 = 0;
const PHASE_CAPTURING: u32 = 1;
const PHASE_AT_TARGET: u32 = 2;
const PHASE_BUBBLING: u32 = 3;

/// Wall-clock origin of this realm, captured on first use. `Event.timeStamp`
/// is milliseconds since that moment.
#[derive(JsLifetime)]
struct TimeOrigin(f64);

/// Construct a `DOMException` from the engine intrinsic.
pub(crate) fn new_dom_exception<'js>(
    ctx: &Ctx<'js>, message: &str, name: &str,
) -> Result<Value<'js>> {
    let ctor: rquickjs::Constructor<'js> = ctx.globals().get("DOMException")?;
    ctor.construct((message, name))
}

fn unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn time_stamp(ctx: &Ctx<'_>) -> f64 {
    let origin = if let Some(origin) = ctx.userdata::<TimeOrigin>() {
        origin.0
    } else {
        let origin = unix_ms();
        let _ = ctx.store_userdata(TimeOrigin(origin));
        origin
    };
    unix_ms() - origin
}

fn to_bool<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<bool> {
    Ok(Coerced::<bool>::from_js(ctx, value)?.0)
}

/// WebIDL `unsigned long`: `ToUint32`, which is `ToInt32` bit-cast.
fn to_unsigned_long<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<u32> {
    Ok(Coerced::<i32>::from_js(ctx, value)?.0 as u32)
}

fn dict_get<'js>(options: Option<&Value<'js>>, key: &str) -> Result<Option<Value<'js>>> {
    let Some(options) = options else {
        return Ok(None);
    };
    if options.is_undefined() {
        return Ok(None);
    }
    let Some(object) = options.as_object() else {
        return Err(Exception::throw_type(
            options.ctx(),
            "The provided value cannot be converted to a dictionary",
        ));
    };
    let value: Value<'js> = object.get(key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn dict_bool<'js>(ctx: &Ctx<'js>, options: Option<&Value<'js>>, key: &str) -> Result<bool> {
    match dict_get(options, key)? {
        Some(value) => to_bool(ctx, value),
        None => Ok(false),
    }
}

fn call_with_this<'js>(
    ctx: &Ctx<'js>, function: &Function<'js>, this: Value<'js>,
    args: impl IntoIterator<Item = Value<'js>>,
) -> Result<Value<'js>> {
    let collected: Vec<Value<'js>> = args.into_iter().collect();
    let mut call = Args::new(ctx.clone(), collected.len());
    call.this(this)?;
    for arg in collected {
        call.push_arg(arg)?;
    }
    function.call_arg(call)
}

pub(crate) fn freeze<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<()> {
    // SAFETY: `JS_FreezeObject` only reads the object header and walks its
    // own properties; a negative return means a pending exception.
    let rc = unsafe { qjs::JS_FreezeObject(ctx.as_raw().as_ptr(), value.as_raw()) };
    if rc < 0 {
        Err(Error::Exception)
    } else {
        Ok(())
    }
}

fn patch_length<'js>(constructors: &Object<'js>, name: &str, length: usize) -> Result<()> {
    let constructor: Function<'js> = constructors.get(name)?;
    constructor.set_length(length)
}

fn patch_event_constants<'js>(ctx: &Ctx<'js>, constructors: &Object<'js>) -> Result<()> {
    let constructor: Object<'js> = constructors.get("Event")?;
    let Some(proto) = Class::<Event>::prototype(ctx)? else {
        return Ok(());
    };
    for (name, value) in [
        ("NONE", PHASE_NONE),
        ("CAPTURING_PHASE", PHASE_CAPTURING),
        ("AT_TARGET", PHASE_AT_TARGET),
        ("BUBBLING_PHASE", PHASE_BUBBLING),
    ] {
        let property = Property::from(value).enumerable();
        constructor.prop(name, property)?;
        proto.prop(name, Property::from(value).enumerable())?;
    }
    Ok(())
}

/// Shared Event attributes. Native subclasses (CustomEvent, …) hold one of
/// these so `Event.prototype` getters can find their state after the prototype
/// is reparented; JS `class ProgressEvent extends Event` is an Event instance
/// and uses [`Event`]'s own copy.
#[derive(Trace, JsLifetime)]
pub struct EventFields<'js> {
    event_type:       String,
    target:           Value<'js>,
    current_target:   Value<'js>,
    event_phase:      u32,
    bubbles:          bool,
    cancelable:       bool,
    composed:         bool,
    canceled:         bool,
    stop_propagation: bool,
    stop_immediate:   bool,
    dispatch:         bool,
    is_trusted:       bool,
    time_stamp:       f64,
}

impl<'js> EventFields<'js> {
    fn new(
        ctx: &Ctx<'js>, event_type: String, bubbles: bool, cancelable: bool, composed: bool,
    ) -> Self {
        Self {
            event_type,
            target: Value::new_null(ctx.clone()),
            current_target: Value::new_null(ctx.clone()),
            event_phase: PHASE_NONE,
            bubbles,
            cancelable,
            composed,
            canceled: false,
            stop_propagation: false,
            stop_immediate: false,
            dispatch: false,
            is_trusted: false,
            time_stamp: time_stamp(ctx),
        }
    }

    fn from_args(
        ctx: &Ctx<'js>, event_type: Value<'js>, options: Option<&Value<'js>>,
    ) -> Result<Self> {
        Ok(Self::new(
            ctx,
            coerce_string(ctx, event_type)?,
            dict_bool(ctx, options, "bubbles")?,
            dict_bool(ctx, options, "cancelable")?,
            dict_bool(ctx, options, "composed")?,
        ))
    }

    fn prevent_default(&mut self) {
        if self.cancelable {
            self.canceled = true;
        }
    }
}

fn with_event_fields<'js, R>(
    ctx: &Ctx<'js>, this: &Value<'js>, body: impl FnOnce(&mut EventFields<'js>) -> Result<R>,
) -> Result<R> {
    if let Ok(event) = Class::<Event>::from_value(this) {
        return body(&mut event.try_borrow_mut()?.fields);
    }
    if let Ok(event) = Class::<CustomEvent>::from_value(this) {
        return body(&mut event.try_borrow_mut()?.fields);
    }
    if let Ok(event) = Class::<MessageEvent>::from_value(this) {
        return body(&mut event.try_borrow_mut()?.fields);
    }
    if let Ok(event) = Class::<ErrorEvent>::from_value(this) {
        return body(&mut event.try_borrow_mut()?.fields);
    }
    if let Ok(event) = Class::<PromiseRejectionEvent>::from_value(this) {
        return body(&mut event.try_borrow_mut()?.fields);
    }
    Err(Exception::throw_type(
        ctx,
        "Illegal invocation: not an Event",
    ))
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct Event<'js> {
    fields: EventFields<'js>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Event<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, event_type: Value<'js>, options: Opt<Value<'js>>) -> Result<Self> {
        Ok(Self {
            fields: EventFields::from_args(&ctx, event_type, options.0.as_ref())?,
        })
    }

    #[qjs(get, rename = "type")]
    pub fn type_(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.event_type.clone()))
    }

    #[qjs(get)]
    pub fn target(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.target.clone()))
    }

    #[qjs(get)]
    pub fn current_target(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.current_target.clone()))
    }

    #[qjs(get)]
    pub fn event_phase(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<u32> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.event_phase))
    }

    #[qjs(get)]
    pub fn bubbles(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.bubbles))
    }

    #[qjs(get)]
    pub fn cancelable(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.cancelable))
    }

    #[qjs(get)]
    pub fn composed(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.composed))
    }

    #[qjs(get)]
    pub fn default_prevented(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.canceled))
    }

    #[qjs(get)]
    pub fn is_trusted(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.is_trusted))
    }

    #[qjs(get)]
    pub fn time_stamp(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<f64> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.time_stamp))
    }

    #[qjs(get)]
    pub fn src_element(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.target.clone()))
    }

    #[qjs(get)]
    pub fn cancel_bubble(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(fields.stop_propagation))
    }

    #[qjs(set, rename = "cancelBubble")]
    pub fn set_cancel_bubble(
        this: This<Value<'js>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        if to_bool(&ctx, value)? {
            with_event_fields(&ctx, &this.0, |fields| {
                fields.stop_propagation = true;
                Ok(())
            })?;
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn return_value(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<bool> {
        with_event_fields(&ctx, &this.0, |fields| Ok(!fields.canceled))
    }

    #[qjs(set, rename = "returnValue")]
    pub fn set_return_value(
        this: This<Value<'js>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        if !to_bool(&ctx, value)? {
            with_event_fields(&ctx, &this.0, |fields| {
                fields.prevent_default();
                Ok(())
            })?;
        }
        Ok(())
    }

    pub fn composed_path(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<Array<'js>> {
        let path = Array::new(ctx.clone())?;
        with_event_fields(&ctx, &this.0, |fields| {
            if fields.dispatch {
                path.set(0, fields.target.clone())?;
            }
            Ok(())
        })?;
        Ok(path)
    }

    pub fn init_event(
        this: This<Value<'js>>, ctx: Ctx<'js>, event_type: Value<'js>, bubbles: Opt<Value<'js>>,
        cancelable: Opt<Value<'js>>,
    ) -> Result<()> {
        let event_type = coerce_string(&ctx, event_type)?;
        let bubbles = match bubbles.0 {
            Some(value) => to_bool(&ctx, value)?,
            None => false,
        };
        let cancelable = match cancelable.0 {
            Some(value) => to_bool(&ctx, value)?,
            None => false,
        };
        with_event_fields(&ctx, &this.0, |fields| {
            if fields.dispatch {
                return Ok(());
            }
            fields.event_type = event_type;
            fields.bubbles = bubbles;
            fields.cancelable = cancelable;
            fields.canceled = false;
            fields.stop_propagation = false;
            fields.stop_immediate = false;
            fields.is_trusted = false;
            fields.target = Value::new_null(ctx.clone());
            Ok(())
        })
    }

    pub fn prevent_default(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<()> {
        with_event_fields(&ctx, &this.0, |fields| {
            fields.prevent_default();
            Ok(())
        })
    }

    pub fn stop_propagation(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<()> {
        with_event_fields(&ctx, &this.0, |fields| {
            fields.stop_propagation = true;
            Ok(())
        })
    }

    pub fn stop_immediate_propagation(this: This<Value<'js>>, ctx: Ctx<'js>) -> Result<()> {
        with_event_fields(&ctx, &this.0, |fields| {
            fields.stop_propagation = true;
            fields.stop_immediate = true;
            Ok(())
        })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Event" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct CustomEvent<'js> {
    fields: EventFields<'js>,
    detail: Value<'js>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> CustomEvent<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, event_type: Value<'js>, options: Opt<Value<'js>>) -> Result<Self> {
        let options = options.0;
        let detail = match dict_get(options.as_ref(), "detail")? {
            Some(value) => value,
            None => Value::new_null(ctx.clone()),
        };
        Ok(Self {
            fields: EventFields::from_args(&ctx, event_type, options.as_ref())?,
            detail,
        })
    }

    #[qjs(get)]
    pub fn detail(&self) -> Value<'js> { self.detail.clone() }

    pub fn init_custom_event(
        this: This<Value<'js>>, ctx: Ctx<'js>, event_type: Value<'js>, bubbles: Opt<Value<'js>>,
        cancelable: Opt<Value<'js>>, detail: Opt<Value<'js>>,
    ) -> Result<()> {
        Event::init_event(
            This(this.0.clone()),
            ctx.clone(),
            event_type,
            bubbles,
            cancelable,
        )?;
        let Ok(event) = Class::<Self>::from_value(&this.0) else {
            return Ok(());
        };
        let mut event = event.try_borrow_mut()?;
        if event.fields.dispatch {
            return Ok(());
        }
        event.detail = detail.0.unwrap_or_else(|| Value::new_null(ctx.clone()));
        Ok(())
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "CustomEvent" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct MessageEvent<'js> {
    fields:        EventFields<'js>,
    data:          Value<'js>,
    origin:        String,
    last_event_id: String,
    source:        Value<'js>,
    ports:         Value<'js>,
}

impl<'js> MessageEvent<'js> {
    fn freeze_ports(ctx: &Ctx<'js>, ports: Option<Value<'js>>) -> Result<Value<'js>> {
        let copy = Array::new(ctx.clone())?;
        if let Some(ports) = ports
            && !ports.is_null()
        {
            let Some(ports) = ports.as_array() else {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to convert ports to an array",
                ));
            };
            for index in 0..ports.len() {
                copy.set(index, ports.get::<Value<'js>>(index)?)?;
            }
        }
        let value = copy.into_value();
        freeze(ctx, &value)?;
        Ok(value)
    }

    fn string_or(ctx: &Ctx<'js>, value: Option<Value<'js>>, fallback: &str) -> Result<String> {
        match value {
            Some(value) if !value.is_undefined() => coerce_string(ctx, value),
            _ => Ok(fallback.to_owned()),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> MessageEvent<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, event_type: Value<'js>, options: Opt<Value<'js>>) -> Result<Self> {
        let options = options.0;
        let data = match dict_get(options.as_ref(), "data")? {
            Some(value) => value,
            None => Value::new_null(ctx.clone()),
        };
        Ok(Self {
            fields: EventFields::from_args(&ctx, event_type, options.as_ref())?,
            data,
            origin: Self::string_or(&ctx, dict_get(options.as_ref(), "origin")?, "")?,
            last_event_id: Self::string_or(&ctx, dict_get(options.as_ref(), "lastEventId")?, "")?,
            source: match dict_get(options.as_ref(), "source")? {
                Some(value) => value,
                None => Value::new_null(ctx.clone()),
            },
            ports: Self::freeze_ports(&ctx, dict_get(options.as_ref(), "ports")?)?,
        })
    }

    #[qjs(get)]
    pub fn data(&self) -> Value<'js> { self.data.clone() }

    #[qjs(get)]
    pub fn origin(&self) -> String { self.origin.clone() }

    #[qjs(get)]
    pub fn last_event_id(&self) -> String { self.last_event_id.clone() }

    #[qjs(get)]
    pub fn source(&self) -> Value<'js> { self.source.clone() }

    #[qjs(get)]
    pub fn ports(&self) -> Value<'js> { self.ports.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "MessageEvent" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct ErrorEvent<'js> {
    fields:   EventFields<'js>,
    message:  String,
    filename: String,
    lineno:   u32,
    colno:    u32,
    error:    Value<'js>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ErrorEvent<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, event_type: Value<'js>, options: Opt<Value<'js>>) -> Result<Self> {
        let options = options.0;
        let message = match dict_get(options.as_ref(), "message")? {
            Some(value) if !value.is_undefined() => coerce_string(&ctx, value)?,
            _ => String::new(),
        };
        let filename = match dict_get(options.as_ref(), "filename")? {
            Some(value) if !value.is_undefined() => coerce_string(&ctx, value)?,
            _ => String::new(),
        };
        let lineno = match dict_get(options.as_ref(), "lineno")? {
            Some(value) => to_unsigned_long(&ctx, value)?,
            None => 0,
        };
        let colno = match dict_get(options.as_ref(), "colno")? {
            Some(value) => to_unsigned_long(&ctx, value)?,
            None => 0,
        };
        let error = match dict_get(options.as_ref(), "error")? {
            Some(value) => value,
            None => Value::new_undefined(ctx.clone()),
        };
        Ok(Self {
            fields: EventFields::from_args(&ctx, event_type, options.as_ref())?,
            message,
            filename,
            lineno,
            colno,
            error,
        })
    }

    #[qjs(get)]
    pub fn message(&self) -> String { self.message.clone() }

    #[qjs(get)]
    pub fn filename(&self) -> String { self.filename.clone() }

    #[qjs(get)]
    pub fn lineno(&self) -> u32 { self.lineno }

    #[qjs(get)]
    pub fn colno(&self) -> u32 { self.colno }

    #[qjs(get)]
    pub fn error(&self) -> Value<'js> { self.error.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ErrorEvent" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct PromiseRejectionEvent<'js> {
    fields:  EventFields<'js>,
    promise: Value<'js>,
    reason:  Value<'js>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> PromiseRejectionEvent<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, event_type: Value<'js>, options: Value<'js>) -> Result<Self> {
        Ok(Self {
            fields:  EventFields::from_args(&ctx, event_type, Some(&options))?,
            promise: dict_get(Some(&options), "promise")?
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            reason:  dict_get(Some(&options), "reason")?
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        })
    }

    #[qjs(get)]
    pub fn promise(&self) -> Value<'js> { self.promise.clone() }

    #[qjs(get)]
    pub fn reason(&self) -> Value<'js> { self.reason.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "PromiseRejectionEvent" }
}

#[derive(Clone, JsLifetime)]
struct Listener<'js> {
    callback: Value<'js>,
    capture:  bool,
    once:     bool,
    removed:  Rc<Cell<bool>>,
}

impl<'js> Trace<'js> for Listener<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.callback.trace(tracer); }
}

#[derive(JsLifetime)]
struct HandlerSlot<'js> {
    value:    Value<'js>,
    listener: Option<Value<'js>>,
}

impl<'js> Trace<'js> for HandlerSlot<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.value.trace(tracer);
        if let Some(listener) = &self.listener {
            listener.trace(tracer);
        }
    }
}

#[derive(Default, JsLifetime)]
struct ListenerTable<'js> {
    by_type:  HashMap<String, Vec<Listener<'js>>>,
    handlers: HashMap<String, HandlerSlot<'js>>,
}

impl<'js> Trace<'js> for ListenerTable<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        for list in self.by_type.values() {
            for listener in list {
                listener.trace(tracer);
            }
        }
        for handler in self.handlers.values() {
            handler.trace(tracer);
        }
    }
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct EventTarget<'js> {
    table: RefCell<ListenerTable<'js>>,
}

impl<'js> Trace<'js> for EventTarget<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(table) = self.table.try_borrow() {
            table.trace(tracer);
        }
    }
}

impl<'js> EventTarget<'js> {
    fn new() -> Self {
        Self {
            table: RefCell::new(ListenerTable::default()),
        }
    }

    /// Copy `addEventListener` / `removeEventListener` / `dispatchEvent` onto
    /// `target`, bound so `this` is that object. The main realm and a worker
    /// global both become EventTargets this way instead of being constructed
    /// as one.
    pub fn bind_on(ctx: &Ctx<'js>, target: &Object<'js>) -> Result<()> {
        let Some(proto) = Class::<Self>::prototype(ctx)? else {
            return Ok(());
        };
        let this = target.clone().into_value();
        for method_name in ["addEventListener", "removeEventListener", "dispatchEvent"] {
            let method: Function<'js> = proto.get(method_name)?;
            let bind: Function<'js> = method.get("bind")?;
            let bound: Function<'js> = bind.call((This(method.clone()), this.clone()))?;
            target.prop(method_name, Property::from(bound).writable().configurable())?;
        }
        Ok(())
    }

    /// The EventTarget whose listener map `this` should use: the instance
    /// itself, or a hidden one attached to an inheriting object.
    pub fn resolve(ctx: &Ctx<'js>, this: &Value<'js>) -> Result<Class<'js, Self>> {
        if let Ok(target) = Class::<Self>::from_value(this) {
            return Ok(target);
        }
        let Some(object) = this.as_object() else {
            return Err(Exception::throw_type(ctx, "Illegal invocation"));
        };
        if let Ok(existing) = object.get::<_, Class<'js, Self>>(TARGET_SLOT) {
            return Ok(existing);
        }
        let target = Class::instance(ctx.clone(), Self::new())?;
        object.prop(TARGET_SLOT, Property::from(target.clone()))?;
        Ok(target)
    }

    fn flatten(options: Option<&Value<'js>>) -> Result<(bool, bool, Option<Value<'js>>)> {
        let Some(options) = options else {
            return Ok((false, false, None));
        };
        if let Some(capture) = options.as_bool() {
            return Ok((capture, false, None));
        }
        if options.is_undefined() || options.is_null() {
            return Ok((false, false, None));
        }
        let Some(object) = options.as_object() else {
            return Ok((false, false, None));
        };
        let capture = match object.get::<_, Value<'js>>("capture")? {
            value if value.is_undefined() => false,
            value => to_bool(object.ctx(), value)?,
        };
        let once = match object.get::<_, Value<'js>>("once")? {
            value if value.is_undefined() => false,
            value => to_bool(object.ctx(), value)?,
        };
        let signal = match object.get::<_, Value<'js>>("signal")? {
            value if value.is_undefined() => None,
            value => Some(value),
        };
        Ok((capture, once, signal))
    }

    fn is_aborted(signal: &Value<'js>) -> Result<bool> {
        let Some(object) = signal.as_object() else {
            return Ok(false);
        };
        match object.get::<_, Value<'js>>("aborted")? {
            value if value.is_undefined() => Ok(false),
            value => to_bool(signal.ctx(), value),
        }
    }

    fn add_abort_listener(
        ctx: &Ctx<'js>, target: Value<'js>, signal: Value<'js>, record: Listener<'js>,
        event_type: String,
    ) -> Result<()> {
        let Some(object) = signal.as_object() else {
            return Ok(());
        };
        let Ok(add) = object.get::<_, Function<'js>>("addEventListener") else {
            return Ok(());
        };
        let remover = Function::new(ctx.clone(), {
            let target = target.clone();
            let record = record.clone();
            let event_type = event_type.clone();
            move |ctx: Ctx<'js>| -> Result<()> {
                Self::remove_record(&ctx, &target, &event_type, &record);
                Ok(())
            }
        })?;
        let options = Object::new(ctx.clone())?;
        options.set("once", true)?;
        let _ = add.call::<_, ()>((This(signal), "abort", remover, options));
        Ok(())
    }

    fn remove_record(ctx: &Ctx<'js>, this: &Value<'js>, event_type: &str, record: &Listener<'js>) {
        record.removed.set(true);
        let Ok(target) = Self::resolve(ctx, this) else {
            return;
        };
        let Ok(inner) = target.try_borrow() else {
            return;
        };
        let Ok(mut table) = inner.table.try_borrow_mut() else {
            return;
        };
        if let Some(list) = table.by_type.get_mut(event_type) {
            list.retain(|other| !Rc::ptr_eq(&other.removed, &record.removed));
        }
    }

    fn invoke_listener(
        ctx: &Ctx<'js>, record: &Listener<'js>, event: &Value<'js>, current_target: &Value<'js>,
    ) {
        let outcome = if let Some(function) = record.callback.as_function() {
            call_with_this(ctx, function, current_target.clone(), [event.clone()]).map(|_| ())
        } else if let Some(object) = record.callback.as_object() {
            match object.get::<_, Value<'js>>("handleEvent") {
                Ok(handle) => {
                    match handle.as_function() {
                        Some(function) => {
                            call_with_this(ctx, function, record.callback.clone(), [event.clone()])
                                .map(|_| ())
                        }
                        None => Err(Exception::throw_type(ctx, "handleEvent is not a function")),
                    }
                }
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        if let Err(error) = outcome {
            match error {
                Error::Exception => report_exception(ctx, &ctx.catch()),
                error => eprintln!("{error}"),
            }
        }
    }

    /// DOM §2.7 dispatch, AT_TARGET only. `trusted` is the user-agent mark:
    /// only [`dispatch_trusted`] sets it, so a script cannot forge `isTrusted`.
    pub fn dispatch(
        ctx: &Ctx<'js>, this: &Value<'js>, event: Value<'js>, trusted: bool,
    ) -> Result<bool> {
        let already = with_event_fields(ctx, &event, |fields| Ok(fields.dispatch))?;
        if already {
            return Err(throw_dom_exception(
                ctx,
                "InvalidStateError",
                "The event is already being dispatched.",
            ));
        }
        with_event_fields(ctx, &event, |fields| {
            fields.is_trusted = trusted;
            fields.dispatch = true;
            fields.target = this.clone();
            fields.current_target = this.clone();
            fields.event_phase = PHASE_AT_TARGET;
            Ok(())
        })?;
        let event_type = with_event_fields(ctx, &event, |fields| Ok(fields.event_type.clone()))?;
        let target = Self::resolve(ctx, this)?;
        let listeners = {
            let inner = target.try_borrow()?;
            let table = inner
                .table
                .try_borrow()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            table.by_type.get(&event_type).cloned().unwrap_or_default()
        };
        for record in listeners {
            if record.removed.get() {
                continue;
            }
            if record.once {
                Self::remove_record(ctx, this, &event_type, &record);
            }
            Self::invoke_listener(ctx, &record, &event, this);
            let stop = with_event_fields(ctx, &event, |fields| Ok(fields.stop_immediate))?;
            if stop {
                break;
            }
        }
        with_event_fields(ctx, &event, |fields| {
            fields.event_phase = PHASE_NONE;
            fields.current_target = Value::new_null(ctx.clone());
            fields.dispatch = false;
            fields.stop_propagation = false;
            fields.stop_immediate = false;
            Ok(!fields.canceled)
        })
    }

    fn add(
        ctx: &Ctx<'js>, this: &Value<'js>, event_type: String, callback: Value<'js>, capture: bool,
        once: bool, signal: Option<Value<'js>>,
    ) -> Result<()> {
        if let Some(signal) = &signal
            && Self::is_aborted(signal)?
        {
            return Ok(());
        }
        if callback.is_null() || callback.is_undefined() {
            return Ok(());
        }
        let target = Self::resolve(ctx, this)?;
        let record = Listener {
            callback: callback.clone(),
            capture,
            once,
            removed: Rc::new(Cell::new(false)),
        };
        {
            let inner = target.try_borrow()?;
            let mut table = inner
                .table
                .try_borrow_mut()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            let list = table.by_type.entry(event_type.clone()).or_default();
            let duplicate = list
                .iter()
                .any(|other| other.callback == callback && other.capture == capture);
            if duplicate {
                return Ok(());
            }
            list.push(record.clone());
        }
        if let Some(signal) = signal {
            Self::add_abort_listener(ctx, this.clone(), signal, record, event_type)?;
        }
        Ok(())
    }

    fn remove(
        ctx: &Ctx<'js>, this: &Value<'js>, event_type: &str, callback: &Value<'js>, capture: bool,
    ) -> Result<()> {
        let target = Self::resolve(ctx, this)?;
        let inner = target.try_borrow()?;
        let mut table = inner
            .table
            .try_borrow_mut()
            .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
        if let Some(list) = table.by_type.get_mut(event_type)
            && let Some(index) = list
                .iter()
                .position(|other| other.callback == *callback && other.capture == capture)
        {
            list[index].removed.set(true);
            list.remove(index);
        }
        Ok(())
    }

    pub(crate) fn handler_value(
        ctx: &Ctx<'js>, this: &Value<'js>, name: &str,
    ) -> Result<Value<'js>> {
        let target = Self::resolve(ctx, this)?;
        let inner = target.try_borrow()?;
        let table = inner
            .table
            .try_borrow()
            .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
        Ok(table
            .handlers
            .get(name)
            .map(|handler| handler.value.clone())
            .unwrap_or_else(|| Value::new_null(ctx.clone())))
    }

    fn set_handler(
        ctx: &Ctx<'js>, this: &Value<'js>, name: &str, event_type: &str, value: Value<'js>,
        global_on_error: bool,
    ) -> Result<()> {
        let stored = if value.is_function() || (value.is_object() && !value.is_null()) {
            value
        } else {
            Value::new_null(ctx.clone())
        };
        let target = Self::resolve(ctx, this)?;
        let existing = {
            let inner = target.try_borrow()?;
            let table = inner
                .table
                .try_borrow()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            table
                .handlers
                .get(name)
                .and_then(|slot| slot.listener.clone())
        };
        if stored.is_null() {
            if let Some(listener) = existing {
                Self::call_listener_method(
                    ctx,
                    this,
                    "removeEventListener",
                    event_type,
                    &listener,
                )?;
            }
            let inner = target.try_borrow()?;
            let mut table = inner
                .table
                .try_borrow_mut()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            table.handlers.insert(name.to_owned(), HandlerSlot {
                value:    stored,
                listener: None,
            });
            return Ok(());
        }
        if existing.is_some() {
            let inner = target.try_borrow()?;
            let mut table = inner
                .table
                .try_borrow_mut()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            if let Some(slot) = table.handlers.get_mut(name) {
                slot.value = stored;
            }
            return Ok(());
        }
        let name_owned = name.to_owned();
        let listener = Function::new(ctx.clone(), {
            let name = name_owned.clone();
            move |ctx: Ctx<'js>, this: This<Value<'js>>, event: Value<'js>| -> Result<()> {
                Self::invoke_handler(&ctx, &this.0, &name, event, global_on_error)
            }
        })?
        .into_value();
        Self::call_listener_method(ctx, this, "addEventListener", event_type, &listener)?;
        {
            let inner = target.try_borrow()?;
            let mut table = inner
                .table
                .try_borrow_mut()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            table.handlers.insert(name_owned, HandlerSlot {
                value:    stored,
                listener: Some(listener),
            });
        }
        Ok(())
    }

    /// HTML §8.1.8.1 goes through `this.addEventListener` so a target that
    /// wrapped those methods (the ref-on-listener rule) still sees the slot.
    fn call_listener_method(
        ctx: &Ctx<'js>, this: &Value<'js>, method: &str, event_type: &str, listener: &Value<'js>,
    ) -> Result<()> {
        if let Some(object) = this.as_object()
            && let Ok(function) = object.get::<_, Function<'js>>(method)
        {
            function.call::<_, ()>((This(this.clone()), event_type, listener.clone()))?;
            return Ok(());
        }
        match method {
            "addEventListener" => {
                Self::add(
                    ctx,
                    this,
                    event_type.to_owned(),
                    listener.clone(),
                    false,
                    false,
                    None,
                )
            }
            "removeEventListener" => Self::remove(ctx, this, event_type, listener, false),
            _ => Ok(()),
        }
    }

    fn invoke_handler(
        ctx: &Ctx<'js>, this: &Value<'js>, name: &str, event: Value<'js>, global_on_error: bool,
    ) -> Result<()> {
        let callback = {
            let target = Self::resolve(ctx, this)?;
            let inner = target.try_borrow()?;
            let table = inner
                .table
                .try_borrow()
                .map_err(|_| Exception::throw_internal(ctx, "EventTarget is busy"))?;
            table
                .handlers
                .get(name)
                .map(|slot| slot.value.clone())
                .unwrap_or_else(|| Value::new_null(ctx.clone()))
        };
        let Some(function) = callback.as_function() else {
            return Ok(());
        };
        let special = global_on_error
            && Class::<ErrorEvent>::from_value(&event).is_ok()
            && with_event_fields(ctx, &event, |fields| Ok(fields.event_type == "error"))?;
        let returned = if special {
            let error_event = Class::<ErrorEvent>::from_value(&event)?;
            let borrowed = error_event.try_borrow()?;
            call_with_this(ctx, function, this.clone(), [
                borrowed.message.clone().into_js(ctx)?,
                borrowed.filename.clone().into_js(ctx)?,
                Value::new_number(ctx.clone(), f64::from(borrowed.lineno)),
                Value::new_number(ctx.clone(), f64::from(borrowed.colno)),
                borrowed.error.clone(),
            ])?
        } else {
            call_with_this(ctx, function, this.clone(), [event.clone()])?
        };
        let cancel = if special {
            returned.as_bool() == Some(true)
        } else {
            returned.as_bool() == Some(false)
        };
        if cancel {
            with_event_fields(ctx, &event, |fields| {
                fields.prevent_default();
                Ok(())
            })?;
        }
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> EventTarget<'js> {
    #[qjs(constructor)]
    pub fn new_js() -> Self { Self::new() }

    pub fn add_event_listener(
        this: This<Value<'js>>, ctx: Ctx<'js>, event_type: Value<'js>, callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        if !callback.is_null()
            && !callback.is_undefined()
            && !callback.is_function()
            && !callback.is_object()
        {
            return Err(Exception::throw_type(
                &ctx,
                "addEventListener: callback is not an object",
            ));
        }
        let (capture, once, signal) = Self::flatten(options.0.as_ref())?;
        Self::add(
            &ctx,
            &this.0,
            coerce_string(&ctx, event_type)?,
            callback,
            capture,
            once,
            signal,
        )
    }

    pub fn remove_event_listener(
        this: This<Value<'js>>, ctx: Ctx<'js>, event_type: Value<'js>, callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        let (capture, _, _) = Self::flatten(options.0.as_ref())?;
        Self::remove(
            &ctx,
            &this.0,
            &coerce_string(&ctx, event_type)?,
            &callback,
            capture,
        )
    }

    pub fn dispatch_event(
        this: This<Value<'js>>, ctx: Ctx<'js>, event: Value<'js>,
    ) -> Result<bool> {
        Self::dispatch(&ctx, &this.0, event, false)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "EventTarget" }
}

/// HTML §8.1.8.1. Defines an `onX` accessor on `target` (usually a prototype).
pub fn define_event_handler<'js>(
    _ctx: Ctx<'js>, target: Object<'js>, name: String, global_on_error: Opt<bool>,
) -> Result<()> {
    let event_type = name.strip_prefix("on").unwrap_or(name.as_str()).to_owned();
    let special = global_on_error.0.unwrap_or(false);
    let name_get = name.clone();
    let name_set = name.clone();
    let type_set = event_type.clone();
    target.prop(
        name,
        Accessor::new(
            move |this: This<Value<'js>>, ctx: Ctx<'js>| -> Result<Value<'js>> {
                EventTarget::handler_value(&ctx, &this.0, &name_get)
            },
            move |this: This<Value<'js>>, ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
                EventTarget::set_handler(&ctx, &this.0, &name_set, &type_set, value, special)
            },
        )
        .configurable(),
    )?;
    Ok(())
}

/// HTML §8.1.4.7. Looks the realm's reporter up at call time so a worker that
/// replaced it is the one that hears about the value.
pub fn report_error<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
    report_exception(&ctx, &value);
    Ok(())
}

/// The runtime-only seam that marks an event trusted. Stored on the natives
/// bag so no script can reach it.
pub fn dispatch_trusted<'js>(ctx: Ctx<'js>, target: Value<'js>, event: Value<'js>) -> Result<bool> {
    EventTarget::dispatch(&ctx, &target, event, true)
}

/// Register the Event family on `target` and finish prototype links, constants
/// and constructor lengths.
pub fn define_on<'js>(target: &Object<'js>) -> Result<()> {
    Class::<EventTarget>::define(target)?;
    Class::<Event>::define(target)?;
    Class::<CustomEvent>::define(target)?;
    Class::<MessageEvent>::define(target)?;
    Class::<ErrorEvent>::define(target)?;
    Class::<PromiseRejectionEvent>::define(target)?;
    finish(target.ctx(), target)
}

/// Prototype chain, Event phase constants, constructor `length`. Safe to call
/// on the module namespace after the macro has exported the constructors.
pub fn finish<'js>(ctx: &Ctx<'js>, constructors: &Object<'js>) -> Result<()> {
    inherit::<CustomEvent, Event>(ctx)?;
    inherit::<MessageEvent, Event>(ctx)?;
    inherit::<ErrorEvent, Event>(ctx)?;
    inherit::<PromiseRejectionEvent, Event>(ctx)?;
    patch_event_constants(ctx, constructors)?;
    patch_length(constructors, "Event", 1)?;
    patch_length(constructors, "CustomEvent", 1)?;
    patch_length(constructors, "MessageEvent", 1)?;
    patch_length(constructors, "ErrorEvent", 1)?;
    patch_length(constructors, "PromiseRejectionEvent", 2)?;
    Ok(())
}

/// Add this module's natives to the `natives` bag: the printer, the trusted
/// dispatch seam, and the handler-slot installer later preludes call.
pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    natives.set(
        "reportException",
        Function::new(ctx.clone(), js_report_exception_js)?,
    )?;
    natives.set(
        "dispatchTrusted",
        Function::new(ctx.clone(), dispatch_trusted)?,
    )?;
    natives.set(
        "__defineEventHandler",
        Function::new(ctx.clone(), define_event_handler)?,
    )?;
    set_exception_sink(ctx, natives)?;
    Ok(())
}

/// `natives.reportException(value)` — the *printer*, not the dispatcher.
#[rquickjs::function(rename = "reportException")]
pub fn report_exception_js<'js>(ctx: Ctx<'js>, value: Value<'js>) { print_exception(&ctx, &value) }

#[cfg(test)]
mod tests {
    use rquickjs::{CatchResultExt, Context, FromJs, Function, Object, Result, Runtime, Value};

    const EXPOSED: [&str; 2] = ["reportError", "__defineEventHandler"];

    /// A fresh runtime whose globals are the native Event family plus the two
    /// pieces of plumbing the tests reach for (`reportError`, the handler
    /// installer, and the private natives bag).
    fn events_context() -> (Runtime, Context) {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let install = || -> Result<()> {
                let natives = Object::new(ctx.clone())?;
                super::install(&ctx, &natives)?;
                super::define_on(&ctx.globals())?;
                let globals = ctx.globals();
                globals.set(
                    "reportError",
                    Function::new(ctx.clone(), super::report_error)?,
                )?;
                globals.set(
                    "__defineEventHandler",
                    Function::new(ctx.clone(), super::define_event_handler)?,
                )?;
                globals.set("__natives", natives)?;
                let _ = EXPOSED;
                Ok(())
            };
            install()
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("event classes install");
        });
        (runtime, context)
    }

    fn eval<T>(source: &str) -> std::result::Result<T, String>
    where
        T: for<'js> FromJs<'js>,
    {
        let (_runtime, context) = events_context();
        context.with(|ctx| {
            ctx.eval::<T, _>(source)
                .catch(&ctx)
                .map_err(|err| err.to_string())
        })
    }

    fn trace(source: &str) -> String { eval::<String>(source).expect("script evaluates") }

    #[test]
    fn listeners_run_in_registration_order_and_once_removes_itself() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                const first = () => log.push("first");
                target.addEventListener("ping", first);
                target.addEventListener("ping", () => log.push("once"), { once: true });
                target.addEventListener("ping", first);
                target.addEventListener("ping", () => log.push("last"));
                target.dispatchEvent(new Event("ping"));
                target.removeEventListener("ping", first);
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,once,last,last"
        );
    }

    #[test]
    fn a_listener_removed_during_dispatch_does_not_run() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                const second = () => log.push("second");
                target.addEventListener("ping", () => {
                  log.push("first");
                  target.removeEventListener("ping", second);
                });
                target.addEventListener("ping", second);
                target.addEventListener("ping", () => log.push("third"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,third"
        );
    }

    #[test]
    fn a_listener_added_during_dispatch_runs_only_on_the_next_one() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", () => {
                  log.push("first");
                  target.addEventListener("ping", () => log.push("added"));
                }, { once: true });
                target.dispatchEvent(new Event("ping"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "first,added"
        );
    }

    #[test]
    fn stop_immediate_propagation_skips_the_remaining_listeners() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", (event) => {
                  log.push("first");
                  event.stopImmediatePropagation();
                }, { once: true });
                target.addEventListener("ping", () => log.push("second"));
                const returned = target.dispatchEvent(new Event("ping"));
                target.dispatchEvent(new Event("ping"));
                `${log.join(",")}|${returned}`
                "#
            ),
            "first,second|true"
        );
    }

    #[test]
    fn prevent_default_only_cancels_a_cancelable_event() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let prevented = "unset";
                target.addEventListener("ping", (event) => {
                  event.preventDefault();
                  prevented = String(event.defaultPrevented);
                });
                const cancelable = target.dispatchEvent(new Event("ping", { cancelable: true }));
                const plain = target.dispatchEvent(new Event("ping"));
                `${cancelable},${plain},${prevented}`
                "#
            ),
            "false,true,false"
        );
    }

    #[test]
    fn a_throwing_listener_is_reported_and_the_next_one_still_runs() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", () => {
                  log.push("threw");
                  throw new Error("reported to stderr, not to the caller");
                });
                target.addEventListener("ping", () => log.push("after"));
                const returned = target.dispatchEvent(new Event("ping"));
                `${log.join(",")}|${returned}`
                "#
            ),
            "threw,after|true"
        );
    }

    #[test]
    fn a_handle_event_object_is_a_listener_and_null_is_tolerated() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const log = [];
                target.addEventListener("ping", null);
                target.addEventListener("ping", {
                  handleEvent(event) { log.push(`${event.type}:${this.tag}`) },
                  tag: "object",
                });
                target.addEventListener("ping", {});
                target.addEventListener("ping", () => log.push("function"));
                target.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "ping:object,function"
        );
    }

    #[test]
    fn dispatch_sets_target_and_phase_and_clears_them_afterwards() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let inside = "unset";
                target.addEventListener("ping", function (event) {
                  inside = [
                    event.target === target, event.currentTarget === target,
                    event.eventPhase === Event.AT_TARGET, this === target, event.isTrusted,
                  ].join(",");
                });
                const event = new Event("ping");
                target.dispatchEvent(event);
                `${inside}|${event.target === target},${event.currentTarget},${event.eventPhase}`
                "#
            ),
            "true,true,true,true,false|true,null,0"
        );
    }

    #[test]
    fn re_dispatching_a_dispatching_event_throws_invalid_state_error() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                let name = "nothing thrown";
                target.addEventListener("ping", (event) => {
                  try { target.dispatchEvent(event) } catch (error) { name = error.name }
                });
                const event = new Event("ping");
                target.dispatchEvent(event);
                `${name},${target.dispatchEvent(event)}`
                "#
            ),
            "InvalidStateError,true"
        );
    }

    #[test]
    fn an_on_x_handler_keeps_the_position_of_its_first_assignment() {
        assert_eq!(
            trace(
                r#"
                class Widget extends EventTarget {}
                __defineEventHandler(Widget.prototype, "onping");
                const widget = new Widget();
                const log = [];
                widget.onping = () => log.push("one");
                widget.addEventListener("ping", () => log.push("two"));
                widget.onping = () => log.push("three");
                widget.dispatchEvent(new Event("ping"));
                widget.onping = null;
                widget.dispatchEvent(new Event("ping"));
                widget.onping = () => log.push("five");
                widget.dispatchEvent(new Event("ping"));
                log.join(",")
                "#
            ),
            "three,two,two,two,five"
        );
    }

    #[test]
    fn an_on_x_slot_reads_back_what_it_stores() {
        assert_eq!(
            trace(
                r#"
                class Widget extends EventTarget {}
                __defineEventHandler(Widget.prototype, "onping");
                const widget = new Widget();
                const handler = () => {};
                const initial = widget.onping;
                widget.onping = handler;
                const stored = widget.onping === handler;
                widget.onping = "not a callback";
                const primitive = widget.onping;
                widget.onping = handler;
                widget.onping = null;
                `${initial},${stored},${primitive},${widget.onping}`
                "#
            ),
            "null,true,null,null"
        );
    }

    #[test]
    fn a_handler_returning_false_cancels_and_a_global_onerror_takes_five_arguments() {
        assert_eq!(
            trace(
                r#"
                const plain = new EventTarget();
                __defineEventHandler(plain, "onping");
                plain.onping = () => false;
                const cancelledByFalse = plain.dispatchEvent(new Event("ping", { cancelable: true }));

                const global = new EventTarget();
                __defineEventHandler(global, "onerror", true);
                let seen = "unset";
                global.onerror = (message, filename, lineno, colno, error) => {
                  seen = [message, filename, lineno, colno, error].join(",");
                  return true;
                };
                const errorEvent = new ErrorEvent("error", {
                  cancelable: true, message: "boom", filename: "worker.js",
                  lineno: 3, colno: 7, error: "carried",
                });
                const cancelledByTrue = global.dispatchEvent(errorEvent);
                `${cancelledByFalse}|${seen}|${cancelledByTrue}`
                "#
            ),
            "false|boom,worker.js,3,7,carried|false"
        );
    }

    #[test]
    fn message_event_carries_a_frozen_port_array() {
        assert_eq!(
            trace(
                r#"
                const ports = ["first", "second"];
                const event = new MessageEvent("message", { data: 42, ports });
                ports.push("added after construction");
                const empty = new MessageEvent("message");
                [
                  event.data, Object.isFrozen(event.ports), event.ports.join("+"),
                  event.origin === "", event.lastEventId === "", event.source === null,
                  empty.data === null, Object.isFrozen(empty.ports), empty.ports.length,
                ].join(",")
                "#
            ),
            "42,true,first+second,true,true,true,true,true,0"
        );
    }

    #[test]
    fn error_event_carries_its_fields_and_defaults() {
        assert_eq!(
            trace(
                r#"
                const event = new ErrorEvent("error", {
                  message: "boom", filename: "worker.js", lineno: 3, colno: 7, error: "carried",
                });
                const empty = new ErrorEvent("error");
                [
                  event.message, event.filename, event.lineno, event.colno, event.error,
                  empty.message === "", empty.lineno, empty.colno, empty.error === undefined,
                ].join(",")
                "#
            ),
            "boom,worker.js,3,7,carried,true,0,0,true"
        );
    }

    #[test]
    fn the_classes_look_like_platform_objects() {
        assert_eq!(
            trace(
                r#"
                [
                  Object.prototype.toString.call(new EventTarget()),
                  Object.prototype.toString.call(new MessageEvent("message")),
                  Object.prototype.toString.call(new ErrorEvent("error")),
                  Object.keys(EventTarget.prototype).length,
                  EventTarget.prototype.addEventListener.length,
                  EventTarget.prototype.dispatchEvent.length,
                  Event.length, MessageEvent.name,
                  Event.AT_TARGET, new Event("ping").eventPhase,
                  new MessageEvent("message") instanceof Event,
                ].join(",")
                "#
            ),
            "[object EventTarget],[object MessageEvent],[object \
             ErrorEvent],0,2,1,1,MessageEvent,2,0,true"
        );
    }

    #[test]
    fn a_rust_side_report_goes_through_the_reporter_the_realm_installed() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        let seen = context.with(|ctx| {
            let report = || -> Result<String> {
                let natives = Object::new(ctx.clone())?;
                super::install(&ctx, &natives)?;
                ctx.globals().set("natives", natives)?;
                ctx.eval::<(), _>(
                    r#"globalThis.seen = "nothing";
                       natives.reportException = (value) => {
                         globalThis.seen = `chain:${value.message}`;
                       };"#,
                )?;
                let thrown = ctx.eval::<Value<'_>, _>("new Error('from rust')")?;
                den_stdlib_core::report::report_exception(&ctx, &thrown);
                ctx.globals().get::<_, String>("seen")
            };
            report()
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("the report is routed")
        });
        assert_eq!(seen, "chain:from rust");
    }

    #[test]
    fn dispatching_a_non_event_throws_a_type_error() {
        assert_eq!(
            trace(
                r#"
                try {
                  new EventTarget().dispatchEvent({ type: "ping" });
                  "no error"
                } catch (error) { error.name }
                "#
            ),
            "TypeError"
        );
    }

    #[test]
    fn only_the_runtime_dispatch_marks_an_event_trusted() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const seen = [];
                target.addEventListener("ping", (event) => {
                  seen.push(event.isTrusted);
                  if (event.type === "ping") target.dispatchEvent(new Event("pong"));
                });
                target.addEventListener("pong", (event) => seen.push(event.isTrusted));
                __natives.dispatchTrusted(target, new Event("ping"));
                target.dispatchEvent(new Event("ping"));
                const once = new Event("ping");
                __natives.dispatchTrusted(target, once);
                target.dispatchEvent(once);
                seen.join(",")
                "#
            ),
            "true,false,false,false,true,false,false,false"
        );
    }

    #[test]
    fn a_trusted_dispatch_reports_whether_the_event_was_cancelled() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                target.addEventListener("boom", (event) => event.preventDefault());
                const cancelable = __natives.dispatchTrusted(target, new Event("boom", { cancelable: true }));
                const plain = __natives.dispatchTrusted(target, new Event("boom"));
                `${cancelable},${plain}`
                "#
            ),
            "false,true"
        );
    }

    #[test]
    fn composed_path_and_src_element_follow_the_dispatch() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const event = new Event("ping");
                const before = `${event.composedPath().length},${event.srcElement}`;
                let during = "";
                target.addEventListener("ping", () => {
                  const path = event.composedPath();
                  during = `${path.length},${path[0] === target},${event.srcElement === target}`;
                });
                target.dispatchEvent(event);
                `${before}|${during}|${event.composedPath().length},${event.srcElement === target}`
                "#
            ),
            "0,null|1,true,true|0,true"
        );
    }

    #[test]
    fn the_legacy_cancel_bubble_and_return_value_flags_move_one_way_only() {
        assert_eq!(
            trace(
                r#"
                const event = new Event("ping", { cancelable: true });
                const fresh = `${event.cancelBubble},${event.returnValue}`;
                event.cancelBubble = false;
                event.returnValue = true;
                const unchanged = `${event.cancelBubble},${event.returnValue}`;
                event.cancelBubble = true;
                event.returnValue = false;
                const set = `${event.cancelBubble},${event.returnValue},${event.defaultPrevented}`;
                const plain = new Event("ping");
                plain.returnValue = false;
                `${fresh}|${unchanged}|${set}|${plain.returnValue}`
                "#
            ),
            "false,true|false,true|true,false,true|true"
        );
    }

    #[test]
    fn init_event_re_initializes_an_event_but_not_during_a_dispatch() {
        assert_eq!(
            trace(
                r#"
                const target = new EventTarget();
                const event = new Event("ping", { cancelable: true });
                event.preventDefault();
                event.initEvent("pong", true, false);
                const reset = `${event.type},${event.bubbles},${event.cancelable},${
                  event.defaultPrevented},${event.target}`;
                let midDispatch = "";
                target.addEventListener("pong", () => {
                  event.initEvent("nope");
                  midDispatch = event.type;
                });
                target.dispatchEvent(event);
                let arity = "no throw";
                try { event.initEvent(); } catch (error) { arity = error.constructor.name }
                `${reset}|${midDispatch}|${arity}`
                "#
            ),
            "pong,true,false,false,null|pong|TypeError"
        );
    }

    #[test]
    fn custom_event_carries_its_detail() {
        assert_eq!(
            trace(
                r#"
                const detail = { id: 1 };
                const event = new CustomEvent("ping", { detail, cancelable: true });
                const empty = new CustomEvent("ping");
                event.initCustomEvent("pong", true, false, "later");
                `${event.detail},${event.type},${empty.detail},${empty instanceof Event},${
                  Object.prototype.toString.call(empty)}`
                "#
            ),
            "later,pong,null,true,[object CustomEvent]"
        );
    }

    #[test]
    fn report_error_hands_the_value_to_the_realm_s_report_hook() {
        assert_eq!(
            trace(
                r#"
                const seen = [];
                __natives.reportException = (value) => seen.push(value);
                const failure = new TypeError("boom");
                reportError(failure);
                let arity = "no throw";
                try { reportError(); } catch (error) { arity = error.constructor.name }
                `${seen.length},${seen[0] === failure},${arity},${reportError.length}`
                "#
            ),
            "1,true,TypeError,1"
        );
    }
}
