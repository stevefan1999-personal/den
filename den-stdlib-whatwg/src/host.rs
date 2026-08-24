//! Shared host helpers: exceptions, buffer sources, events, prototype wiring.

use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    rc::Rc,
};

use den_util::{BufferSource, ObjectExt as _, Probe as _};
use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, Object, Result, Symbol, Value,
    atom::PredefinedAtom,
    class::JsClass,
    function::{Constructor, FuncArg, Opt, Rest, This},
    object::{Accessor, Property},
    qjs,
};

use crate::{
    blob::{Blob, File},
    form_data::FormData,
};

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
        if CString::new(name).is_ok() && CString::new(message).is_ok() {
            return den_util::throw_dom_exception(ctx, name, message);
        }
        if let Ok(exc) = den_util::new_dom_exception(ctx, message, name) {
            return ctx.throw(exc);
        }
        let code: i32 = match name {
            "IndexSizeError" => 1,
            "HierarchyRequestError" => 3,
            "WrongDocumentError" => 4,
            "InvalidCharacterError" => 5,
            "NoModificationAllowedError" => 7,
            "NotFoundError" => 8,
            "NotSupportedError" => 9,
            "InUseAttributeError" => 10,
            "InvalidStateError" => 11,
            "SyntaxError" => 12,
            "InvalidModificationError" => 13,
            "NamespaceError" => 14,
            "InvalidAccessError" => 15,
            "TypeMismatchError" => 17,
            "SecurityError" => 18,
            "NetworkError" => 19,
            "AbortError" => 20,
            "URLMismatchError" => 21,
            "QuotaExceededError" => 22,
            "TimeoutError" => 23,
            "InvalidNodeTypeError" => 24,
            "DataCloneError" => 25,
            _ => 0,
        };
        if let Ok(error_ctor) = ctx.globals().get::<_, Constructor>("Error")
            && let Ok(exc) = error_ctor.construct::<_, Object>((message,))
        {
            let _ = exc.set("name", name);
            let _ = exc.set("message", message);
            let _ = exc.set("code", code);
            return ctx.throw(exc.into_value());
        }
        Exception::throw_message(ctx, message)
    }

    /// WebIDL USVString: ToString, then replace unpaired UTF-16 surrogates with U+FFFD.
    pub fn coerce_usv_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
        let value = if value.is_string() {
            value
        } else {
            Coerced::<rquickjs::String>::from_js(ctx, value)?
                .0
                .into_value()
        };
        if let Some(string) = value.as_string()
            && let Ok(text) = string.to_string()
        {
            return Ok(text);
        }
        let mut len = std::mem::MaybeUninit::uninit();
        // SAFETY: `JS_ToCStringLenUTF16` writes the unit count and returns a
        // buffer QuickJS owns until `JS_FreeCStringUTF16`. Null means a JS
        // exception is pending. The slice is only used before that free.
        let ptr = unsafe {
            qjs::JS_ToCStringLenUTF16(ctx.as_raw().as_ptr(), len.as_mut_ptr(), value.as_raw())
        };
        if ptr.is_null() {
            return Err(rquickjs::Error::Exception);
        }
        let len = usize::try_from(unsafe { len.assume_init() }).unwrap_or(0);
        let units = unsafe { std::slice::from_raw_parts(ptr, len) };
        let text = String::from_utf16_lossy(units);
        unsafe {
            qjs::JS_FreeCStringUTF16(ctx.as_raw().as_ptr(), ptr);
        }
        Ok(text)
    }

    /// Lenient `BufferSource` read: `None` for anything that is not a buffer
    /// source, empty bytes for detached or unusable ones — never throws.
    pub fn buffer_source_bytes<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Option<Vec<u8>>> {
        // SAFETY: `JS_IsArrayBuffer` is a pure class-id check with no side
        // effects. Unlike `from_js`/`from_value` (both `JS_GetArrayBuffer`,
        // which refuses detached buffers) it holds for a detached buffer too —
        // still a buffer source, and it must read as empty instead of leaking
        // into the string-coercion branch.
        if unsafe { qjs::JS_IsArrayBuffer(value.as_raw()) } {
            return Ok(Some(
                ArrayBuffer::from_value(value.clone())
                    .and_then(|buffer| buffer.as_bytes().map(<[u8]>::to_vec))
                    .unwrap_or_default(),
            ));
        }
        let is_view = ctx
            .probe(|| BufferSource::is_array_buffer_view(ctx, &value).ok())
            .unwrap_or(false);
        if !is_view {
            return Ok(None);
        }
        Ok(Some(
            ctx.probe(|| BufferSource::view_bytes(ctx, &value).ok())
                .unwrap_or_default(),
        ))
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
            value.to_ascii_lowercase()
        } else {
            String::new()
        }
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
        ctx: &Ctx<'js>, type_: &str, data: Value<'js>, origin: &str, last_event_id: &str,
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
        ctx: &Ctx<'js>, type_: &str, length_computable: bool, loaded: f64, total: f64,
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
        ctx: &Ctx<'js>, code: u16, reason: &str, was_clean: bool,
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

    pub async fn maybe_await<'js>(value: Value<'js>) -> Result<Value<'js>> {
        if value.is_promise() {
            value.into_promise().unwrap().into_future().await
        } else {
            Ok(value)
        }
    }

    /// Install `document` after testharness chooses its environment.
    /// `'document' in globalThis` at testharness load would select WindowTestEnvironment.
    pub fn install_fileapi_document<'js>(ctx: &Ctx<'js>) -> Result<()> {
        let globals = ctx.globals();
        let hooked: Value = globals.get("__denFileapiDocHook")?;
        if hooked.as_bool().unwrap_or(false) {
            return Ok(());
        }
        globals.set("__denFileapiDocHook", true)?;

        let object_ctor: Object = globals.get("Object")?;
        let get_desc: Function = object_ctor.get("getOwnPropertyDescriptor")?;
        let existing: Value = get_desc.call((globals.clone(), "setup"))?;
        if let Some(desc) = existing.as_object()
            && let Ok(value) = desc.get::<_, Value>("value")
            && value.as_function().is_some()
        {
            globals.set("setup", wrap_setup(ctx, value)?)?;
            return Ok(());
        }
        globals.prop(
            "setup",
            Accessor::new(
                || Ok::<(), rquickjs::Error>(()),
                |ctx: Ctx<'js>, fn_: Value<'js>| -> Result<()> {
                    ctx.globals().prop(
                        "setup",
                        Property::from(wrap_setup(&ctx, fn_)?)
                            .configurable()
                            .enumerable()
                            .writable(),
                    )
                },
            )
            .configurable(),
        )?;
        Ok(())
    }
}

fn wrap_setup<'js>(ctx: &Ctx<'js>, func: Value<'js>) -> Result<Function<'js>> {
    let wrapped = Function::new(
        ctx.clone(),
        |this: This<Value<'js>>,
         callee: FuncArg<Function<'js>>,
         ctx: Ctx<'js>,
         args: Rest<Value<'js>>|
         -> Result<Value<'js>> {
            install_html_document(&ctx)?;
            let orig: Value = callee.0.get("__denOrigSetup")?;
            let Some(func) = orig.as_function() else {
                return Err(Exception::throw_type(&ctx, "fn.apply is not a function"));
            };
            func.call((This(this.0), Rest(args.0)))
        },
    )?;
    wrapped.prop("__denOrigSetup", Property::from(func))?;
    Ok(wrapped)
}

fn install_html_document<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    if globals.has_own("document")? {
        return Ok(());
    }
    for key in ["parent", "top"] {
        let value: Value = globals.get(key)?;
        if value.is_null() || value.is_undefined() {
            globals.set(key, globals.clone())?;
        }
    }
    let document = tagged(ctx, "HTMLDocument")?;
    document.set("readyState", "complete")?;
    document.set(
        "body",
        fileapi_create_element(ctx, Some(js_string(ctx, "body")?))?,
    )?;
    document.set(
        "documentElement",
        fileapi_create_element(ctx, Some(js_string(ctx, "html")?))?,
    )?;
    document.set("defaultView", globals.clone())?;
    document.set(
        "createElement",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, name: Opt<Value<'js>>| {
            fileapi_create_element(&ctx, name.0)
        })?,
    )?;
    document.set(
        "createElementNS",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, _ns: Opt<Value<'js>>, name: Opt<Value<'js>>| {
                fileapi_create_element(&ctx, name.0)
            },
        )?,
    )?;
    document.set(
        "getElementsByTagName",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, _: Opt<Value<'js>>| {
            collection(&ctx, Rc::new(RefCell::new(Vec::new())), "HTMLCollection")
        })?,
    )?;
    document.set(
        "getElementById",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, _: Opt<Value<'js>>| {
            Ok::<Value<'js>, rquickjs::Error>(Value::new_null(ctx))
        })?,
    )?;
    globals.set("document", document)?;
    Ok(())
}

fn js_string<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    rquickjs::String::from_str(ctx.clone(), text).map(rquickjs::String::into_value)
}

fn tagged<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.prop(PredefinedAtom::SymbolToStringTag, Property::from(name))?;
    Ok(object)
}

fn collection_iter<'js>(ctx: &Ctx<'js>) -> Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        |this: This<Object<'js>>, ctx: Ctx<'js>| -> Result<Object<'js>> {
            let collection = this.0;
            let index = Rc::new(Cell::new(0_u32));
            let iter = Object::new(ctx.clone())?;
            iter.set(
                "next",
                Function::new(ctx.clone(), {
                    let collection = collection.clone();
                    move |ctx: Ctx<'js>| -> Result<Object<'js>> {
                        let len = collection.get::<_, u32>("length").unwrap_or(0);
                        let at = index.get();
                        let result = Object::new(ctx.clone())?;
                        if at >= len {
                            result.set("done", true)?;
                            result.set("value", Value::new_undefined(ctx))?;
                        } else {
                            index.set(at + 1);
                            let value = collection
                                .get::<_, Value>(at)
                                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                            result.set("done", false)?;
                            result.set("value", value)?;
                        }
                        Ok(result)
                    }
                })?,
            )?;
            Ok(iter)
        },
    )
}

fn collection<'js>(
    ctx: &Ctx<'js>, items: Rc<RefCell<Vec<Value<'js>>>>, tag: &'static str,
) -> Result<Object<'js>> {
    let col = tagged(ctx, tag)?;
    for (index, item) in items.borrow().iter().enumerate() {
        col.set(index as u32, item.clone())?;
    }
    col.prop(
        "length",
        Accessor::new(
            {
                let items = Rc::clone(&items);
                move || items.borrow().len() as u32
            },
            {
                let items = Rc::clone(&items);
                move |ctx: Ctx<'js>, n: Value<'js>| -> Result<()> {
                    let n = Coerced::<f64>::from_js(&ctx, n)?.0.max(0.0) as usize;
                    let mut list = items.borrow_mut();
                    if n < list.len() {
                        list.truncate(n);
                    } else {
                        list.resize(n, Value::new_undefined(ctx.clone()));
                    }
                    Ok(())
                }
            },
        )
        .configurable()
        .enumerable(),
    )?;
    col.set(PredefinedAtom::SymbolIterator, collection_iter(ctx)?)?;
    Ok(col)
}

fn sync_select<'js>(
    ctx: &Ctx<'js>, el: &Object<'js>, tag: &str, kids: &Rc<RefCell<Vec<Value<'js>>>>,
) -> Result<()> {
    if tag != "select" {
        return Ok(());
    }
    for (index, child) in kids.borrow().iter().enumerate() {
        el.set(index as u32, child.clone())?;
    }
    let kids = Rc::clone(kids);
    el.prop(
        "length",
        Accessor::from(move || kids.borrow().len() as u32)
            .configurable()
            .enumerable(),
    )?;
    el.set(PredefinedAtom::SymbolIterator, collection_iter(ctx)?)?;
    Ok(())
}

fn fileapi_create_element<'js>(ctx: &Ctx<'js>, name: Option<Value<'js>>) -> Result<Object<'js>> {
    let name = name.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let tag = Coerced::<String>::from_js(ctx, name)?
        .0
        .to_ascii_lowercase();
    let type_name = match tag.as_str() {
        "select" => "HTMLSelectElement",
        "option" => "HTMLOptionElement",
        "p" => "HTMLParagraphElement",
        "body" => "HTMLBodyElement",
        "div" => "HTMLDivElement",
        _ => "HTMLElement",
    };
    let el = tagged(ctx, type_name)?;
    let kids = Rc::new(RefCell::new(Vec::new()));
    let attrs = Rc::new(RefCell::new(Vec::new()));
    el.set("tagName", tag.to_ascii_uppercase())?;
    el.set("localName", tag.clone())?;
    el.set("namespaceURI", "http://www.w3.org/1999/xhtml")?;
    el.set(
        "children",
        collection(ctx, Rc::clone(&kids), "HTMLCollection")?,
    )?;
    el.set(
        "attributes",
        collection(ctx, Rc::clone(&attrs), "NamedNodeMap")?,
    )?;
    el.set(
        "appendChild",
        Function::new(ctx.clone(), {
            let kids = Rc::clone(&kids);
            let tag = tag.clone();
            move |this: This<Object<'js>>,
                  ctx: Ctx<'js>,
                  child: Opt<Value<'js>>|
                  -> Result<Value<'js>> {
                let child = child.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                kids.borrow_mut().push(child.clone());
                this.0.set(
                    "children",
                    collection(&ctx, Rc::clone(&kids), "HTMLCollection")?,
                )?;
                sync_select(&ctx, &this.0, &tag, &kids)?;
                Ok(child)
            }
        })?,
    )?;
    el.set(
        "setAttribute",
        Function::new(ctx.clone(), {
            let attrs = Rc::clone(&attrs);
            move |this: This<Object<'js>>,
                  ctx: Ctx<'js>,
                  attr_name: Opt<Value<'js>>,
                  value: Opt<Value<'js>>|
                  -> Result<()> {
                let attr = tagged(&ctx, "Attr")?;
                attr.set(
                    "name",
                    attr_name
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                )?;
                let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                attr.set("value", Coerced::<String>::from_js(&ctx, value)?.0)?;
                attrs.borrow_mut().push(attr.clone().into_value());
                this.0.set(
                    "attributes",
                    collection(&ctx, Rc::clone(&attrs), "NamedNodeMap")?,
                )?;
                Ok(())
            }
        })?,
    )?;
    sync_select(ctx, &el, &tag, &kids)?;
    Ok(el)
}
