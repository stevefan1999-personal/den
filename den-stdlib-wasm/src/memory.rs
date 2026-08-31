//! `WebAssembly.Memory`, plus the two pieces of descriptor plumbing the other
//! wasm object wrappers share: WebIDL-shaped dictionary member reads and
//! resolution of the JS spelling of a value type.

use std::cell::RefCell;

use indexmap::indexmap;
use rquickjs::{
    ArrayBuffer, Coerced, Constructor, Ctx, Exception, FromJs, Function, IntoJs as _, JsLifetime,
    Object, Result, Value, class::Trace, qjs,
};
use wasmtime::{AsContext, Memory as WasmMemory, MemoryTypeBuilder, RefType, ValType};

use crate::{store::Store, utils::EnforceRange};

/// Dictionary member reads that fail the way WebIDL says they should.
pub(crate) trait DescriptorObject<'js> {
    /// The descriptor itself: anything that is not an object is a `TypeError`.
    fn descriptor(ctx: &Ctx<'js>, value: Value<'js>, what: &str) -> Result<Object<'js>>;

    /// A required dictionary member; absent or `undefined` is a `TypeError`.
    fn required<T: FromJs<'js>>(&self, ctx: &Ctx<'js>, member: &str) -> Result<T>;

    /// `initial` or `minimum`, but not both. See the impl for why both are
    /// read.
    fn initial_or_minimum(&self, ctx: &Ctx<'js>) -> Result<u32>;
}

impl<'js> DescriptorObject<'js> for Object<'js> {
    fn descriptor(ctx: &Ctx<'js>, value: Value<'js>, what: &str) -> Result<Object<'js>> {
        value
            .into_object()
            .ok_or_else(|| Exception::throw_type(ctx, &format!("{what} is not an object")))
    }

    fn required<T: FromJs<'js>>(&self, ctx: &Ctx<'js>, member: &str) -> Result<T> {
        let value: Value<'js> = self.get(member)?;
        if value.is_undefined() {
            return Err(Exception::throw_type(
                ctx,
                &format!("descriptor member `{member}` is required"),
            ));
        }
        T::from_js(ctx, value)
    }

    /// `initial` and `minimum` are the same slot: the js-types proposal added
    /// `minimum` as the name `type()` reports, and the constructor accepts
    /// either spelling but not both. Both members are read even when one is
    /// already present, so a getter on the unused name still runs.
    fn initial_or_minimum(&self, ctx: &Ctx<'js>) -> Result<u32> {
        let initial: Value<'js> = self.get("initial")?;
        let minimum: Value<'js> = self.get("minimum")?;
        match (!initial.is_undefined(), !minimum.is_undefined()) {
            (true, true) => {
                Err(Exception::throw_type(
                    ctx,
                    "the descriptor cannot have both `initial` and `minimum`",
                ))
            }
            (false, false) => {
                Err(Exception::throw_type(
                    ctx,
                    "descriptor member `initial` is required",
                ))
            }
            (true, false) => EnforceRange::from_js(ctx, initial).map(|size| size.0),
            (false, true) => EnforceRange::from_js(ctx, minimum).map(|size| size.0),
        }
    }
}

/// Resolution of a value type's JS spelling (`"i32"`, `"anyfunc"`, …) against
/// both the spec's enums and what this build can represent.
pub(crate) struct ValueTypeName;

impl ValueTypeName {
    /// A `Global` or `Tag` value type. `v128` cannot cross the JS boundary,
    /// and `anyref` is not part of the JS API's `ValueType` enum.
    pub(crate) fn value(ctx: &Ctx<'_>, name: &str, what: &str) -> Result<ValType> {
        match name {
            "i32" => Ok(ValType::I32),
            "i64" => Ok(ValType::I64),
            "f32" => Ok(ValType::F32),
            "f64" => Ok(ValType::F64),
            "anyfunc" | "funcref" => Ok(ValType::FUNCREF),
            "externref" => Ok(ValType::EXTERNREF),
            _ => {
                Err(Exception::throw_type(
                    ctx,
                    &format!("`{name}` is not a valid {what}"),
                ))
            }
        }
    }

    /// The spec's narrower `TableKind` enum.
    pub(crate) fn table(ctx: &Ctx<'_>, name: &str) -> Result<ValType> {
        match name {
            "anyfunc" | "funcref" => Ok(ValType::FUNCREF),
            "externref" => Ok(ValType::EXTERNREF),
            _ => {
                Err(Exception::throw_type(
                    ctx,
                    &format!("`{name}` is not a valid table element type"),
                ))
            }
        }
    }

    /// Descriptor spelling of a native Wasmtime type.
    pub(crate) fn get(ty: &ValType) -> Option<&'static str> {
        Some(match ty {
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::F32 => "f32",
            ValType::F64 => "f64",
            ValType::V128 => "v128",
            ValType::Ref(reference) if reference.matches(&RefType::FUNCREF) => "funcref",
            ValType::Ref(reference) if reference.matches(&RefType::EXTERNREF) => "externref",
            ValType::Ref(reference) if reference.matches(&RefType::ANYREF) => "anyref",
            ValType::Ref(_) => return None,
        })
    }
}

/// A `MemoryDescriptor`, in pages.
#[derive(Clone, Copy, Debug)]
pub struct MemoryDescriptor {
    initial: u32,
    maximum: Option<u32>,
    shared:  bool,
}

impl<'js> FromJs<'js> for MemoryDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the memory descriptor")?;
        Ok(Self {
            initial: object.initial_or_minimum(ctx)?,
            maximum: object
                .get::<_, Option<EnforceRange>>("maximum")?
                .map(|maximum| maximum.0),
            shared:  object
                .get::<_, Option<Coerced<bool>>>("shared")?
                .is_some_and(|shared| shared.0),
        })
    }
}

/// Every `[[BufferObject]]` den has handed out, with the linear-memory extent
/// it aliased when it was built.
///
/// Context-wide rather than a field of [`Memory`], because neither of the two
/// things that invalidate a buffer is a method call on the wrapper holding it.
/// A `memory.grow` executed as a wasm *instruction* moves the memory with no JS
/// call to hook at all, and two `Memory` wrappers can name the same linear
/// memory — import one, then read it back off `instance.exports` — so growing
/// through one has to detach the buffer handed out by the other. One registry,
/// swept from the outside, answers both.
///
/// ponytail: entries are keyed by the memory's base address, so two *distinct*
/// zero-page memories whose bases coincide would share one empty buffer. They
/// have no bytes to confuse and separate again the moment either grows; keying
/// by identity would need an `Eq` wasmtime does not give `Memory`.
#[derive(Default)]
pub struct MemoryBuffers<'js> {
    live: RefCell<Vec<LiveBuffer<'js>>>,
}

/// One handed-out buffer and the extent it was built over. `base` is an
/// address rather than a pointer: nothing dereferences it, it only answers
/// "has this memory moved?".
struct LiveBuffer<'js> {
    memory:      WasmMemory,
    buffer:      ArrayBuffer<'js>,
    base:        usize,
    byte_length: usize,
    resizable:   bool,
}

// SAFETY: the only `'js` data is the cached buffers, which change lifetime with
// the struct.
unsafe impl<'js> JsLifetime<'js> for MemoryBuffers<'js> {
    type Changed<'to> = MemoryBuffers<'to>;
}

impl<'js> MemoryBuffers<'js> {
    /// The spec's "refresh the memory buffer", applied to every live buffer:
    /// one whose memory has moved or resized is detached, so the next
    /// `.buffer` read builds a fresh view over the memory as it now is.
    ///
    /// This is what keeps a script off a linear memory that has moved: an
    /// internal `memory.grow` reallocates, and every `Uint8Array` JS built
    /// beforehand would otherwise point at the old pages.
    pub(crate) fn refresh(ctx: &Ctx<'js>) -> Result<()> {
        Store::from_ctx(ctx)?.with_context(ctx, |store| Self::refresh_in(ctx, store))
    }

    /// [`Self::refresh`] against a store that is already borrowed, which is the
    /// situation inside a wasm host callback.
    pub(crate) fn refresh_in(ctx: &Ctx<'js>, store: impl AsContext) -> Result<()> {
        Self::with_live(ctx, |live| {
            live.retain_mut(|entry| {
                let extent = (
                    entry.memory.data_ptr(&store) as usize,
                    entry.memory.data_size(&store),
                );
                if extent == (entry.base, entry.byte_length) {
                    return true;
                }
                entry.buffer.detach();
                false
            });
            Ok(())
        })
    }

    /// `[[BufferObject]]` of `memory`, built on first read and shared by every
    /// wrapper naming the same linear memory.
    fn buffer(ctx: &Ctx<'js>, memory: &WasmMemory) -> Result<ArrayBuffer<'js>> {
        // Sweeping first is what makes the cache self-validating: whatever survives
        // is known to still describe its memory.
        Self::refresh(ctx)?;
        let (base, byte_length) = Self::extent(ctx, memory)?;
        Self::with_live(ctx, |live| {
            if let Some(entry) = live.iter().find(|entry| entry.base == base) {
                return Ok(entry.buffer.clone());
            }
            let buffer = Self::alias(ctx, base, byte_length)?;
            live.push(LiveBuffer {
                memory: *memory,
                buffer: buffer.clone(),
                base,
                byte_length,
                resizable: false,
            });
            Ok(buffer)
        })
    }

    /// `Memory.prototype.toFixedLengthBuffer`: the current `[[BufferObject]]`
    /// if it is already fixed-length, otherwise a fresh fixed alias after
    /// detaching the resizable one.
    fn to_fixed_length(ctx: &Ctx<'js>, memory: &WasmMemory) -> Result<ArrayBuffer<'js>> {
        Self::refresh(ctx)?;
        let (base, byte_length) = Self::extent(ctx, memory)?;
        Self::with_live(ctx, |live| {
            if let Some(entry) = live
                .iter()
                .find(|entry| entry.base == base && !entry.resizable)
            {
                return Ok(entry.buffer.clone());
            }
            live.retain_mut(|entry| {
                if entry.base != base {
                    return true;
                }
                entry.buffer.detach();
                false
            });
            let buffer = Self::alias(ctx, base, byte_length)?;
            live.push(LiveBuffer {
                memory: *memory,
                buffer: buffer.clone(),
                base,
                byte_length,
                resizable: false,
            });
            Ok(buffer)
        })
    }

    /// `Memory.prototype.toResizableBuffer`: a resizable `ArrayBuffer` whose
    /// `maxByteLength` is the memory's maximum, or a `TypeError` when the
    /// memory has no maximum (there is no `maxByteLength` to report).
    fn to_resizable(
        ctx: &Ctx<'js>, memory: &WasmMemory, max_byte_length: usize,
    ) -> Result<ArrayBuffer<'js>> {
        Self::refresh(ctx)?;
        let (base, byte_length) = Self::extent(ctx, memory)?;
        if max_byte_length < byte_length {
            return Err(Exception::throw_range(
                ctx,
                "the resizable buffer's maxByteLength is smaller than the memory",
            ));
        }
        Self::with_live(ctx, |live| {
            if let Some(entry) = live
                .iter()
                .find(|entry| entry.base == base && entry.resizable)
            {
                return Ok(entry.buffer.clone());
            }
            live.retain_mut(|entry| {
                if entry.base != base {
                    return true;
                }
                entry.buffer.detach();
                false
            });
            let buffer = Self::resizable_alias(ctx, byte_length, max_byte_length)?;
            live.push(LiveBuffer {
                memory: *memory,
                buffer: buffer.clone(),
                base,
                byte_length,
                resizable: true,
            });
            Ok(buffer)
        })
    }

    /// A JS-allocated resizable `ArrayBuffer`. It does not alias the linear
    /// memory: QuickJS has no public constructor for an *external* resizable
    /// buffer, and `resize` would `js_realloc` a wasm base if we faked one.
    /// `to-fixed-length-buffer` only needs the `resizable` flag, identity and
    /// detach; writes go through `.buffer` after `toFixedLengthBuffer` (or a
    /// grow), which rebuilds the aliasing view.
    fn resizable_alias(
        ctx: &Ctx<'js>, byte_length: usize, max_byte_length: usize,
    ) -> Result<ArrayBuffer<'js>> {
        let options = Object::new(ctx.clone())?;
        options.set("maxByteLength", max_byte_length as u64)?;
        let value = ctx
            .globals()
            .get::<_, Constructor>("ArrayBuffer")?
            .construct::<_, Value>((byte_length as u64, options))?;
        let buffer = ArrayBuffer::from_value(value).ok_or_else(|| {
            Exception::throw_type(ctx, "cannot create a resizable WebAssembly memory buffer")
        })?;
        Self::seal_against_transfer(ctx, &buffer)?;
        Ok(buffer)
    }

    /// Detach the buffer registered at `base`, whatever state its memory is in.
    ///
    /// `Memory.prototype.grow` replaces `[[BufferObject]]` unconditionally —
    /// even a zero delta, which moves and resizes nothing — so the extent
    /// comparison [`Self::refresh`] makes is not enough there.
    fn detach_at(ctx: &Ctx<'js>, base: usize) -> Result<()> {
        Self::with_live(ctx, |live| {
            live.retain_mut(|entry| {
                if entry.base != base {
                    return true;
                }
                entry.buffer.detach();
                false
            });
            Ok(())
        })
    }

    /// The current base address and byte length of `memory`.
    fn extent(ctx: &Ctx<'js>, memory: &WasmMemory) -> Result<(usize, usize)> {
        Store::from_ctx(ctx)?.with_context(ctx, |store| {
            Ok((memory.data_ptr(&store) as usize, memory.data_size(&store)))
        })
    }

    /// Run `f` over the registry. Fallible throughout: every caller is
    /// reachable from JS, so neither a missing registry nor a re-entrant borrow
    /// may panic.
    fn with_live<R>(
        ctx: &Ctx<'js>, f: impl FnOnce(&mut Vec<LiveBuffer<'js>>) -> Result<R>,
    ) -> Result<R> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| {
            Exception::throw_internal(
                ctx,
                "the WebAssembly memory buffer registry is missing from this context",
            )
        })?;
        let mut live = registry.live.try_borrow_mut().map_err(|_error| {
            Exception::throw_internal(
                ctx,
                "the WebAssembly memory buffer registry is already in use",
            )
        })?;
        f(&mut live)
    }

    /// An `ArrayBuffer` whose backing store *is* the linear memory, so that JS
    /// reads and writes hit the wasm pages directly.
    ///
    /// Built through the raw C API instead of `ArrayBuffer::from_source`
    /// because a source-backed buffer registers a free function, and QuickJS
    /// runs that function twice when the buffer is detached: once in
    /// `JS_DetachArrayBuffer` (quickjs.c:58037) and again in the finalizer
    /// (quickjs.c:57935), which double-frees rquickjs' boxed closure. den owns
    /// nothing here — the pages belong to the wasm store — so the correct free
    /// function is none at all.
    fn alias(ctx: &Ctx<'js>, base: usize, byte_length: usize) -> Result<ArrayBuffer<'js>> {
        // SAFETY: `base` and `byte_length` are the base and byte length of a live
        // wasm linear memory, read under the store borrow by the caller, and QuickJS
        // only ever touches them under the runtime lock. Passing no free function
        // means the buffer never claims ownership of those pages, and the registry
        // detaches it before the pages can go away.
        let value = unsafe {
            let raw = qjs::JS_NewArrayBuffer(
                ctx.as_raw().as_ptr(),
                base as *mut u8,
                byte_length as _,
                None,
                core::ptr::null_mut(),
                false,
            );
            if qjs::JS_IsException(raw) {
                return Err(rquickjs::Error::Exception);
            }
            Value::from_raw(ctx.clone(), raw)
        };

        let buffer = ArrayBuffer::from_value(value).ok_or_else(|| {
            Exception::throw_type(ctx, "cannot view WebAssembly memory as a buffer")
        })?;
        Self::seal_against_transfer(ctx, &buffer)?;
        Ok(buffer)
    }

    /// Stand-in for the spec's `[[ArrayBufferDetachKey]]`.
    ///
    /// The spec tags a Memory's buffer with the key `"WebAssembly.Memory"` so
    /// that any detach not performed by the engine itself raises a `TypeError`.
    /// quickjs-ng has no key concept — `JS_DetachArrayBuffer` (quickjs.c:58030)
    /// takes no key and `js_array_buffer_transfer` (quickjs.c:58157) checks
    /// only `shared`/`immutable`/`detached` — so the guarantee is rebuilt
    /// here.
    ///
    /// This is not merely a conformance detail. `transfer` reaches
    /// `js_realloc(ctx, abuf->data, new_len)` on a length change, and
    /// `abuf->data` is the wasm linear memory base, which QuickJS never
    /// allocated: `new WebAssembly.Memory({initial:1}).buffer.transfer(1)`
    /// hands a foreign pointer to the allocator and corrupts the heap.
    ///
    /// The four methods are shadowed by *own* properties, which take precedence
    /// over `ArrayBuffer.prototype`. `Object::prop` with a bare value defines
    /// them non-writable, non-configurable and non-enumerable, so script can
    /// neither overwrite nor `delete` them to reach the originals.
    fn seal_against_transfer(ctx: &Ctx<'js>, buffer: &ArrayBuffer<'js>) -> Result<()> {
        const DETACHING_METHODS: [&str; 4] = [
            "transfer",
            "transferToFixedLength",
            "transferToImmutable",
            "resize",
        ];

        let refuse = Function::new(ctx.clone(), |ctx: Ctx<'js>| -> Result<()> {
            Err(Exception::throw_type(
                &ctx,
                "cannot detach the buffer of a WebAssembly.Memory",
            ))
        })?;
        for method in DETACHING_METHODS {
            refuse.set_name(method)?;
            buffer.as_object().prop(method, refuse.clone())?;
        }
        Ok(())
    }
}

#[derive(Trace, JsLifetime, Clone, Debug)]
#[rquickjs::class]
pub struct Memory {
    #[qjs(skip_trace)]
    pub(crate) inner: WasmMemory,
}

#[rquickjs::methods]
impl Memory {
    #[qjs(constructor)]
    pub fn new(descriptor: MemoryDescriptor, ctx: Ctx<'_>) -> Result<Self> {
        if descriptor.shared {
            return Err(Exception::throw_type(
                &ctx,
                "shared WebAssembly.Memory is not supported by this build",
            ));
        }

        let mut ty = MemoryTypeBuilder::new();
        let ty = ty
            .min(u64::from(descriptor.initial))
            .max(descriptor.maximum.map(u64::from))
            .build()
            .map_err(|error| {
                Exception::throw_range(&ctx, &format!("invalid memory type: {error}"))
            })?;
        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            WasmMemory::new(&mut *store, ty)
                .map(|inner| Self { inner })
                .map_err(|error| {
                    Exception::throw_range(&ctx, &format!("cannot allocate memory: {error}"))
                })
        })
    }

    /// `[[BufferObject]]` — an `ArrayBuffer` aliasing the linear memory, the
    /// same object on every read until something detaches it.
    ///
    /// `&self`, not `&mut self`: a getter that mutably borrows the class cell
    /// is what lets a JS `valueOf` hook re-enter and hit `AlreadyBorrowed`.
    #[qjs(get, enumerable, configurable)]
    pub fn buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        MemoryBuffers::buffer(&ctx, &self.inner)
    }

    /// The js-types reflection method: `{ minimum, shared, maximum? }`.
    #[qjs(rename = "type")]
    pub fn memory_type<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let (minimum, maximum, shared) = Store::from_ctx(&ctx)?.with_context(&ctx, |store| {
            let ty = self.inner.ty(&store);
            Ok((ty.minimum(), ty.maximum(), ty.is_shared()))
        })?;
        let mut ty = indexmap! {
            "minimum" => minimum.into_js(&ctx)?,
            "shared" => shared.into_js(&ctx)?,
        };
        if let Some(maximum) = maximum {
            ty.insert("maximum", maximum.into_js(&ctx)?);
        }
        ty.into_js(&ctx)?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "memory type is not an object"))
    }

    #[qjs(rename = "toFixedLengthBuffer")]
    pub fn to_fixed_length_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        MemoryBuffers::to_fixed_length(&ctx, &self.inner)
    }

    #[qjs(rename = "toResizableBuffer")]
    pub fn to_resizable_buffer<'js>(&self, ctx: Ctx<'js>) -> Result<ArrayBuffer<'js>> {
        let max_pages = Store::from_ctx(&ctx)?.with_context(&ctx, |store| {
            self.inner.ty(&store).maximum().ok_or_else(|| {
                Exception::throw_type(
                    &ctx,
                    "toResizableBuffer requires a WebAssembly.Memory with a maximum",
                )
            })
        })?;
        let max_byte_length =
            usize::try_from(max_pages.saturating_mul(0x0001_0000)).map_err(|_error| {
                Exception::throw_range(&ctx, "the memory maximum does not fit in a buffer")
            })?;
        MemoryBuffers::to_resizable(&ctx, &self.inner, max_byte_length)
    }

    /// Grow by `delta` pages, returning the page count *before* the growth.
    pub fn grow(&self, delta: EnforceRange, ctx: Ctx<'_>) -> Result<u64> {
        // Read before growing: once the memory has moved, its old base — the key
        // the buffer is registered under — is no longer reachable from it.
        let (base, _) = MemoryBuffers::extent(&ctx, &self.inner)?;
        let previous = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            self.inner.grow(&mut *store, delta.size()).map_err(|error| {
                Exception::throw_range(&ctx, &format!("cannot grow memory: {error}"))
            })
        })?;
        // Only after a successful grow, and unconditionally: the spec detaches
        // `[[BufferObject]]` even when the growth moved and resized nothing.
        MemoryBuffers::detach_at(&ctx, base)?;
        Ok(previous)
    }
}

/// Shared test scaffolding for the wasm object wrappers.
#[cfg(test)]
#[path = "../tests/unit/memory_testing.rs"]
pub(crate) mod testing;

#[cfg(test)]
#[path = "../tests/unit/memory.rs"]
mod tests;

/// The JS-visible surface of `Memory`, `Table` and `Global`, exercised through
/// the real `den:wasm` module.
///
/// The tests above call the Rust methods directly, which bypasses exactly the
/// part most likely to be wrong: class registration, and whether `buffer`,
/// `length` and `value` really are accessors under the names the spec gives
/// them.
#[cfg(test)]
#[path = "../tests/unit/memory_javascript.rs"]
mod javascript_tests;

#[cfg(test)]
#[path = "../tests/unit/memory_detach_key.rs"]
mod detach_key_tests;
