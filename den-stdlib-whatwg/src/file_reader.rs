//! WHATWG FileReader.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use den_util::coerce_string;
use rquickjs::{
    ArrayBuffer, Class, Ctx, Function, JsLifetime, Object, Promise, Result, TypedArray, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::{
    event_target::{HostEventTarget, SharedEvents},
    host::Host,
};

const EMPTY: i32 = 0;
const LOADING: i32 = 1;
const DONE: i32 = 2;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct FileReader<'js> {
    events: SharedEvents<'js>,
    ready_state: i32,
    result: Value<'js>,
    error: Value<'js>,
    #[qjs(skip_trace)]
    aborted: Rc<Cell<bool>>,
}

impl<'js> Trace<'js> for FileReader<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.result.trace(tracer);
        self.error.trace(tracer);
        if let Ok(events) = self.events.try_borrow() {
            events.trace(tracer);
        }
    }
}

impl<'js> FileReader<'js> {
    fn dispatch(this: &Class<'js, Self>, ctx: &Ctx<'js>, event: Value<'js>) -> Result<()> {
        let events = this.borrow().events.clone();
        HostEventTarget::dispatch_shared(&events, ctx, this.as_inner(), event)?;
        Ok(())
    }

    fn is_loading(this: &Class<'js, Self>, aborted: &Rc<Cell<bool>>) -> bool {
        !aborted.get() && this.borrow().ready_state == LOADING
    }

    fn fire_progress(
        this: &Class<'js, Self>, ctx: &Ctx<'js>, type_: &str, loaded: f64, total: f64,
    ) -> Result<()> {
        let event = Host::progress_event(ctx, type_, true, loaded, total)?;
        Self::dispatch(this, ctx, event)
    }

    fn handler(this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str) -> Value<'js> {
        this.0.borrow().events.borrow().handler_or_null(&ctx, type_)
    }

    fn set_handler(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: &'static str, value: Value<'js>,
    ) -> Result<()> {
        this.0.borrow().events.borrow_mut().set_handler(
            &ctx,
            this.0.as_inner().clone(),
            type_,
            value,
        )
    }

    /// Web IDL `const` members also live on the prototype so `reader.EMPTY`
    /// works. rquickjs rejects a second getter with the same rename.
    pub fn install_idl_constants(ctx: &Ctx<'js>) -> Result<()> {
        let Some(proto) = Class::<Self>::prototype(ctx)? else {
            return Ok(());
        };
        proto.set("EMPTY", EMPTY)?;
        proto.set("LOADING", LOADING)?;
        proto.set("DONE", DONE)?;
        Host::install_fileapi_document(ctx)?;
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> FileReader<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Self {
        Self {
            events: HostEventTarget::share(),
            ready_state: EMPTY,
            result: Value::new_null(ctx.clone()),
            error: Value::new_null(ctx),
            aborted: Rc::new(Cell::new(false)),
        }
    }

    #[qjs(static, get, rename = "EMPTY")]
    pub fn empty_const() -> i32 {
        EMPTY
    }

    #[qjs(static, get, rename = "LOADING")]
    pub fn loading_const() -> i32 {
        LOADING
    }

    #[qjs(static, get, rename = "DONE")]
    pub fn done_const() -> i32 {
        DONE
    }

    #[qjs(get)]
    pub fn ready_state(&self) -> i32 {
        self.ready_state
    }

    #[qjs(get)]
    pub fn result(&self) -> Value<'js> {
        self.result.clone()
    }

    #[qjs(get)]
    pub fn error(&self) -> Value<'js> {
        self.error.clone()
    }

    pub fn add_event_listener(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, type_: String, callback: Value<'js>,
        options: Opt<Value<'js>>,
    ) -> Result<()> {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .add(&ctx, type_, callback, options.0)
    }

    pub fn remove_event_listener(
        this: This<Class<'js, Self>>, type_: String, callback: Value<'js>, options: Opt<Value<'js>>,
    ) {
        this.0
            .borrow()
            .events
            .borrow_mut()
            .remove(&type_, &callback, options.0);
    }

    pub fn dispatch_event(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, event: Value<'js>,
    ) -> Result<bool> {
        HostEventTarget::dispatch_shared(&this.0.borrow().events, &ctx, this.0.as_inner(), event)
    }

    pub fn abort(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<()> {
        let ready = this.0.borrow().ready_state;
        if ready == EMPTY || ready == DONE {
            this.0.borrow_mut().result = Value::new_null(ctx.clone());
            return Ok(());
        }
        if ready == LOADING {
            let mut reader = this.0.borrow_mut();
            reader.ready_state = DONE;
            reader.result = Value::new_null(ctx.clone());
            reader.aborted.set(true);
        }
        Self::dispatch(
            &this.0,
            &ctx,
            Host::progress_event(&ctx, "abort", false, 0.0, 0.0)?,
        )?;
        if this.0.borrow().ready_state != LOADING {
            Self::dispatch(
                &this.0,
                &ctx,
                Host::progress_event(&ctx, "loadend", false, 0.0, 0.0)?,
            )?;
        }
        Ok(())
    }

    pub fn read_as_array_buffer(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, blob: Value<'js>,
    ) -> Result<()> {
        Self::read(this, ctx, blob, ReadKind::ArrayBuffer, None)
    }

    #[qjs(rename = "readAsDataURL")]
    pub fn read_as_data_url(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, blob: Value<'js>,
    ) -> Result<()> {
        Self::read(this, ctx, blob, ReadKind::DataUrl, None)
    }

    pub fn read_as_text(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, blob: Value<'js>, encoding: Opt<Value<'js>>,
    ) -> Result<()> {
        let encoding = match encoding.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => Some(coerce_string(&ctx, value)?),
        };
        Self::read(this, ctx, blob, ReadKind::Text, encoding)
    }

    pub fn read_as_binary_string(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, blob: Value<'js>,
    ) -> Result<()> {
        Self::read(this, ctx, blob, ReadKind::BinaryString, None)
    }

    #[qjs(get, rename = "onload")]
    pub fn onload(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "load")
    }

    #[qjs(set, rename = "onload")]
    pub fn set_onload(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "load", value)
    }

    #[qjs(get, rename = "onerror")]
    pub fn onerror(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "error")
    }

    #[qjs(set, rename = "onerror")]
    pub fn set_onerror(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "error", value)
    }

    #[qjs(get, rename = "onloadend")]
    pub fn onloadend(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "loadend")
    }

    #[qjs(set, rename = "onloadend")]
    pub fn set_onloadend(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "loadend", value)
    }

    #[qjs(get, rename = "onloadstart")]
    pub fn onloadstart(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "loadstart")
    }

    #[qjs(set, rename = "onloadstart")]
    pub fn set_onloadstart(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "loadstart", value)
    }

    #[qjs(get, rename = "onprogress")]
    pub fn onprogress(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "progress")
    }

    #[qjs(set, rename = "onprogress")]
    pub fn set_onprogress(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "progress", value)
    }

    #[qjs(get, rename = "onabort")]
    pub fn onabort(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Value<'js> {
        Self::handler(this, ctx, "abort")
    }

    #[qjs(set, rename = "onabort")]
    pub fn set_onabort(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, value: Value<'js>,
    ) -> Result<()> {
        Self::set_handler(this, ctx, "abort", value)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "FileReader"
    }
}

#[derive(Clone, Copy)]
enum ReadKind {
    ArrayBuffer,
    BinaryString,
    DataUrl,
    Text,
}

impl<'js> FileReader<'js> {
    fn read(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, blob: Value<'js>, kind: ReadKind,
        encoding: Option<String>,
    ) -> Result<()> {
        if this.0.borrow().ready_state == LOADING {
            return Err(Host::throw_dom(
                &ctx,
                "Invalid FileReader state",
                "InvalidStateError",
            ));
        }
        if blob.is_null() || blob.is_undefined() {
            return Err(Host::throw_type(&ctx, "FileReader: argument is not a Blob"));
        }
        {
            let mut reader = this.0.borrow_mut();
            reader.ready_state = LOADING;
            reader.result = Value::new_null(ctx.clone());
            reader.error = Value::new_null(ctx.clone());
            reader.aborted = Rc::new(Cell::new(false));
        }
        let aborted = Rc::clone(&this.0.borrow().aborted);
        let this = this.0.clone();
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                if !FileReader::is_loading(&this, &aborted) {
                    return;
                }
                if let Ok(event) = Host::progress_event(&ctx, "loadstart", false, 0.0, 0.0) {
                    if FileReader::is_loading(&this, &aborted) {
                        let _ = FileReader::dispatch(&this, &ctx, event);
                    }
                }
                if !FileReader::is_loading(&this, &aborted) {
                    return;
                }
                FileReader::later(&ctx, move |ctx| {
                    let ctx_run = ctx.clone();
                    ctx_run.spawn(async move {
                        FileReader::finish_read(this, ctx, blob, kind, encoding, aborted).await;
                    });
                });
            }
        });
        Ok(())
    }

    async fn finish_read(
        this: Class<'js, Self>, ctx: Ctx<'js>, blob: Value<'js>, kind: ReadKind,
        encoding: Option<String>, aborted: Rc<Cell<bool>>,
    ) {
        if !FileReader::is_loading(&this, &aborted) {
            return;
        }
        let mime = Host::blob_like_type(&blob);
        let bytes = match FileReader::take_bytes(ctx.clone(), blob).await {
            Ok(bytes) => bytes,
            Err(error) => {
                if !FileReader::is_loading(&this, &aborted) {
                    return;
                }
                let reason = match error {
                    rquickjs::Error::Exception => ctx.catch(),
                    _ => Value::new_undefined(ctx.clone()),
                };
                {
                    let mut reader = this.borrow_mut();
                    reader.ready_state = DONE;
                    reader.error = reason;
                }
                let _ = FileReader::dispatch(
                    &this,
                    &ctx,
                    Host::progress_event(&ctx, "error", false, 0.0, 0.0)
                        .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                );
                if this.borrow().ready_state != LOADING {
                    let _ = FileReader::dispatch(
                        &this,
                        &ctx,
                        Host::progress_event(&ctx, "loadend", false, 0.0, 0.0)
                            .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
                    );
                }
                return;
            }
        };
        if !FileReader::is_loading(&this, &aborted) {
            return;
        }
        let size = bytes.len() as f64;
        if !bytes.is_empty() {
            let _ = FileReader::fire_progress(&this, &ctx, "progress", size, size);
            if !FileReader::is_loading(&this, &aborted) {
                return;
            }
        }
        FileReader::later(&ctx, move |ctx| {
            let ctx_run = ctx.clone();
            ctx_run.spawn(async move {
                FileReader::finish_load(this, ctx, bytes, kind, encoding, mime, aborted).await;
            });
        });
    }

    async fn finish_load(
        this: Class<'js, Self>, ctx: Ctx<'js>, bytes: Vec<u8>, kind: ReadKind,
        encoding: Option<String>, mime: String, aborted: Rc<Cell<bool>>,
    ) {
        if !FileReader::is_loading(&this, &aborted) {
            return;
        }
        let size = bytes.len() as f64;
        let result = match kind {
            ReadKind::ArrayBuffer => ArrayBuffer::new_copy(ctx.clone(), &bytes)
                .map(|buffer| buffer.into_value())
                .unwrap_or_else(|_| Value::new_null(ctx.clone())),
            ReadKind::Text => {
                let label = FileReader::resolve_encoding(encoding.as_deref(), &mime);
                let text = FileReader::decode(&ctx, &bytes, &label);
                rquickjs::IntoJs::into_js(text, &ctx)
                    .unwrap_or_else(|_| Value::new_null(ctx.clone()))
            }
            ReadKind::BinaryString => {
                let text: String = bytes.iter().map(|&byte| byte as char).collect();
                rquickjs::IntoJs::into_js(text, &ctx)
                    .unwrap_or_else(|_| Value::new_null(ctx.clone()))
            }
            ReadKind::DataUrl => {
                let media = if mime.is_empty() {
                    "application/octet-stream"
                } else {
                    mime.as_str()
                };
                let url = format!("data:{media};base64,{}", den_util::base64_encode(&bytes));
                rquickjs::IntoJs::into_js(url, &ctx)
                    .unwrap_or_else(|_| Value::new_null(ctx.clone()))
            }
        };
        if !FileReader::is_loading(&this, &aborted) {
            return;
        }
        {
            let mut reader = this.borrow_mut();
            reader.ready_state = DONE;
            reader.result = result;
        }
        let _ = FileReader::fire_progress(&this, &ctx, "load", size, size);
        FileReader::later(&ctx, move |ctx| {
            if this.borrow().ready_state != LOADING {
                let _ = FileReader::fire_progress(&this, &ctx, "loadend", size, size);
            }
        });
    }

    async fn take_bytes(ctx: Ctx<'js>, blob: Value<'js>) -> Result<Vec<u8>> {
        if let Some(bytes) = Host::blob_like_bytes(&ctx, &blob) {
            return Ok(bytes);
        }
        let Some(obj) = blob.as_object() else {
            return Err(Host::throw_type(&ctx, "FileReader: argument is not a Blob"));
        };
        let Ok(array_buffer) = obj.get::<_, Function>("arrayBuffer") else {
            return Err(Host::throw_type(&ctx, "FileReader: argument is not a Blob"));
        };
        let promise: Promise = array_buffer.call((This(obj.clone()),))?;
        let buffer: ArrayBuffer = promise.into_future().await?;
        Ok(buffer
            .as_bytes()
            .map(|bytes| bytes.to_vec())
            .unwrap_or_default())
    }

    fn decode(ctx: &Ctx<'js>, bytes: &[u8], encoding: &str) -> String {
        if let Ok(ctor) = ctx
            .globals()
            .get::<_, rquickjs::function::Constructor>("TextDecoder")
        {
            if let Ok(decoder) = ctor.construct::<_, Object>((encoding,)) {
                if let Ok(decode) = decoder.get::<_, Function>("decode") {
                    if let Ok(view) = TypedArray::<u8>::new_copy(ctx.clone(), bytes) {
                        if let Ok(text) = decode.call::<_, String>((This(decoder), view)) {
                            return text;
                        }
                    }
                }
            }
        }
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn later<F>(ctx: &Ctx<'js>, work: F)
    where
        F: FnOnce(Ctx<'js>) + 'js,
    {
        let work = Rc::new(RefCell::new(Some(work)));
        if let Ok(set_timeout) = ctx.globals().get::<_, Function>("setTimeout")
            && let Ok(callback) = Function::new(ctx.clone(), {
                let work = Rc::clone(&work);
                move |ctx: Ctx<'js>| {
                    if let Some(work) = work.borrow_mut().take() {
                        work(ctx);
                    }
                    Ok::<(), rquickjs::Error>(())
                }
            })
        {
            let _ = set_timeout.call::<_, Value>((callback, 0));
            return;
        }
        if let Some(work) = work.borrow_mut().take() {
            work(ctx.clone());
        }
    }

    fn resolve_encoding(encoding_name: Option<&str>, mime: &str) -> String {
        if let Some(name) = encoding_name.filter(|name| !name.is_empty()) {
            return name.to_string();
        }
        if let Some(charset) = charset_from_type(mime) {
            return charset;
        }
        "utf-8".to_string()
    }

}

fn charset_from_type(mime: &str) -> Option<String> {
    let lower = mime.to_ascii_lowercase();
    let rest = lower.split("charset=").nth(1)?;
    let token = rest
        .split(';')
        .next()
        .unwrap_or(rest)
        .trim()
        .trim_matches(|ch| ch == '"' || ch == '\'');
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
