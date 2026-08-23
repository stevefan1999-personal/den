//! `WebAssembly.Memory`, plus the two pieces of descriptor plumbing the other
//! wasm object wrappers share: WebIDL-shaped dictionary member reads and
//! resolution of the JS spelling of a value type.

use std::cell::RefCell;

use indexmap::indexmap;
use rquickjs::{
    ArrayBuffer, Coerced, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Object, Result,
    Value, class::Trace, qjs,
};
use wasmtime::{AsContext, Memory as WasmMemory, ValType};

use crate::{
    backend::{self, ValKind},
    store::Store,
    utils::EnforceRange,
};

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
    /// The types a `Global` or a `Tag` parameter may have: the spec's
    /// `ValueType` enum, minus `v128`, whose values cannot cross into JS at
    /// all, and without `anyref`, which is not in that enum however much it
    /// looks like it should be.
    pub(crate) const GLOBAL: &'static [ValKind] = &[
        ValKind::I32,
        ValKind::I64,
        ValKind::F32,
        ValKind::F64,
        ValKind::ExternRef,
        ValKind::FuncRef,
    ];
    /// The spec's `TableKind`.
    pub(crate) const TABLE_ELEMENT: &'static [ValKind] = &[ValKind::FuncRef, ValKind::ExternRef];

    /// `ToValueType(name)`, restricted to `allowed`; `what` names the position
    /// the type appears in, for the error message.
    pub(crate) fn resolve(
        ctx: &Ctx<'_>, name: &str, allowed: &[ValKind], what: &str,
    ) -> Result<ValType> {
        let kind = ValKind::parse(name)
            .filter(|kind| allowed.contains(kind))
            .ok_or_else(|| {
                Exception::throw_type(ctx, &format!("`{name}` is not a valid {what}"))
            })?;
        backend::val_type_from_kind(kind).ok_or_else(|| {
            Exception::throw_type(
                ctx,
                &format!(
                    "the {} type is not representable in this build",
                    kind.name()
                ),
            )
        })
    }
}

/// A `MemoryDescriptor`, in pages.
#[derive(Clone, Copy, Debug)]
pub struct MemoryDescriptor {
    initial: u64,
    maximum: Option<u64>,
    shared:  bool,
}

impl<'js> FromJs<'js> for MemoryDescriptor {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let object = Object::descriptor(ctx, value, "the memory descriptor")?;
        Ok(Self {
            initial: u64::from(object.initial_or_minimum(ctx)?),
            maximum: object
                .get::<_, Option<EnforceRange>>("maximum")?
                .map(|maximum| u64::from(maximum.0)),
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
        Store::from_ctx(ctx)?.with_mut(ctx, |store| Self::refresh_in(ctx, &*store))
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
        let make: Function =
            ctx.eval("(length, maxByteLength) => new ArrayBuffer(length, { maxByteLength })")?;
        let value: Value = make.call((byte_length as u64, max_byte_length as u64))?;
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
        Store::from_ctx(ctx)?.with_mut(ctx, |store| {
            Ok((memory.data_ptr(&*store) as usize, memory.data_size(&*store)))
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
        let mut live = registry.live.try_borrow_mut().map_err(|_| {
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
    /// Requested `shared` from the descriptor. wasmtime cannot allocate a
    /// shared memory we can alias as `SharedArrayBuffer`, so the engine
    /// memory is always unshared; `type()` still reports what JS asked for.
    shared:           bool,
}

impl From<WasmMemory> for Memory {
    fn from(inner: WasmMemory) -> Self {
        Self {
            inner,
            shared: false,
        }
    }
}

#[rquickjs::methods]
impl Memory {
    #[qjs(constructor)]
    pub fn new(descriptor: MemoryDescriptor, ctx: Ctx<'_>) -> Result<Self> {
        if descriptor.shared && descriptor.maximum.is_none() {
            return Err(Exception::throw_type(
                &ctx,
                "a shared WebAssembly.Memory requires a maximum size",
            ));
        }

        // Always allocate an unshared wasmtime memory: a shared type is refused
        // by `Memory::new`, and den cannot alias one as a SharedArrayBuffer.
        // `type()` still reports the requested `shared` flag.
        let ty = backend::new_memory_type(descriptor.initial, descriptor.maximum, false).map_err(
            |error| Exception::throw_range(&ctx, &format!("invalid memory type: {error}")),
        )?;

        Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            WasmMemory::new(&mut *store, ty)
                .map(|inner| {
                    Self {
                        inner,
                        shared: descriptor.shared,
                    }
                })
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
        let (minimum, maximum) = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            let ty = self.inner.ty(&*store);
            Ok((ty.minimum(), ty.maximum()))
        })?;
        let mut ty = indexmap! {
            "minimum" => minimum.into_js(&ctx)?,
            "shared" => self.shared.into_js(&ctx)?,
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
        let max_pages = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            self.inner.ty(&*store).maximum().ok_or_else(|| {
                Exception::throw_type(
                    &ctx,
                    "toResizableBuffer requires a WebAssembly.Memory with a maximum",
                )
            })
        })?;
        let max_byte_length = usize::try_from(max_pages.saturating_mul(65536)).map_err(|_| {
            Exception::throw_range(&ctx, "the memory maximum does not fit in a buffer")
        })?;
        MemoryBuffers::to_resizable(&ctx, &self.inner, max_byte_length)
    }

    /// Grow by `delta` pages, returning the page count *before* the growth.
    pub fn grow(&self, delta: Coerced<u64>, ctx: Ctx<'_>) -> Result<u64> {
        // Read before growing: once the memory has moved, its old base — the key
        // the buffer is registered under — is no longer reachable from it.
        let (base, _) = MemoryBuffers::extent(&ctx, &self.inner)?;
        let previous = Store::from_ctx(&ctx)?.with_mut(&ctx, |store| {
            self.inner.grow(&mut *store, delta.0).map_err(|error| {
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
pub(crate) mod testing {
    use rquickjs::{Context, Ctx, Object, Runtime, Value};

    use crate::{backend, error::WebAssemblyErrors, memory::MemoryBuffers, store::Store};

    /// A fresh runtime whose context carries everything the wrappers expect:
    /// the shared wasm store and the three WebAssembly error classes.
    pub(crate) fn with_wasm_context<R>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let engine = backend::new_engine().expect("engine");
            ctx.store_userdata(Store::new(&engine, &ctx))
                .expect("store userdata");
            ctx.store_userdata(MemoryBuffers::default())
                .expect("memory buffer registry userdata");
            let namespace = Object::new(ctx.clone()).expect("namespace");
            WebAssemblyErrors::install(&ctx, &namespace).expect("error classes");
            f(&ctx)
        })
    }

    /// The `name` of the pending JS error, e.g. `"TypeError"`, so that tests
    /// pin the spec's error *class* rather than its wording.
    pub(crate) fn pending_error_name(ctx: &Ctx<'_>) -> String {
        ctx.catch()
            .as_exception()
            .and_then(|exception| exception.get::<_, String>("name").ok())
            .unwrap_or_else(|| "not a JS error".to_owned())
    }

    /// Evaluate a snippet to a JS value, for feeding arguments to the wrappers.
    pub(crate) fn js<'js>(ctx: &Ctx<'js>, source: &str) -> Value<'js> {
        ctx.eval(source).expect("test snippet evaluates")
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::FromJs;

    /// One wasm page, the unit memory sizes are counted in.
    const PAGE_SIZE: usize = 65536;

    use super::{
        testing::{js, pending_error_name, with_wasm_context},
        *,
    };

    fn memory(ctx: &Ctx<'_>, descriptor: &str) -> core::result::Result<Memory, String> {
        MemoryDescriptor::from_js(ctx, js(ctx, descriptor))
            .and_then(|descriptor| Memory::new(descriptor, ctx.clone()))
            .map_err(|_| pending_error_name(ctx))
    }

    #[test]
    fn buffer_aliases_the_linear_memory_and_is_the_same_object_between_grows() {
        with_wasm_context(|ctx| {
            let memory = memory(ctx, "({ initial: 2 })").expect("two pages");
            let buffer = memory.buffer(ctx.clone()).expect("buffer");
            assert_eq!(buffer.len(), 2 * PAGE_SIZE);
            let again = memory.buffer(ctx.clone()).expect("buffer");
            assert_eq!(buffer.as_value(), again.as_value());
        })
    }

    #[test]
    fn an_empty_memory_still_has_a_zero_length_buffer() {
        with_wasm_context(|ctx| {
            let memory = memory(ctx, "({ initial: 0 })").expect("no pages");
            assert_eq!(memory.buffer(ctx.clone()).expect("buffer").len(), 0);
        })
    }

    #[test]
    fn grow_returns_the_previous_page_count_and_detaches_the_old_buffer() {
        with_wasm_context(|ctx| {
            let memory = memory(ctx, "({ initial: 1, maximum: 4 })").expect("one page");
            let stale = memory.buffer(ctx.clone()).expect("buffer");
            assert_eq!(stale.len(), PAGE_SIZE);

            let previous = memory.grow(Coerced(2), ctx.clone()).expect("grow");
            assert_eq!(previous, 1);
            assert_eq!(stale.as_bytes(), None, "the old buffer must be detached");

            let fresh = memory.buffer(ctx.clone()).expect("buffer");
            assert_eq!(fresh.len(), 3 * PAGE_SIZE);
            assert_ne!(stale.as_value(), fresh.as_value());
        })
    }

    #[test]
    fn growing_past_the_maximum_is_a_range_error() {
        with_wasm_context(|ctx| {
            let memory = memory(ctx, "({ initial: 1, maximum: 2 })").expect("one page");
            let _ = memory
                .grow(Coerced(5), ctx.clone())
                .expect_err("over maximum");
            assert_eq!(pending_error_name(ctx), "RangeError");
        })
    }

    #[test]
    fn a_descriptor_without_initial_is_a_type_error() {
        with_wasm_context(|ctx| {
            assert_eq!(memory(ctx, "({ maximum: 1 })").unwrap_err(), "TypeError");
            assert_eq!(memory(ctx, "(1)").unwrap_err(), "TypeError");
        })
    }

    #[test]
    fn minimum_is_accepted_as_an_alias_of_initial_but_not_alongside_it() {
        with_wasm_context(|ctx| {
            let wasm_memory = memory(ctx, "({ minimum: 2 })").expect("minimum");
            assert_eq!(
                wasm_memory.buffer(ctx.clone()).expect("buffer").len(),
                2 * PAGE_SIZE
            );
            assert_eq!(
                memory(ctx, "({ initial: 1, minimum: 1 })").unwrap_err(),
                "TypeError"
            );
        })
    }

    #[test]
    fn a_maximum_below_the_initial_size_is_a_range_error() {
        with_wasm_context(|ctx| {
            assert_eq!(
                memory(ctx, "({ initial: 4, maximum: 1 })").unwrap_err(),
                "RangeError"
            );
        })
    }

    #[test]
    fn shared_memory_needs_a_maximum_and_reports_shared_on_type() {
        with_wasm_context(|ctx| {
            assert_eq!(
                memory(ctx, "({ initial: 1, shared: true })").unwrap_err(),
                "TypeError"
            );

            let shared = memory(ctx, "({ initial: 1, maximum: 2, shared: true })")
                .expect("shared memories are allocated unshared and report the requested flag");
            ctx.globals()
                .set("type", shared.memory_type(ctx.clone()).expect("type()"))
                .expect("bind");
            assert_eq!(
                ctx.eval::<String, _>("`${type.minimum}:${type.maximum}:${type.shared}`")
                    .expect("render"),
                "1:2:true"
            );
        })
    }
}

/// The JS-visible surface of `Memory`, `Table` and `Global`, exercised through
/// the real `den:wasm` module.
///
/// The tests above call the Rust methods directly, which bypasses exactly the
/// part most likely to be wrong: class registration, and whether `buffer`,
/// `length` and `value` really are accessors under the names the spec gives
/// them.
#[cfg(test)]
mod javascript_tests {
    use rquickjs::{Context, Module, Runtime};

    const EXERCISE: &str = r#"
      const assert = (holds, what) => { if (!holds) throw new Error(what); };

      const memory = new WebAssembly.Memory({ initial: 1, maximum: 2 });
      const buffer = memory.buffer;
      assert(buffer === memory.buffer, "buffer is not the same object between grows");
      assert(buffer.byteLength === 65536, "buffer does not span the linear memory");
      new Uint8Array(buffer)[0] = 7;
      assert(memory.grow(1) === 1, "grow does not return the previous page count");
      assert(buffer.byteLength === 0, "the old buffer was not detached");
      assert(memory.buffer.byteLength === 131072, "the fresh buffer has the wrong size");
      assert(new Uint8Array(memory.buffer)[0] === 7, "growing lost the memory contents");

      const table = new WebAssembly.Table({ element: "anyfunc", initial: 2 });
      assert(table.length === 2, "length is wrong");
      assert(table.get(0) === null, "a fresh anyfunc table is not full of nulls");
      table.set(1, null);
      assert(table.grow(1) === 2, "grow does not return the previous length");
      assert(table.length === 3, "grow did not grow");

      const mutable = new WebAssembly.Global({ value: "i32", mutable: true }, 5);
      assert(mutable.value === 5, "the value getter is missing or wrong");
      mutable.value = 6;
      assert(mutable.valueOf() === 6, "the value setter or valueOf is missing or wrong");

      const constant = new WebAssembly.Global({ value: "i64" }, 1n);
      assert(constant.value === 1n, "an i64 global does not read as a BigInt");
      try {
        constant.value = 2n;
        throw new Error("an immutable global accepted a write");
      } catch (error) {
        assert(error instanceof TypeError, "writing an immutable global is not a TypeError");
      }
      true
    "#;

    #[test]
    fn the_wasm_objects_behave_the_way_scripts_expect() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let (_, evaluation) =
                Module::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")
                    .expect("den:wasm evaluates");
            evaluation.finish::<()>().expect("den:wasm finishes");
            let held: bool = ctx.eval(EXERCISE).expect("every assertion holds");
            assert!(held);
        })
    }
}

#[cfg(test)]
mod detach_key_tests {
    use super::{
        testing::{pending_error_name, with_wasm_context},
        *,
    };

    /// One wasm page, the unit memory sizes are counted in.
    const PAGE_SIZE: usize = 65536;

    fn one_page(ctx: &Ctx<'_>) -> Memory {
        Memory::new(
            MemoryDescriptor {
                initial: 1,
                maximum: None,
                shared:  false,
            },
            ctx.clone(),
        )
        .expect("one page")
    }

    /// `transfer` reaches `js_realloc` on the wasm linear memory base, so
    /// before the buffer was sealed this corrupted the heap and crashed the
    /// process rather than merely misbehaving.
    #[test]
    fn the_buffer_refuses_every_method_that_would_detach_it() {
        with_wasm_context(|ctx| {
            let memory = one_page(ctx);
            ctx.globals()
                .set("buf", memory.buffer(ctx.clone()).expect("buffer"))
                .expect("bind");

            for call in [
                "buf.transfer()",
                "buf.transfer(1)",
                "buf.transferToFixedLength()",
                "buf.transferToImmutable()",
                "buf.resize(0)",
            ] {
                let outcome: rquickjs::Result<rquickjs::Value> = ctx.eval(call);
                assert!(outcome.is_err(), "{call} must not succeed");
                assert_eq!(pending_error_name(ctx), "TypeError", "{call}");
            }

            // Sealing must not have cost the buffer its ordinary behaviour.
            let length: usize = ctx.eval("buf.byteLength").expect("byteLength");
            assert_eq!(length, PAGE_SIZE);
            let written: u8 = ctx
                .eval("(new Uint8Array(buf))[7] = 42, (new Uint8Array(buf))[7]")
                .expect("writable");
            assert_eq!(written, 42);
        })
    }

    /// Script must not be able to `delete` the guards to reach the originals on
    /// `ArrayBuffer.prototype`.
    #[test]
    fn the_guards_are_neither_writable_nor_configurable() {
        with_wasm_context(|ctx| {
            let memory = one_page(ctx);
            ctx.globals()
                .set("buf", memory.buffer(ctx.clone()).expect("buffer"))
                .expect("bind");

            let descriptor: String =
                ctx
                    .eval(
                        "(() => { const d = Object.getOwnPropertyDescriptor(buf, 'transfer'); \
                         return                      \
                         `${d.writable}/${d.configurable}/${d.enumerable}` })()",
                    )
                    .expect("descriptor");
            assert_eq!(descriptor, "false/false/false");

            // Non-configurable, so `delete` reports failure — as a thrown
            // TypeError under strict mode, which is how rquickjs evaluates.
            let deleted: String = ctx
                .eval(
                    "(() => { try { return String(delete buf.transfer) } catch (e) { return \
                     e.name } })()",
                )
                .expect("delete");
            assert!(
                deleted == "false" || deleted == "TypeError",
                "the guard must not be deletable, got {deleted}"
            );

            let still_refused: bool = ctx
                .eval("(() => { try { buf.transfer(1); return false } catch { return true } })()")
                .expect("transfer");
            assert!(still_refused, "transfer must still be refused");
        })
    }
}
