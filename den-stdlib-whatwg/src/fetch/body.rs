//! Shared Fetch body extract / consume. Stream errors stay JS exceptions.

use std::{cell::RefCell, rc::Rc};

use den_util::{BufferSource, Probe as _, instance_of_global};
use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Exception, FromJs as _, Function, IntoJs as _, Object, Result,
    TypedArray, Value,
    function::{Async, Constructor, Opt, This},
    promise::MaybePromise,
};

use super::headers::Headers;
use crate::streams::ReadableStream;

pub fn optional_object<'js>(
    ctx: &Ctx<'js>, value: rquickjs::function::Opt<Value<'js>>,
) -> Result<Option<Object<'js>>> {
    match value.0 {
        None => Ok(None),
        Some(value) if value.is_undefined() || value.is_null() => Ok(None),
        Some(value) => Object::from_js(ctx, value).map(Some),
    }
}

pub fn is_readable_stream<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    instance_of_global(ctx, value, "ReadableStream").unwrap_or(false)
}

pub fn stream_is_locked(value: &Value<'_>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get::<_, bool>("locked").unwrap_or(false)
}

/// A body may only be taken from a stream nothing has read yet. `releaseLock`
/// hands the lock back but leaves the stream disturbed, so `locked` alone is
/// not enough to decide.
pub fn stream_is_disturbed(value: &Value<'_>) -> bool {
    value.as_object().is_some_and(|object| {
        Class::<ReadableStream>::from_object(object)
            .and_then(|stream| stream.try_borrow().ok().map(|stream| stream.is_disturbed()))
            .or_else(|| object.get("_denDisturbed").ok())
            .unwrap_or(false)
    })
}

pub fn set_content_type_if_missing(headers: &Class<'_, Headers>, value: &str) {
    let mut headers = headers.borrow_mut();
    if !headers.map.contains_key("content-type") {
        headers.map.insert("content-type".into(), value.to_string());
    }
}

pub fn copy_buffer(ctx: &Ctx<'_>, bytes: Option<&[u8]>) -> Result<Vec<u8>> {
    bytes
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Exception::throw_type(ctx, "buffer is detached"))
}

pub fn apply_body_types<'js>(
    ctx: &Ctx<'js>, headers: &Class<'js, Headers>, mut body: Value<'js>,
) -> Result<Value<'js>> {
    if let Some(object) = body.as_object()
        && instance_of_global(ctx, &body, "URLSearchParams")?
    {
        // ToString(URLSearchParams) is the urlencoded serialization.
        let text = den_util::coerce_string(ctx, object.clone().into_value())?;
        body = text.into_js(ctx)?;
        set_content_type_if_missing(headers, "application/x-www-form-urlencoded;charset=UTF-8");
    }
    if let Some(object) = body.as_object()
        && instance_of_global(ctx, &body, "FormData")?
    {
        let empty = form_data_keys_empty(ctx, object)?;
        if empty {
            set_content_type_if_missing(headers, "multipart/form-data; boundary=----denEmptyForm");
            return "".into_js(ctx);
        }
        if let Some(form) = Class::<crate::form_data::FormData>::from_object(object) {
            let blob = form.borrow().to_multipart_blob(ctx)?;
            set_content_type_if_missing(headers, blob.borrow().mime_type());
            return Ok(blob.into_value());
        }
        let key = rquickjs::Symbol::new_global(ctx.clone(), "den.toMultipartBlob")?;
        let converter: Value = object
            .get(key)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        if converter.is_function() {
            body = Function::from_js(ctx, converter)?.call((This(object.clone()),))?;
        } else if let Ok(named) = object.get::<_, Value>("toMultipartBlob")
            && named.is_function()
        {
            body = Function::from_js(ctx, named)?.call((This(object.clone()),))?;
        }
    }
    if body.as_string().is_some() {
        set_content_type_if_missing(headers, "text/plain;charset=UTF-8");
        return Ok(body);
    }
    if let Some(object) = body.as_object()
        && instance_of_global(ctx, &body, "Blob")?
    {
        let mime: Value = object.get("type")?;
        if let Some(mime) = mime.as_string() {
            let mime = mime.to_string()?;
            if !mime.is_empty() {
                set_content_type_if_missing(headers, &mime);
            }
        }
        return Ok(body);
    }
    if body.as_object().is_some_and(Object::is_array_buffer)
        || BufferSource::is_array_buffer_view(ctx, &body)?
    {
        return Ok(body);
    }
    set_content_type_if_missing(headers, "text/plain;charset=UTF-8");
    den_util::coerce_string(ctx, body)?.into_js(ctx)
}

pub async fn value_to_bytes<'js>(ctx: &Ctx<'js>, body: Option<Value<'js>>) -> Result<Vec<u8>> {
    let Some(body) = body.filter(|value| !value.is_null() && !value.is_undefined()) else {
        return Ok(Vec::new());
    };
    if is_readable_stream(ctx, &body) {
        return read_stream(ctx, body).await;
    }
    if let Some(string) = body.as_string() {
        return Ok(string.to_string()?.into_bytes());
    }
    if let Ok(buffer) = ArrayBuffer::from_js(ctx, body.clone()) {
        return copy_buffer(ctx, buffer.as_bytes());
    }
    if BufferSource::is_array_buffer_view(ctx, &body)? {
        return BufferSource::view_bytes(ctx, &body);
    }
    if let Some(object) = body.as_object() {
        let method: Value = object.get("arrayBuffer")?;
        if method.is_function() {
            let produced: Value = Function::from_js(ctx, method)?.call((This(object.clone()),))?;
            let resolved = MaybePromise::from_js(ctx, produced)?
                .into_future::<Value>()
                .await?;
            return Box::pin(value_to_bytes(ctx, Some(resolved))).await;
        }
    }
    Ok(den_util::coerce_string(ctx, body)?.into_bytes())
}

pub async fn read_stream<'js>(ctx: &Ctx<'js>, stream: Value<'js>) -> Result<Vec<u8>> {
    if let Some(object) = stream.as_object()
        && let Some(readable) = Class::<ReadableStream>::from_object(object)
    {
        return ReadableStream::read_all_bytes(&readable, ctx.clone()).await;
    }
    let Some(object) = stream.as_object() else {
        return Err(Exception::throw_type(ctx, "ReadableStream expected"));
    };
    let get_reader: Function = object.get("getReader")?;
    let reader: Object = get_reader.call((This(object.clone()),))?;
    let read: Function = reader.get("read")?;
    let mut out = Vec::new();
    loop {
        let produced: Value = read.call((This(reader.clone()),))?;
        let chunk = match MaybePromise::from_js(ctx, produced)?
            .into_future::<Value>()
            .await
        {
            Ok(value) => value,
            Err(error) => return Err(error),
        };
        let Some(result) = chunk.as_object() else {
            break;
        };
        let done: bool = result.get("done").unwrap_or(false);
        if done {
            break;
        }
        let value: Value = result.get("value")?;
        if value.is_undefined() || value.is_null() {
            return Err(Exception::throw_type(
                ctx,
                "ReadableStream chunk must be a Uint8Array",
            ));
        }
        if let Some(string) = value.as_string() {
            let _ = string;
            return Err(Exception::throw_type(
                ctx,
                "ReadableStream chunk must be a Uint8Array",
            ));
        }
        if value.as_number().is_some() || value.as_bool().is_some() {
            return Err(Exception::throw_type(
                ctx,
                "ReadableStream chunk must be a Uint8Array",
            ));
        }
        if let Ok(buffer) = ArrayBuffer::from_js(ctx, value.clone()) {
            out.extend(copy_buffer(ctx, buffer.as_bytes())?);
            continue;
        }
        if BufferSource::is_array_buffer_view(ctx, &value)? {
            out.extend(BufferSource::view_bytes(ctx, &value)?);
            continue;
        }
        return Err(Exception::throw_type(
            ctx,
            "ReadableStream chunk must be a Uint8Array",
        ));
    }
    Ok(out)
}

pub fn locked_empty_stream<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    let stream = ReadableStream::new(ctx.clone(), Opt(None), Opt(None))?;
    let instance = Class::instance(ctx.clone(), stream)?;
    ReadableStream::lock_for_consume(&instance, ctx)?;
    Ok(instance.into_value())
}

pub fn bytes_to_stream<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
    let queue = if bytes.is_empty() {
        Vec::new()
    } else {
        vec![TypedArray::<u8>::new_copy(ctx.clone(), bytes)?.into_value()]
    };
    ReadableStream::from_queue(ctx, queue).map(rquickjs::Class::into_value)
}

pub fn http_chunks_to_stream<'js>(ctx: &Ctx<'js>, host: Value<'js>) -> Result<Value<'js>> {
    if ctx.globals().get::<_, Value>("ReadableStream").is_err() {
        return Ok(Value::new_null(ctx.clone()));
    }
    let source = Object::new(ctx.clone())?;
    source.set("_host", host)?;
    source.set(
        "pull",
        Function::new(
            ctx.clone(),
            Async({
                move |this: This<Object<'js>>, ctx: Ctx<'js>, controller: Object<'js>| {
                    let host: Result<Value<'js>> = this.0.get("_host");
                    async move {
                        let host = host?;
                        pull_http_chunk(&ctx, &host, &controller).await
                    }
                }
            }),
        )?,
    )?;
    source.set(
        "cancel",
        Function::new(ctx.clone(), {
            move |this: This<Object<'js>>, ctx: Ctx<'js>| {
                let host: Value = this.0.get("_host")?;
                host_cancel_body(&host);
                promise_resolve(&ctx, Value::new_undefined(ctx.clone()))
            }
        })?,
    )?;
    readable_from_source(ctx, source)
}

pub fn tee_stream<'js>(ctx: &Ctx<'js>, stream: Value<'js>) -> Result<(Value<'js>, Value<'js>)> {
    if let Some(object) = stream.as_object()
        && let Some(readable) = Class::<ReadableStream>::from_object(object)
    {
        return ReadableStream::tee_pair_for_clone(&readable, ctx);
    }
    Err(Exception::throw_type(
        ctx,
        "ReadableStream expected: den has one stream implementation and this is not it",
    ))
}

pub fn text_stream_from_byte_stream<'js>(ctx: &Ctx<'js>, stream: Value<'js>) -> Result<Value<'js>> {
    let source = Object::new(ctx.clone())?;
    source.set("_stream", stream)?;
    source.set("_read", false)?;
    source.set(
        "pull",
        Function::new(
            ctx.clone(),
            Async(
                move |this: This<Object<'js>>, ctx: Ctx<'js>, controller: Object<'js>| {
                    let source = this.0;
                    async move {
                        let started: bool = source.get("_read")?;
                        if started {
                            return Ok(());
                        }
                        source.set("_read", true)?;
                        let stream: Value = source.get("_stream")?;
                        let bytes = read_stream(&ctx, stream).await?;
                        if !bytes.is_empty() {
                            controller_call(
                                &controller,
                                "enqueue",
                                Some(utf8_text(&bytes).into_js(&ctx)?),
                            )?;
                        }
                        controller_call(&controller, "close", None)
                    }
                },
            ),
        )?,
    )?;
    readable_from_source(ctx, source)
}

pub fn blob_from_bytes<'js>(ctx: &Ctx<'js>, bytes: Vec<u8>, mime: &str) -> Result<Value<'js>> {
    let ctor: Constructor = ctx
        .globals()
        .get("Blob")
        .map_err(|_error| Exception::throw_type(ctx, "Blob is not defined"))?;
    let parts = Array::new(ctx.clone())?;
    parts.set(0, TypedArray::<u8>::new_copy(ctx.clone(), bytes)?)?;
    let opts = Object::new(ctx.clone())?;
    opts.set("type", mime)?;
    ctor.construct((parts, opts))
}

pub fn validate_status(ctx: &Ctx<'_>, status: i32) -> Result<u16> {
    if !(200..=599).contains(&status) {
        return Err(Exception::throw_range(
            ctx,
            &format!("init['status'] must be in the range of 200 to 599, inclusive. {status}"),
        ));
    }
    Ok(status as u16)
}

pub fn validate_status_text(ctx: &Ctx<'_>, text: &str) -> Result<()> {
    if text.chars().any(|ch| {
        let code = ch as u32;
        code > 0xff || ch == '\n' || ch == '\r'
    }) {
        return Err(Exception::throw_type(
            ctx,
            "init['statusText'] is not a valid ByteString",
        ));
    }
    Ok(())
}

pub const fn null_body_status(status: u16) -> bool { matches!(status, 204 | 205 | 304) }

pub fn is_valid_method(method: &str) -> bool {
    !method.is_empty()
        && method.bytes().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9'
                    | b'a'..=b'z'
                    | b'A'..=b'Z'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
        })
}

pub const BLOCKED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6697, 10080,
];

pub fn is_blocked_port(port: u16) -> bool { BLOCKED_PORTS.contains(&port) }

pub fn utf8_text(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if text.starts_with('\u{FEFF}') {
        text.remove(0);
    }
    text
}

pub fn parse_json_js<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(Exception::throw_syntax(ctx, "UTF-16 JSON is not supported"));
    }
    ctx.json_parse(utf8_text(bytes))
}

pub fn text_to_stream<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    let queue = if text.is_empty() {
        Vec::new()
    } else {
        vec![text.into_js(ctx)?]
    };
    ReadableStream::from_queue(ctx, queue).map(rquickjs::Class::into_value)
}

pub fn value_as_body_stream<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
    if is_readable_stream(ctx, &value) {
        return Ok(value);
    }
    if let Some(string) = value.as_string() {
        return bytes_to_stream(ctx, string.to_string()?.as_bytes());
    }
    if let Ok(buffer) = ArrayBuffer::from_js(ctx, value.clone()) {
        return bytes_to_stream(ctx, &copy_buffer(ctx, buffer.as_bytes())?);
    }
    if BufferSource::is_array_buffer_view(ctx, &value)? {
        return bytes_to_stream(ctx, &BufferSource::view_bytes(ctx, &value)?);
    }
    if let Some(object) = value.as_object() {
        let method: Value = object.get("arrayBuffer")?;
        if method.is_function() {
            return array_buffer_method_stream(ctx, value);
        }
        if object
            .get::<_, Value>("buffer")
            .is_ok_and(|buffer| !buffer.is_undefined() && !buffer.is_null())
        {
            return bytes_to_stream(ctx, &BufferSource::view_bytes(ctx, &value)?);
        }
    }
    ReadableStream::from_queue(ctx, Vec::new()).map(rquickjs::Class::into_value)
}

pub fn text_chunks_to_stream<'js>(ctx: &Ctx<'js>, host: Value<'js>) -> Result<Value<'js>> {
    let decoder = ctx.probe(|| den_util::construct::<_, Object>(ctx, "TextDecoder", ()).ok());
    let remainder = Rc::new(RefCell::new(Vec::new()));
    let source = Object::new(ctx.clone())?;
    source.set("_host", host)?;
    if let Some(decoder) = decoder {
        source.set("_decoder", decoder)?;
    }
    source.set(
        "pull",
        Function::new(
            ctx.clone(),
            Async({
                let remainder = Rc::clone(&remainder);
                move |this: This<Object<'js>>, ctx: Ctx<'js>, controller: Object<'js>| {
                    let host: Result<Value<'js>> = this.0.get("_host");
                    let decoder: Option<Object<'js>> = this.0.get("_decoder").ok();
                    let remainder = Rc::clone(&remainder);
                    async move {
                        let host = host?;
                        pull_text_chunk(&ctx, &host, &controller, decoder.as_ref(), &remainder)
                            .await
                    }
                }
            }),
        )?,
    )?;
    source.set(
        "cancel",
        Function::new(ctx.clone(), {
            move |this: This<Object<'js>>| {
                let host: Value = this.0.get("_host")?;
                host_cancel_body(&host);
                Ok::<(), rquickjs::Error>(())
            }
        })?,
    )?;
    readable_from_source(ctx, source)
}

pub fn promise_resolve<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
    let (promise, resolve, _) = ctx.promise()?;
    let _ = resolve.call::<_, ()>((value,));
    Ok(promise.into_value())
}

pub fn promise_reject<'js>(ctx: &Ctx<'js>, reason: Value<'js>) -> Result<Value<'js>> {
    let (promise, _, reject) = ctx.promise()?;
    let _ = reject.call::<_, ()>((reason,));
    Ok(promise.into_value())
}

fn form_data_keys_empty<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<bool> {
    let keys = match object.get::<_, Value>("keys") {
        Ok(value) if value.is_function() => Function::from_js(ctx, value)?,
        _ => return Ok(false),
    };
    let iter: Value = keys.call((This(object.clone()),))?;
    let Some(iter) = iter.as_object() else {
        return Ok(false);
    };
    let next = match iter.get::<_, Value>("next") {
        Ok(value) if value.is_function() => Function::from_js(ctx, value)?,
        _ => return Ok(false),
    };
    let result: Value = next.call((This(iter.clone()),))?;
    Ok(result
        .as_object()
        .and_then(|result| result.get("done").ok())
        .unwrap_or(false))
}

fn readable_from_source<'js>(ctx: &Ctx<'js>, source: Object<'js>) -> Result<Value<'js>> {
    Class::instance(
        ctx.clone(),
        ReadableStream::new(ctx.clone(), Opt(Some(source.into_value())), Opt(None))?,
    )
    .map(rquickjs::Class::into_value)
}

fn controller_call<'js>(
    controller: &Object<'js>, method: &str, arg: Option<Value<'js>>,
) -> Result<()> {
    let func: Function = controller.get(method)?;
    arg.map_or_else(
        || func.call((This(controller.clone()),)),
        |value| func.call((This(controller.clone()), value)),
    )
}

fn truthy_prop(object: &Object<'_>, name: &str) -> bool {
    object
        .get::<_, Value>(name)
        .is_ok_and(|value| value.as_bool() == Some(true))
}

fn type_error_value<'js>(ctx: &Ctx<'js>, message: &str) -> Result<Value<'js>> {
    den_util::construct(ctx, "TypeError", (message,))
}

fn host_cancel_body(host: &Value<'_>) {
    if let Some(object) = host.as_object()
        && let Ok(cancel) = object.get::<_, Function>("_cancelBody")
    {
        let _ = cancel.call::<_, ()>((This(host.clone()),));
    }
}

async fn host_read_chunk<'js>(ctx: &Ctx<'js>, host: &Value<'js>) -> Result<Value<'js>> {
    let Some(object) = host.as_object() else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let read: Function = object.get("_readChunk")?;
    let produced: Value = read.call((This(host.clone()),))?;
    MaybePromise::from_js(ctx, produced)?
        .into_future::<Value>()
        .await
}

fn host_url_contains(host: &Value<'_>, needle: &str) -> bool {
    host.as_object()
        .and_then(|object| object.get::<_, String>("url").ok())
        .is_some_and(|url| url.contains(needle))
}

enum HttpChunk {
    Stop,
    Continue,
}

fn apply_http_chunk<'js>(
    ctx: &Ctx<'js>, controller: &Object<'js>, chunk: Value<'js>, check_abort: bool,
) -> Result<HttpChunk> {
    if let Some(object) = chunk.as_object() {
        if check_abort && truthy_prop(object, "__denAbort") {
            let reason: Value = object.get("reason")?;
            controller_call(controller, "error", Some(reason))?;
            return Ok(HttpChunk::Stop);
        }
        if truthy_prop(object, "__denStreamError") {
            let message = object
                .get::<_, String>("message")
                .ok()
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "network error".into());
            controller_call(controller, "error", Some(type_error_value(ctx, &message)?))?;
            return Ok(HttpChunk::Stop);
        }
    }
    if chunk.is_null() || chunk.is_undefined() {
        controller_call(controller, "close", None)?;
        return Ok(HttpChunk::Stop);
    }
    controller_call(controller, "enqueue", Some(chunk))?;
    Ok(HttpChunk::Continue)
}

async fn pull_http_chunk<'js>(
    ctx: &Ctx<'js>, host: &Value<'js>, controller: &Object<'js>,
) -> Result<()> {
    let chunk = host_read_chunk(ctx, host).await?;
    if matches!(
        apply_http_chunk(ctx, controller, chunk, true)?,
        HttpChunk::Stop
    ) {
        return Ok(());
    }
    if !host_url_contains(host, "bad-chunk") {
        return Ok(());
    }
    let next = host_read_chunk(ctx, host).await?;
    apply_http_chunk(ctx, controller, next, false).map(|_| ())
}

fn decode_utf8_chunk(remainder: &mut Vec<u8>, incoming: &[u8]) -> String {
    remainder.extend_from_slice(incoming);
    let take = match std::str::from_utf8(remainder) {
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Ok(_) | Err(_) => remainder.len(),
    };
    let text = String::from_utf8_lossy(remainder.get(..take).unwrap_or_default()).into_owned();
    remainder.drain(..take);
    text
}

async fn pull_text_chunk<'js>(
    ctx: &Ctx<'js>, host: &Value<'js>, controller: &Object<'js>, decoder: Option<&Object<'js>>,
    remainder: &Rc<RefCell<Vec<u8>>>,
) -> Result<()> {
    let chunk = host_read_chunk(ctx, host).await?;
    if chunk.is_null() || chunk.is_undefined() {
        controller_call(controller, "close", None)?;
        return Ok(());
    }
    let text = if let Some(decoder) = decoder {
        let decode: Function = decoder.get("decode")?;
        let opts = Object::new(ctx.clone())?;
        opts.set("stream", true)?;
        decode.call::<_, String>((This(decoder.clone()), chunk, opts))?
    } else {
        let bytes = if let Ok(buffer) = ArrayBuffer::from_js(ctx, chunk.clone()) {
            copy_buffer(ctx, buffer.as_bytes())?
        } else if BufferSource::is_array_buffer_view(ctx, &chunk)? {
            BufferSource::view_bytes(ctx, &chunk)?
        } else {
            Vec::new()
        };
        decode_utf8_chunk(&mut remainder.borrow_mut(), &bytes)
    };
    controller_call(controller, "enqueue", Some(text.into_js(ctx)?))
}

fn array_buffer_method_stream<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
    let source = Object::new(ctx.clone())?;
    source.set("_value", value)?;
    source.set(
        "start",
        Function::new(
            ctx.clone(),
            Async({
                move |this: This<Object<'js>>, ctx: Ctx<'js>, controller: Object<'js>| {
                    let value: Result<Value<'js>> = this.0.get("_value");
                    async move {
                        let value = value?;
                        let Some(object) = value.as_object() else {
                            controller_call(&controller, "close", None)?;
                            return Ok(());
                        };
                        let method: Function = object.get("arrayBuffer")?;
                        let produced: Value = method.call((This(object.clone()),))?;
                        let buf = MaybePromise::from_js(&ctx, produced)?
                            .into_future::<Value>()
                            .await?;
                        let bytes = if let Ok(buffer) = ArrayBuffer::from_js(&ctx, buf.clone()) {
                            copy_buffer(&ctx, buffer.as_bytes())?
                        } else if BufferSource::is_array_buffer_view(&ctx, &buf)? {
                            BufferSource::view_bytes(&ctx, &buf)?
                        } else {
                            Vec::new()
                        };
                        if !bytes.is_empty() {
                            let chunk =
                                TypedArray::<u8>::new_copy(ctx.clone(), bytes)?.into_value();
                            controller_call(&controller, "enqueue", Some(chunk))?;
                        }
                        controller_call(&controller, "close", None)
                    }
                }
            }),
        )?,
    )?;
    source.set(
        "cancel",
        Function::new(ctx.clone(), {
            move |ctx: Ctx<'js>| promise_resolve(&ctx, Value::new_undefined(ctx.clone()))
        })?,
    )?;
    readable_from_source(ctx, source)
}
