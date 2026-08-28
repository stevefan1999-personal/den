//! The single wasm store of a JS context.

use std::{cell::RefCell, ptr::NonNull, rc::Rc};

#[cfg(feature = "wasi")] use den_util::Probe;
#[cfg(feature = "wasi")]
use rquickjs::{Class, Object, Value, class::Trace};
use rquickjs::{Ctx, Exception, JsLifetime, Result};
use wasmtime::{AsContext, AsContextMut, Func, StoreContext, Val};

#[cfg(feature = "wasi")]
use crate::error::throw_link_error;
use crate::{backend, error::throw_runtime_error};

/// Handle to the one wasmtime [`backend::Store`] a JS context owns.
///
/// Every `Instance`, `Memory`, `Table` and `Global` of a context lives in this
/// store, which is what makes them interchangeable as imports.
#[derive(Clone, JsLifetime)]
pub struct Store {
    /// Prefer [`Store::with_mut`]: a bare `borrow_mut` panics on re-entry.
    pub(crate) inner: Rc<RefCell<backend::Store>>,
}

impl Store {
    /// Why a re-entrant use was refused, naming both what is holding the store
    /// and what the caller can do instead.
    const REENTRY_REFUSED: &'static str =
        "a WebAssembly export is still running and has called back into JS: this build cannot \
         re-enter its wasm store, so calling another export — or creating a Memory, Table, Global \
         or Tag — is unsupported until that call returns";

    pub fn new(engine: &wasmtime::Engine, ctx: &Ctx<'_>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(backend::Store::new(
                engine,
                backend::StoreData::new(ctx),
            ))),
        }
    }

    /// The store `den:wasm` installed in this context.
    pub fn from_ctx(ctx: &Ctx<'_>) -> Result<Self> {
        ctx.userdata::<Self>()
            .map(|store| store.clone())
            .ok_or_else(|| {
                Exception::throw_internal(ctx, "the WebAssembly store is missing from this context")
            })
    }

    /// Run `f` with the store mutably borrowed, refusing re-entrant use.
    ///
    /// The refusal is a `WebAssembly.RuntimeError`, never a panic: the only way
    /// to get here twice is from JS — a host function called by a running
    /// export — so a `borrow_mut` would be a JS-reachable abort.
    ///
    /// ponytail: creating a Memory / Table / Global / Tag still needs
    /// [`Self::with_mut`] and is refused from a host callback. Calling another
    /// export goes through [`Self::invoke`], which uses the parked `Caller`.
    pub fn with_mut<R>(
        &self, ctx: &Ctx<'_>, f: impl FnOnce(&mut backend::Store) -> Result<R>,
    ) -> Result<R> {
        match self.inner.try_borrow_mut() {
            Ok(mut store) => f(&mut store),
            Err(_) => Err(Self::refuse_reentry(ctx)),
        }
    }

    pub(crate) fn refuse_reentry(ctx: &Ctx<'_>) -> rquickjs::Error {
        throw_runtime_error(ctx, Self::REENTRY_REFUSED)
    }

    /// A store context, including from inside a host callback.
    ///
    /// A running export holds the `RefCell` for the whole call, so
    /// [`Self::with_mut`] cannot succeed from a host function. The `Caller`
    /// wasmtime handed that function *is* that borrow; [`ActiveHostCall`]
    /// parks it so wrappers that only need [`AsContext`] — `Memory.buffer`
    /// being the one JS WASI shims actually hit — can still see linear
    /// memory.
    pub fn with_context<R>(
        &self, ctx: &Ctx<'_>, f: impl FnOnce(StoreContext<'_, backend::StoreData>) -> Result<R>,
    ) -> Result<R> {
        match self.inner.try_borrow() {
            Ok(store) => f(store.as_context()),
            Err(_) => {
                ActiveHostCall::with_caller(ctx, |caller| f(caller.as_context()))
                    .unwrap_or_else(|| Err(Self::refuse_reentry(ctx)))
            }
        }
    }

    /// Call `func`, including from inside a host callback.
    ///
    /// The outer export still holds the `RefCell`. wasmtime already handed that
    /// frame a `Caller`; [`ActiveHostCall`] parks it so a host function can
    /// invoke another export (`Memory.buffer` is the read-only cousin).
    pub fn invoke(
        &self, ctx: &Ctx<'_>, func: &Func, params: &[Val], results: &mut [Val],
    ) -> core::result::Result<(), wasmtime::Error> {
        match self.inner.try_borrow_mut() {
            Ok(mut store) => func.call(&mut *store, params, results),
            Err(_) => {
                match ActiveHostCall::with_caller_mut(ctx, |caller| {
                    func.call(caller.as_context_mut(), params, results)
                }) {
                    Some(result) => result,
                    None => Err(wasmtime::Error::msg(Self::REENTRY_REFUSED)),
                }
            }
        }
    }
}

/// The `Caller` of each host callback currently on the stack.
///
/// Parked as context userdata rather than on [`Store`]: the store's `RefCell`
/// is already mutably borrowed for the export that called out, so nothing
/// inside it is reachable from JS.
#[derive(Default)]
pub struct ActiveHostCall {
    stack: RefCell<Vec<NonNull<backend::Caller<'static>>>>,
}

// SAFETY: the stack holds pointers into `HostFunction::run` frames, not JS
// values; the `'js` lifetime on the impl is only what userdata requires.
unsafe impl<'js> JsLifetime<'js> for ActiveHostCall {
    type Changed<'to> = ActiveHostCall;
}

/// Pops the `Caller` [`ActiveHostCall::enter`] pushed, even if the host
/// function throws.
pub struct ActiveHostCallGuard<'js> {
    ctx: Ctx<'js>,
}

impl ActiveHostCall {
    fn install(ctx: &Ctx<'_>) -> Result<()> {
        if ctx.userdata::<Self>().is_some() {
            return Ok(());
        }
        ctx.store_userdata(Self::default())
            .map_err(|_| {
                Exception::throw_internal(ctx, "the WebAssembly host-call stack is already in use")
            })
            .map(|_| ())
    }

    /// Remember `caller` for the JS frame that is about to run.
    pub fn enter<'js>(
        ctx: &Ctx<'js>, caller: &backend::Caller<'_>,
    ) -> Result<ActiveHostCallGuard<'js>> {
        Self::install(ctx)?;
        let slot = ctx.userdata::<Self>().ok_or_else(|| {
            Exception::throw_internal(
                ctx,
                "the WebAssembly host-call stack is missing from this context",
            )
        })?;
        let ptr = NonNull::from(caller).cast::<backend::Caller<'static>>();
        slot.stack
            .try_borrow_mut()
            .map_err(|_| {
                Exception::throw_internal(ctx, "the WebAssembly host-call stack is already in use")
            })?
            .push(ptr);
        Ok(ActiveHostCallGuard { ctx: ctx.clone() })
    }

    fn current(ctx: &Ctx<'_>) -> Option<NonNull<backend::Caller<'static>>> {
        let slot = ctx.userdata::<Self>()?;
        let stack = slot.stack.try_borrow().ok()?;
        stack.last().copied()
    }

    /// The innermost parked `Caller`, if a host callback is on the stack.
    fn with_caller<R>(
        ctx: &Ctx<'_>, f: impl FnOnce(&backend::Caller<'_>) -> Result<R>,
    ) -> Option<Result<R>> {
        let ptr = Self::current(ctx)?;
        // SAFETY: `enter` pushed a pointer to the `Caller` still on
        // `HostFunction::run`'s stack; the matching guard has not dropped.
        Some(f(unsafe { ptr.as_ref() }))
    }

    /// Mutable cousin of [`Self::with_caller`], for calling another export.
    fn with_caller_mut<R>(
        ctx: &Ctx<'_>, f: impl FnOnce(&mut backend::Caller<'_>) -> R,
    ) -> Option<R> {
        let mut ptr = Self::current(ctx)?;
        // SAFETY: same as [`Self::with_caller`]; the `RefCell` is not held
        // across `f`, so a nested host frame can `enter`.
        Some(f(unsafe { ptr.as_mut() }))
    }
}

impl Drop for ActiveHostCallGuard<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.ctx.userdata::<ActiveHostCall>()
            && let Ok(mut stack) = slot.stack.try_borrow_mut()
        {
            stack.pop();
        }
    }
}

/// The opaque object `den:wasm`'s `wasiImports()` hands back.
///
/// WASI preview1 is implemented by the *engine*: every call reads and writes
/// the calling instance's own linear memory, which nothing reachable from JS
/// can stand in for, so the namespace cannot be an ordinary bag of functions.
/// It is this marker instead, and `Instance::read_imports` recognising it in
/// the place of the `wasi_snapshot_preview1` namespace is the one and only way
/// WASI ever reaches a linker:
///
/// ```js
/// import { wasiImports } from "den:wasm";
/// await WebAssembly.instantiate(bytes, { wasi_snapshot_preview1: wasiImports() });
/// ```
///
/// It lives on `den:wasm` rather than on `WebAssembly`, which is exactly the
/// namespace the spec says it is and nothing more.
#[cfg(feature = "wasi")]
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct WasiImports {}

#[cfg(feature = "wasi")]
impl WasiImports {
    /// `wasiImports()`: the marker for Wasmtime's preview1 implementation.
    ///
    /// Nothing is built here — the host's stdio and environment are inherited
    /// by [`backend::link_wasi`], at instantiation — so holding the marker
    /// grants nothing on its own.
    pub fn namespace<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
        Ok(Class::instance(ctx.clone(), Self {})?.into_value())
    }

    /// Whether `namespace` is the marker rather than an import namespace to
    /// read names out of.
    ///
    /// Probed, because a failed `from_object` throws (see `Probe`) and every
    /// import object that is *not* WASI passes through here.
    pub fn is_marker(ctx: &Ctx<'_>, namespace: &Object<'_>) -> bool {
        ctx.probe(|| Class::<Self>::from_object(namespace))
            .is_some()
    }

    /// Link the engine's preview1 implementation, once the caller has asked for
    /// it under the namespace it actually implements.
    pub fn link(ctx: &Ctx<'_>, linker: &mut backend::Linker, namespace: &str) -> Result<()> {
        if namespace != backend::WASI_NAMESPACE {
            return Err(throw_link_error(
                ctx,
                format_args!(
                    "wasiImports() implements the \"{}\" namespace, not \"{namespace}\"",
                    backend::WASI_NAMESPACE
                ),
            ));
        }
        backend::link_wasi(linker).map_err(|err| throw_link_error(ctx, err))
    }
}

#[cfg(test)]
#[path = "../tests/unit/store.rs"]
mod tests;
