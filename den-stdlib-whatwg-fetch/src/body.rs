//! Shared Fetch body extract / consume. Stream errors stay JS exceptions.

use den_util::BufferSource;
use rquickjs::{
    Array, ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, IntoJs, Object, Result,
    TypedArray, Value,
    function::{Constructor, This},
    promise::MaybePromise,
};

use crate::headers::Headers;

pub(crate) fn optional_object<'js>(
    ctx: &Ctx<'js>, value: rquickjs::function::Opt<Value<'js>>,
) -> Result<Option<Object<'js>>> {
    match value.0 {
        None => Ok(None),
        Some(value) if value.is_undefined() || value.is_null() => Ok(None),
        Some(value) => Object::from_js(ctx, value).map(Some),
    }
}

pub(crate) fn is_readable_stream<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    is_instance_of_global(ctx, object, "ReadableStream")
}

pub(crate) fn stream_is_locked<'js>(value: &Value<'js>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get::<_, bool>("locked").unwrap_or(false)
}

pub(crate) fn is_instance_of_global<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, name: &str,
) -> Result<bool> {
    let Ok(ctor) = ctx.globals().get::<_, Value>(name) else {
        return Ok(false);
    };
    Ok(ctor.is_function() && object.is_instance_of(&ctor))
}

pub(crate) fn set_content_type_if_missing(
    headers: &Class<'_, Headers>, value: &str,
) -> Result<()> {
    let mut headers = headers.borrow_mut();
    if !headers.map.contains_key("content-type") {
        headers.map.insert("content-type".into(), value.to_string());
    }
    Ok(())
}

pub(crate) fn copy_buffer(ctx: &Ctx<'_>, bytes: Option<&[u8]>) -> Result<Vec<u8>> {
    bytes
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Exception::throw_type(ctx, "buffer is detached"))
}

pub(crate) fn copy_view<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
    BufferSource::view_bytes(ctx, value)
}

pub(crate) fn apply_body_types<'js>(
    ctx: &Ctx<'js>, headers: &Class<'js, Headers>, mut body: Value<'js>,
) -> Result<Value<'js>> {
    if let Some(object) = body.as_object()
        && is_instance_of_global(ctx, object, "URLSearchParams")?
    {
        // ToString(URLSearchParams) is the urlencoded serialization.
        let text = den_util::coerce_string(ctx, object.clone().into_value())?;
        body = text.into_js(ctx)?;
        set_content_type_if_missing(
            headers,
            "application/x-www-form-urlencoded;charset=UTF-8",
        )?;
    }
    if let Some(object) = body.as_object()
        && is_instance_of_global(ctx, object, "FormData")?
    {
        ctx.globals().set("__denFd", object.clone())?;
        let empty: bool = ctx.eval(
            r#"(function () {
              var object = globalThis.__denFd;
              delete globalThis.__denFd;
              if (!object || typeof object.keys !== "function") {
                return false;
              }
              var iter = object.keys();
              return !!(iter && typeof iter.next === "function" && iter.next().done);
            })()"#,
        )?;
        if empty {
            set_content_type_if_missing(
                headers,
                "multipart/form-data; boundary=----denEmptyForm",
            )?;
            return "".into_js(ctx);
        }
        if let Some(form) =
            Class::<den_stdlib_whatwg::form_data::FormData>::from_object(object)
        {
            let blob = form.borrow().to_multipart_blob(ctx)?;
            set_content_type_if_missing(headers, blob.borrow().mime_type())?;
            return Ok(blob.into_value());
        }
        let key = rquickjs::Symbol::new_global(ctx.clone(), "den.toMultipartBlob")?;
        let converter: Value = object.get(key).unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
        if converter.is_function() {
            body = Function::from_js(ctx, converter)?.call((This(object.clone()),))?;
        } else if let Ok(named) = object.get::<_, Value>("toMultipartBlob")
            && named.is_function()
        {
            body = Function::from_js(ctx, named)?.call((This(object.clone()),))?;
        }
    }
    if body.as_string().is_some() {
        set_content_type_if_missing(headers, "text/plain;charset=UTF-8")?;
    } else if let Some(object) = body.as_object()
        && is_instance_of_global(ctx, object, "Blob")?
    {
        let mime: Value = object.get("type")?;
        if let Some(mime) = mime.as_string() {
            let mime = mime.to_string()?;
            if !mime.is_empty() {
                set_content_type_if_missing(headers, &mime)?;
            }
        }
    }
    Ok(body)
}

pub(crate) async fn value_to_bytes<'js>(
    ctx: &Ctx<'js>, body: Option<Value<'js>>,
) -> Result<Vec<u8>> {
    let Some(body) = body.filter(|value| !value.is_null() && !value.is_undefined()) else {
        return Ok(Vec::new());
    };
    if is_readable_stream(ctx, &body)? {
        return read_stream(ctx, body).await;
    }
    if let Some(string) = body.as_string() {
        return Ok(string.to_string()?.into_bytes());
    }
    if let Ok(buffer) = ArrayBuffer::from_js(ctx, body.clone()) {
        return copy_buffer(ctx, buffer.as_bytes());
    }
    if BufferSource::is_array_buffer_view(ctx, &body)? {
        return copy_view(ctx, &body);
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
    Ok(Coerced::<String>::from_js(ctx, body)?.0.into_bytes())
}

pub(crate) async fn read_stream<'js>(ctx: &Ctx<'js>, stream: Value<'js>) -> Result<Vec<u8>> {
    if let Some(object) = stream.as_object()
        && let Some(readable) = Class::<den_stdlib_whatwg::streams::ReadableStream>::from_object(object)
    {
        return den_stdlib_whatwg::streams::ReadableStream::read_all_bytes(
            &readable,
            ctx.clone(),
        )
        .await;
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
        let chunk = match MaybePromise::from_js(ctx, produced)?.into_future::<Value>().await {
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
            out.extend(copy_view(ctx, &value)?);
            continue;
        }
        return Err(Exception::throw_type(
            ctx,
            "ReadableStream chunk must be a Uint8Array",
        ));
    }
    Ok(out)
}

pub(crate) fn locked_empty_stream<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    ctx.eval(
        r#"
          (function () {
            var stream = new ReadableStream();
            try {
              stream.getReader();
            } catch (error) {}
            return stream;
          })()
        "#,
    )
}

pub(crate) fn bytes_to_stream<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
    let queue = if bytes.is_empty() {
        Vec::new()
    } else {
        vec![TypedArray::<u8>::new_copy(ctx.clone(), bytes)?.into_value()]
    };
    den_stdlib_whatwg::streams::ReadableStream::from_queue(ctx, queue)
        .map(|stream| stream.into_value())
}

pub(crate) fn http_chunks_to_stream<'js>(
    ctx: &Ctx<'js>, host: Value<'js>,
) -> Result<Value<'js>> {
    if ctx.globals().get::<_, Value>("ReadableStream").is_err() {
        return Ok(Value::new_null(ctx.clone()));
    }
    ctx.globals().set("__denStreamHost", host)?;
    ctx.eval(
        r#"
          (function () {
            var host = globalThis.__denStreamHost;
            delete globalThis.__denStreamHost;
            var source = Object.create(null);
            var ctrl;
            source.start = function (controller) { ctrl = controller; };
            source.pull = function (controller) {
              ctrl = controller;
              return host._readChunk().then(function (chunk) {
                if (chunk && chunk.__denAbort) {
                  controller.error(chunk.reason);
                  return;
                }
                if (chunk && chunk.__denStreamError) {
                  controller.error(new TypeError(chunk.message || "network error"));
                  return;
                }
                if (chunk == null) {
                  controller.close();
                  return;
                }
                controller.enqueue(chunk);
                if (String(host.url || "").indexOf("bad-chunk") === -1) {
                  return;
                }
                return host._readChunk().then(function (next) {
                  if (next && next.__denStreamError) {
                    controller.error(new TypeError(next.message || "network error"));
                  } else if (next == null) {
                    controller.close();
                  } else {
                    controller.enqueue(next);
                  }
                });
              });
            };
            source.cancel = function () {
              host._cancelBody();
              return Promise.resolve();
            };
            var stream = new ReadableStream(source);
            stream._denAbort = function (reason) {
              if (!ctrl) {
                return;
              }
              try { ctrl.error(reason); } catch (error) {}
            };
            return stream;
          })()
        "#,
    )
}

pub(crate) fn tee_stream<'js>(ctx: &Ctx<'js>, stream: Value<'js>) -> Result<(Value<'js>, Value<'js>)> {
    if let Some(object) = stream.as_object()
        && let Some(readable) = Class::<den_stdlib_whatwg::streams::ReadableStream>::from_object(object)
    {
        return den_stdlib_whatwg::streams::ReadableStream::tee_pair(&readable, ctx);
    }
    if let Some(object) = stream.as_object()
        && let Ok(tee) = object.get::<_, Function>("tee")
    {
        let result: Value = tee.call((This(object.clone()),))?;
        if let Some(pair) = result.as_object() {
            return Ok((pair.get(0)?, pair.get(1)?));
        }
    }
    ctx.globals().set("__denTeeSrc", stream)?;
    let pair: Array = ctx.eval(
        r#"
          (function () {
            var stream = globalThis.__denTeeSrc;
            delete globalThis.__denTeeSrc;
            var reader = stream.getReader();
            var left = [];
            var right = [];
            var closed = false;
            var failed = null;
            var waiters = [];
            function wake() {
              for (var i = 0; i < waiters.length; i++) waiters[i]();
              waiters = [];
            }
            function pump() {
              return reader.read().then(function (result) {
                if (result.done) {
                  closed = true;
                  wake();
                  return;
                }
                left.push(result.value);
                right.push(result.value);
                wake();
                return pump();
              }, function (error) {
                failed = error;
                wake();
              });
            }
            pump();
            function branch(queue) {
              var source = Object.create(null);
              source.pull = function (controller) {
                function take() {
                  if (failed) {
                    return Promise.reject(failed);
                  }
                  if (queue.length) {
                    controller.enqueue(queue.shift());
                    return Promise.resolve();
                  }
                  if (closed) {
                    controller.close();
                    return Promise.resolve();
                  }
                  return new Promise(function (resolve) {
                    waiters.push(resolve);
                  }).then(take);
                }
                return take();
              };
              return new ReadableStream(source);
            }
            return [branch(left), branch(right)];
          })()
        "#,
    )?;
    Ok((pair.get(0)?, pair.get(1)?))
}

pub(crate) fn blob_from_bytes<'js>(
    ctx: &Ctx<'js>, bytes: Vec<u8>, mime: &str,
) -> Result<Value<'js>> {
    let ctor: Constructor = ctx
        .globals()
        .get("Blob")
        .map_err(|_| Exception::throw_type(ctx, "Blob is not defined"))?;
    let parts = Array::new(ctx.clone())?;
    parts.set(0, TypedArray::<u8>::new_copy(ctx.clone(), bytes)?)?;
    let opts = Object::new(ctx.clone())?;
    opts.set("type", mime)?;
    ctor.construct((parts, opts))
}

pub(crate) fn validate_status(ctx: &Ctx<'_>, status: i32) -> Result<u16> {
    if !(200..=599).contains(&status) {
        return Err(Exception::throw_range(
            ctx,
            &format!("init['status'] must be in the range of 200 to 599, inclusive. {status}"),
        ));
    }
    Ok(status as u16)
}

pub(crate) fn validate_status_text(ctx: &Ctx<'_>, text: &str) -> Result<()> {
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

pub(crate) fn null_body_status(status: u16) -> bool {
    matches!(status, 204 | 205 | 304)
}

pub(crate) fn is_valid_method(method: &str) -> bool {
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

pub(crate) const BLOCKED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6697, 10080,
];

pub(crate) fn is_blocked_port(port: u16) -> bool { BLOCKED_PORTS.contains(&port) }

pub(crate) fn utf8_text(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if text.starts_with('\u{FEFF}') {
        text.remove(0);
    }
    text
}

pub(crate) fn parse_json_js<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(Exception::throw_syntax(ctx, "UTF-16 JSON is not supported"));
    }
    let text = utf8_text(bytes);
    let json: Object = ctx.globals().get("JSON")?;
    let parse: Function = json.get("parse")?;
    parse.call((text,))
}

pub(crate) fn text_to_stream<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    ctx.globals().set("__denStreamText", text)?;
    ctx.eval(
        r#"
          (function () {
            var text = globalThis.__denStreamText;
            delete globalThis.__denStreamText;
            var source = Object.create(null);
            source.start = function (controller) {
              if (text) {
                controller.enqueue(text);
              }
              controller.close();
            };
            return new ReadableStream(source);
          })()
        "#,
    )
}

pub(crate) fn value_as_body_stream<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
    if is_readable_stream(ctx, &value)? {
        return Ok(value);
    }
    ctx.globals().set("__denBodyVal", value)?;
    ctx.eval(
        r#"
          (function () {
            var value = globalThis.__denBodyVal;
            delete globalThis.__denBodyVal;
            var source = Object.create(null);
            source.start = function (controller) {
              function pushBytes(bytes) {
                if (bytes && bytes.byteLength) {
                  controller.enqueue(bytes);
                }
                controller.close();
              }
              if (typeof value === "string") {
                pushBytes(new TextEncoder().encode(value));
                return;
              }
              if (value && typeof value.arrayBuffer === "function") {
                return value.arrayBuffer().then(function (buf) {
                  pushBytes(new Uint8Array(buf));
                });
              }
              if (value && value.buffer) {
                pushBytes(new Uint8Array(value.buffer, value.byteOffset || 0, value.byteLength));
                return;
              }
              controller.close();
            };
            source.cancel = function () {
              return Promise.resolve();
            };
            return new ReadableStream(source);
          })()
        "#,
    )
}
