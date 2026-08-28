//! JS functions C can call — `docs/research/19-den-ffi.md` §4.7.
//!
//! libffi allocates an executable trampoline whose code pointer C may hold and
//! whose userdata is a plain-data [`Slot`]. The JS function itself never
//! reaches the trampoline: no JS value is `'static`, so the slot carries an
//! index into the realm's registry, exactly as `den-stdlib-wasm` does for a
//! wasm import (`instance.rs`, `ImportedFunctions`).
//!
//! **This phase serves the owner thread only.** A trampoline entered from a
//! thread C created returns the zero value and writes one line to stderr; the
//! mailbox that makes it a real call is phase 5.
//!
//! # The lifetime contract, which is the whole hazard
//!
//! C holds a raw code pointer. Nothing tells den when C stops holding it, and
//! a call into a freed trampoline is a jump into unmapped memory — den cannot
//! turn that into a throw. So den never frees one while its realm lives:
//! `Symbol.dispose` drops the *JS function* (which is what stops the callback
//! being callable and lets it be collected) and leaves the trampoline mapped,
//! answering zero. What den cannot survive is C calling after the realm is
//! gone; that is the caller's contract, and it is in the `.d.ts`.

use std::{
    cell::RefCell,
    ffi::c_void,
    panic::{self, AssertUnwindSafe},
    pin::Pin,
    sync::Arc,
    thread::{self, ThreadId},
};

use den_stdlib_core::exceptions::report_uncaught;
use den_util::OwnedCtx;
use libffi::{
    low::ffi_cif,
    middle::{Cif, Closure},
};
use rquickjs::{
    Class, Ctx, Exception, Function, JsLifetime, Result, Value, class::Trace, function::Args,
};

use crate::{
    error::ErrorKind,
    library,
    marshal::{self, ArgumentCell},
    pointer::Pointer,
    schema::{FnSig, NativeType},
};

/// The handle JS holds. Everything mutable about a callback lives in the
/// realm's registry, so a disposed handle cannot answer a stale address.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "Callback")]
pub struct Callback {
    #[qjs(skip_trace)]
    index:     usize,
    /// What the handle claims to be, checked against the slot it is passed to.
    #[qjs(skip_trace)]
    signature: Arc<FnSig>,
}

#[rquickjs::methods]
impl Callback {
    /// The trampoline's address, as an ordinary [`Pointer`] — for a C API that
    /// takes a bare `pointer` rather than a declared `{ callback }` slot. It
    /// carries no library provenance: den made this address itself.
    #[qjs(get)]
    pub fn pointer<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let address = FfiRealm::code_address(&ctx, self.index)?;
        Pointer::to_js(&ctx, address, None)
    }
}

impl Callback {
    /// The address to pass for a declared `{ callback }` parameter.
    ///
    /// The signature check is the run-time half of the `.d.ts` brand: a handle
    /// whose signature is not the slot's would have libffi read the wrong
    /// registers, which is undefined behaviour with no diagnostic anywhere
    /// (§0 fact 1). Both halves are cheap; only this one holds for plain JS.
    pub fn code_pointer<'js>(
        ctx: &Ctx<'js>, declared: &Arc<FnSig>, value: &Value<'js>,
    ) -> Result<*mut c_void> {
        let handle = value
            .as_object()
            .and_then(Class::<Self>::from_object)
            .ok_or_else(|| {
                ErrorKind::BadArgument.throw(
                    ctx,
                    format_args!(
                        "expected a Callback for `{}` — a plain function is not a C function \
                         pointer",
                        declared.describe()
                    ),
                )
            })?;
        let handle = handle.borrow();
        if *handle.signature != **declared {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                format_args!(
                    "this callback is `{}`, but the symbol declares `{}`",
                    handle.signature.describe(),
                    declared.describe()
                ),
            ));
        }
        Ok(std::ptr::with_exposed_provenance_mut(
            FfiRealm::code_address(ctx, handle.index)?,
        ))
    }
}

/// `callback(def, fn)` — mint a C function pointer for `function`.
pub fn create<'js>(
    ctx: Ctx<'js>, definition: Value<'js>, function: Function<'js>,
) -> Result<Value<'js>> {
    let signature = Arc::new(FnSig::parse(&ctx, "callback", &definition)?);
    let cif = Cif::new(
        signature.params.iter().map(|param| param.ffi_type()),
        signature.result.ffi_type(),
    );
    let index = FfiRealm::register(&ctx, function, cif, &signature)?;

    // `Symbol.dispose` is an own property rather than a class method for the
    // same reason `Library`'s is (§4.8): it keeps the reserved key a symbol.
    let object = Class::instance(ctx.clone(), Callback { index, signature })?.into_inner();
    object.set(
        library::dispose_key(&ctx)?,
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            FfiRealm::release(&ctx, index)
        })?
        .with_name("[Symbol.dispose]")?,
    )?;
    Ok(object.into_value())
}

/// Every callback this realm has minted, parked in context userdata.
///
/// Userdata is the one place a `'js` value may be held for the realm's whole
/// life, and rquickjs clears it *before* `JS_FreeRuntime`
/// (`rquickjs-core/src/runtime/opaque.rs`) — which is what makes a live
/// callback at exit an ordinary shutdown rather than QuickJS's
/// `list_empty(&rt->gc_obj_list)` abort. `Box::leak` would be that abort.
#[derive(JsLifetime, Default)]
pub struct FfiRealm<'js> {
    registered: RefCell<Vec<Registration<'js>>>,
}

/// One callback: the machinery C sees, and the function JS supplied.
///
/// Declaration order is drop order, and the trampoline reaches the function
/// through the registry, so the executable code dies first.
struct Registration<'js> {
    entry:    CallbackEntry,
    /// `None` once disposed. It is the single liveness flag: dropping the
    /// function is what makes every later dispatch — a `.pointer` read, a
    /// marshalled argument, a call from C — see a dead callback.
    function: Option<Function<'js>>,
}

// SAFETY: the only `'js` value here is `function` — `CallbackEntry` is
// `'static` — so this is the same type for every choice of `'to`, which is
// what the `JsLifetime` derive would write if it could see through a field
// that borrows no lifetime.
unsafe impl<'js> JsLifetime<'js> for Registration<'js> {
    type Changed<'to> = Registration<'to>;
}

/// The libffi closure and the slot it points at.
///
/// **Drop order is load-bearing.** Struct fields drop in declaration order,
/// and `closure` holds a raw pointer into `slot`, so `closure` is declared —
/// and therefore dropped — first.
struct CallbackEntry {
    closure: Closure<'static>,
    #[expect(
        dead_code,
        reason = "never read: it owns the pointee `closure` holds, and drops after it"
    )]
    slot:    Pin<Box<Slot>>,
}

impl CallbackEntry {
    fn new(cif: Cif, slot: Slot) -> Self {
        let slot = Box::pin(slot);
        // SAFETY: the one lifetime widening this crate performs, and the
        // alternative to `Box::leak`, which shuts the process down with an
        // assertion (§0 fact 11). `Closure::new` *borrows* its userdata and a
        // `Closure<'static>` therefore demands a `&'static Slot`; the pointee
        // is pinned behind a `Box`, so its address is stable for this struct's
        // whole life, `closure` is declared first and so is dropped before it,
        // and no `&Slot` derived from this ever escapes the trampoline.
        let userdata: &'static Slot = unsafe { &*std::ptr::from_ref(&*slot) };
        Self {
            closure: Closure::new(cif, trampoline, userdata),
            slot,
        }
    }

    /// The trampoline's own address. `Closure::code_ptr` hands back a
    /// *reference to* the code pointer, so this dereferences it: the reference
    /// itself points into the `Closure`, and passing that to C is a jump into
    /// data.
    fn address(&self) -> usize { *self.closure.code_ptr() as usize }
}

/// What the trampoline gets instead of a JS value: an index, a signature, the
/// thread that may run JS, and the parked context to run it in.
struct Slot {
    index:     usize,
    owner:     ThreadId,
    signature: Arc<FnSig>,
    /// Read **only** after `owner` matches the calling thread.
    reentrant: OwnedCtx,
}

// SAFETY: C decides which thread calls a trampoline, so `&Slot` crosses
// threads whether den likes it or not. Every field but `reentrant` is
// `Send + Sync` plain data. `reentrant` is an `OwnedCtx`, which is neither,
// and [`Slot::invoke`] reaches it only after `thread::current().id() ==
// self.owner` — i.e. only from the thread that built it. That comparison is
// the invariant this impl asserts, and it is the seam phase 5 replaces with a
// mailbox rather than removes.
unsafe impl Sync for Slot {}

impl Slot {
    /// # Safety
    ///
    /// `out` and `arguments` must be the buffers libffi handed the trampoline
    /// for the CIF this slot's signature built.
    unsafe fn invoke(&self, out: *mut c_void, arguments: *const *const c_void) {
        if thread::current().id() != self.owner {
            // Phase 5 posts to a mailbox and blocks here. Until then the honest
            // answer is the zero value and a line saying so: no JS value in the
            // process may be touched from this thread.
            eprintln!(
                "den:ffi: a callback `{}` was invoked from a thread den does not own; C got the \
                 zero value. Off-thread callbacks are not implemented yet.",
                self.signature.describe()
            );
            return;
        }
        // SAFETY: this is the owner thread, and C can only have reached this
        // thread through a call JS made — so the frame below us is a den:ffi
        // symbol call, which holds the runtime lock for its whole duration.
        // That is exactly what `OwnedCtx::with` requires of its caller.
        self.reentrant.with(|ctx| {
            // SAFETY: the caller's contract, restated on this function.
            let outcome = unsafe { self.call(ctx, out, arguments) };
            // A JS throw has no C caller to propagate to: C gets the zero
            // value written before this ran, and the exception is reported the
            // way any uncaught one is.
            report_uncaught(ctx, outcome);
        });
    }

    /// # Safety
    ///
    /// As [`Slot::invoke`].
    unsafe fn call(
        &self, ctx: &Ctx<'_>, out: *mut c_void, arguments: *const *const c_void,
    ) -> Result<()> {
        let function = FfiRealm::function(ctx, self.index)?;
        let marshalled = self
            .signature
            .params
            .iter()
            .enumerate()
            .map(|(position, declared)| {
                // SAFETY: libffi hands one pointer per parameter the CIF
                // declares, and the CIF was built from this same signature, so
                // `position` is in bounds and the pointee has the declared
                // type. A callback argument carries no library provenance.
                unsafe {
                    let cell = *arguments.add(position);
                    marshal::read(ctx, *declared, cell.cast::<u8>(), None)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let mut arguments = Args::new(ctx.clone(), marshalled.len());
        arguments.push_args(marshalled)?;
        let returned: Value<'_> = function.call_arg(arguments)?;

        if self.signature.result == NativeType::Void {
            return Ok(());
        }
        let cell = ArgumentCell::scalar(ctx, self.signature.result, &returned)?;
        // SAFETY: the caller's contract, and `cell` holds the type this slot's
        // CIF declares as its result.
        unsafe { marshal::write_return(out, &cell) };
        Ok(())
    }
}

/// The `extern "C"` function libffi's trampoline calls.
///
/// Wrapped whole in `catch_unwind`: a panic unwinding out of an `extern "C"`
/// frame is an abort (§0 fact 7), and the panic paths here are all reachable
/// from JS.
unsafe extern "C" fn trampoline(
    _cif: &ffi_cif, result: &mut c_void, arguments: *const *const c_void, slot: &Slot,
) {
    let out = std::ptr::from_mut(result);
    // Zero first, so that every path which cannot produce a value — a throw, a
    // panic, a foreign thread — leaves C a defined answer instead of whatever
    // the buffer held. It is still a wrong answer, and each of those paths
    // says so on stderr.
    //
    // SAFETY: `result` is libffi's return buffer for this closure's CIF, which
    // was built from `slot.signature`.
    unsafe { marshal::write_zero(out, slot.signature.result) };

    // SAFETY: `arguments` is libffi's argument vector for that same CIF.
    let ran = panic::catch_unwind(AssertUnwindSafe(|| unsafe { slot.invoke(out, arguments) }));
    if ran.is_err() {
        eprintln!("den:ffi: a callback panicked; C got the zero value.");
    }
}

impl<'js> FfiRealm<'js> {
    /// Install the registry. Called once, when `den:ffi` is evaluated.
    pub fn install(ctx: &Ctx<'js>) -> Result<()> {
        if ctx.userdata::<Self>().is_none() {
            ctx.store_userdata(Self::default())
                .map_err(|_borrowed| Self::busy(ctx))?;
        }
        Ok(())
    }

    fn register(
        ctx: &Ctx<'js>, function: Function<'js>, cif: Cif, signature: &Arc<FnSig>,
    ) -> Result<usize> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let mut registered = registry
            .registered
            .try_borrow_mut()
            .map_err(|_in_use| Self::busy(ctx))?;
        let index = registered.len();
        registered.push(Registration {
            entry:    CallbackEntry::new(cif, Slot {
                index,
                owner: thread::current().id(),
                signature: Arc::clone(signature),
                reentrant: OwnedCtx::new(ctx),
            }),
            function: Some(function),
        });
        Ok(index)
    }

    /// `Symbol.dispose`: the function goes, the trampoline stays (see the
    /// module docs). Idempotent, like every `Symbol.dispose`.
    fn release(ctx: &Ctx<'js>, index: usize) -> Result<()> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let mut registered = registry
            .registered
            .try_borrow_mut()
            .map_err(|_in_use| Self::busy(ctx))?;
        if let Some(entry) = registered.get_mut(index) {
            entry.function = None;
        }
        Ok(())
    }

    fn function(ctx: &Ctx<'js>, index: usize) -> Result<Function<'js>> {
        Self::with_entry(ctx, index, |entry| entry.function.clone())
    }

    fn code_address(ctx: &Ctx<'js>, index: usize) -> Result<usize> {
        Self::with_entry(ctx, index, |entry| {
            entry.function.as_ref().map(|_live| entry.entry.address())
        })
    }

    /// A disposed callback is `Closed`, the same word a disposed library uses.
    fn with_entry<R>(
        ctx: &Ctx<'js>, index: usize, read: impl FnOnce(&Registration<'js>) -> Option<R>,
    ) -> Result<R> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let registered = registry
            .registered
            .try_borrow()
            .map_err(|_in_use| Self::busy(ctx))?;
        registered
            .get(index)
            .and_then(read)
            .ok_or_else(|| ErrorKind::Closed.throw(ctx, "this callback is disposed"))
    }

    fn missing(ctx: &Ctx<'_>) -> rquickjs::Error {
        Exception::throw_internal(
            ctx,
            "the den:ffi callback registry is missing from this realm",
        )
    }

    /// Nothing here runs JS while the registry is borrowed, so this is
    /// belt and braces — but a `RefCell` on a JS-reachable path may not panic.
    fn busy(ctx: &Ctx<'_>) -> rquickjs::Error {
        Exception::throw_internal(ctx, "the den:ffi callback registry is already in use")
    }
}
