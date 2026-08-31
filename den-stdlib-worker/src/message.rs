//! Structured clone, Rust half: a serialised value that is `Send`.
//!
//! The heavy lifting is quickjs-ng's own `JS_WriteObject2` / `JS_ReadObject2`,
//! which carry primitives, BigInt, boxed primitives, Date, RegExp, Array,
//! plain objects, ArrayBuffer, every typed array, Map, Set, and — crucially —
//! cycles, shared references and typed-array/buffer aliasing. What they cannot
//! carry (Error, DOMException, DataView), wrongly accept (Symbol, accessor
//! properties), or report with the wrong error type is handled by the Rust
//! pre/post pass in [`clone`]. The whole investigation, with quickjs.c line
//! references, is docs/research/10-structured-clone-strategy.md.

use std::{ptr, slice};

use den_util::throw_dom_exception;
use rquickjs::{
    ArrayBuffer, Class, Ctx, Error, Exception, Object, Result, Value, object::Filter, qjs,
};

use crate::{port::NativePort, transport::PortHandle};

pub(crate) mod clone;

/// A structured-clone-serialised value, plus the channel ends of every
/// transferred `MessagePort`.
///
/// Owns everything it carries and has no tie to any runtime: being `Send` is
/// the entire point, since this is what crosses to a worker thread.
#[derive(Debug)]
pub struct Message {
    bytes: Vec<u8>,
    ports: Vec<PortHandle>,
}

/// The `Send` bound is load-bearing, so it is checked at compile time.
const _: fn() = || {
    const fn assert_send<T: Send>() {}
    assert_send::<Message>();
};

impl Message {
    /// Serialise `value` out of `ctx`, transferring each buffer and port.
    ///
    /// `transfer_buffers` are raw `Value`s rather than [`ArrayBuffer`]s on
    /// purpose: converting a *detached* buffer through `FromJs` goes to
    /// `JS_GetArrayBuffer`, which arms a pending `TypeError` that would then
    /// surface at the next unrelated call (docs/research/10 §4.5). The type,
    /// detach and immutability checks are done here instead, and every failure
    /// is a `DataCloneError`.
    ///
    /// The order is `StructuredSerializeWithTransfer`'s: validate the transfer
    /// list, serialise, and only then move the ports and detach the buffers —
    /// so a clone that fails leaves the sender's buffers and ports intact.
    ///
    /// Transferring is *atomic*: every refusal is decided before the first
    /// mutation. A transfer that detached buffers and moved half the ports and
    /// only then threw would leave the sender holding a `MessagePort` whose
    /// channel is gone — a silent loss no `catch` can undo.
    pub fn serialize<'js>(
        ctx: &Ctx<'js>, value: Value<'js>, transfer_buffers: Vec<Value<'js>>,
        transfer_ports: Vec<Class<'js, NativePort>>,
    ) -> Result<Self> {
        Self::validate_transfer(ctx, &transfer_buffers, &transfer_ports)?;

        let prepared = clone::prepare(ctx, value, &transfer_ports)?;
        let bytes = Self::write(ctx, &prepared)?;

        // The walk just ran arbitrary script — every getter in the graph — and
        // a getter is free to `buffer.transfer()` an entry of the transfer
        // list, `close()` a port in it or `start()` one. Whatever it did, the
        // decision to transfer has to be taken on the state that holds *now*.
        Self::validate_transfer(ctx, &transfer_buffers, &transfer_ports)?;

        // Mutation starts here, and from here nothing may fail: the conversion
        // is fallible only for a value the validation above already accepted as
        // a live ArrayBuffer, and `take_handle` refuses only what
        // `validate_ports` has just ruled out — plus a re-entrant `RefCell`
        // borrow, which is impossible while no script runs. Both stay
        // `DataCloneError`s rather than panics, because a den bug must not be
        // an abort reachable from `postMessage`.
        let buffers = transfer_buffers
            .into_iter()
            .map(|entry| {
                ArrayBuffer::from_value(entry).ok_or_else(|| {
                    throw_data_clone(ctx, "an ArrayBuffer in the transfer list is unusable")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let ports = transfer_ports
            .iter()
            .map(|port| {
                port.try_borrow()
                    .ok()
                    .and_then(|port| port.take_handle())
                    .ok_or_else(|| {
                        throw_data_clone(ctx, "a transferred MessagePort is already detached")
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        for mut buffer in buffers {
            buffer.detach();
        }
        Ok(Self { bytes, ports })
    }

    /// Everything `StructuredSerializeWithTransfer` refuses, decided without
    /// touching anything — so it can be run twice, before and after the
    /// serialisation walk.
    fn validate_transfer<'js>(
        ctx: &Ctx<'js>, buffers: &[Value<'js>], ports: &[Class<'js, NativePort>],
    ) -> Result<()> {
        Self::validate_buffers(ctx, buffers)?;
        Self::validate_ports(ctx, ports)
    }

    /// A second copy of the same serialised graph, for a fan-out — one
    /// `BroadcastChannel.postMessage` serialises once and hands a copy to every
    /// other subscriber (HTML §9.5).
    ///
    /// `None` when the message carries transferred ports: a channel end is
    /// unique and cannot be duplicated. Broadcasting serialises with an empty
    /// transfer list, so the clone pre-pass has already refused any port in the
    /// graph and this case is unreachable from script — it is `Option` rather
    /// than a panic because that is the only way to keep it so.
    pub fn try_clone(&self) -> Option<Self> {
        self.ports.is_empty().then(|| {
            Self {
                bytes: self.bytes.clone(),
                ports: Vec::new(),
            }
        })
    }

    /// Rebuild the value inside `ctx` — normally a different runtime on a
    /// different OS thread. The second element is what becomes
    /// `MessageEvent.ports`: the transferred ports, in transfer-list order.
    ///
    /// A failure here is *not* a `DataCloneError`; the caller turns it into a
    /// `messageerror` event (HTML §9.4.4).
    pub fn deserialize<'js>(
        self, ctx: &Ctx<'js>,
    ) -> Result<(Value<'js>, Vec<Class<'js, NativePort>>)> {
        let Self { bytes, ports } = self;
        let value = Self::read(ctx, &bytes)?;
        let ports = ports
            .into_iter()
            .map(|handle| Class::instance(ctx.clone(), NativePort::from_handle(handle)))
            .collect::<Result<Vec<_>>>()?;
        let value = clone::restore(ctx, value, &ports)?;
        Ok((value, ports))
    }

    /// Spec step 2 of `StructuredSerializeWithTransfer`, by hand, because
    /// `JS_DetachArrayBuffer` (quickjs.c:58030) guards none of it: it has no
    /// immutability check, no detach-key concept, and calls the buffer's free
    /// hook unconditionally.
    fn validate_buffers<'js>(ctx: &Ctx<'js>, entries: &[Value<'js>]) -> Result<()> {
        for (index, entry) in entries.iter().enumerate() {
            // ponytail: O(n²) duplicate scan; a transfer list is a handful of
            // entries, and a hash of JSValue bits would need its own newtype.
            if entries.iter().take(index).any(|seen| seen == entry) {
                return Err(throw_data_clone(
                    ctx,
                    "the same object appears twice in the transfer list",
                ));
            }
            // SAFETY: `as_raw` borrows the value for the call; `JS_IsArrayBuffer`
            // only reads the class id and never throws.
            if !unsafe { qjs::JS_IsArrayBuffer(entry.as_raw()) } {
                return Err(throw_data_clone(
                    ctx,
                    "the transfer list contains a value that cannot be transferred",
                ));
            }
            let object = entry
                .as_object()
                .ok_or_else(|| throw_data_clone(ctx, "the transfer list contains a non-object"))?;
            // The `detached` getter, never `ArrayBuffer::from_value`: see the
            // doc comment on `serialize`.
            if object.get::<_, bool>("detached")? {
                return Err(throw_data_clone(
                    ctx,
                    "an ArrayBuffer in the transfer list is detached",
                ));
            }
            // SAFETY: same as above — a pure read of the buffer's header.
            if unsafe { qjs::JS_IsImmutableArrayBuffer(entry.as_raw()) } != 0 {
                return Err(throw_data_clone(
                    ctx,
                    "an ArrayBuffer in the transfer list is immutable",
                ));
            }
            if Self::has_detach_key(object)? {
                return Err(throw_data_clone(
                    ctx,
                    "an ArrayBuffer in the transfer list has a detach key",
                ));
            }
        }
        Ok(())
    }

    /// `[[ArrayBufferDetachKey]]`, den-flavoured: `WebAssembly.Memory#buffer`
    /// is sealed against transfer by shadowing `transfer` with a throwing
    /// **own** property (den-stdlib-wasm/src/memory.rs), and a Rust-level
    /// detach would walk straight past that seal and yank the wasm pages out
    /// from under the instance. An own `transfer` property is therefore read as
    /// "this buffer has a detach key"; a script that adds one to a plain buffer
    /// has merely opted out of transferring it.
    fn has_detach_key(object: &Object<'_>) -> Result<bool> {
        for key in object.own_keys::<String>(Filter::new().string()) {
            if key? == "transfer" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A port may be transferred once, and only while it is still entangled
    /// and not yet started.
    ///
    /// Every refusal here has to match one of `take_handle`'s, or the transfer
    /// is no longer atomic: whatever this misses fails halfway through the
    /// mutation instead, after other ports have already been moved out.
    fn validate_ports<'js>(ctx: &Ctx<'js>, ports: &[Class<'js, NativePort>]) -> Result<()> {
        for (index, port) in ports.iter().enumerate() {
            if ports
                .iter()
                .take(index)
                .any(|seen| seen.as_inner().as_value() == port.as_inner().as_value())
            {
                return Err(throw_data_clone(
                    ctx,
                    "the same MessagePort appears twice in the transfer list",
                ));
            }
            let Ok(port) = port.try_borrow() else {
                return Err(throw_data_clone(
                    ctx,
                    "a MessagePort in the transfer list is in use",
                ));
            };
            if !port.is_open() {
                return Err(throw_data_clone(
                    ctx,
                    "a MessagePort in the transfer list is already detached",
                ));
            }
            // A started port's inbox lives inside its pump future, in this
            // runtime, and cannot be moved to another one — `take_handle`
            // refuses it, so it has to be refused here too, before anything is
            // detached.
            if port.is_started() {
                return Err(throw_data_clone(
                    ctx,
                    "a started MessagePort cannot be transferred; transfer it before start()",
                ));
            }
        }
        Ok(())
    }

    fn write<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
        // `REFERENCE` is what encodes cycles and shared references. Never
        // `BYTECODE` — it would serialise functions and turn a message channel
        // into an arbitrary-code-execution path — and not `SAB`, which rquickjs
        // installs no allocator hooks for (docs/research/10 §3).
        const FLAGS: i32 = qjs::JS_WRITE_OBJ_REFERENCE as i32;
        let mut len: qjs::size_t = 0;
        // SAFETY: `as_raw()` borrows both the context and the value — the
        // writer consumes neither. A null `psab_tab` makes quickjs free its own
        // SAB table (quickjs.c:38461) instead of handing ownership over.
        let buffer = unsafe {
            qjs::JS_WriteObject2(
                ctx.as_raw().as_ptr(),
                &raw mut len,
                value.as_raw(),
                FLAGS,
                ptr::null_mut(),
            )
        };
        if buffer.is_null() {
            return Err(Self::rethrow_as_data_clone(ctx));
        }
        // SAFETY: on success the writer returns a `len`-byte allocation.
        let bytes = unsafe { slice::from_raw_parts(buffer, len as usize) }.to_vec();
        // SAFETY: the buffer came from this context's allocator (with
        // `rust-alloc`, its `RustAllocator`), so it must go back through
        // `js_free`, not Rust's `dealloc`; nothing references it after the copy.
        unsafe { qjs::js_free(ctx.as_raw().as_ptr(), buffer.cast()) };
        Ok(bytes)
    }

    fn read<'js>(ctx: &Ctx<'js>, bytes: &[u8]) -> Result<Value<'js>> {
        // `REFERENCE` is mandatory or an object reference is a hard error
        // (quickjs.c:39609). `BYTECODE` stays off on the read side too: it is
        // what makes a hostile message merely fail instead of executing.
        const FLAGS: i32 = qjs::JS_READ_OBJ_REFERENCE as i32;
        // SAFETY: the reader borrows `bytes` for the duration of the call and
        // returns an owned `JSValue`; a null `psab_tab` is the no-SAB case.
        let raw = unsafe {
            qjs::JS_ReadObject2(
                ctx.as_raw().as_ptr(),
                bytes.as_ptr(),
                bytes.len() as _,
                FLAGS,
                ptr::null_mut(),
            )
        };
        // SAFETY: reading the tag of a value the reader just returned.
        if unsafe { qjs::JS_IsException(raw) } {
            return Err(Error::Exception);
        }
        // SAFETY: the reader hands over ownership of the value, which is what
        // `from_raw` expects, and it belongs to `ctx`'s runtime by construction.
        Ok(unsafe { Value::from_raw(ctx.clone(), raw) })
    }

    /// The writer reports every refusal as a `TypeError` ("unsupported object
    /// class", "ArrayBuffer is detached", "only value properties are
    /// supported"). Once the pre-pass has run, every remaining refusal *is* a
    /// non-serialisable value, so re-tagging them all as `DataCloneError` is
    /// honest — and it is what the spec requires callers to observe.
    fn rethrow_as_data_clone(ctx: &Ctx<'_>) -> Error {
        let pending = ctx.catch();
        // An interrupt (`worker.terminate()`) is not a clone failure and must
        // stay uncatchable.
        if pending.is_uncatchable_error() {
            return ctx.throw(pending);
        }
        let detail = pending
            .as_exception()
            .and_then(Exception::message)
            .unwrap_or_else(|| "unsupported value".to_owned());
        throw_data_clone(ctx, &format!("the value could not be cloned: {detail}"))
    }
}

/// Throw `DOMException(message, "DataCloneError")`, always returning the `Err`
/// payload to propagate.
///
/// quickjs-ng ships `DOMException` natively and `JS_NewContext` registers it
/// unconditionally, so nothing has to be built in JS-land for this.
pub fn throw_data_clone(ctx: &Ctx<'_>, message: &str) -> Error {
    throw_dom_exception(ctx, "DataCloneError", message)
}

/// `structuredClone(value, { transfer })`.
pub fn structured_clone<'js>(
    ctx: Ctx<'js>, value: Value<'js>, options: rquickjs::function::Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let transfer = match options.0 {
        Some(options) if options.is_object() => {
            options
                .as_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "clone options must be an object"))?
                .get("transfer")?
        }
        _ => None,
    };
    let (buffers, ports) = clone::split_transfer(&ctx, transfer)?;
    clone_with_transfer(ctx, value, buffers, ports)
}

fn clone_with_transfer<'js>(
    ctx: Ctx<'js>, value: Value<'js>, buffers: Vec<Value<'js>>, ports: Vec<Class<'js, NativePort>>,
) -> Result<Value<'js>> {
    let message = Message::serialize(&ctx, value, buffers, ports)?;
    message
        .deserialize(&ctx)
        .map(|(value, _)| value)
        .map_err(|error| {
            match error {
                Error::Exception => Message::rethrow_as_data_clone(&ctx),
                error => error,
            }
        })
}

#[rquickjs::function(rename = "splitTransfer")]
fn split_transfer_js<'js>(
    ctx: Ctx<'js>, transfer: rquickjs::function::Opt<Value<'js>>,
) -> Result<Object<'js>> {
    let (buffers, ports) = clone::split_transfer(&ctx, transfer.0)?;
    let out = Object::new(ctx.clone())?;
    out.set("buffers", buffers)?;
    out.set("ports", ports)?;
    Ok(out)
}

pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    let port_handle = clone::CloneState::install(ctx)?;
    natives.set("portHandleKey", port_handle)?;
    natives.set("splitTransfer", js_split_transfer_js)?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/message.rs"]
mod tests;
