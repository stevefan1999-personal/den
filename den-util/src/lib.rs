//! Shared rquickjs helpers for den's stdlib crates.
//!
//! A helper lives here only once at least two crates would otherwise copy it;
//! crate-specific logic stays in the crate.

use std::{ffi::CString, mem::MaybeUninit};

use rquickjs::{
    ArrayBuffer, Class, Coerced, Constructor, Ctx, Error, Exception, FromJs, Function, JsLifetime,
    Object, Proxy, Result, Value,
    atom::PredefinedAtom,
    class::JsClass,
    function::{IntoArgs, This},
    object::Filter,
    proxy::ProxyHandler,
    qjs,
    runtime::UserDataError,
};

pub mod stack;

/// WebIDL `BufferSource` — an `ArrayBuffer` or `ArrayBufferView`, with its
/// bytes copied up front so later mutation of the source cannot change what
/// the caller saw.
pub struct BufferSource(Vec<u8>);

#[derive(JsLifetime)]
struct BufferSourceIntrinsics<'js> {
    buffer: Function<'js>,
    offset: Function<'js>,
    length: Function<'js>,
}

/// Capture DataView's native accessors before user code can replace globals or
/// shadow properties on individual views.
pub fn install_buffer_source_intrinsics(ctx: &Ctx<'_>) -> Result<()> {
    let data_view: Object = ctx.globals().get("DataView")?;
    let prototype: Object = data_view.get("prototype")?;
    let object: Object = ctx.globals().get("Object")?;
    let descriptor: Function = object.get(PredefinedAtom::GetOwnPropertyDescriptor)?;
    let getter = |name| -> Result<Function> {
        descriptor
            .call::<_, Object>((prototype.clone(), name))?
            .get("get")
    };
    ctx.store_userdata(BufferSourceIntrinsics {
        buffer: getter("buffer")?,
        offset: getter("byteOffset")?,
        length: getter("byteLength")?,
    })
    .map(|_| ())
    .map_err(|_error| Error::UserData(UserDataError(())))
}

impl BufferSource {
    pub fn bytes(&self) -> &[u8] { &self.0 }

    pub fn into_bytes(self) -> Vec<u8> { self.0 }

    /// Intrinsic brand check for every typed array and `DataView`.
    pub fn is_array_buffer_view(_ctx: &Ctx<'_>, value: &Value<'_>) -> Result<bool> {
        let raw = value.as_raw();
        // SAFETY: both functions only inspect the tag/class of a live JSValue.
        Ok(unsafe { qjs::JS_GetTypedArrayType(raw) >= 0 || qjs::JS_IsDataView(raw) })
    }

    /// Copy an `ArrayBufferView` through QuickJS's native window metadata.
    pub fn view_bytes<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
        if unsafe { qjs::JS_GetTypedArrayType(value.as_raw()) >= 0 } {
            return Self::typed_array_bytes(ctx, value);
        }
        if !unsafe { qjs::JS_IsDataView(value.as_raw()) } {
            return Err(Exception::throw_type(ctx, "expected a BufferSource"));
        }
        let (buffer_getter, offset_getter, length_getter) = Self::data_view_intrinsics(ctx)?;
        let buffer: ArrayBuffer = buffer_getter.call((This(value.clone()),))?;
        let offset: usize = offset_getter.call((This(value.clone()),))?;
        let length: usize = length_getter.call((This(value.clone()),))?;
        Self::copy_window(ctx, &buffer, offset, length)
    }

    fn typed_array_bytes<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
        let mut offset = MaybeUninit::<qjs::size_t>::uninit();
        let mut length = MaybeUninit::<qjs::size_t>::uninit();
        // SAFETY: the native brand was checked above; QuickJS owns the returned
        // buffer value and initializes both window outputs on success.
        let raw = unsafe {
            qjs::JS_GetTypedArrayBuffer(
                ctx.as_raw().as_ptr(),
                value.as_raw(),
                offset.as_mut_ptr(),
                length.as_mut_ptr(),
                core::ptr::null_mut(),
            )
        };
        if unsafe { qjs::JS_IsException(raw) } {
            return Err(Error::Exception);
        }
        // SAFETY: JS_GetTypedArrayBuffer transfers one owned value reference.
        let buffer = unsafe { Value::from_raw(ctx.clone(), raw) };
        let buffer = ArrayBuffer::from_value(buffer)
            .ok_or_else(|| Exception::throw_type(ctx, "expected a BufferSource"))?;
        let offset = usize::try_from(unsafe { offset.assume_init() })
            .map_err(|_error| Exception::throw_type(ctx, "the view offset is too large"))?;
        let length = usize::try_from(unsafe { length.assume_init() })
            .map_err(|_error| Exception::throw_type(ctx, "the view length is too large"))?;
        Self::copy_window(ctx, &buffer, offset, length)
    }

    fn copy_window(
        ctx: &Ctx<'_>, buffer: &ArrayBuffer<'_>, offset: usize, length: usize,
    ) -> Result<Vec<u8>> {
        let bytes = buffer
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(ctx, "the buffer is detached"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Exception::throw_type(ctx, "the view is out of bounds of its buffer"))?;
        bytes
            .get(offset..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Exception::throw_type(ctx, "the view is out of bounds of its buffer"))
    }

    fn data_view_intrinsics<'js>(
        ctx: &Ctx<'js>,
    ) -> Result<(Function<'js>, Function<'js>, Function<'js>)> {
        if ctx.userdata::<BufferSourceIntrinsics<'js>>().is_none() {
            install_buffer_source_intrinsics(ctx)?;
        }
        let intrinsics = ctx
            .userdata::<BufferSourceIntrinsics<'js>>()
            .ok_or_else(|| Exception::throw_type(ctx, "BufferSource intrinsics are unavailable"))?;
        Ok((
            intrinsics.buffer.clone(),
            intrinsics.offset.clone(),
            intrinsics.length.clone(),
        ))
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

/// Install a native Rust class as a JavaScript constructor that rejects calls
/// without `new`.
///
/// rquickjs class constructors are callable objects; a constructor proxy lets
/// QuickJS enforce the distinction while forwarding `new.target` unchanged.
pub trait ConstructorInstaller<'js> {
    fn install_constructor<C: JsClass<'js>>(&self, length: usize) -> Result<()>;
}

impl<'js> ConstructorInstaller<'js> for Object<'js> {
    fn install_constructor<C: JsClass<'js>>(&self, length: usize) -> Result<()> {
        let ctx = self.ctx();
        let constructor = Class::<C>::create_constructor(ctx)?.ok_or_else(|| {
            Exception::throw_internal(ctx, &format!("{} has no constructor", C::NAME))
        })?;
        constructor.set_length(length)?;

        let handler = Object::new(ctx.clone())?;
        handler.set(
            "apply",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| -> Result<()> {
                Err(Exception::throw_type(
                    &ctx,
                    "class constructors must be invoked with 'new'",
                ))
            })?,
        )?;
        let constructor = Proxy::new(
            ctx.clone(),
            constructor,
            ProxyHandler::from_object(handler)?,
        )?;
        if let Some(prototype) = Class::<C>::prototype(ctx)? {
            prototype.set(PredefinedAtom::Constructor, constructor.clone())?;
        }
        self.set(C::NAME, constructor)
    }
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

/// QuickJS class id (`JS_GetClassID`).
pub trait ClassId {
    fn class_id(&self) -> qjs::JSClassID;
}

impl ClassId for Value<'_> {
    fn class_id(&self) -> qjs::JSClassID { unsafe { qjs::JS_GetClassID(self.as_raw()) } }
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

/// Owning, lifetime-erased handle to a QuickJS context.
///
/// A host callback the engine below calls — a wasmtime host function, a libffi
/// closure — has to be `'static`, so it cannot borrow `'js`. `Ctx` is
/// refcounted (`Clone` is `JS_DupContext`), so parking one keeps the
/// `JSContext` alive and [`OwnedCtx::with`] mints a callback-scoped `'js` on
/// demand — `Ctx` is invariant in `'js`, so it has to be minted rather than
/// reborrowed.
///
/// This is the most delicate `unsafe` in den; keep it to this one type.
///
/// Deliberately *not* `Sync`: a `JSContext` is not shareable between threads,
/// and asserting it would make every container of one look as if it were.
pub struct OwnedCtx(Ctx<'static>);

impl OwnedCtx {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        // SAFETY: `from_raw` takes a reference of its own via `JS_DupContext`, and the
        // caller is inside `ctx`, so the runtime lock is held right now.
        Self(unsafe { Ctx::from_raw(ctx.as_raw()) })
    }

    /// Re-narrow the erased context to a callback-scoped `'js`.
    ///
    /// This is the only way to reach the context: a `fn ctx(&self) -> Ctx<'_>`
    /// would hand out a lifetime the caller could outlive.
    ///
    /// # Safety of the *call site*, which this type cannot check
    ///
    /// The reference itself is sound — `Ctx::from_raw` performs
    /// `JS_DupContext`, and the runtime drops its userdata (hence every value
    /// of this type) before `JS_FreeRuntime`, so the context outlives the
    /// handle. What the caller must supply is the **runtime lock**: `f` runs
    /// JS, so this may only be entered from a frame that already holds it —
    /// for a host callback, one a JS call reached. den's two users check that
    /// differently: a wasm host callback is only ever entered from such a
    /// frame, while a libffi trampoline compares thread ids first, because C
    /// may call it from a thread of its own.
    pub fn with<R, F: FnOnce(&Ctx<'_>) -> R>(&self, f: F) -> R {
        // SAFETY: `self.0` holds a live reference to this context, and the caller
        // holds the runtime lock (see above). The minted `Ctx` never escapes.
        let ctx = unsafe { Ctx::from_raw(self.0.as_raw()) };
        f(&ctx)
    }
}
