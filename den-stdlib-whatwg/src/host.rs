//! Shared host helpers: exceptions, buffer sources, events, prototype wiring.

use std::ffi::CString;

use den_util::{BufferSource, Probe as _};
use rquickjs::{
    ArrayBuffer, Class, Coerced, Ctx, Exception, FromJs, Function, Object, Result, Symbol, Value,
    class::JsClass, function::Constructor, qjs,
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
        if let Ok(name_c) = CString::new(name)
            && let Ok(message_c) = CString::new(message)
        {
            // SAFETY: `JS_ThrowDOMException` vsnprintf's into a 256-byte stack
            // buffer (quickjs.c:62309), so the caller's text is passed as an
            // *argument* to a constant `%s` format, never as the format itself.
            // Both C strings outlive the call.
            unsafe {
                qjs::JS_ThrowDOMException(
                    ctx.as_raw().as_ptr(),
                    name_c.as_ptr(),
                    c"%s".as_ptr(),
                    message_c.as_ptr(),
                );
            }
            return rquickjs::Error::Exception;
        }
        if let Ok(ctor) = ctx.globals().get::<_, Constructor>("DOMException")
            && let Ok(exc) = ctor.construct::<_, Value>((message, name))
        {
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
        let js = Coerced::<rquickjs::String>::from_js(ctx, value.clone())?;
        match js.to_string() {
            Ok(text) => Ok(text),
            Err(_) => {
                let convert: Function = ctx.eval(
                    "(v) => { const s = String(v); let o = ''; for (let i = 0; i < s.length; i++) { const c = s.charCodeAt(i); if (c >= 0xD800 && c <= 0xDBFF) { const d = i + 1 < s.length ? s.charCodeAt(i + 1) : 0; if (d >= 0xDC00 && d <= 0xDFFF) { o += String.fromCharCode(c, d); i++; } else { o += '\\uFFFD'; } } else if (c >= 0xDC00 && c <= 0xDFFF) { o += '\\uFFFD'; } else { o += String.fromCharCode(c); } } return o; }",
                )?;
                convert.call((value,))
            }
        }
    }

    /// Lenient `BufferSource` read: `None` for anything that is not a buffer
    /// source, empty bytes for detached or unusable ones — never throws.
    pub fn buffer_source_bytes<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Option<Vec<u8>>> {
        if let Ok(buffer) = ArrayBuffer::from_js(ctx, value.clone()) {
            return Ok(Some(
                buffer.as_bytes().map(<[u8]>::to_vec).unwrap_or_default(),
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

    /// Install `document` after testharness chooses its environment.
    /// `'document' in globalThis` at testharness load would select WindowTestEnvironment.
    pub fn install_fileapi_document(ctx: &Ctx<'_>) -> Result<()> {
        ctx.eval::<(), _>(FILEAPI_DOCUMENT_HOOK)
    }
}

const FILEAPI_DOCUMENT_HOOK: &str = r#"
(function () {
  if (globalThis.__denFileapiDocHook) return;
  globalThis.__denFileapiDocHook = true;

  function installDocument() {
  if (Object.getOwnPropertyDescriptor(globalThis, "document")) return;
  if (globalThis.parent == null) globalThis.parent = globalThis;
  if (globalThis.top == null) globalThis.top = globalThis;

  function tagged(name) {
    var object = {};
    Object.defineProperty(object, Symbol.toStringTag, { value: name });
    return object;
  }

  function collection(items, tag) {
    var col = tagged(tag || "HTMLCollection");
    var list = items;
    for (var i = 0; i < list.length; i++) col[i] = list[i];
    Object.defineProperty(col, "length", {
      configurable: true,
      enumerable: true,
      get: function () { return list.length; },
      set: function (n) { list.length = Number(n); }
    });
    col[Symbol.iterator] = function () {
      var index = 0;
      var self = this;
      return {
        next: function () {
          var len = self.length;
          if (index >= len) return { done: true, value: undefined };
          return { done: false, value: self[index++] };
        }
      };
    };
    return col;
  }

  function createElement(name) {
    var tag = String(name).toLowerCase();
    var typeName =
      tag === "select" ? "HTMLSelectElement" :
      tag === "option" ? "HTMLOptionElement" :
      tag === "p" ? "HTMLParagraphElement" :
      tag === "body" ? "HTMLBodyElement" :
      tag === "div" ? "HTMLDivElement" :
      "HTMLElement";
    var el = tagged(typeName);
    var kids = [];
    var attrs = [];
    el.tagName = tag.toUpperCase();
    el.localName = tag;
    el.namespaceURI = "http://www.w3.org/1999/xhtml";
    function syncSelect() {
      if (tag !== "select") return;
      for (var i = 0; i < kids.length; i++) el[i] = kids[i];
      Object.defineProperty(el, "length", {
        configurable: true,
        enumerable: true,
        get: function () { return kids.length; }
      });
      el[Symbol.iterator] = collection(kids, "HTMLOptionsCollection")[Symbol.iterator];
    }
    el.children = collection(kids, "HTMLCollection");
    el.attributes = collection(attrs, "NamedNodeMap");
    el.appendChild = function (child) {
      kids.push(child);
      el.children = collection(kids, "HTMLCollection");
      syncSelect();
      return child;
    };
    el.setAttribute = function (attrName, value) {
      var attr = tagged("Attr");
      attr.name = attrName;
      attr.value = String(value);
      attrs.push(attr);
      el.attributes = collection(attrs, "NamedNodeMap");
    };
    syncSelect();
    return el;
  }

  var document = tagged("HTMLDocument");
  document.readyState = "complete";
  document.body = createElement("body");
  document.documentElement = createElement("html");
  document.defaultView = globalThis;
  document.createElement = createElement;
  document.createElementNS = function (_ns, name) { return createElement(name); };
  document.getElementsByTagName = function () { return collection([], "HTMLCollection"); };
  document.getElementById = function () { return null; };
  globalThis.document = document;
  }

  function wrapSetup(fn) {
    return function () {
      installDocument();
      return fn.apply(this, arguments);
    };
  }

  var existing = Object.getOwnPropertyDescriptor(globalThis, "setup");
  if (existing && typeof existing.value === "function") {
    globalThis.setup = wrapSetup(existing.value);
    return;
  }
  Object.defineProperty(globalThis, "setup", {
    configurable: true,
    enumerable: false,
    get: function () { return undefined; },
    set: function (fn) {
      Object.defineProperty(globalThis, "setup", {
        configurable: true,
        enumerable: true,
        writable: true,
        value: wrapSetup(fn)
      });
    }
  });
})();
"#;
