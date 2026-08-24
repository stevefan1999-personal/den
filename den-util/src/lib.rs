//! Shared rquickjs helpers for den's stdlib crates.
//!
//! A helper lives here only once at least two crates would otherwise copy it;
//! crate-specific logic stays in the crate.

use std::ffi::CString;

use base64::{engine::Engine as _, prelude::BASE64_STANDARD};
use rquickjs::{
    ArrayBuffer, Class, Coerced, Constructor, Ctx, Error, Exception, FromJs, Function, Object,
    Result, Value, class::JsClass, function::IntoArgs, object::Filter, qjs,
};

/// WebIDL `BufferSource` — an `ArrayBuffer` or `ArrayBufferView`, with its
/// bytes copied up front so later mutation of the source cannot change what
/// the caller saw.
pub struct BufferSource(Vec<u8>);

impl BufferSource {
    pub fn bytes(&self) -> &[u8] { &self.0 }

    pub fn into_bytes(self) -> Vec<u8> { self.0 }

    /// `ArrayBuffer.isView` — the one brand check that covers every typed
    /// array and `DataView` without enumerating them, and that no ordinary
    /// object can forge.
    pub fn is_array_buffer_view<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
        ctx.globals()
            .get::<_, Object<'js>>("ArrayBuffer")?
            .get::<_, Function<'js>>("isView")?
            .call((value.clone(),))
    }

    /// Copy the bytes held by an `ArrayBufferView` through its
    /// `buffer`/`byteOffset`/`byteLength` window.
    pub fn view_bytes<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
        let type_error = || Exception::throw_type(ctx, "expected a BufferSource");
        let view = value.as_object().ok_or_else(type_error)?;
        let buffer: ArrayBuffer<'js> = view.get("buffer").map_err(|_error| type_error())?;
        let offset: usize = view.get("byteOffset").map_err(|_error| type_error())?;
        let length: usize = view.get("byteLength").map_err(|_error| type_error())?;
        let bytes = buffer
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(ctx, "the buffer is detached"))?;
        bytes
            .get(offset..offset.saturating_add(length))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Exception::throw_type(ctx, "the view is out of bounds of its buffer"))
    }
}

impl<'js> FromJs<'js> for BufferSource {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        if Self::is_array_buffer_view(ctx, &value)? {
            return Self::view_bytes(ctx, &value).map(Self);
        }
        ArrayBuffer::from_value(value).map_or_else(
            || Err(Exception::throw_type(ctx, "expected a BufferSource")),
            |buffer| {
                buffer
                    .as_bytes()
                    .map(<[u8]>::to_vec)
                    .map(Self)
                    .ok_or_else(|| Exception::throw_type(ctx, "the buffer is detached"))
            },
        )
    }
}

/// Throw `DOMException(message, name)` and return the pending-exception error.
pub fn throw_dom_exception(ctx: &Ctx<'_>, name: &str, message: &str) -> Error {
    let name = CString::new(name).unwrap_or_default();
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: `JS_ThrowDOMException` vsnprintf's into a 256-byte stack buffer
    // (quickjs.c:62309), so the caller's text is passed as an *argument* to a
    // constant `%s` format, never as the format itself. Both C strings outlive
    // the call.
    unsafe {
        qjs::JS_ThrowDOMException(
            ctx.as_raw().as_ptr(),
            name.as_ptr(),
            c"%s".as_ptr(),
            message.as_ptr(),
        );
    }
    Error::Exception
}

/// WebIDL ToString coercion.
pub fn coerce_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    Ok(Coerced::<String>::from_js(ctx, value)?.0)
}

/// Re-parent `Sub`'s class prototype onto `Super`'s, so one Rust class can
/// extend another's prototype chain.
pub fn inherit<'js, Sub, Super>(ctx: &Ctx<'js>) -> Result<()>
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

/// Speculative conversions that leave no pending exception behind.
///
/// `Class::from_object` is `JS_GetOpaque2` (quickjs.c:11681), which *throws* a
/// `TypeError` when the object belongs to some other class, and a failed
/// `FromJs` probe throws just the same. Code that reads such a failure as
/// "not this shape, try the next one" has to take that exception back out of
/// the context: it stays pending otherwise and surfaces as somebody else's
/// error.
pub trait Probe {
    /// Run `attempt`, discarding whatever exception it leaves pending when it
    /// yields `None`.
    fn probe<T>(&self, attempt: impl FnOnce() -> Option<T>) -> Option<T>;
}

impl Probe for Ctx<'_> {
    fn probe<T>(&self, attempt: impl FnOnce() -> Option<T>) -> Option<T> {
        let outcome = attempt();
        if outcome.is_none() && self.has_exception() {
            // `catch` is `JS_GetException`, which is what clears the slot; the
            // value itself is of no interest, the caller reports its own error.
            drop(self.catch());
        }
        outcome
    }
}

/// Standard-alphabet base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String { BASE64_STANDARD.encode(bytes) }

/// WHATWG forgiving-base64 decode: ASCII whitespace stripped, non-zero
/// trailing bits ignored, and padding dropped only when the spec says so.
pub fn base64_forgiving_decode(text: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    use base64::{
        alphabet,
        engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    };

    const FORGIVING: GeneralPurpose = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    let mut stripped: String = text
        .chars()
        .filter(|char| !char.is_ascii_whitespace())
        .collect();
    // Infra only strips one or two trailing `=` when the padded length is a
    // multiple of four; any other `=` stays and fails the alphabet check.
    if stripped.len().is_multiple_of(4) {
        let pads = stripped
            .bytes()
            .rev()
            .take_while(|&byte| byte == b'=')
            .count();
        if pads == 1 || pads == 2 {
            stripped.truncate(stripped.len() - pads);
        }
    }
    FORGIVING.decode(stripped)
}

/// Construct by calling the global constructor `name` with `args`.
pub fn construct<'js, A, R>(ctx: &Ctx<'js>, name: &str, args: A) -> Result<R>
where
    A: IntoArgs<'js>,
    R: FromJs<'js>,
{
    let ctor: Constructor<'js> = ctx.globals().get(name)?;
    ctor.construct(args)
}

/// `value instanceof <global name>`, tolerant of a missing or non-function
/// global (both read as `false`) and of a non-object LHS (`false`). Native
/// `JS_IsInstanceOf`; no eval.
pub fn instance_of_global<'js>(ctx: &Ctx<'js>, value: &Value<'js>, name: &str) -> Result<bool> {
    let Ok(ctor) = ctx.globals().get::<_, Value<'js>>(name) else {
        return Ok(false);
    };
    if !ctor.is_function() {
        return Ok(false);
    }
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    Ok(object.is_instance_of(&ctor))
}

/// Construct a `DOMException` from the engine intrinsic.
pub fn new_dom_exception<'js>(ctx: &Ctx<'js>, message: &str, name: &str) -> Result<Value<'js>> {
    let ctor: Constructor<'js> = ctx.globals().get("DOMException")?;
    ctor.construct((message, name))
}

/// WebIDL dictionary member read: `None` for a missing/`undefined` bag or key,
/// a `TypeError` for a bag that is not an object.
pub fn dict_get<'js>(options: Option<&Value<'js>>, key: &str) -> Result<Option<Value<'js>>> {
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

/// `JSON.parse(text)`.
pub fn json_parse<'js>(ctx: &Ctx<'js>, text: &str) -> Result<Value<'js>> {
    let json: Object = ctx.globals().get("JSON")?;
    let parse: Function = json.get("parse")?;
    parse.call((text,))
}

/// `JSON.stringify(value)`.
pub fn json_stringify<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Value<'js>> {
    let json: Object = ctx.globals().get("JSON")?;
    let stringify: Function = json.get("stringify")?;
    stringify.call((value.clone(),))
}

/// The QuickJS class id of a value (`JS_GetClassID`).
pub fn class_id(value: &Value<'_>) -> qjs::JSClassID {
    unsafe { qjs::JS_GetClassID(value.as_raw()) }
}

/// Own-property helpers rquickjs does not ship.
pub trait ObjectExt {
    /// Whether `key` is an OWN string key. `Object::contains_key` is the
    /// wrong tool: it is `JS_HasProperty`, which walks the prototype chain.
    fn has_own(&self, key: &str) -> Result<bool>;
}

impl ObjectExt for Object<'_> {
    fn has_own(&self, key: &str) -> Result<bool> {
        for name in self.own_keys::<String>(Filter::new().string()) {
            if name? == key {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::base64_forgiving_decode;

    /// WPT `fetch/data-urls/base64.any.js` cases that a strict standard
    /// decoder rejects: missing padding and non-zero trailing bits.
    #[test]
    fn forgiving_decode_matches_wpt() {
        assert_eq!(base64_forgiving_decode("ab").unwrap(), [0x69]);
        assert_eq!(base64_forgiving_decode("ab==").unwrap(), [0x69]);
        assert_eq!(
            base64_forgiving_decode("ab\t\n\u{c}\r =\t\n\u{c}\r =").unwrap(),
            [0x69]
        );
        assert_eq!(base64_forgiving_decode("A/").unwrap(), [0x03]);
        assert_eq!(base64_forgiving_decode("AA/").unwrap(), [0x00, 0x0f]);
        assert_eq!(base64_forgiving_decode("YR").unwrap(), [0x61]);
        assert_eq!(base64_forgiving_decode("abc=").unwrap(), [0x69, 0xb7]);
    }

    #[test]
    fn forgiving_decode_rejects_garbage() {
        // Length % 4 == 1 is the one unrecoverable length; `$` is off-alphabet.
        assert!(base64_forgiving_decode("abcde").is_err());
        assert!(base64_forgiving_decode("a$").is_err());
        // Spec strips only one or two `=` on a multiple-of-four length.
        assert!(base64_forgiving_decode("ab=").is_err());
        assert!(base64_forgiving_decode("a===").is_err());
    }
}
