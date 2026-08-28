//! Addresses, and the four things JS may do with one — §4.5.
//!
//! A `Pointer` is opaque and unforgeable: there is no `create(bigint)` and no
//! `offset(p, n)`, so every address JS holds came out of a real call. It also
//! carries provenance — the library whose `dlclose` invalidates it — so a
//! dereference after `lib[Symbol.dispose]()` is a typed throw.
//!
//! That is **not** a validity check, and `view`'s mandatory length is **not** a
//! bounds check: den cannot know how large a pointee is, and `live` tracks only
//! den's own `dlclose`. §5.1 items 2 and 4; the same warning is in the `.d.ts`.

use std::{ffi::c_void, rc::Rc, slice};

use rquickjs::{
    BigInt, Class, Ctx, Function, JsLifetime, Object, Result, TypedArray, Value, class::Trace,
    function::Opt,
};

use crate::{error::ErrorKind, library::LoadedLibrary, marshal};

/// How far `cstring` scans when the caller names no bound. A C string with no
/// NUL in four kibibytes is a bug, and an unbounded scan is a fault waiting for
/// the first page boundary.
const DEFAULT_CSTRING_LIMIT: usize = 4096;

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "Pointer")]
pub struct Pointer {
    #[qjs(skip_trace)]
    address: usize,
    /// The library whose `dlclose` invalidates this address, when den knows
    /// which one that is. A pointer C passed *in* — a callback's argument —
    /// has none: den is told an address and nothing about where it came from,
    /// so there is nothing to check it against and it is never `Closed`.
    #[qjs(skip_trace)]
    library: Option<Rc<LoadedLibrary>>,
}

impl Pointer {
    /// A C address as a JS value: the null pointer is JS `null`, so a script
    /// never holds a `Pointer` it must not dereference.
    pub fn to_js<'js>(
        ctx: &Ctx<'js>, address: usize, library: Option<&Rc<LoadedLibrary>>,
    ) -> Result<Value<'js>> {
        if address == 0 {
            return Ok(Value::new_null(ctx.clone()));
        }
        Ok(Class::instance(ctx.clone(), Self {
            address,
            library: library.map(Rc::clone),
        })?
        .into_value())
    }

    /// A pointer argument. `null` is the null pointer; anything that is not a
    /// `Pointer` is refused rather than coerced, and a `Pointer` whose library
    /// is gone is `Closed` rather than a call into unmapped pages.
    pub fn marshal<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<*mut c_void> {
        if value.is_null() {
            return Ok(std::ptr::null_mut());
        }
        let instance = Self::from_value(ctx, value)?;
        let address = instance.borrow().live(ctx)?;
        Ok(std::ptr::with_exposed_provenance_mut(address))
    }

    fn from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Class<'js, Self>> {
        value
            .as_object()
            .and_then(Class::<Self>::from_object)
            .ok_or_else(|| {
                ErrorKind::BadArgument.throw(
                    ctx,
                    "expected a Pointer — addresses cannot be made from numbers",
                )
            })
    }

    /// The address, if the library that produced it is still mapped.
    fn live(&self, ctx: &Ctx<'_>) -> Result<usize> {
        match &self.library {
            Some(library) if !library.is_live() => {
                Err(ErrorKind::Closed.throw_at(
                    ctx,
                    "the library this pointer came from is closed",
                    library.path(),
                ))
            }
            _known_live_or_unowned => Ok(self.address),
        }
    }
}

/// The `ptr` namespace. Four functions, built once at module evaluation —
/// deliberately not a class, and deliberately without `create`/`offset`.
pub fn namespace<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let namespace = Object::new(ctx.clone())?;
    namespace.set(
        "value",
        Function::new(ctx.clone(), value)?.with_name("value")?,
    )?;
    namespace.set(
        "equals",
        Function::new(ctx.clone(), equals)?.with_name("equals")?,
    )?;
    namespace.set("view", Function::new(ctx.clone(), view)?.with_name("view")?)?;
    namespace.set(
        "cstring",
        Function::new(ctx.clone(), cstring)?.with_name("cstring")?,
    )?;
    Ok(namespace)
}

/// The address as a `bigint`, for logging and for equality against C's own
/// idea of the same address. It cannot be turned back into a `Pointer`.
fn value<'js>(ctx: Ctx<'js>, pointer: Value<'js>) -> Result<BigInt<'js>> {
    let pointer = Pointer::from_value(&ctx, &pointer)?;
    let address = pointer.borrow().address as u64;
    BigInt::from_u64(ctx, address)
}

/// Address equality, null-safe. Two `Pointer` objects are distinct JS values
/// even when they name the same address, so `===` is not this.
fn equals<'js>(ctx: Ctx<'js>, left: Value<'js>, right: Value<'js>) -> Result<bool> {
    Ok(address_of(&ctx, &left)? == address_of(&ctx, &right)?)
}

/// The address a pointer slot holds, where JS `null` is the null pointer.
fn address_of<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<usize> {
    if value.is_null() {
        Ok(0)
    } else {
        Ok(Pointer::from_value(ctx, value)?.borrow().address)
    }
}

/// A **copy** of `byteLength` bytes at `p`.
///
/// The length is mandatory and is not a bounds check — den cannot know how
/// large the pointee is (§5.1 item 4). What it buys is that the resulting
/// `Uint8Array` has a length at all, so every read after this one is bounds
/// checked by the engine. The bytes are copied, per den's rule that no JS
/// value ever aliases memory the engine does not own; writing to the result
/// does not write back to C.
fn view<'js>(
    ctx: Ctx<'js>, pointer: Value<'js>, byte_length: Opt<Value<'js>>,
) -> Result<TypedArray<'js, u8>> {
    let pointer = Pointer::from_value(&ctx, &pointer)?;
    let address = pointer.borrow().live(&ctx)?;
    // Mandatory, and den's own error rather than the engine's arity check, so
    // that every refusal out of `ptr` carries a `kind`.
    let byte_length = byte_length.0.ok_or_else(|| {
        ErrorKind::BadArgument.throw(
            &ctx,
            "`ptr.view` needs a byteLength — den cannot know how large a pointee is",
        )
    })?;
    let length: usize = marshal::integer(&ctx, "byteLength", &byte_length)?;
    // SAFETY: this is an arbitrary read and den says so — §5.1 item 4 and the
    // `.d.ts`. `address` came from a real C value and belongs to a library that
    // is still mapped, but nothing here knows the pointee's size; that `length`
    // is within it is the caller's contract.
    let bytes =
        unsafe { slice::from_raw_parts(std::ptr::with_exposed_provenance(address), length) };
    TypedArray::new_copy(ctx.clone(), bytes)
}

/// A NUL-terminated C string, scanned one byte at a time so that nothing past
/// the terminator is ever read. No NUL inside the bound is `Range`, never a
/// truncated string.
fn cstring<'js>(
    ctx: Ctx<'js>, pointer: Value<'js>, max_byte_length: Opt<Value<'js>>,
) -> Result<String> {
    let pointer = Pointer::from_value(&ctx, &pointer)?;
    let address = pointer.borrow().live(&ctx)?;
    let limit = max_byte_length
        .0
        .map(|declared| marshal::integer(&ctx, "maxByteLength", &declared))
        .transpose()?
        .unwrap_or(DEFAULT_CSTRING_LIMIT);

    let start: *const u8 = std::ptr::with_exposed_provenance(address);
    let mut bytes = Vec::new();
    for offset in 0..limit {
        // SAFETY: the caller's contract that `address` is a C string of at most
        // `limit` bytes (§5.1 item 4). The scan stops at the terminator, so no
        // byte past the string is touched — which is why this reads one byte at
        // a time rather than a `limit`-sized slice.
        let byte = unsafe { start.add(offset).read() };
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|_not_utf8| {
                ErrorKind::BadArgument.throw(&ctx, "the C string is not valid UTF-8")
            });
        }
        bytes.push(byte);
    }
    Err(ErrorKind::Range.throw(
        &ctx,
        format_args!("no NUL byte in the first {limit} bytes at this address"),
    ))
}
