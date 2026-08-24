//! Structured-clone pre/post pass: everything `JS_WriteObject2` gets wrong.
//!
//! See docs/research/10-structured-clone-strategy.md.

use std::collections::HashMap;

use den_util::{BufferSource, ObjectExt, class_id, construct, instance_of_global};
use rquickjs::{
    Array, Class, Coerced, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Object, Result,
    Symbol, Value,
    object::Property,
    qjs,
};

use crate::{
    events::new_dom_exception, message::throw_data_clone, port::NativePort, report::sink_hook,
};

const TAG: &str = "\0den:structured-clone";

/// Per-realm clone state: the port-handle symbol JS MessagePort wrappers
/// keep their [`NativePort`] under until that wrapper is itself a Rust class.
#[derive(JsLifetime)]
pub struct CloneState<'js> {
    pub port_handle: Symbol<'js>,
}

impl<'js> CloneState<'js> {
    pub fn install(ctx: &Ctx<'js>) -> Result<Symbol<'js>> {
        let port_handle = Symbol::with_description(ctx.clone(), "den:port-handle")?;
        ctx.store_userdata(Self {
            port_handle: port_handle.clone(),
        })
        .map_err(|_| Exception::throw_internal(ctx, "den:worker is already installed"))?;
        Ok(port_handle)
    }

    pub fn port_handle(ctx: &Ctx<'js>) -> Result<Symbol<'js>> {
        ctx.userdata::<Self>()
            .map(|state| state.port_handle.clone())
            .ok_or_else(|| Exception::throw_internal(ctx, "den:worker is not installed"))
    }
}

fn fail(ctx: &Ctx<'_>, what: &str) -> rquickjs::Error {
    throw_data_clone(ctx, &format!("{what} could not be cloned."))
}

fn define_data<'js>(target: &Object<'js>, key: &str, value: Value<'js>) -> Result<()> {
    target.prop(
        key,
        Property::from(value).writable().enumerable().configurable(),
    )
}

fn is_leaf<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    if BufferSource::is_array_buffer_view(ctx, value)?
        && !instance_of_global(ctx, value, "DataView")?
    {
        return Ok(true);
    }
    Ok(instance_of_global(ctx, value, "ArrayBuffer")?
        || instance_of_global(ctx, value, "Date")?
        || instance_of_global(ctx, value, "Number")?
        || instance_of_global(ctx, value, "String")?
        || instance_of_global(ctx, value, "Boolean")?
        || instance_of_global(ctx, value, "BigInt")?)
}

fn is_out_of_bounds<'js>(ctx: &Ctx<'js>, view: &Value<'js>) -> Result<bool> {
    let outcome = if instance_of_global(ctx, view, "DataView")? {
        let proto: Object<'js> = ctx
            .globals()
            .get::<_, Function<'js>>("DataView")?
            .get("prototype")?;
        let get: Function<'js> = {
            let desc: Object<'js> = ctx
                .globals()
                .get::<_, Object<'js>>("Object")?
                .get::<_, Function<'js>>("getOwnPropertyDescriptor")?
                .call((proto, "byteOffset"))?;
            desc.get("get")?
        };
        get.call::<_, Value<'js>>((rquickjs::function::This(view.clone()),))
            .map(|_| ())
    } else {
        let uint8: Function<'js> = ctx.globals().get("Uint8Array")?;
        let proto: Object<'js> = uint8.get("prototype")?;
        let typed_proto: Object<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Object")?
            .get::<_, Function<'js>>("getPrototypeOf")?
            .call((proto,))?;
        let at: Function<'js> = typed_proto.get("at")?;
        at.call::<_, Value<'js>>((rquickjs::function::This(view.clone()), 0))
            .map(|_| ())
    };
    Ok(outcome.is_err())
}

fn forbidden_name<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<&'static str>> {
    if value.is_promise() {
        return Ok(Some("Promise"));
    }
    if instance_of_global(ctx, value, "WeakMap")? {
        return Ok(Some("WeakMap"));
    }
    if instance_of_global(ctx, value, "WeakSet")? {
        return Ok(Some("WeakSet"));
    }
    if instance_of_global(ctx, value, "WeakRef")? {
        return Ok(Some("WeakRef"));
    }
    if instance_of_global(ctx, value, "FinalizationRegistry")? {
        return Ok(Some("FinalizationRegistry"));
    }
    if ctx
        .globals()
        .get::<_, Value<'js>>("SharedArrayBuffer")
        .is_ok_and(|ctor| ctor.is_function())
        && instance_of_global(ctx, value, "SharedArrayBuffer")?
    {
        return Ok(Some("SharedArrayBuffer"));
    }
    Ok(None)
}

fn tag_object<'js>(ctx: &Ctx<'js>, kind: &str) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    define_data(&object, TAG, kind.into_js(ctx)?)?;
    Ok(object)
}

fn with_stack<'js>(error: Object<'js>, stack: Option<Value<'js>>) -> Result<Object<'js>> {
    if let Some(stack) = stack
        && stack.is_string()
    {
        error.prop("stack", Property::from(stack).writable().configurable())?;
    }
    Ok(error)
}

fn error_name(name: &str) -> &str {
    match name {
        "Error" | "EvalError" | "RangeError" | "ReferenceError" | "SyntaxError" | "TypeError"
        | "URIError" => name,
        _ => "Error",
    }
}

/// Sender-side walk. `ports` are the NativePorts of the transfer list, in
/// order.
pub fn prepare<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, ports: &[Class<'js, NativePort>],
) -> Result<Value<'js>> {
    let port_handle = CloneState::port_handle(ctx)?;
    let plain_object_class = class_id(&Object::new(ctx.clone())?.into_value());
    let mut transferred = HashMap::new();
    for (index, port) in ports.iter().enumerate() {
        transferred.insert(port.as_inner().as_value().clone(), index);
    }
    let mut seen = HashMap::new();
    copy(
        ctx,
        value,
        &port_handle,
        plain_object_class,
        &transferred,
        &mut seen,
    )
}

fn copy<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, port_handle: &Symbol<'js>,
    plain_object_class: qjs::JSClassID, transferred: &HashMap<Value<'js>, usize>,
    seen: &mut HashMap<Value<'js>, Value<'js>>,
) -> Result<Value<'js>> {
    if value.is_symbol() {
        let description = value
            .as_symbol()
            .and_then(|symbol| symbol.description().ok())
            .filter(|value| !value.is_undefined());
        let text = match description {
            Some(description) => {
                let description: String = Coerced::from_js(ctx, description)
                    .map(|Coerced(s)| s)
                    .unwrap_or_default();
                format!("Symbol({description})")
            }
            None => "Symbol()".to_owned(),
        };
        return Err(fail(ctx, &text));
    }
    if value.is_function() {
        let name: String = value
            .as_object()
            .and_then(|object| object.get::<_, String>("name").ok())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "(anonymous)".to_owned());
        return Err(fail(ctx, &format!("function {name}")));
    }
    if value.is_null()
        || value.is_undefined()
        || value.is_bool()
        || value.is_number()
        || value.is_string()
        || value.is_big_int()
    {
        return Ok(value);
    }
    if let Some(hit) = seen.get(&value) {
        return Ok(hit.clone());
    }
    if value.is_proxy() {
        return Err(fail(ctx, "#<Proxy>"));
    }
    if let Some(name) = forbidden_name(ctx, &value)? {
        return Err(fail(ctx, name));
    }
    if let Some(object) = value.as_object()
        && let Ok(port) = object.get::<_, Class<'js, NativePort>>(port_handle.clone())
    {
        let Some(&index) = transferred.get(port.as_inner().as_value()) else {
            return Err(fail(ctx, "a MessagePort that is not in the transfer list"));
        };
        let tagged = tag_object(ctx, "Port")?;
        define_data(&tagged, "index", (index as u32).into_js(ctx)?)?;
        let out = tagged.into_value();
        seen.insert(value, out.clone());
        return Ok(out);
    }
    if BufferSource::is_array_buffer_view(ctx, &value)? && is_out_of_bounds(ctx, &value)? {
        return Err(fail(
            ctx,
            "an ArrayBufferView over a detached or resized buffer",
        ));
    }
    if is_leaf(ctx, &value)? {
        seen.insert(value.clone(), value.clone());
        return Ok(value);
    }
    if instance_of_global(ctx, &value, "RegExp")? {
        let tagged = tag_object(ctx, "RegExp")?;
        let object = value.as_object().expect("RegExp");
        define_data(&tagged, "source", object.get("source")?)?;
        define_data(&tagged, "flags", object.get("flags")?)?;
        let out = tagged.into_value();
        seen.insert(value, out.clone());
        return Ok(out);
    }
    if instance_of_global(ctx, &value, "DataView")? {
        let tagged = tag_object(ctx, "DataView")?;
        let object = value.as_object().expect("DataView");
        define_data(
            &tagged,
            "buffer",
            copy(
                ctx,
                object.get("buffer")?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?,
        )?;
        define_data(&tagged, "byteOffset", object.get("byteOffset")?)?;
        define_data(&tagged, "byteLength", object.get("byteLength")?)?;
        let out = tagged.into_value();
        seen.insert(value, out.clone());
        return Ok(out);
    }
    if instance_of_global(ctx, &value, "DOMException")? {
        let tagged = tag_object(ctx, "DOMException")?;
        let object = value.as_object().expect("DOMException");
        define_data(&tagged, "name", object.get("name")?)?;
        define_data(&tagged, "message", object.get("message")?)?;
        if let Ok(stack) = object.get::<_, Value<'js>>("stack")
            && stack.is_string()
        {
            define_data(&tagged, "stack", stack)?;
        }
        let out = tagged.into_value();
        seen.insert(value, out.clone());
        return Ok(out);
    }
    if value.is_error() {
        let object = value.as_object().expect("Error");
        let name: String = object
            .get::<_, String>("name")
            .unwrap_or_else(|_| "Error".to_owned());
        let tagged = tag_object(ctx, "Error")?;
        define_data(&tagged, "name", error_name(&name).into_js(ctx)?)?;
        if object.has_own("message")? {
            let message: Value<'js> = object.get("message")?;
            define_data(
                &tagged,
                "message",
                Coerced::<String>::from_js(ctx, message)?.0.into_js(ctx)?,
            )?;
        }
        if let Ok(stack) = object.get::<_, Value<'js>>("stack")
            && stack.is_string()
        {
            define_data(&tagged, "stack", stack)?;
        }
        let out = tagged.into_value();
        seen.insert(value.clone(), out.clone());
        if object.has_own("cause")? {
            let cause = copy(
                ctx,
                object.get("cause")?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?;
            define_data(out.as_object().expect("tag"), "cause", cause)?;
        }
        return Ok(out);
    }
    if value.is_array() {
        let array = value.as_array().expect("array");
        let length = array.len();
        let dest = Array::new(ctx.clone())?;
        let _ = length;
        let dest_value = dest.clone().into_value();
        seen.insert(value.clone(), dest_value.clone());
        copy_own(
            ctx,
            &value,
            dest.as_inner(),
            port_handle,
            plain_object_class,
            transferred,
            seen,
        )?;
        return Ok(dest_value);
    }
    if instance_of_global(ctx, &value, "Map")? {
        let dest: Object<'js> = construct(ctx, "Map", ())?;
        let dest_value = dest.clone().into_value();
        seen.insert(value.clone(), dest_value.clone());
        let entries: Array<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Array")?
            .get::<_, Function<'js>>("from")?
            .call((value.clone(),))?;
        for index in 0..entries.len() {
            let pair: Array<'js> = entries.get(index)?;
            let key = copy(
                ctx,
                pair.get(0)?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?;
            let item = copy(
                ctx,
                pair.get(1)?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?;
            dest.get::<_, Function<'js>>("set")?.call::<_, ()>((
                rquickjs::function::This(dest_value.clone()),
                key,
                item,
            ))?;
        }
        return Ok(dest_value);
    }
    if instance_of_global(ctx, &value, "Set")? {
        let dest: Object<'js> = construct(ctx, "Set", ())?;
        let dest_value = dest.clone().into_value();
        seen.insert(value.clone(), dest_value.clone());
        let items: Array<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Array")?
            .get::<_, Function<'js>>("from")?
            .call((value.clone(),))?;
        for index in 0..items.len() {
            let item = copy(
                ctx,
                items.get(index)?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?;
            dest.get::<_, Function<'js>>("add")?
                .call::<_, ()>((rquickjs::function::This(dest_value.clone()), item))?;
        }
        return Ok(dest_value);
    }
    if class_id(&value) == plain_object_class {
        let dest = Object::new(ctx.clone())?;
        let dest_value = dest.clone().into_value();
        seen.insert(value.clone(), dest_value.clone());
        copy_own(
            ctx,
            &value,
            &dest,
            port_handle,
            plain_object_class,
            transferred,
            seen,
        )?;
        return Ok(dest_value);
    }
    seen.insert(value.clone(), value.clone());
    Ok(value)
}

fn copy_own<'js>(
    ctx: &Ctx<'js>, from: &Value<'js>, to: &Object<'js>, port_handle: &Symbol<'js>,
    plain_object_class: qjs::JSClassID, transferred: &HashMap<Value<'js>, usize>,
    seen: &mut HashMap<Value<'js>, Value<'js>>,
) -> Result<()> {
    let Some(object) = from.as_object() else {
        return Ok(());
    };
    if let Ok(tag) = object.get::<_, Value<'js>>(TAG)
        && !tag.is_undefined()
    {
        define_data(to, TAG, tag)?;
    }
    let keys: Vec<String> = object.keys().collect::<Result<Vec<_>>>()?;
    for key in keys {
        if key == TAG {
            continue;
        }
        if object.contains_key(&key)? {
            let copied = copy(
                ctx,
                object.get(&key)?,
                port_handle,
                plain_object_class,
                transferred,
                seen,
            )?;
            define_data(to, &key, copied)?;
        }
    }
    Ok(())
}

/// Receiver-side walk.
pub fn restore<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, ports: &[Class<'js, NativePort>],
) -> Result<Value<'js>> {
    let mut seen = HashMap::new();
    revive(ctx, value, ports, &mut seen)
}

fn wrap_port<'js>(ctx: &Ctx<'js>, port: Class<'js, NativePort>) -> Result<Value<'js>> {
    if let Some(wrap) = sink_hook(ctx, "wrapPort") {
        return wrap.call((port,));
    }
    Err(fail(ctx, "MessagePort"))
}

fn revive<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, ports: &[Class<'js, NativePort>],
    seen: &mut HashMap<Value<'js>, Value<'js>>,
) -> Result<Value<'js>> {
    if !value.is_object() || value.is_null() {
        return Ok(value);
    }
    if let Some(hit) = seen.get(&value) {
        return Ok(hit.clone());
    }
    let Some(object) = value.as_object() else {
        return Ok(value);
    };
    if let Ok(kind) = object.get::<_, String>(TAG) {
        match kind.as_str() {
            "Port" => {
                let index: usize = object.get("index")?;
                let wrapped = wrap_port(
                    ctx,
                    ports.get(index).cloned().ok_or_else(|| {
                        Exception::throw_internal(ctx, "clone port index out of range")
                    })?,
                )?;
                seen.insert(value, wrapped.clone());
                return Ok(wrapped);
            }
            "RegExp" => {
                let revived: Value<'js> = construct(
                    ctx,
                    "RegExp",
                    (
                        object.get::<_, Value<'js>>("source")?,
                        object.get::<_, Value<'js>>("flags")?,
                    ),
                )?;
                seen.insert(value, revived.clone());
                return Ok(revived);
            }
            "DataView" => {
                let revived: Value<'js> = construct(
                    ctx,
                    "DataView",
                    (
                        revive(ctx, object.get("buffer")?, ports, seen)?,
                        object.get::<_, Value<'js>>("byteOffset")?,
                        object.get::<_, Value<'js>>("byteLength")?,
                    ),
                )?;
                seen.insert(value, revived.clone());
                return Ok(revived);
            }
            "DOMException" => {
                let exception = new_dom_exception(
                    ctx,
                    &object.get::<_, String>("message").unwrap_or_default(),
                    &object.get::<_, String>("name").unwrap_or_default(),
                )?;
                let object_out = exception.as_object().expect("DOMException").clone();
                let stacked = with_stack(object_out, object.get::<_, Value<'js>>("stack").ok())?;
                let revived = stacked.into_value();
                seen.insert(value, revived.clone());
                return Ok(revived);
            }
            "Error" => {
                let name: String = object.get("name").unwrap_or_else(|_| "Error".to_owned());
                let ctor_name = error_name(&name);
                let message: OptMessage = object.get("message").unwrap_or(OptMessage(None));
                let revived_obj: Object<'js> = match message.0 {
                    Some(message) => construct(ctx, ctor_name, (message,))?,
                    None => construct(ctx, ctor_name, ())?,
                };
                let stacked = with_stack(
                    revived_obj.clone(),
                    object.get::<_, Value<'js>>("stack").ok(),
                )?;
                let revived = stacked.into_value();
                seen.insert(value.clone(), revived.clone());
                if object.has_own("cause")? {
                    let cause = revive(ctx, object.get("cause")?, ports, seen)?;
                    define_data(revived.as_object().expect("Error"), "cause", cause)?;
                }
                return Ok(revived);
            }
            _ => {}
        }
    }
    seen.insert(value.clone(), value.clone());
    if value.is_array() {
        let array = value.as_array().expect("array");
        let object: &Object<'js> = array.as_inner();
        let keys: Vec<String> = object.keys().collect::<Result<Vec<_>>>()?;
        for key in keys {
            let revived = revive(ctx, object.get(&key)?, ports, seen)?;
            object.set(key, revived)?;
        }
    } else if instance_of_global(ctx, &value, "Map")? {
        let entries: Array<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Array")?
            .get::<_, Function<'js>>("from")?
            .call((value.clone(),))?;
        object
            .get::<_, Function<'js>>("clear")?
            .call::<_, ()>((rquickjs::function::This(value.clone()),))?;
        let set: Function<'js> = object.get("set")?;
        for index in 0..entries.len() {
            let pair: Array<'js> = entries.get(index)?;
            let key = revive(ctx, pair.get(0)?, ports, seen)?;
            let item = revive(ctx, pair.get(1)?, ports, seen)?;
            set.call::<_, ()>((rquickjs::function::This(value.clone()), key, item))?;
        }
    } else if instance_of_global(ctx, &value, "Set")? {
        let items: Array<'js> = ctx
            .globals()
            .get::<_, Object<'js>>("Array")?
            .get::<_, Function<'js>>("from")?
            .call((value.clone(),))?;
        object
            .get::<_, Function<'js>>("clear")?
            .call::<_, ()>((rquickjs::function::This(value.clone()),))?;
        let add: Function<'js> = object.get("add")?;
        for index in 0..items.len() {
            let item = revive(ctx, items.get(index)?, ports, seen)?;
            add.call::<_, ()>((rquickjs::function::This(value.clone()), item))?;
        }
    } else if class_id(&value) == class_id(&Object::new(ctx.clone())?.into_value()) {
        let keys: Vec<String> = object.keys().collect::<Result<Vec<_>>>()?;
        for key in keys {
            let revived = revive(ctx, object.get(&key)?, ports, seen)?;
            object.set(key, revived)?;
        }
    }
    Ok(value)
}

struct OptMessage(Option<String>);

impl<'js> rquickjs::FromJs<'js> for OptMessage {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if value.is_undefined() {
            Ok(Self(None))
        } else {
            Ok(Self(Some(Coerced::<String>::from_js(ctx, value)?.0)))
        }
    }
}

/// Split a transfer list into ArrayBuffers and NativePorts.
pub fn split_transfer<'js>(
    ctx: &Ctx<'js>, transfer: Option<Value<'js>>,
) -> Result<(Vec<Value<'js>>, Vec<Class<'js, NativePort>>)> {
    let mut buffers = Vec::new();
    let mut ports = Vec::new();
    let Some(transfer) = transfer else {
        return Ok((buffers, ports));
    };
    if transfer.is_undefined() || transfer.is_null() {
        return Ok((buffers, ports));
    }
    let port_handle = CloneState::port_handle(ctx)?;
    let Some(list) = transfer.as_array() else {
        return Err(fail(ctx, "a value in the transfer list"));
    };
    for index in 0..list.len() {
        let entry: Value<'js> = list.get(index)?;
        if instance_of_global(ctx, &entry, "ArrayBuffer")? {
            buffers.push(entry);
            continue;
        }
        if let Some(object) = entry.as_object()
            && let Ok(port) = object.get::<_, Class<'js, NativePort>>(port_handle.clone())
        {
            ports.push(port);
            continue;
        }
        return Err(fail(ctx, "a value in the transfer list"));
    }
    Ok((buffers, ports))
}
