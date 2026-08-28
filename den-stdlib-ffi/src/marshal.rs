//! JS values in, C values out, and back — `docs/research/19-den-ffi.md` §4.4.
//!
//! Two rules carry most of the weight here. 64-bit integers cross through
//! their **decimal string**, never `BigInt::to_i64` (doc 09 fact 12). And a
//! result lands in [`Bytes`] rather than in a typed local, because
//! `call_return_into` corrects sub-register returns itself and writes exactly
//! `type.size()` bytes (§0 fact 3) into a buffer libffi requires to be at
//! least `ffi_arg`-wide — and because a struct returned through a hidden
//! `sret` pointer needs as many bytes as it has, not a register's worth.

use std::{ffi::c_void, rc::Rc, str::FromStr, sync::Arc};

use libffi::{
    low::{ffi_arg, ffi_sarg},
    middle::{Arg, Cif, CodePtr, Ret},
};
use rquickjs::{BigInt, Class, Coerced, Ctx, FromJs as _, Object, Result, TypedArray, Value, qjs};

use crate::{
    callback::Callback,
    error::ErrorKind,
    library::{LoadedLibrary, Mapped},
    pointer::Pointer,
    schema::{CallMode, NativeType, ParamType, StructLayout},
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

/// Owned bytes for one C value that a [`Cell`] cannot hold on its own: a
/// result libffi is about to write, or a struct argument den packs field by
/// field.
///
/// Eight-aligned, because it is [`Cell`]s underneath and eight is every
/// alignment den's vocabulary has — no field is wider than a pointer or a
/// `double`. `size` is the declared type's own size, which is what may be
/// copied out; the buffer itself is rounded up to whole cells and is never
/// smaller than one, because libffi documents a return buffer of at least
/// `ffi_arg` whatever the declared type's size is (§0 fact 3). Rounding up is
/// load-bearing on the argument side too: libffi reads a struct's registers an
/// eightbyte at a time (`ffi64.c:719`), which for a twelve-byte struct is a
/// read of the four bytes past it.
pub struct Bytes {
    cells: Vec<Cell>,
    size:  usize,
}

impl Bytes {
    pub fn for_type(declared: &NativeType) -> Self { Self::zeroed(declared.size()) }

    fn zeroed(size: usize) -> Self {
        Self {
            cells: vec![Cell::default(); size.max(CELL_BYTES).div_ceil(CELL_BYTES)],
            size,
        }
    }

    pub const fn as_ptr(&self) -> *const u8 { self.cells.as_ptr().cast() }

    const fn as_mut_ptr(&mut self) -> *mut u8 { self.cells.as_mut_ptr().cast() }

    /// The buffer libffi writes into, or reads a struct argument out of. It is
    /// handed over as a slice so that `Arg`/`Ret` take its address and nothing
    /// else — both accept a `?Sized` reference and keep only the data pointer.
    fn as_slice(&self) -> &[Cell] { &self.cells }
}

impl Cell {
    /// Copy one C value out of the buffer libffi is pointing at.
    ///
    /// # Safety
    ///
    /// `address` must point at an initialised, readable C value of the type
    /// `declared` names, and that type must fit a cell — every type but a
    /// struct, which is refused in a callback signature for this reason.
    pub unsafe fn read_from(declared: &NativeType, address: *const u8) -> Self {
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
    /// A struct by value: its fields already written at their computed offsets
    /// into den's own bytes. C is handed the address of these, and libffi does
    /// the register-or-memory placement the ABI calls for.
    Struct(Bytes),
}

/// An argument whose cell holds an *address*, kept alive so that den can take
/// that address again once the last JS of the call has run.
///
/// Marshalling any argument runs script — a struct field is an ordinary
/// property read, and so is the `resizable` check on a buffer — so an address
/// taken for argument N is only still true until argument N+1 is marshalled.
/// In between, JS can detach a buffer or dispose the library a pointer came
/// from, and libffi would then hand C an address den has already been told is
/// dead. [`Self::settle`] is the second pass that closes that window: it runs
/// after the whole argument list is marshalled and before anything else can
/// run (§4.6).
pub enum Borrowed<'js> {
    /// A `Uint8Array` whose store JS may detach, transfer or resize.
    Buffer(TypedArray<'js, u8>),
    /// A `Pointer` whose library JS may dispose.
    Pointer(Class<'js, Pointer>),
}

impl<'js> Borrowed<'js> {
    /// Take every borrowed address again, into the cells C is about to be
    /// handed. A buffer that is detached by now is `BadArgument` and a pointer
    /// whose library is closed by now is `Closed` — the same refusals the
    /// first pass makes, at the only moment they are guaranteed to still hold.
    ///
    /// Nothing here runs script: `as_raw` is `JS_GetTypedArrayBuffer` and a
    /// pointer's liveness is a Rust flag, so no fourth party can invalidate an
    /// argument between this and the call.
    pub fn settle(
        ctx: &Ctx<'js>, cells: &mut [ArgumentCell], borrowed: &[(usize, Self)],
    ) -> Result<()> {
        for (position, handle) in borrowed {
            let address = match handle {
                Self::Buffer(view) => ArgumentCell::address_of(ctx, view)?,
                Self::Pointer(pointer) => pointer.borrow().live(ctx)?,
            };
            // `position` came from enumerating these same cells, so the miss
            // is unreachable; refusing the call is still better than a panic
            // on a path JS can reach.
            *cells.get_mut(*position).ok_or_else(|| {
                ErrorKind::BadArgument.throw(ctx, "an argument went missing before the call")
            })? = ArgumentCell::Pointer(address);
        }
        Ok(())
    }
}

impl ArgumentCell {
    /// One argument's cell, plus the JS handle behind it when that cell is an
    /// address [`Borrowed::settle`] must take again.
    pub fn marshal<'js>(
        ctx: &Ctx<'js>, declared: &ParamType, value: &Value<'js>, site: CallSite<'_>,
        mapped: &mut Vec<Mapped>,
    ) -> Result<(Self, Option<Borrowed<'js>>)> {
        match declared {
            ParamType::Value(NativeType::Pointer) => {
                let (address, handle) = Pointer::marshal(ctx, value, mapped)?;
                Ok((Self::Pointer(address), handle.map(Borrowed::Pointer)))
            }
            ParamType::Value(declared) => Ok((Self::value(ctx, declared, value, mapped)?, None)),
            ParamType::Buffer => {
                let (address, view) = Self::buffer(ctx, value)?;
                Ok((Self::Pointer(address), Some(Borrowed::Buffer(view))))
            }
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
                Ok((
                    Self::Pointer(Callback::code_pointer(ctx, signature, value, site.origin)?),
                    None,
                ))
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
    /// The view itself comes back with the address, because the address alone
    /// is only true until the *next* argument's JS runs — [`Borrowed::settle`]
    /// takes it again once nothing more can run.
    fn buffer<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<(usize, TypedArray<'js, u8>)> {
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
        // The address is taken only now: the `resizable` read above is a JS
        // getter, and JS can detach a buffer — so an address read before that
        // check could already be dangling here.
        let address = Self::address_of(ctx, &view)?;
        Ok((address, view))
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
    /// Sharedness is asked of the engine, not of the realm:
    /// `JS_IsArrayBuffer` is `JS_GetClassID(obj) == JS_CLASS_ARRAY_BUFFER`
    /// (`quickjs.c:57843`), so neither a replaced
    /// `globalThis.SharedArrayBuffer` nor a `Symbol.hasInstance` of one's
    /// own can make a shared store read as unshared — which an `instanceof`
    /// test could.
    ///
    /// ponytail: `resizable` is still read through the prototype getter, and an
    /// own data property on the buffer shadows it. quickjs-ng exports no
    /// `max_byte_length` accessor, so the unspoofable version means reading a
    /// private struct field through `JS_GetAnyOpaque`. Spoofing it is not a
    /// memory-safety hole by itself: a store only moves when script runs, and
    /// no script runs between [`Borrowed::settle`] and the call — a re-entrant
    /// callback is refused for exactly that reason
    /// ([`crate::callback::InsideCall`]).
    fn refuse_unstable_store<'js>(ctx: &Ctx<'js>, view: &TypedArray<'js, u8>) -> Result<()> {
        let buffer = view.arraybuffer()?;
        // SAFETY: `buffer` is a live value of this realm, and `JS_IsArrayBuffer`
        // only reads its class id — it neither takes a reference nor runs JS,
        // so passing the borrowed raw value is enough.
        if !unsafe { qjs::JS_IsArrayBuffer(buffer.as_value().as_raw()) } {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                "a SharedArrayBuffer cannot back a `buffer` argument — another agent could write \
                 it while C holds the address",
            ));
        }
        let buffer = buffer.as_object();
        if buffer.get::<_, Value<'js>>("resizable")?.as_bool() == Some(true) {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                "a resizable ArrayBuffer cannot back a `buffer` argument — resizing moves the \
                 bytes out from under C",
            ));
        }
        Ok(())
    }

    /// One struct's own bytes, packed field by field at den's computed
    /// offsets. A field the object does not carry is a `Schema` error naming
    /// it: den will not invent a zero for something the caller forgot (§4.4).
    ///
    /// ponytail: a `pointer` field is packed as it is read, and reading a later
    /// field runs JS, so a nested address is not re-taken the way a top-level
    /// one is ([`Borrowed::settle`]). What holds instead is `mapped`: the share
    /// of the library taken here keeps the pages mapped for the whole call, so
    /// the worst a mid-marshal `Symbol.dispose` can do is deprive C of the
    /// `Closed` throw. Re-packing the bytes is what it would take to do better.
    fn structure<'js>(
        ctx: &Ctx<'js>, layout: &StructLayout, value: &Value<'js>, mapped: &mut Vec<Mapped>,
    ) -> Result<Bytes> {
        let object = value.as_object().ok_or_else(|| {
            ErrorKind::BadArgument.throw(ctx, "expected an object for a `struct` argument")
        })?;
        let mut bytes = Bytes::zeroed(layout.size);
        for field in &layout.fields {
            let declared = object
                .get::<_, Option<Value<'js>>>(field.name.as_str())?
                .ok_or_else(|| {
                    ErrorKind::Schema.throw_for(
                        ctx,
                        format_args!("this struct has no `{}` field", field.name),
                        &field.name,
                    )
                })?;
            let cell = Self::value(ctx, &field.declared, &declared, mapped)?;
            // SAFETY: `offset + size` of every field is at most `layout.size`
            // by construction, and `bytes` is that many bytes of den's own
            // memory.
            unsafe { cell.write_field(bytes.as_mut_ptr().add(field.offset)) };
        }
        Ok(bytes)
    }

    /// `mapped` collects a share of the library behind every `pointer` this
    /// reads, so that a `Symbol.dispose` cannot unmap an address C is already
    /// holding. A callback's *return* value passes a scratch vector: den's
    /// involvement with that address ends as the trampoline returns.
    pub fn value<'js>(
        ctx: &Ctx<'js>, declared: &NativeType, value: &Value<'js>, mapped: &mut Vec<Mapped>,
    ) -> Result<Self> {
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
            NativeType::Pointer => Self::Pointer(Pointer::marshal(ctx, value, mapped)?.0),
            NativeType::Struct(layout) => {
                Self::Struct(Self::structure(ctx, layout, value, mapped)?)
            }
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
            // A struct is passed by the address of its bytes, and libffi is
            // what turns that into registers or a copy on the stack.
            Self::Struct(bytes) => Arg::new(bytes.as_slice()),
        }
    }

    /// Write this value into a struct's own bytes, at the natural width of its
    /// declared type.
    ///
    /// Deliberately *not* [`write_return`]'s widening: that one fills a
    /// closure's register-wide return buffer, and doing it to a struct field
    /// would write over the field next door.
    ///
    /// # Safety
    ///
    /// `out` must be writable for this value's declared size, inside a buffer
    /// den owns.
    const unsafe fn write_field(&self, out: *mut u8) {
        // SAFETY: the caller's contract, restated on this function. Each arm
        // writes exactly the declared type's own width.
        unsafe {
            match self {
                Self::I8(value) => out.cast::<i8>().write_unaligned(*value),
                Self::U8(value) => out.write_unaligned(*value),
                Self::I16(value) => out.cast::<i16>().write_unaligned(*value),
                Self::U16(value) => out.cast::<u16>().write_unaligned(*value),
                Self::I32(value) => out.cast::<i32>().write_unaligned(*value),
                Self::U32(value) => out.cast::<u32>().write_unaligned(*value),
                Self::I64(value) => out.cast::<i64>().write_unaligned(*value),
                Self::U64(value) => out.cast::<u64>().write_unaligned(*value),
                Self::Isize(value) => out.cast::<isize>().write_unaligned(*value),
                Self::Usize(value) | Self::Pointer(value) => {
                    out.cast::<usize>().write_unaligned(*value)
                }
                Self::F32(value) => out.cast::<f32>().write_unaligned(*value),
                Self::F64(value) => out.cast::<f64>().write_unaligned(*value),
                Self::Struct(bytes) => {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.size);
                }
            }
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
    declared: &NativeType, cif: &Cif, address: CodePtr, cells: &[ArgumentCell],
) -> Bytes {
    let args: Vec<Arg<'_>> = cells.iter().map(ArgumentCell::as_arg).collect();
    let mut cell = Bytes::for_type(declared);
    // A `void` function has nowhere to write, and libffi wants to be told so
    // rather than handed a buffer it must not touch.
    let returned = if *declared == NativeType::Void {
        Ret::void()
    } else {
        Ret::new(cell.cells.as_mut_slice())
    };
    // SAFETY: the caller's contract, restated on this function. The return
    // buffer is at least `CELL_BYTES` wide and at least `declared.size()` —
    // libffi's documented minimum, and the number of bytes `call_return_into`
    // writes (§0 fact 3). A struct large enough to be returned through a
    // hidden `sret` pointer is written into these same bytes.
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
    ctx: &Ctx<'js>, declared: &NativeType, address: *const u8, library: Option<&Rc<LoadedLibrary>>,
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
        // A fresh object per read: no JS value ever aliases the C bytes.
        NativeType::Struct(layout) => {
            let object = Object::new(ctx.clone())?;
            for field in &layout.fields {
                // SAFETY: the caller's contract — `address` is a struct of
                // this layout, so each field's own offset is inside it and
                // holds a value of the type the field declares.
                let value =
                    unsafe { read(ctx, &field.declared, address.add(field.offset), library)? };
                object.set(field.name.as_str(), value)?;
            }
            Ok(object.into_value())
        }
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
            // Unreachable while a callback may not return a struct (§4.2), and
            // written correctly rather than as a panic, because a panic here
            // is an abort (§0 fact 7).
            ArgumentCell::Struct(bytes) => {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.size);
            }
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
pub unsafe fn write_zero(out: *mut c_void, declared: &NativeType) {
    // SAFETY: the caller's contract, restated on this function.
    unsafe {
        match declared {
            NativeType::Void => {}
            NativeType::F32 => out.cast::<f32>().write(0.0),
            NativeType::F64 => out.cast::<f64>().write(0.0),
            NativeType::Pointer => out.cast::<usize>().write(0),
            NativeType::I64 | NativeType::Isize => out.cast::<i64>().write(0),
            NativeType::U64 | NativeType::Usize => out.cast::<u64>().write(0),
            // As [`write_return`]: unreachable today, and correct anyway.
            NativeType::Struct(layout) => out.cast::<u8>().write_bytes(0, layout.size),
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
