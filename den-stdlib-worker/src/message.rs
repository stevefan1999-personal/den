//! Structured clone, Rust half: a serialised value that is `Send`.
//!
//! The heavy lifting is quickjs-ng's own `JS_WriteObject2` / `JS_ReadObject2`,
//! which carry primitives, BigInt, boxed primitives, Date, RegExp, Array,
//! plain objects, ArrayBuffer, every typed array, Map, Set, and — crucially —
//! cycles, shared references and typed-array/buffer aliasing. What they cannot
//! carry (Error, DOMException, DataView), wrongly accept (Symbol, accessor
//! properties), or report with the wrong error type is handled by the JS
//! pre/post pass in `src/prelude/clone.js`. The whole investigation, with
//! quickjs.c line references, is docs/research/10-structured-clone-strategy.md.

use std::{ffi::CString, ptr, slice};

use rquickjs::{
    Array, ArrayBuffer, Class, Ctx, Error, Exception, Function, JsLifetime, Object, Result, Value,
    object::Filter, qjs,
};

use crate::{port::NativePort, transport::PortHandle};

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
    fn assert_send<T: Send>() {}
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
        ctx: &Ctx<'js>,
        value: Value<'js>,
        transfer_buffers: Vec<Value<'js>>,
        transfer_ports: Vec<Class<'js, NativePort>>,
    ) -> Result<Self> {
        Self::validate_transfer(ctx, &transfer_buffers, &transfer_ports)?;

        let prepare = CloneHooks::prepare(ctx)?;
        let prepared: Value<'js> =
            prepare.call((value, Self::port_array(ctx, &transfer_ports)?))?;
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
        ctx: &Ctx<'js>,
        buffers: &[Value<'js>],
        ports: &[Class<'js, NativePort>],
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
        self,
        ctx: &Ctx<'js>,
    ) -> Result<(Value<'js>, Vec<Class<'js, NativePort>>)> {
        let Self { bytes, ports } = self;
        let value = Self::read(ctx, &bytes)?;
        let ports = ports
            .into_iter()
            .map(|handle| Class::instance(ctx.clone(), NativePort::from_handle(handle)))
            .collect::<Result<Vec<_>>>()?;
        let restore = CloneHooks::restore(ctx)?;
        let value: Value<'js> = restore.call((value, Self::port_array(ctx, &ports)?))?;
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
            if entries[..index].iter().any(|seen| seen == entry) {
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
            if ports[..index]
                .iter()
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

    fn port_array<'js>(ctx: &Ctx<'js>, ports: &[Class<'js, NativePort>]) -> Result<Array<'js>> {
        let array = Array::new(ctx.clone())?;
        for (index, port) in ports.iter().enumerate() {
            array.set(index, port.clone())?;
        }
        Ok(array)
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
                &mut len,
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
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: `JS_ThrowDOMException` vsnprintf's into a 256-byte stack buffer
    // (quickjs.c:62309), so the caller's text is passed as an *argument* to a
    // constant `%s` format, never as the format itself. Both C strings outlive
    // the call. The returned `JS_EXCEPTION` tag owns nothing to free.
    unsafe {
        qjs::JS_ThrowDOMException(
            ctx.as_raw().as_ptr(),
            c"DataCloneError".as_ptr(),
            c"%s".as_ptr(),
            message.as_ptr(),
        )
    };
    Error::Exception
}

/// The `prepare` / `restore` pair from `src/prelude/clone.js`, kept in the
/// context userdata so that serialising does not depend on the globals a script
/// may have replaced.
#[derive(JsLifetime)]
pub struct CloneHooks<'js> {
    prepare: Function<'js>,
    restore: Function<'js>,
}

impl<'js> CloneHooks<'js> {
    fn prepare(ctx: &Ctx<'js>) -> Result<Function<'js>> {
        Self::pick(ctx, |hooks| hooks.prepare.clone())
    }

    fn restore(ctx: &Ctx<'js>) -> Result<Function<'js>> {
        Self::pick(ctx, |hooks| hooks.restore.clone())
    }

    /// The guard is dropped before the picked function is ever called: a live
    /// userdata guard blocks `store_userdata` for *every* type, which would
    /// break an unrelated module installing its own state from inside a getter.
    fn pick<T>(ctx: &Ctx<'js>, pick: impl FnOnce(&Self) -> T) -> Result<T> {
        ctx.userdata::<Self>()
            .map(|hooks| pick(&hooks))
            .ok_or_else(|| Exception::throw_internal(ctx, "den:worker is not installed"))
    }
}

/// `natives.registerClone(prepare, restore)` — called once by the prelude.
#[rquickjs::function(rename = "registerClone")]
pub fn register_clone<'js>(
    ctx: Ctx<'js>,
    prepare: Function<'js>,
    restore: Function<'js>,
) -> Result<()> {
    ctx.store_userdata(CloneHooks { prepare, restore })
        .map_err(|_| Exception::throw_internal(&ctx, "den:worker is already installed"))?;
    Ok(())
}

/// `natives.classIdOf(value)` — the only way to ask "is this an ordinary
/// object?": the `JS_CLASS_*` ids are a private enum in quickjs.c, so JS
/// compares the id of a value against that of a freshly made `{}`.
#[rquickjs::function(rename = "classIdOf")]
pub fn class_id_of(value: Value<'_>) -> qjs::JSClassID {
    // SAFETY: a pure read of the value's class id; it throws nothing and takes
    // no ownership.
    unsafe { qjs::JS_GetClassID(value.as_raw()) }
}

/// `natives.isProxy(value)`. `Object.getPrototypeOf` and `instanceof` both run
/// Proxy traps, so the pre-pass has to ask before it touches anything.
#[rquickjs::function(rename = "isProxy")]
pub fn is_proxy(value: Value<'_>) -> bool {
    value.is_proxy()
}

/// `natives.isError(value)` — class-id based, so unlike `instanceof Error` it
/// cannot be forged, and it is correctly false for a `DOMException`.
#[rquickjs::function(rename = "isError")]
pub fn is_error(value: Value<'_>) -> bool {
    value.is_error()
}

/// `natives.cloneWithTransfer(value, buffers, ports)` — the whole pipeline in
/// one context, which is what `structuredClone(value, { transfer })` is.
#[rquickjs::function(rename = "cloneWithTransfer")]
pub fn clone_with_transfer<'js>(
    ctx: Ctx<'js>,
    value: Value<'js>,
    buffers: Vec<Value<'js>>,
    ports: Vec<Class<'js, NativePort>>,
) -> Result<Value<'js>> {
    let message = Message::serialize(&ctx, value, buffers, ports)?;
    // Same realm, so unlike a worker there is no far side to hand a
    // `messageerror` to. A graph this side wrote and cannot read back was not
    // cloneable after all, and `structuredClone` owes the caller a
    // `DataCloneError` for that rather than the reader's `RangeError`. The
    // known instance of it — an ArrayBufferView left out of bounds by shrinking
    // its resizable buffer — is now refused during the graph walk, so this is
    // the net that catches whatever else quickjs writes and cannot read.
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

pub fn install<'js>(_ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
    natives.set("registerClone", js_register_clone)?;
    natives.set("classIdOf", js_class_id_of)?;
    natives.set("isProxy", js_is_proxy)?;
    natives.set("isError", js_is_error)?;
    natives.set("cloneWithTransfer", js_clone_with_transfer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rquickjs::{
        ArrayBuffer, AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Class, Context,
        FromJs, Function, Module, Runtime, Value,
    };

    use super::Message;
    use crate::{
        port::NativePort,
        transport::{Envelope, PortHandle},
    };

    /// A fresh runtime with `den:worker` installed. One per test: the module
    /// keeps its clone hooks in the context userdata, so contexts are not
    /// shared.
    fn worker_context() -> (Runtime, Context) {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let install = || {
                let (_, evaluated) =
                    Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                evaluated.finish::<()>()
            };
            install()
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("den:worker evaluates");
        });
        (runtime, context)
    }

    /// The `name` of whatever a failed clone threw. A `DOMException` is not a
    /// `JS_CLASS_ERROR`, so rquickjs catches it as a plain thrown value and its
    /// `Display` says nothing useful.
    fn thrown_name(error: CaughtError<'_>) -> String {
        match error {
            CaughtError::Value(value) => {
                value
                    .as_object()
                    .and_then(|object| object.get::<_, String>("name").ok())
                    .unwrap_or_else(|| "a value that is not a DOMException".to_owned())
            }
            other => other.to_string(),
        }
    }

    /// Evaluate `source` — an expression — with `den:worker` installed.
    fn eval<T>(source: &str) -> Result<T, String>
    where
        T: for<'js> FromJs<'js>,
    {
        let (_runtime, context) = worker_context();
        context.with(|ctx| {
            ctx.eval::<T, _>(source)
                .catch(&ctx)
                .map_err(|err| err.to_string())
        })
    }

    /// `"DataCloneError"` when cloning `expression` throws the right thing,
    /// and whatever went wrong otherwise — so a failing assertion names it.
    fn clone_failure(expression: &str) -> String {
        eval::<String>(&format!(
            r#"(() => {{
                 try {{ structuredClone({expression}); return "no throw"; }}
                 catch (error) {{
                   return error instanceof DOMException && error instanceof Error
                     ? error.name : `wrong error: ${{error}}`;
                 }}
               }})()"#
        ))
        .expect("the snippet evaluates")
    }

    /// Assert a JS expression over `structuredClone`'s result is true.
    fn assert_clone(source: &str) {
        assert_eq!(eval::<bool>(source), Ok(true), "{source}");
    }

    #[test]
    fn primitives_round_trip_including_negative_zero_and_nan() {
        assert_clone(
            r#"(() => {
                 const value = { u: undefined, n: null, t: true, i: 42, f: 1.5,
                                 s: "héllo😀", zero: -0, nan: NaN, inf: -Infinity };
                 const out = structuredClone(value);
                 return out.u === undefined && out.n === null && out.t === true
                   && out.i === 42 && out.f === 1.5 && Object.is(out.zero, -0)
                   && Number.isNaN(out.nan) && out.inf === -Infinity && out.s === value.s;
               })()"#,
        );
    }

    #[test]
    fn boxed_primitives_stay_boxed() {
        assert_clone(
            r#"(() => {
                 const out = structuredClone([Object(1), Object("s"), Object(true), Object(5n)]);
                 return out.map((v) => typeof v).join() === "object,object,object,object"
                   && out[0].valueOf() === 1 && out[1].valueOf() === "s"
                   && out[2].valueOf() === true && out[3].valueOf() === 5n;
               })()"#,
        );
    }

    #[test]
    fn date_preserves_its_time_value() {
        assert_clone(
            r#"(() => {
                 const out = structuredClone({ at: new Date(1234567890123), bad: new Date(NaN) });
                 return out.at instanceof Date && out.at.getTime() === 1234567890123
                   && Number.isNaN(out.bad.getTime());
               })()"#,
        );
    }

    #[test]
    fn regexp_preserves_source_and_flags_and_resets_last_index() {
        // `lastIndex` is deliberately not carried: HTML step 12 clones source
        // and flags only, and so does `JS_WriteRegExp`.
        assert_clone(
            r#"(() => {
                 const pattern = /ab+c/gi;
                 pattern.lastIndex = 3;
                 const out = structuredClone(pattern);
                 return out instanceof RegExp && out.source === "ab+c"
                   && out.flags === "gi" && out.lastIndex === 0;
               })()"#,
        );
    }

    /// Regression test for a quickjs-ng reader bug: `JS_ReadRegExp`
    /// (quickjs.c:39435) is the one reader that does not register the object it
    /// built in the reference table, while the writer registers every object —
    /// so before clone.js started rebuilding RegExp from its parts, one RegExp
    /// anywhere in a graph shifted every later back-reference by one and the
    /// read failed outright. Two views over one buffer are the cheapest
    /// back-reference there is.
    #[test]
    fn a_regexp_does_not_shift_the_back_references_that_follow_it() {
        assert_clone(
            r#"(() => {
                 const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
                 const out = structuredClone({
                   pattern: /ab+c/gi,
                   view: new Uint16Array(buffer, 2, 1),
                   dataView: new DataView(buffer, 1, 2),
                 });
                 return out.pattern.source === "ab+c"
                   && out.view.buffer === out.dataView.buffer
                   && out.view.byteOffset === 2 && out.dataView.byteLength === 2;
               })()"#,
        );
    }

    #[test]
    fn array_buffer_round_trips_as_a_copy() {
        assert_clone(
            r#"(() => {
                 const buffer = new Uint8Array([1, 2, 3]).buffer;
                 const out = structuredClone(buffer);
                 return out instanceof ArrayBuffer && out !== buffer
                   && new Uint8Array(out).join() === "1,2,3";
               })()"#,
        );
    }

    #[test]
    fn every_typed_array_kind_round_trips() {
        // Float16Array is quickjs-ng's; the check skips what a build lacks.
        assert_clone(
            r#"(() => {
                 const kinds = ["Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array",
                                "Uint16Array", "Int32Array", "Uint32Array", "Float32Array",
                                "Float64Array", "Float16Array", "BigInt64Array", "BigUint64Array"];
                 const broken = kinds.filter((kind) => {
                   const constructor = globalThis[kind];
                   if (typeof constructor !== "function") return false;
                   const big = kind.startsWith("Big");
                   const source = new constructor(big ? [1n, 2n] : [1, 2]);
                   const out = structuredClone(source);
                   return !(out instanceof constructor) || out.length !== 2
                     || out[0] !== source[0] || out[1] !== source[1];
                 });
                 return broken.length === 0;
               })()"#,
        );
    }

    #[test]
    fn typed_array_preserves_offset_length_and_buffer_aliasing() {
        assert_clone(
            r#"(() => {
                 const buffer = new ArrayBuffer(8);
                 const value = { buffer, head: new Uint8Array(buffer, 2, 4), all: new Uint8Array(buffer) };
                 value.head.set([9, 8, 7, 6]);
                 const out = structuredClone(value);
                 return out.head.byteOffset === 2 && out.head.length === 4
                   && out.head.buffer === out.buffer && out.all.buffer === out.buffer
                   && out.head.join() === "9,8,7,6";
               })()"#,
        );
    }

    #[test]
    fn data_view_round_trips_and_shares_its_buffer_with_a_sibling_view() {
        assert_clone(
            r#"(() => {
                 const buffer = new ArrayBuffer(8);
                 const view = new DataView(buffer, 2, 4);
                 view.setUint8(0, 42);
                 const out = structuredClone({ view, bytes: new Uint8Array(buffer) });
                 return out.view instanceof DataView && out.view.byteOffset === 2
                   && out.view.byteLength === 4 && out.view.getUint8(0) === 42
                   && out.view.buffer === out.bytes.buffer;
               })()"#,
        );
    }

    #[test]
    fn map_and_set_round_trip_preserving_insertion_order() {
        assert_clone(
            r#"(() => {
                 const map = new Map([["a", 1], [2, "b"], [true, null]]);
                 const set = new Set(["x", 7, false]);
                 const out = structuredClone({ map, set });
                 return out.map instanceof Map && out.set instanceof Set
                   && JSON.stringify([...out.map]) === JSON.stringify([["a", 1], [2, "b"], [true, null]])
                   && JSON.stringify([...out.set]) === JSON.stringify(["x", 7, false]);
               })()"#,
        );
    }

    #[test]
    fn arrays_and_nested_objects_round_trip() {
        assert_clone(
            r#"(() => {
                 const value = { list: [1, [2, [3, { deep: "yes" }]]], flag: false };
                 const out = structuredClone(value);
                 return JSON.stringify(out) === JSON.stringify(value) && out.list !== value.list;
               })()"#,
        );
    }

    #[test]
    fn bigint_round_trips_beyond_the_i64_boundary() {
        // `BigInt::to_i64` silently returns 0 outside i64, so nothing in the
        // pipeline is allowed to go through it.
        assert_clone(
            r#"(() => {
                 const values = [0n, 1n, -1n, 2n ** 63n, -(2n ** 64n) - 1n, 2n ** 200n];
                 const out = structuredClone(values);
                 return out.every((value, index) => value === values[index]);
               })()"#,
        );
    }

    #[test]
    fn every_error_subtype_round_trips_with_message_and_stack() {
        assert_clone(
            r#"(() => {
                 const names = ["Error", "EvalError", "RangeError", "ReferenceError",
                                "SyntaxError", "TypeError", "URIError"];
                 return names.every((name) => {
                   const out = structuredClone(new globalThis[name]("boom"));
                   return out instanceof globalThis[name] && out.name === name
                     && out.message === "boom" && typeof out.stack === "string";
                 });
               })()"#,
        );
    }

    #[test]
    fn error_subclass_degrades_to_error_and_cause_survives() {
        assert_clone(
            r#"(() => {
                 class MyError extends Error {}
                 const value = new MyError("outer", { cause: new RangeError("inner") });
                 const out = structuredClone(value);
                 return out.constructor === Error && out.name === "Error"
                   && out.message === "outer" && out.cause instanceof RangeError
                   && out.cause.message === "inner";
               })()"#,
        );
    }

    #[test]
    fn error_without_an_own_message_gets_none() {
        assert_clone(
            r#"(() => {
                 const out = structuredClone(new Error());
                 return !Object.hasOwn(out, "message") && out.message === "";
               })()"#,
        );
    }

    #[test]
    fn dom_exception_round_trips_preserving_name_and_code() {
        assert_clone(
            r#"(() => {
                 const out = structuredClone(new DOMException("gone", "NotFoundError"));
                 return out instanceof DOMException && out.name === "NotFoundError"
                   && out.message === "gone" && out.code === 8;
               })()"#,
        );
    }

    #[test]
    fn cycles_and_shared_references_are_preserved() {
        assert_clone(
            r#"(() => {
                 const shared = { id: 1 };
                 const value = { first: shared, second: shared, list: [] };
                 value.self = value;
                 value.list.push(value.list);
                 const out = structuredClone(value);
                 return out.self === out && out.first === out.second
                   && out.list[0] === out.list;
               })()"#,
        );
    }

    #[test]
    fn a_shared_error_reachable_by_two_paths_stays_one_error() {
        // Proves the tagged replacements join the serialiser's reference table.
        assert_clone(
            r#"(() => {
                 const error = new TypeError("once");
                 error.cause = error;
                 const out = structuredClone({ a: error, b: [error] });
                 return out.a === out.b[0] && out.a.cause === out.a;
               })()"#,
        );
    }

    #[test]
    fn a_shared_object_used_as_a_map_key_and_value_stays_one_object() {
        assert_clone(
            r#"(() => {
                 const key = { k: 1 };
                 const map = new Map([[key, key]]);
                 const out = structuredClone({ map, key });
                 const [[outKey, outValue]] = [...out.map];
                 return outKey === outValue && outKey === out.key;
               })()"#,
        );
    }

    #[test]
    fn getters_are_invoked_once_and_become_data_properties() {
        assert_clone(
            r#"(() => {
                 let calls = 0;
                 const value = { get computed() { calls += 1; return { deep: true }; } };
                 const out = structuredClone(value);
                 return calls === 1 && out.computed.deep === true
                   && Object.getOwnPropertyDescriptor(out, "computed").value !== undefined;
               })()"#,
        );
    }

    #[test]
    fn symbol_keys_are_dropped_and_the_prototype_is_flattened() {
        assert_clone(
            r#"(() => {
                 class Point { constructor() { this.x = 1; this[Symbol("tag")] = 2; } }
                 const out = structuredClone(new Point());
                 return Object.getPrototypeOf(out) === Object.prototype
                   && Object.getOwnPropertySymbols(out).length === 0
                   && out.x === 1 && !(out instanceof Point);
               })()"#,
        );
    }

    #[test]
    fn array_holes_become_undefined_and_non_index_properties_are_dropped() {
        // Both are deliberate v1 divergences from the spec, inherited from
        // `JS_WriteArray`: fixing them costs a full property walk for no
        // observable gain. Pinned here so a change is a decision, not a
        // surprise.
        assert_clone(
            r#"(() => {
                 const value = [1, , 3];
                 value.label = "extra";
                 const out = structuredClone(value);
                 return out.length === 3 && 1 in out && out[1] === undefined
                   && out.label === undefined;
               })()"#,
        );
    }

    #[test]
    fn a_map_with_a_live_iterator_parked_past_a_deleted_key_round_trips_intact() {
        // quickjs-ng's `js_map_write` announces `record_count` entries but
        // writes every record, zombies included, which desynchronises the whole
        // stream and eats the *sibling* property. The pre-pass rebuilds every
        // Map and Set to dodge it (docs/research/10 §4.4).
        assert_clone(
            r#"(() => {
                 const map = new Map([["a", 1], ["b", 2], ["c", 3], ["d", 4]]);
                 const parked = map[Symbol.iterator]();
                 parked.next();
                 parked.next();
                 map.delete("b");
                 const out = structuredClone({ map, sentinel: "S" });
                 return parked !== undefined && out.sentinel === "S"
                   && JSON.stringify([...out.map]) === JSON.stringify([["a", 1], ["c", 3], ["d", 4]]);
               })()"#,
        );
    }

    #[test]
    fn a_transferred_buffer_arrives_with_its_bytes_and_leaves_the_source_detached() {
        assert_clone(
            r#"(() => {
                 const source = new Uint8Array([9, 8, 7, 6]).buffer;
                 const view = new Uint8Array(source);
                 const out = structuredClone({ buffer: source }, { transfer: [source] });
                 return new Uint8Array(out.buffer).join() === "9,8,7,6"
                   && source.detached === true && source.byteLength === 0 && view.length === 0;
               })()"#,
        );
    }

    #[test]
    fn a_failed_clone_leaves_transferred_buffers_attached() {
        // Spec order: serialise first, detach only after it succeeded.
        assert_clone(
            r#"(() => {
                 const buffer = new ArrayBuffer(4);
                 try { structuredClone({ buffer, bad: () => {} }, { transfer: [buffer] }); }
                 catch { return buffer.detached === false && buffer.byteLength === 4; }
                 return false;
               })()"#,
        );
    }

    #[test]
    fn a_duplicate_in_the_transfer_list_throws_data_clone_error() {
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const buffer = new ArrayBuffer(4);
                     try { structuredClone(buffer, { transfer: [buffer, buffer] }); return "no throw"; }
                     catch (error) { return error.name; }
                   })()"#
            ),
            Ok("DataCloneError".to_owned())
        );
    }

    #[test]
    fn a_detached_buffer_in_the_transfer_list_throws_data_clone_error_and_leaves_no_pending_exception()
     {
        // `ArrayBuffer::from_value` on a detached buffer arms a pending
        // TypeError that would surface at the next unrelated call, so the
        // detach probe is the `detached` getter — and the follow-up call here
        // is what proves it.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const buffer = new ArrayBuffer(4);
                     structuredClone(buffer, { transfer: [buffer] });
                     let name = "no throw";
                     try { structuredClone(buffer, { transfer: [buffer] }); }
                     catch (error) { name = error.name; }
                     return `${name}:${structuredClone({ ok: 1 }).ok}`;
                   })()"#
            ),
            Ok("DataCloneError:1".to_owned())
        );
    }

    #[test]
    fn a_non_transferable_in_the_transfer_list_throws_data_clone_error() {
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     try { structuredClone({}, { transfer: [{}] }); return "no throw"; }
                     catch (error) { return error.name; }
                   })()"#
            ),
            Ok("DataCloneError".to_owned())
        );
    }

    #[test]
    fn every_forbidden_type_throws_data_clone_error() {
        for expression in [
            "Symbol('x')",
            "Object(Symbol('x'))",
            "() => {}",
            "class Nope {}",
            "new Proxy({}, {})",
            "Promise.resolve()",
            "new WeakMap()",
            "new WeakSet()",
            "new WeakRef({})",
            "new FinalizationRegistry(() => {})",
            "(function* generate() {})()",
            "(function () { return arguments; })()",
        ] {
            assert_eq!(clone_failure(expression), "DataCloneError", "{expression}");
        }
    }

    #[test]
    fn a_detached_buffer_inside_the_graph_throws_data_clone_error() {
        assert_eq!(
            clone_failure(
                "(() => { const b = new ArrayBuffer(4); b.transfer(); return { b }; })()"
            ),
            "DataCloneError"
        );
    }

    #[test]
    fn a_proxy_is_refused_without_running_a_single_trap() {
        assert_clone(
            r#"(() => {
                 let traps = 0;
                 const proxy = new Proxy({}, new Proxy({}, { get: () => { traps += 1; return undefined; } }));
                 try { structuredClone({ proxy }); } catch { return traps === 0; }
                 return false;
               })()"#,
        );
    }

    #[test]
    fn a_message_round_trips_between_two_runtimes() {
        // The real topology: serialise under one runtime's lock, rebuild under
        // another's. Only the `Message` crosses.
        let source = r#"(() => {
             const shared = { id: 7 };
             const value = { when: new Date(1000), why: new TypeError("boom"),
                             pair: new Map([["k", shared]]), also: shared,
                             bytes: new Uint8Array([1, 2, 3]) };
             value.self = value;
             return value;
           })()"#;
        let (_sender_runtime, sender) = worker_context();
        let message = sender.with(|ctx| {
            let value: Value<'_> = ctx.eval(source).expect("the fixture evaluates");
            Message::serialize(&ctx, value, vec![], vec![])
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("the fixture serialises")
        });

        let (_receiver_runtime, receiver) = worker_context();
        let summary: String = receiver.with(|ctx| {
            let (value, ports) = message
                .deserialize(&ctx)
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("the message deserialises");
            assert!(ports.is_empty());
            ctx.globals().set("received", value).expect("global set");
            ctx.eval::<String, _>(
                r#"[received.when.getTime(), received.why instanceof TypeError,
                    received.why.message, received.pair.get("k") === received.also,
                    received.bytes.join("-"), received.self === received].join()"#,
            )
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the checks evaluate")
        });
        assert_eq!(summary, "1000,true,boom,true,1-2-3,true");
    }
    #[test]
    fn a_transferred_port_moves_its_channel_and_detaches_the_source() {
        // The JS `MessagePort` wrapper is the port prelude's business; what is
        // owned here is that the channel end travels with the message and the
        // sender's port is left detached, so a second transfer fails.
        let (moved, peer) = PortHandle::pair();
        let (_sender_runtime, sender) = worker_context();
        let (message, resent) = sender.with(|ctx| {
            let port = Class::instance(ctx.clone(), NativePort::from_handle(moved))
                .expect("the native port instantiates");
            let message = Message::serialize(
                &ctx,
                Value::new_null(ctx.clone()),
                vec![],
                vec![port.clone()],
            )
            .catch(&ctx)
            .map_err(|err| err.to_string())
            .expect("the port transfers");
            assert!(!port.borrow().is_open(), "the source port is detached");
            let resent = Message::serialize(&ctx, Value::new_null(ctx.clone()), vec![], vec![port])
                .catch(&ctx)
                .map_err(thrown_name)
                .unwrap_err();
            (message, resent)
        });
        assert_eq!(resent, "DataCloneError");

        let (_receiver_runtime, receiver) = worker_context();
        receiver.with(|ctx| {
            let (_, ports) = message
                .deserialize(&ctx)
                .catch(&ctx)
                .map_err(|err| err.to_string())
                .expect("the message deserialises");
            assert_eq!(ports.len(), 1, "exactly one port arrived");
            let port = &ports[0];
            assert!(port.borrow().is_open());
            port.borrow()
                .take_handle()
                .expect("the port still holds its channel")
                .send(Envelope::Close)
                .expect("the peer is still listening");
        });

        let mut peer = peer;
        assert!(matches!(
            peer.take_receiver()
                .expect("the peer keeps its inbox")
                .try_recv(),
            Ok(Envelope::Close)
        ));
    }

    #[test]
    fn the_same_port_twice_in_the_transfer_list_throws_data_clone_error() {
        let (handle, _peer) = PortHandle::pair();
        let (_runtime, context) = worker_context();
        let failure = context.with(|ctx| {
            let port = Class::instance(ctx.clone(), NativePort::from_handle(handle))
                .expect("the native port instantiates");
            Message::serialize(
                &ctx,
                Value::new_null(ctx.clone()),
                vec![],
                vec![port.clone(), port],
            )
            .catch(&ctx)
            .map_err(thrown_name)
            .unwrap_err()
        });
        assert_eq!(failure, "DataCloneError");
    }

    #[test]
    fn a_getter_that_invalidates_the_transfer_list_during_the_walk_transfers_nothing() {
        // The transfer list is validated before the serialisation walk, but the
        // walk runs every getter in the graph, and a getter is free to close a
        // port or detach a buffer that was valid a moment earlier. Revalidating
        // afterwards is what keeps the refusal atomic: without it the buffer is
        // detached first and the port's refusal arrives too late to undo it.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const kept = new MessageChannel();
                     const closed = new MessageChannel();
                     const buffer = new ArrayBuffer(8);
                     const value = { get sneaky() { closed.port1.close(); return 1; } };
                     let name = "no throw";
                     try {
                       structuredClone(value, { transfer: [buffer, kept.port1, closed.port1] });
                     } catch (error) { name = error.name; }
                     // The port listed *before* the offending one must still
                     // hold its channel, which is only observable by
                     // transferring it: a moved-out port refuses.
                     let again = "no throw";
                     try { structuredClone(null, { transfer: [kept.port1] }); again = "transferable"; }
                     catch (error) { again = error.name; }
                     return `${name}:${buffer.detached}:${again}`;
                   })()"#
            ),
            Ok("DataCloneError:false:transferable".to_owned())
        );
    }

    #[test]
    fn a_view_left_out_of_bounds_by_a_shrunk_resizable_buffer_throws_data_clone_error() {
        // quickjs' writer records the view's stale offset without complaint and
        // the *reader* is the one that refuses it ("invalid offset"), so the
        // failure used to surface as a `RangeError` — or, across a worker, as a
        // far-side `messageerror`.
        assert_eq!(
            clone_failure(
                r#"(() => {
                     const buffer = new ArrayBuffer(8, { maxByteLength: 8 });
                     const view = new Uint8Array(buffer, 4);
                     buffer.resize(0);
                     return view;
                   })()"#
            ),
            "DataCloneError"
        );
    }

    /// The DataView half of the same rule. This one always threw
    /// synchronously — but with quickjs's own `TypeError: ArrayBuffer is
    /// detached or resized`, escaping from the `byteOffset` read the DataView
    /// branch of the walk does, where HTML asks for a DataCloneError.
    #[test]
    fn an_out_of_bounds_data_view_throws_data_clone_error_rather_than_a_type_error() {
        assert_eq!(
            clone_failure(
                r#"(() => {
                     const buffer = new ArrayBuffer(8, { maxByteLength: 8 });
                     const view = new DataView(buffer, 4);
                     buffer.resize(0);
                     return view;
                   })()"#
            ),
            "DataCloneError"
        );
    }

    /// The rule must not swallow the legitimately empty view it resembles:
    /// quickjs reports byteOffset 0 and length 0 for an out-of-bounds typed
    /// array, which is exactly what a zero-length view in bounds reports too.
    #[test]
    fn a_zero_length_view_in_bounds_still_clones() {
        assert_clone(
            r#"(() => {
                 const resizable = new ArrayBuffer(8, { maxByteLength: 8 });
                 const out = structuredClone({
                   empty: new Uint8Array(new ArrayBuffer(0)),
                   emptyInResizable: new Uint8Array(resizable, 0, 0),
                   emptyView: new DataView(resizable, 8),
                 });
                 return out.empty.length === 0 && out.emptyInResizable.length === 0
                   && out.emptyView.byteLength === 0;
               })()"#,
        );
    }

    #[test]
    fn a_buffer_sealed_with_an_own_transfer_property_is_refused_and_left_intact() {
        // `WebAssembly.Memory#buffer` is sealed exactly this way
        // (den-stdlib-wasm/src/memory.rs `seal_against_transfer`), which is how
        // den spells `[[ArrayBufferDetachKey]]`. Transferring one would detach
        // the wasm linear memory out from under a live instance, so the guard
        // has to hold for any buffer carrying an own `transfer`.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const buffer = new ArrayBuffer(8);
                     Object.defineProperty(buffer, "transfer", {
                       value: () => { throw new TypeError("sealed"); },
                     });
                     let name = "no throw";
                     try { structuredClone(buffer, { transfer: [buffer] }); }
                     catch (error) { name = error.name; }
                     return `${name}:${buffer.detached}:${buffer.byteLength}`;
                   })()"#
            ),
            Ok("DataCloneError:false:8".to_owned())
        );
    }

    /// Transfer is all-or-nothing. A refusal that `validate_ports` misses is
    /// found by `take_handle` instead — halfway through the mutation, with the
    /// buffers already detached and the earlier ports already moved out, and
    /// nothing hands those back. A started port is exactly such a refusal.
    #[tokio::test]
    async fn a_started_port_is_refused_before_any_buffer_or_port_is_transferred() {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        // The peers are held for the length of the test: a dropped peer closes
        // the port, which would make `is_open` false for the wrong reason.
        let (first, _first_peer) = PortHandle::pair();
        let (second, _second_peer) = PortHandle::pair();
        let outcome: String = context
            .with(|ctx| {
                let run = || -> Result<String, rquickjs::Error> {
                    let (_, evaluated) =
                        Module::evaluate_def::<crate::js_worker, _>(ctx.clone(), "den:worker")?;
                    evaluated.finish::<()>()?;

                    let moved = Class::instance(ctx.clone(), NativePort::from_handle(first))?;
                    let started = Class::instance(ctx.clone(), NativePort::from_handle(second))?;
                    let noop = Function::new(ctx.clone(), || {})?;
                    started
                        .borrow()
                        .start(ctx.clone(), noop.clone(), noop.clone(), noop.clone());

                    let buffer = ArrayBuffer::new_copy(ctx.clone(), [1u8, 2, 3, 4])?;
                    let failure = Message::serialize(
                        &ctx,
                        Value::new_null(ctx.clone()),
                        vec![buffer.as_value().clone()],
                        vec![moved.clone(), started],
                    )
                    .catch(&ctx)
                    .map_err(thrown_name)
                    .expect_err("a started port cannot be transferred");
                    Ok(format!(
                        "{failure}:{}:{}",
                        moved.borrow().is_open(),
                        buffer.as_object().get::<_, bool>("detached")?
                    ))
                };
                run().catch(&ctx).map_err(|err| err.to_string())
            })
            .await
            .expect("the fixture runs");
        // The port earlier in the list kept its channel and the buffer is still
        // attached: the refusal cost the caller nothing.
        assert_eq!(outcome, "DataCloneError:true:false");
    }

    #[test]
    fn an_own_proto_data_property_survives_without_reparenting_the_clone() {
        // What `JSON.parse('{"__proto__":1}')` produces: an own *data* property
        // whose name is the one `Object.prototype` exposes as an accessor. Built
        // with assignment, the accessor swallows it and the property vanishes
        // from every cloned object; built with CreateDataProperty it survives,
        // and the clone's prototype is still `Object.prototype`.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const value = JSON.parse('{"__proto__": {"polluted": true}, "keep": 2}');
                     const out = structuredClone(value);
                     return [Object.hasOwn(out, "__proto__"),
                             out.__proto__.polluted === true,
                             Object.getPrototypeOf(out) === Object.prototype,
                             out.keep].join();
                   })()"#
            ),
            Ok("true,true,true,2".to_owned())
        );
    }

    #[test]
    fn a_poisoned_object_prototype_accessor_neither_sees_nor_swallows_a_cloned_property() {
        // An inherited setter intercepts [[Set]] on a fresh output object: the
        // data reaches the attacker and no own property is created, so the
        // clone silently loses the key. `cause` covers both halves at once —
        // the sender's tag object and the receiver's rebuilt `Error` are each
        // given one, and neither has it as an own property beforehand.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const leaked = [];
                     const poison = {
                       configurable: true,
                       set(value) { leaked.push(value); },
                       get() { return "intercepted"; },
                     };
                     Object.defineProperty(Object.prototype, "secret", poison);
                     Object.defineProperty(Object.prototype, "cause", poison);
                     try {
                       const out = structuredClone({ secret: 5 });
                       const error = structuredClone(new Error("boom", { cause: "why" }));
                       return [leaked.length, Object.hasOwn(out, "secret"), out.secret,
                               Object.hasOwn(error, "cause"), error.cause].join();
                     } finally {
                       Reflect.deleteProperty(Object.prototype, "secret");
                       Reflect.deleteProperty(Object.prototype, "cause");
                     }
                   })()"#
            ),
            Ok("0,true,5,true,why".to_owned())
        );
    }

    #[test]
    fn a_key_deleted_by_an_earlier_getter_is_omitted_rather_than_cloned_as_undefined() {
        // The walk snapshots the key list, so a key deleted while it runs is
        // still in that snapshot; ownership is re-checked to keep it out of the
        // output instead of reviving it as an own `undefined`.
        assert_eq!(
            eval::<String>(
                r#"(() => {
                     const value = { first: 0 };
                     // Ahead of `second` in insertion order, so the delete lands
                     // while `second` is still in the snapshot the walk took.
                     Object.defineProperty(value, "trap", {
                       enumerable: true, configurable: true,
                       get() { delete value.second; return 1; },
                     });
                     value.second = 2;
                     const out = structuredClone(value);
                     return [Object.hasOwn(out, "second"), out.trap, Object.keys(out).join("+")].join();
                   })()"#
            ),
            Ok("false,1,first+trap".to_owned())
        );
    }

    #[test]
    fn every_forbidden_type_names_itself_in_the_data_clone_error_message() {
        // `every_forbidden_type_throws_data_clone_error` passes with the JS
        // pre-screen deleted, because the writer refuses these too — with a
        // `TypeError` that Rust re-tags as "the value could not be cloned: …".
        // The message is the only evidence of which of the two refused, so it
        // is what pins the pre-screen.
        for (expression, message) in [
            ("Promise.resolve()", "Promise could not be cloned."),
            ("new WeakMap()", "WeakMap could not be cloned."),
            ("new WeakSet()", "WeakSet could not be cloned."),
            ("new WeakRef({})", "WeakRef could not be cloned."),
            (
                "new FinalizationRegistry(() => {})",
                "FinalizationRegistry could not be cloned.",
            ),
            (
                "new SharedArrayBuffer(8)",
                "SharedArrayBuffer could not be cloned.",
            ),
            ("new Proxy({}, {})", "#<Proxy> could not be cloned."),
            ("Symbol('x')", "Symbol(x) could not be cloned."),
            ("function named() {}", "function named could not be cloned."),
            ("() => {}", "function (anonymous) could not be cloned."),
        ] {
            assert_eq!(
                eval::<String>(&format!(
                    r#"(() => {{
                         try {{ structuredClone({expression}); return "no throw"; }}
                         catch (error) {{ return `${{error.name}}: ${{error.message}}`; }}
                       }})()"#
                )),
                Ok(format!("DataCloneError: {message}")),
                "{expression}"
            );
        }
    }
}
