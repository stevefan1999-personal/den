//! JS functions C can call — `docs/research/19-den-ffi.md` §4.7.
//!
//! libffi allocates an executable trampoline whose code pointer C may hold and
//! whose userdata is a plain-data [`Slot`]. The JS function itself never
//! reaches the trampoline: no JS value is `'static`, so the slot carries an
//! index into the realm's registry, exactly as `den-stdlib-wasm` does for a
//! wasm import (`instance.rs`, `ImportedFunctions`).
//!
//! A trampoline entered from the realm's own thread re-enters JS directly. One
//! entered from a thread C created posts to a mailbox and blocks on the reply,
//! and a `ctx.spawn`-ed pump on the realm side answers it. That pump is also
//! what keeps den alive while a callback is armed (ARCHITECTURE §7.5 rule 1);
//! `Symbol.dispose` is what takes it back.
//!
//! The wait for that reply is **bounded**, and the bound is not a nicety. A
//! callback stored by C and fired from a thread a *synchronous* symbol then
//! joins is a cycle den cannot see coming (§4.7): the realm is inside C, so it
//! can never reach the pump. On expiry the trampoline hands C the zero value
//! and says so on stderr, which lets C's thread finish, the join return, and
//! the realm resume. That converts a permanent deadlock into a bounded stall
//! plus a wrong answer plus a diagnostic, which is the only trade available
//! (§5.1 item 5).
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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, SyncSender, sync_channel},
    },
    thread::{self, ThreadId},
    time::Duration,
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
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::ErrorKind,
    library,
    marshal::{self, ArgumentCell, Cell},
    pointer::Pointer,
    schema::{FnSig, NativeType},
};

/// How long a foreign thread waits for the realm before giving up, handing C
/// the zero value and saying so.
///
/// Long enough that a realm merely busy with other work answers in time; short
/// enough that §4.7's residual deadlock is a stall a human notices rather than
/// a process that never comes back.
const FOREIGN_CALL_TIMEOUT: Duration = Duration::from_secs(5);

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
        ctx: &Ctx<'js>, declared: &Arc<FnSig>, value: &Value<'js>, origin: &Arc<str>,
    ) -> Result<usize> {
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
        // Where C got this address is the only attribution den has when the
        // callback comes back on a thread of C's own and nobody answers.
        FfiRealm::record_origin(ctx, handle.index, origin)?;
        FfiRealm::code_address(ctx, handle.index)
    }
}

/// `callback(def, fn)` — mint a C function pointer for `function`.
///
/// Registering one spawns the realm-side pump, so from here until
/// `Symbol.dispose` the process stays alive: C may call at any moment, and
/// den has no way to know it will not.
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
    /// Dropping this ends the realm-side pump, which is what lets `idle()`
    /// resolve again (ARCHITECTURE §7.5 rule 1). Held rather than sent through:
    /// the drop *is* the message.
    stop:     Option<oneshot::Sender<()>>,
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
        // and no `&Slot` derived from this outlives the registry borrow it is
        // taken through.
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
/// thread that may run JS, the parked context to run it in, and the way back
/// into the realm from any other thread.
struct Slot {
    index:     usize,
    owner:     ThreadId,
    signature: Arc<FnSig>,
    /// The realm-side pump's end. Closed once the callback is disposed, which
    /// is how a late call from C becomes a logged zero rather than a wait.
    mailbox:   mpsc::UnboundedSender<Call>,
    /// `lib.so::symbol`, the last place C was handed this address — written on
    /// the realm thread at marshal time, read from a foreign thread when the
    /// wait expires. Nothing but the diagnostic depends on it, so a miss is a
    /// vaguer message and never a wrong call.
    origin:    Mutex<Option<Arc<str>>>,
    /// Read **only** after `owner` matches the calling thread.
    reentrant: OwnedCtx,
}

/// One foreign-thread callback on its way to the realm, and the way back.
struct Call {
    index:     usize,
    signature: Arc<FnSig>,
    /// The arguments as raw C bytes: this is built on a thread that may not
    /// touch a JS value, so the realm is what turns them into one.
    arguments: Vec<Cell>,
    /// The realm answers with one value, or with `None` for a `void` result.
    /// Dropping the sender instead is how a throw reaches C — the zero the
    /// trampoline already wrote stands, and the exception is reported on the
    /// realm side.
    reply:     SyncSender<Option<ArgumentCell>>,
    /// Cleared when the foreign thread gives up waiting. A request that
    /// outlived its caller must not run: C's frame is gone, so any address
    /// among the arguments is now dangling.
    ///
    /// ponytail: this narrows the window rather than closing it — a request the
    /// pump starts serving in the same instant the wait expires still runs.
    /// Closing it needs the reply channel to report its own disconnection,
    /// which `std::sync::mpsc` does not offer and `tokio::sync::oneshot` offers
    /// without a bounded blocking receive.
    alive:     Arc<AtomicBool>,
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
        // SAFETY: the caller's contract, restated on this function.
        let arguments = unsafe { self.arguments(arguments) };
        if thread::current().id() == self.owner {
            // C called back inside a call JS made, so the runtime lock is held
            // by the frame below us — which is exactly what `OwnedCtx::with`
            // requires of its caller.
            self.reentrant.with(|ctx| {
                match FfiRealm::serve(ctx, self.index, &self.signature, &arguments) {
                    // SAFETY: the caller's contract, and `cell` holds the type
                    // this slot's CIF declares as its result.
                    Ok(Some(cell)) => unsafe { marshal::write_return(out, &cell) },
                    Ok(None) => {}
                    // A JS throw has no C caller to propagate to: C keeps the
                    // zero value written before this ran, and the exception is
                    // reported the way any uncaught one is.
                    Err(thrown) => report_uncaught(ctx, Err(thrown)),
                }
            });
            return;
        }
        // SAFETY: the caller's contract, restated on this function.
        unsafe { self.post(out, arguments) };
    }

    /// The foreign-thread branch: no JS value in this process may be touched
    /// from here, so the arguments travel as bytes and the realm's pump does
    /// the call.
    ///
    /// # Safety
    ///
    /// As [`Slot::invoke`].
    unsafe fn post(&self, out: *mut c_void, arguments: Vec<Cell>) {
        let (reply, answer) = sync_channel(1);
        let alive = Arc::new(AtomicBool::new(true));
        let posted = self.mailbox.send(Call {
            index: self.index,
            signature: Arc::clone(&self.signature),
            arguments,
            reply,
            alive: Arc::clone(&alive),
        });
        if posted.is_err() {
            eprintln!(
                "den:ffi: `{}` called a disposed callback `{}` from its own thread; C got the \
                 zero value.",
                self.describe_origin(),
                self.signature.describe()
            );
            return;
        }
        match answer.recv_timeout(FOREIGN_CALL_TIMEOUT) {
            // SAFETY: the caller's contract, and the realm marshalled `cell`
            // against this slot's declared result type.
            Ok(Some(cell)) => unsafe { marshal::write_return(out, &cell) },
            // A `void` result, or a throw the realm has already reported.
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => {
                alive.store(false, Ordering::Release);
                eprintln!(
                    "den:ffi: no answer from the realm within {:?} for the callback `{}` that \
                     `{}` was given; C got the zero value. The realm is most likely inside a \
                     synchronous call to that library which is itself waiting on this thread — \
                     see docs/research/19-den-ffi.md §5.1 item 5.",
                    FOREIGN_CALL_TIMEOUT,
                    self.signature.describe(),
                    self.describe_origin()
                );
            }
        }
    }

    /// Copy every argument out of libffi's vector, so that the bytes outlive
    /// the trampoline frame and can cross a thread.
    ///
    /// # Safety
    ///
    /// As [`Slot::invoke`].
    unsafe fn arguments(&self, arguments: *const *const c_void) -> Vec<Cell> {
        self.signature
            .params
            .iter()
            .enumerate()
            .map(|(position, declared)| {
                // SAFETY: libffi hands one pointer per parameter the CIF
                // declares, and the CIF was built from this same signature, so
                // `position` is in bounds and the pointee has the declared type.
                unsafe { Cell::read_from(*declared, (*arguments.add(position)).cast::<u8>()) }
            })
            .collect()
    }

    /// The symbol C was handed this callback through, for a diagnostic. A
    /// callback passed as a bare `pointer` was never recorded, and a poisoned
    /// lock is not worth a second failure on a path that is already failing.
    fn describe_origin(&self) -> Arc<str> {
        self.origin
            .lock()
            .ok()
            .and_then(|origin| origin.clone())
            .unwrap_or_else(|| Arc::from("an unrecorded symbol"))
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
        let (mailbox, requests) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel();
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
                mailbox,
                origin: Mutex::new(None),
                reentrant: OwnedCtx::new(ctx),
            }),
            function: Some(function),
            stop:     Some(stop),
        });
        // Before the pump exists there is nothing borrowing the registry, and
        // the pump borrows it on every request.
        drop(registered);
        ctx.spawn(Self::pump(ctx.clone(), requests, stopped));
        Ok(index)
    }

    /// The realm side of the mailbox.
    ///
    /// This future is the process-lifetime mechanism for callbacks, the same
    /// one `den-stdlib-worker`'s ports use: `AsyncRuntime::idle()` resolves
    /// only when no `ctx.spawn`-ed future is left (doc 09 §2.2), so an armed
    /// callback keeps den running and `Symbol.dispose` — which drops the
    /// `stop` sender — is how that is taken back. There is no `ref()`/`unref()`
    /// because there is nothing left for one to do.
    async fn pump(
        ctx: Ctx<'js>, mut requests: mpsc::UnboundedReceiver<Call>, stopped: oneshot::Receiver<()>,
    ) {
        let mut stopped = std::pin::pin!(stopped);
        loop {
            let call = tokio::select! {
                // Biased, and disposal first: a disposed callback must not
                // serve a request that raced it in.
                biased;
                _disposed = &mut stopped => return,
                call = requests.recv() => match call {
                    Some(call) => call,
                    None => return,
                },
            };
            if !call.alive.load(Ordering::Acquire) {
                continue;
            }
            match Self::serve(&ctx, call.index, &call.signature, &call.arguments) {
                Ok(answer) => {
                    // The foreign thread is blocked on this and nothing else
                    // holds the other end, so a failure here means it gave up
                    // waiting; C already has the zero value.
                    let _gave_up = call.reply.send(answer);
                }
                // Dropping `call` drops the reply sender, which is what tells
                // the foreign thread to keep the zero it already wrote.
                Err(thrown) => report_uncaught(&ctx, Err(thrown)),
            }
        }
    }

    /// Run one callback on the realm thread, whichever branch asked for it.
    /// `None` is a `void` result, which writes nothing at all.
    fn serve(
        ctx: &Ctx<'js>, index: usize, signature: &FnSig, arguments: &[Cell],
    ) -> Result<Option<ArgumentCell>> {
        let function = Self::function(ctx, index)?;
        let marshalled = signature
            .params
            .iter()
            .zip(arguments)
            .map(|(declared, cell)| {
                // SAFETY: `cell` holds the bytes of a C value of exactly this
                // declared type, copied out of libffi's argument vector. A
                // callback argument carries no library provenance: den is told
                // an address and nothing about where it came from.
                unsafe { marshal::read(ctx, *declared, cell.as_ptr(), None) }
            })
            .collect::<Result<Vec<_>>>()?;
        let mut call = Args::new(ctx.clone(), marshalled.len());
        call.push_args(marshalled)?;
        let returned: Value<'_> = function.call_arg(call)?;

        if signature.result == NativeType::Void {
            return Ok(None);
        }
        Ok(Some(ArgumentCell::scalar(
            ctx,
            signature.result,
            &returned,
        )?))
    }

    /// Note which symbol C was handed this callback through, for the
    /// foreign-thread diagnostic. Last writer wins: a callback passed to two
    /// symbols names the more recent one, which is the better guess.
    fn record_origin(ctx: &Ctx<'js>, index: usize, origin: &Arc<str>) -> Result<()> {
        Self::with_entry(ctx, index, |entry| {
            entry.entry.slot.origin.lock().ok().map(|mut recorded| {
                *recorded = Some(Arc::clone(origin));
            })
        })
    }

    /// `Symbol.dispose`: the function goes and the pump ends, the trampoline
    /// stays (see the module docs). Idempotent, like every `Symbol.dispose`.
    fn release(ctx: &Ctx<'js>, index: usize) -> Result<()> {
        let registry = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let mut registered = registry
            .registered
            .try_borrow_mut()
            .map_err(|_in_use| Self::busy(ctx))?;
        if let Some(entry) = registered.get_mut(index) {
            entry.function = None;
            entry.stop = None;
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
