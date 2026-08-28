//! JS values in, C values out, and back — `docs/research/19-den-ffi.md` §4.4.
//!
//! Two rules carry most of the weight here. 64-bit integers cross through
//! their **decimal string**, never `BigInt::to_i64` (doc 09 fact 12). And a
//! result lands in a [`Cell`] rather than in a typed local, because
//! `call_return_into` corrects sub-register returns itself and writes exactly
//! `type.size()` bytes (§0 fact 3) into a buffer libffi requires to be at
//! least `ffi_arg`-wide.

use std::{ffi::c_void, rc::Rc, str::FromStr, sync::Arc};

use libffi::{
    low::{ffi_arg, ffi_sarg},
    middle::{Arg, Cif, CodePtr, Ret},
};
use rquickjs::{BigInt, Coerced, Ctx, FromJs as _, Object, Result, TypedArray, Value};

use crate::{
    callback::Callback,
    error::ErrorKind,
    library::LoadedLibrary,
    pointer::Pointer,
    schema::{CallMode, NativeType, ParamType},
};

/// The largest integer a JS `number` holds exactly. Past it the value was
/// already lost before den saw it, so narrowing it into a C integer would hand
/// C a wrong answer it cannot detect.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The bytes one C value occupies while it is neither a JS value nor an
/// [`ArgumentCell`]: a result libffi has written, or an argument a trampoline
/// was handed on a thread that may not touch JS.
///
/// Eight bytes, eight-aligned. That is at least libffi's `ffi_arg`, which is
/// the minimum size libffi documents for a return buffer, and at least every
/// scalar den marshals — `call_return_into` then writes exactly the declared
/// type's size into it (§0 fact 3).
#[derive(Clone, Copy, Default)]
#[repr(C, align(8))]
pub struct Cell([u8; CELL_BYTES]);

const CELL_BYTES: usize = size_of::<u64>();
const _: () = assert!(
    size_of::<ffi_arg>() <= CELL_BYTES && size_of::<*mut c_void>() <= CELL_BYTES,
    "a C value den marshals does not fit its cell"
);

impl Cell {
    /// Copy one C value out of the buffer libffi is pointing at.
    ///
    /// # Safety
    ///
    /// `address` must point at an initialised, readable C value of the type
    /// `declared` names.
    pub unsafe fn read_from(declared: NativeType, address: *const u8) -> Self {
        let mut cell = Self::default();
        // SAFETY: the caller's contract, and `declared.size()` — which is that
        // value's width — never exceeds this cell.
        unsafe { std::ptr::copy_nonoverlapping(address, cell.0.as_mut_ptr(), declared.size()) };
        cell
    }

    pub const fn as_ptr(&self) -> *const u8 { self.0.as_ptr() }
}

/// Where an argument is going: what makes a `Callback` legal, and what a
/// foreign-thread callback names when it gives up waiting for the realm.
#[derive(Clone, Copy)]
pub struct CallSite<'a> {
    pub mode:   CallMode,
    /// `/path/to/lib.so::symbol`, recorded on a callback as the last place C
    /// was handed its address.
    pub origin: &'a Arc<str>,
}

/// One argument, owned for exactly the duration of one call: `Arg::new` takes
/// the cell's address, and libffi may rewrite the argument array in place, so
/// cells are built fresh per call and never cached (§4.3).
///
/// Plain data, and therefore `Send`: this is what a `nonblocking` call carries
/// to its worker thread and what the realm sends back to a foreign-thread
/// callback. An address rides as a `usize` for exactly that reason — see
/// [`ArgumentCell::as_arg`].
pub enum ArgumentCell {
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Isize(isize),
    Usize(usize),
    F32(f32),
    F64(f64),
    /// An address, already exposed for the C side to reconstitute. A raw
    /// pointer here would make the whole cell `!Send` for no gain: libffi
    /// copies the bytes and Rust's provenance cannot follow them into C.
    Pointer(usize),
}

impl ArgumentCell {
    pub fn marshal<'js>(
        ctx: &Ctx<'js>, declared: &ParamType, value: &Value<'js>, site: CallSite<'_>,
    ) -> Result<Self> {
        match declared {
            ParamType::Scalar(scalar) => Self::scalar(ctx, *scalar, value),
            ParamType::Buffer => Ok(Self::Pointer(Self::buffer(ctx, value)?)),
            // §4.7, and the whole of fact 6: a C function handed a callback is
            // free to spawn a thread, call it and join. On the realm thread
            // that is an unbreakable deadlock — the realm is inside C, so it
            // can never service the callback — so a `Callback` may only be
            // handed to a symbol that runs off the realm thread. Deterministic,
            // checked here, and satisfied by one word in the schema.
            ParamType::Callback(_) if site.mode == CallMode::Blocking => {
                Err(ErrorKind::BadArgument.throw(
                    ctx,
                    format_args!(
                        "`{}` must be declared `nonblocking: true` to take a callback — C may \
                         call it from a thread of its own, and this thread is the only one that \
                         may run JS",
                        site.origin
                    ),
                ))
            }
            ParamType::Callback(signature) => {
                Ok(Self::Pointer(Callback::code_pointer(
                    ctx,
                    signature,
                    value,
                    site.origin,
                )?))
            }
        }
    }

    /// A `Uint8Array`'s own bytes, borrowed for exactly one call.
    ///
    /// `as_raw()` and not `as_bytes()`: the latter hands back a `&[u8]`, which
    /// cannot back a C function that *writes* into the buffer. Both are
    /// byteOffset-correct — `get_raw_bytes` adds the view's offset — and both
    /// report detachment and nothing else, so the two backing stores that can
    /// move or be mutated while C holds the address need their own reads
    /// (§4.6).
    ///
    /// The address is never stored: the `Value` this reads lives in the
    /// caller's argument list for the whole of `BoundFn::call`, and the
    /// cell dies with the call.
    fn buffer<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<usize> {
        let view = value
            .as_object()
            .cloned()
            .and_then(|object| TypedArray::<u8>::from_object(object).ok())
            .ok_or_else(|| {
                ErrorKind::BadArgument.throw(ctx, "expected a Uint8Array for `buffer`")
            })?;
        // Detachment is the one hazard `as_raw` reports by itself, and it is
        // checked first because the reads below need a buffer to ask about.
        Self::address_of(ctx, &view)?;
        Self::refuse_unstable_store(ctx, &view)?;
        // The address is taken only now: `Symbol.hasInstance` on the realm's
        // `SharedArrayBuffer` is a JS hook, and JS can detach a buffer — so an
        // address read before that check could already be dangling here.
        Self::address_of(ctx, &view)
    }

    /// The address of the view's first byte, or the typed refusal for a
    /// detached buffer.
    fn address_of<'js>(ctx: &Ctx<'js>, view: &TypedArray<'js, u8>) -> Result<usize> {
        view.as_raw()
            // Exposed rather than merely addressed: this is handed to C, which
            // is exactly what `expose_provenance` documents.
            .map(|raw| raw.ptr.as_ptr().expose_provenance())
            .ok_or_else(|| {
                ErrorKind::BadArgument.throw(ctx, "the buffer behind this Uint8Array is detached")
            })
    }

    /// A shared or resizable backing store can be written or moved by another
    /// agent *while C holds the pointer*, and rquickjs 0.12.2 reports neither:
    /// `ArrayBuffer`'s whole surface is `len`/`as_bytes`/`as_raw`/`detach`.
    /// So both are explicit reads, per §4.6.
    ///
    /// The buffer comes from `TypedArray::arraybuffer()`, which is
    /// `JS_GetTypedArrayBuffer` rather than a `.buffer` property read, so an
    /// own property on the view cannot aim this at a different store.
    ///
    /// ponytail: `resizable` is still read through the prototype getter, so a
    /// script that shadows it on its own buffer defeats this one check. The
    /// unspoofable version is a `JS_GetAnyOpaque` through `rquickjs-sys`, which
    /// is a new dependency for a hole a caller can only dig for itself.
    fn refuse_unstable_store<'js>(ctx: &Ctx<'js>, view: &TypedArray<'js, u8>) -> Result<()> {
        let buffer = view.arraybuffer()?;
        let buffer = buffer.as_object();
        if let Some(shared) = ctx
            .globals()
            .get::<_, Option<Object<'js>>>("SharedArrayBuffer")?
            && buffer.is_instance_of(shared)
        {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                "a SharedArrayBuffer cannot back a `buffer` argument — another agent could write \
                 it while C holds the address",
            ));
        }
        if buffer.get::<_, Option<bool>>("resizable")? == Some(true) {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                "a resizable ArrayBuffer cannot back a `buffer` argument — resizing moves the \
                 bytes out from under C",
            ));
        }
        Ok(())
    }

    pub fn scalar<'js>(ctx: &Ctx<'js>, declared: NativeType, value: &Value<'js>) -> Result<Self> {
        let name = declared.name();
        Ok(match declared {
            NativeType::I8 => Self::I8(integer(ctx, name, value)?),
            NativeType::U8 => Self::U8(integer(ctx, name, value)?),
            NativeType::I16 => Self::I16(integer(ctx, name, value)?),
            NativeType::U16 => Self::U16(integer(ctx, name, value)?),
            NativeType::I32 => Self::I32(integer(ctx, name, value)?),
            NativeType::U32 => Self::U32(integer(ctx, name, value)?),
            NativeType::I64 => Self::I64(big(ctx, name, value)?),
            NativeType::U64 => Self::U64(big(ctx, name, value)?),
            NativeType::Isize => Self::Isize(big(ctx, name, value)?),
            NativeType::Usize => Self::Usize(big(ctx, name, value)?),
            NativeType::F32 => Self::F32(narrowing(ctx, name, value)?),
            NativeType::F64 => Self::F64(float(ctx, name, value)?),
            // C's `_Bool` is one byte wide, and only `true`/`false` reach it:
            // a truthy string is not a boolean.
            NativeType::Bool => {
                Self::U8(u8::from(value.as_bool().ok_or_else(|| {
                    ErrorKind::BadArgument.throw(ctx, "expected a boolean for `bool`")
                })?))
            }
            NativeType::Pointer => Self::Pointer(Pointer::marshal(ctx, value)?),
            NativeType::Void => {
                return Err(
                    ErrorKind::Schema.throw(ctx, "`void` is a result type, not a parameter type")
                );
            }
        })
    }

    /// `Arg::new` passes the *address of* the cell, and libffi reads the
    /// declared width out of it. A `usize` and a `*mut c_void` have the same
    /// size and layout (asserted at [`CELL_BYTES`]), so a pointer argument is
    /// the address of the `usize` holding it, exactly as it would be the
    /// address of a pointer local.
    pub fn as_arg(&self) -> Arg<'_> {
        match self {
            Self::I8(value) => Arg::new(value),
            Self::U8(value) => Arg::new(value),
            Self::I16(value) => Arg::new(value),
            Self::U16(value) => Arg::new(value),
            Self::I32(value) => Arg::new(value),
            Self::U32(value) => Arg::new(value),
            Self::I64(value) => Arg::new(value),
            Self::U64(value) => Arg::new(value),
            Self::Isize(value) => Arg::new(value),
            // A pointer *is* a `usize` here, so these are one arm.
            Self::Usize(value) | Self::Pointer(value) => Arg::new(value),
            Self::F32(value) => Arg::new(value),
            Self::F64(value) => Arg::new(value),
        }
    }
}

/// A JS `number` that is exactly an integer, narrowed to the declared C width.
/// `1.5`, `2**31` as an `i32` and `NaN` are all refusals, never roundings.
pub fn integer<T: TryFrom<i64>>(ctx: &Ctx<'_>, declared: &str, value: &Value<'_>) -> Result<T> {
    let number = value.as_number().ok_or_else(|| {
        ErrorKind::BadArgument.throw(ctx, format_args!("expected a number for `{declared}`"))
    })?;
    if number.fract() != 0.0 || number.abs() > MAX_SAFE_INTEGER {
        return Err(ErrorKind::Range.throw(
            ctx,
            format_args!("{number} is not an exact integer for `{declared}`"),
        ));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "checked exact and inside the safe-integer range immediately above"
    )]
    T::try_from(number as i64).map_err(|_out_of_range| {
        ErrorKind::Range.throw(
            ctx,
            format_args!("{number} is out of range for `{declared}`"),
        )
    })
}

/// A JS `bigint`, through its decimal string.
///
/// ponytail: exact, and immune to both engine bugs of doc 09 fact 12 —
/// `BigInt::to_i64` silently returns `-1` for `2n**64n - 1n` and `0` for
/// `2n**70n`, and quickjs-ng's `BigInt.asUintN` is wrong for every bit width
/// that is a multiple of 32, so it cannot be the guard either. Swap for a limb
/// API only if a profile ever shows this next to a foreign call.
fn big<'js, T: FromStr>(ctx: &Ctx<'js>, declared: &str, value: &Value<'js>) -> Result<T> {
    if !value.is_big_int() {
        return Err(ErrorKind::BadArgument.throw(
            ctx,
            format_args!(
                "expected a bigint for `{declared}` — a number cannot hold every 64-bit value"
            ),
        ));
    }
    let text = Coerced::<String>::from_js(ctx, value.clone())?.0;
    T::from_str(&text).map_err(|_unparsable| {
        ErrorKind::Range.throw(ctx, format_args!("{text}n does not fit `{declared}`"))
    })
}

fn float(ctx: &Ctx<'_>, declared: &str, value: &Value<'_>) -> Result<f64> {
    value.as_number().ok_or_else(|| {
        ErrorKind::BadArgument.throw(ctx, format_args!("expected a number for `{declared}`"))
    })
}

/// `f32` loses precision silently, exactly as C does at the same call.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the declared C parameter is a float; narrowing is the conversion"
)]
fn narrowing(ctx: &Ctx<'_>, declared: &str, value: &Value<'_>) -> Result<f32> {
    Ok(float(ctx, declared, value)? as f32)
}

/// Call `address` and hand back the raw bytes of its result.
///
/// Raw rather than a JS value, because this is the one step a `nonblocking`
/// symbol performs on a worker thread that has no realm to build a value in;
/// [`read`] turns the cell into a `Value` back on the realm thread. The
/// synchronous path does both in a row.
///
/// # Safety
///
/// `cif`, `cells` and `declared` must all describe the same signature, and that
/// signature must be the one `address` really has. The latter is the caller's
/// contract (§5.1) and is checkable at no layer.
pub unsafe fn invoke(
    declared: NativeType, cif: &Cif, address: CodePtr, cells: &[ArgumentCell],
) -> Cell {
    let args: Vec<Arg<'_>> = cells.iter().map(ArgumentCell::as_arg).collect();
    let mut cell = Cell::default();
    // A `void` function has nowhere to write, and libffi wants to be told so
    // rather than handed a buffer it must not touch.
    let returned = if declared == NativeType::Void {
        Ret::void()
    } else {
        Ret::new(&mut cell.0)
    };
    // SAFETY: the caller's contract, restated on this function. The return
    // buffer is `CELL_BYTES` wide, which is at least libffi's documented
    // minimum and at least `declared.size()`, the number of bytes
    // `call_return_into` writes (§0 fact 3).
    unsafe { cif.call_return_into(address, &args, returned) };
    cell
}

/// Read one C value of `declared` out of `address` and build its JS value.
///
/// A returned cell, a static symbol and a callback's incoming argument all
/// land here, so the paths cannot drift apart. `library` is the provenance a
/// resulting `Pointer` carries; a callback argument has none, because den is
/// not told which library the address it is handed came from.
///
/// # Safety
///
/// `address` must point at an initialised, readable C object of the type
/// `declared` names. For a static symbol that the schema is describing
/// correctly is the caller's contract (§5.1).
pub unsafe fn read<'js>(
    ctx: &Ctx<'js>, declared: NativeType, address: *const u8, library: Option<&Rc<LoadedLibrary>>,
) -> Result<Value<'js>> {
    macro_rules! cell {
        ($ty:ty) => {
            // SAFETY: the caller's contract, restated on this function.
            // `read_unaligned` also covers a static whose alignment den has no
            // way to assert.
            unsafe { address.cast::<$ty>().read_unaligned() }
        };
    }
    macro_rules! number {
        ($ty:ty) => {
            Ok(Value::new_number(ctx.clone(), f64::from(cell!($ty))))
        };
    }

    match declared {
        NativeType::I8 => number!(i8),
        NativeType::U8 => number!(u8),
        NativeType::I16 => number!(i16),
        NativeType::U16 => number!(u16),
        NativeType::I32 => number!(i32),
        NativeType::U32 => number!(u32),
        NativeType::F32 => number!(f32),
        NativeType::F64 => Ok(Value::new_number(ctx.clone(), cell!(f64))),
        NativeType::I64 => Ok(BigInt::from_i64(ctx.clone(), cell!(i64))?.into_value()),
        NativeType::U64 => Ok(BigInt::from_u64(ctx.clone(), cell!(u64))?.into_value()),
        NativeType::Isize => Ok(BigInt::from_i64(ctx.clone(), cell!(isize) as i64)?.into_value()),
        NativeType::Usize => Ok(BigInt::from_u64(ctx.clone(), cell!(usize) as u64)?.into_value()),
        NativeType::Bool => Ok(Value::new_bool(ctx.clone(), cell!(u8) != 0)),
        NativeType::Pointer => {
            Pointer::to_js(ctx, cell!(*const c_void).expose_provenance(), library)
        }
        NativeType::Void => Ok(Value::new_undefined(ctx.clone())),
    }
}

/// Write the JS side's answer into a closure's return buffer.
///
/// libffi(3) on closures: the buffer is at least `sizeof(ffi_arg)` wide, and
/// an integral result narrower than that **must be widened** to `ffi_arg` —
/// the trampoline loads the whole register. Floats and everything already
/// register-wide are written as themselves.
///
/// # Safety
///
/// `out` must be the `result` buffer libffi handed the trampoline, and `cell`
/// must hold the type that closure's CIF declares as its result.
pub unsafe fn write_return(out: *mut c_void, cell: &ArgumentCell) {
    // SAFETY: the caller's contract, restated on this function. Each arm
    // writes at most `max(size_of::<ffi_arg>(), size of the declared result)`
    // bytes, which is the buffer libffi guarantees.
    unsafe {
        match cell {
            ArgumentCell::I8(value) => widen_signed(out, ffi_sarg::from(*value)),
            ArgumentCell::I16(value) => widen_signed(out, ffi_sarg::from(*value)),
            ArgumentCell::I32(value) => widen_signed(out, ffi_sarg::from(*value)),
            ArgumentCell::U8(value) => widen_unsigned(out, ffi_arg::from(*value)),
            ArgumentCell::U16(value) => widen_unsigned(out, ffi_arg::from(*value)),
            ArgumentCell::U32(value) => widen_unsigned(out, ffi_arg::from(*value)),
            ArgumentCell::I64(value) => out.cast::<i64>().write(*value),
            ArgumentCell::U64(value) => out.cast::<u64>().write(*value),
            ArgumentCell::Isize(value) => out.cast::<isize>().write(*value),
            ArgumentCell::Usize(value) | ArgumentCell::Pointer(value) => {
                out.cast::<usize>().write(*value)
            }
            ArgumentCell::F32(value) => out.cast::<f32>().write(*value),
            ArgumentCell::F64(value) => out.cast::<f64>().write(*value),
        }
    }
}

/// The zero of `declared`, which is what C is handed whenever JS could not
/// produce an answer — a throw, a panic, a callback reached from a thread this
/// phase cannot serve. Every one of those also writes a line to stderr: the
/// value is a lie, and a silent lie is worse than a loud one (§4.7).
///
/// # Safety
///
/// As [`write_return`].
pub const unsafe fn write_zero(out: *mut c_void, declared: NativeType) {
    // SAFETY: the caller's contract, restated on this function.
    unsafe {
        match declared {
            NativeType::Void => {}
            NativeType::F32 => out.cast::<f32>().write(0.0),
            NativeType::F64 => out.cast::<f64>().write(0.0),
            NativeType::Pointer => out.cast::<usize>().write(0),
            NativeType::I64 | NativeType::Isize => out.cast::<i64>().write(0),
            NativeType::U64 | NativeType::Usize => out.cast::<u64>().write(0),
            _narrower_than_a_register => widen_unsigned(out, 0),
        }
    }
}

/// # Safety
///
/// As [`write_return`].
const unsafe fn widen_signed(out: *mut c_void, value: ffi_sarg) {
    // SAFETY: the caller's contract. `ffi_sarg` is `ffi_arg`'s width, which is
    // the buffer's minimum size.
    unsafe { out.cast::<ffi_sarg>().write(value) }
}

/// # Safety
///
/// As [`write_return`].
const unsafe fn widen_unsigned(out: *mut c_void, value: ffi_arg) {
    // SAFETY: the caller's contract, as above.
    unsafe { out.cast::<ffi_arg>().write(value) }
}
