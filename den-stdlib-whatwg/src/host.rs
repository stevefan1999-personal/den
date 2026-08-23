//! Shared host helpers: exceptions, buffer sources, events, prototype wiring.

use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, Object, Result, Symbol,
    TypedArray, Value, class::JsClass, function::Constructor,
};

use crate::blob::{Blob, File};
use crate::form_data::FormData;

pub struct Host;

impl Host {
    pub fn throw_type(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
        Exception::throw_type(ctx, message)
    }

    pub fn throw_message(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
        Exception::throw_message(ctx, message)
    }

    pub fn throw_range(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
        Exception::throw_range(ctx, message)
    }

    pub fn throw_syntax(ctx: &Ctx<'_>, message: &str) -> rquickjs::Error {
        Exception::throw_syntax(ctx, message)
    }

    pub fn throw_dom(ctx: &Ctx<'_>, message: &str, name: &str) -> rquickjs::Error {
        if let Ok(ctor) = ctx.globals().get::<_, Constructor>("DOMException") {
            if let Ok(exc) = ctor.construct::<_, Value>((message, name)) {
                return ctx.throw(exc);
            }
        }
        Exception::throw_message(ctx, message)
    }

    pub fn coerce_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
        Ok(Coerced::<String>::from_js(ctx, value)?.0)
    }

    pub fn buffer_source_bytes<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Option<Vec<u8>>> {
        if let Ok(view) = TypedArray::<u8>::from_js(ctx, value.clone()) {
            return Ok(view.as_bytes().map(|bytes| bytes.to_vec()));
        }
        if let Ok(buffer) = ArrayBuffer::from_js(ctx, value.clone()) {
            return Ok(buffer.as_bytes().map(|bytes| bytes.to_vec()));
        }
        if let Some(obj) = value.as_object() {
            if obj.contains_key("byteLength")? && obj.contains_key("buffer")? {
                if let (Ok(buffer), Ok(offset), Ok(len)) = (
                    obj.get::<_, ArrayBuffer>("buffer"),
                    obj.get::<_, usize>("byteOffset"),
                    obj.get::<_, usize>("byteLength"),
                ) {
                    if let Some(bytes) = buffer.as_bytes() {
                        let end = offset.saturating_add(len).min(bytes.len());
                        let start = offset.min(end);
                        return Ok(Some(bytes[start..end].to_vec()));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn blob_like_bytes<'js>(_ctx: &Ctx<'js>, value: &Value<'js>) -> Option<Vec<u8>> {
        if let Ok(blob) = Class::<Blob>::from_value(value) {
            return Some(blob.borrow().bytes().to_vec());
        }
        if let Ok(file) = Class::<File>::from_value(value) {
            return Some(file.borrow().bytes().to_vec());
        }
        None
    }

    pub fn blob_like_type<'js>(value: &Value<'js>) -> String {
        if let Ok(blob) = Class::<Blob>::from_value(value) {
            return blob.borrow().mime_type().to_string();
        }
        if let Ok(file) = Class::<File>::from_value(value) {
            return file.borrow().mime_type().to_string();
        }
        String::new()
    }

    pub fn is_blob_like<'js>(value: &Value<'js>) -> bool {
        Class::<Blob>::from_value(value).is_ok() || Class::<File>::from_value(value).is_ok()
    }

    pub fn is_file_like<'js>(value: &Value<'js>) -> bool {
        Class::<File>::from_value(value).is_ok()
    }

    pub fn file_name<'js>(value: &Value<'js>) -> Option<String> {
        Class::<File>::from_value(value)
            .ok()
            .map(|file| file.borrow().file_name().to_string())
    }

    pub fn ascii_type(value: &str) -> String {
        if value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
            value.to_string()
        } else {
            String::new()
        }
    }

    pub fn encode_base64(bytes: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(b2 & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    pub fn construct<'js, A, R>(ctx: &Ctx<'js>, name: &str, args: A) -> Result<R>
    where
        A: rquickjs::function::IntoArgs<'js>,
        R: FromJs<'js>,
    {
        let ctor: Constructor<'js> = ctx.globals().get(name)?;
        ctor.construct(args)
    }

    pub fn event<'js>(ctx: &Ctx<'js>, type_: &str) -> Result<Value<'js>> {
        match ctx.globals().get::<_, Constructor<'js>>("Event") {
            Ok(ctor) => ctor.construct((type_,)),
            Err(_) => {
                let object = Object::new(ctx.clone())?;
                object.set("type", type_)?;
                Ok(object.into_value())
            }
        }
    }

    pub fn message_event<'js>(
        ctx: &Ctx<'js>,
        type_: &str,
        data: Value<'js>,
        origin: &str,
        last_event_id: &str,
    ) -> Result<Value<'js>> {
        match ctx.globals().get::<_, Constructor<'js>>("MessageEvent") {
            Ok(ctor) => {
                let opts = Object::new(ctx.clone())?;
                opts.set("data", data)?;
                opts.set("origin", origin)?;
                opts.set("lastEventId", last_event_id)?;
                ctor.construct((type_, opts))
            }
            Err(_) => {
                let object = Object::new(ctx.clone())?;
                object.set("type", type_)?;
                object.set("data", data)?;
                object.set("origin", origin)?;
                object.set("lastEventId", last_event_id)?;
                Ok(object.into_value())
            }
        }
    }

    pub fn error_event<'js>(ctx: &Ctx<'js>, message: &str) -> Result<Value<'js>> {
        match ctx.globals().get::<_, Constructor<'js>>("ErrorEvent") {
            Ok(ctor) => {
                let opts = Object::new(ctx.clone())?;
                opts.set("message", message)?;
                ctor.construct(("error", opts))
            }
            Err(_) => {
                let object = Object::new(ctx.clone())?;
                object.set("type", "error")?;
                object.set("message", message)?;
                Ok(object.into_value())
            }
        }
    }

    pub fn progress_event<'js>(
        ctx: &Ctx<'js>,
        type_: &str,
        length_computable: bool,
        loaded: f64,
        total: f64,
    ) -> Result<Value<'js>> {
        let opts = Object::new(ctx.clone())?;
        opts.set("lengthComputable", length_computable)?;
        opts.set("loaded", loaded)?;
        opts.set("total", total)?;
        match ctx.globals().get::<_, Constructor<'js>>("ProgressEvent") {
            Ok(ctor) => ctor.construct((type_, opts)),
            Err(_) => {
                let event = Self::event(ctx, type_)?;
                if let Some(object) = event.as_object() {
                    object.set("lengthComputable", length_computable)?;
                    object.set("loaded", loaded)?;
                    object.set("total", total)?;
                }
                Ok(event)
            }
        }
    }

    pub fn close_event<'js>(
        ctx: &Ctx<'js>,
        code: u16,
        reason: &str,
        was_clean: bool,
    ) -> Result<Value<'js>> {
        let opts = Object::new(ctx.clone())?;
        opts.set("code", code)?;
        opts.set("reason", reason)?;
        opts.set("wasClean", was_clean)?;
        match ctx.globals().get::<_, Constructor<'js>>("CloseEvent") {
            Ok(ctor) => ctor.construct(("close", opts)),
            Err(_) => {
                let event = Self::event(ctx, "close")?;
                if let Some(object) = event.as_object() {
                    object.set("code", code)?;
                    object.set("reason", reason)?;
                    object.set("wasClean", was_clean)?;
                }
                Ok(event)
            }
        }
    }

    pub fn set_super_class<'js, Sub, Super>(ctx: &Ctx<'js>) -> Result<()>
    where
        Sub: JsClass<'js>,
        Super: JsClass<'js>,
    {
        if let (Some(sub), Some(super_proto)) = (
            Class::<Sub>::prototype(ctx)?,
            Class::<Super>::prototype(ctx)?,
        ) {
            sub.set_prototype(Some(&super_proto))?;
        }
        Ok(())
    }

    pub fn set_event_target_proto<'js, C: JsClass<'js>>(ctx: &Ctx<'js>, name: &str) -> Result<()> {
        let Some(sub) = Class::<C>::prototype(ctx)? else {
            return Ok(());
        };
        let Ok(ctor) = ctx.globals().get::<_, Object>(name) else {
            return Ok(());
        };
        let Ok(proto) = ctor.get::<_, Object>("prototype") else {
            return Ok(());
        };
        sub.set_prototype(Some(&proto))?;
        Ok(())
    }

    pub fn install_formdata_symbol<'js>(ctx: &Ctx<'js>) -> Result<()> {
        let Some(proto) = Class::<FormData>::prototype(ctx)? else {
            return Ok(());
        };
        let key = Symbol::new_global(ctx.clone(), "den.toMultipartBlob")?;
        let method: Function = proto.get("toMultipartBlob")?;
        proto.set(key, method)?;
        Ok(())
    }

    pub fn report_listener_error(ctx: &Ctx<'_>, error: rquickjs::Error) {
        match error {
            rquickjs::Error::Exception => {
                let value = ctx.catch();
                if let Ok(report) = ctx.globals().get::<_, Function>("reportError") {
                    let _ = report.call::<_, ()>((value,));
                }
            }
            _ => {}
        }
    }

    pub fn json_parse<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
        let json: Object = ctx.globals().get("JSON")?;
        let parse: Function = json.get("parse")?;
        parse.call((text,))
    }

    pub async fn maybe_await<'js>(value: Value<'js>) -> Result<Value<'js>> {
        if value.is_promise() {
            value.into_promise().unwrap().into_future().await
        } else {
            Ok(value)
        }
    }
}
