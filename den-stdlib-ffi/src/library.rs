//! `dlopen` + libffi: turning a schema entry into a callable JS function.

use std::{
    cell::RefCell,
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use libffi::middle::{Cif, CodePtr};
use rquickjs::{
    CaughtError, Ctx, Function, Object, Result, Value,
    function::{Async, Rest},
};

use crate::{
    error::ErrorKind,
    grant::FfiGrant,
    marshal::{self, ArgumentCell, CallSite, Cell},
    schema::{CallMode, FnSig, ParamType, SymbolKind, SymbolSpec},
};

/// The loaded library. Every bound symbol and every `Pointer` holds an `Rc` of
/// this, which is what keeps the pages mapped for as long as JS can still reach
/// an address into them: `dlopen2::raw::Library::symbol` hands back a bare
/// pointer with no borrow tying it to the handle.
///
/// `Option<Library>` *is* the liveness flag — dropping the handle is the
/// `dlclose`, so there is one state, not a state plus a bool that can drift
/// from it.
pub struct LoadedLibrary {
    /// The handle is behind an `Arc` so that a `nonblocking` call can take a
    /// share of it across to its worker: `Symbol.dispose` drops the realm's
    /// share, and the `dlclose` waits for the call that is inside the library
    /// right now. Without that, disposing a library mid-call unmaps the code
    /// the worker is executing.
    handle: RefCell<Option<Arc<dlopen2::raw::Library>>>,
    path:   PathBuf,
}

impl LoadedLibrary {
    /// `RTLD_LAZY` (dlopen2's default) defers relocation to the first call,
    /// where a missing dependency is a SIGSEGV nobody can catch. `RTLD_LOCAL`
    /// keeps this library's symbols from satisfying another library's
    /// undefined ones. Neither Deno nor Bun lets a caller see these; den fixes
    /// them rather than exposing them (§4.3).
    #[cfg(unix)]
    const FLAGS: Option<i32> = Some(libc::RTLD_NOW | libc::RTLD_LOCAL);
    #[cfg(not(unix))]
    const FLAGS: Option<i32> = None;

    fn open(ctx: &Ctx<'_>, path: PathBuf) -> Result<Self> {
        let handle = dlopen2::raw::Library::open_with_flags(&path, Self::FLAGS)
            .map_err(|error| ErrorKind::Open.throw_at(ctx, error, &path))?;
        Ok(Self {
            handle: RefCell::new(Some(Arc::new(handle))),
            path,
        })
    }

    /// The address of `symbol`, resolved once at `open()` — so a name the
    /// library does not export is an error that names the schema key, not a
    /// surprise at the first call. `None` is "no such name", which only an
    /// `optional` entry is allowed to survive.
    fn address(&self, ctx: &Ctx<'_>, symbol: &str) -> Result<Option<*const c_void>> {
        let handle = self.handle.borrow();
        let handle = handle
            .as_ref()
            .ok_or_else(|| ErrorKind::Closed.throw_at(ctx, "library is closed", &self.path))?;
        // SAFETY: `symbol::<T>` only transmutes the `dlsym` result into a
        // pointer-sized `T`, and `*const c_void` is exactly that; it reads
        // nothing through the address. Whether that address is really the
        // function or object the schema describes is the caller's contract
        // (§5.1) and cannot be checked at any layer.
        Ok(unsafe { handle.symbol::<*const c_void>(symbol) }.ok())
    }

    pub fn is_live(&self) -> bool { self.handle.borrow().is_some() }

    /// A share of the handle, for a call that outlives the borrow. Holding one
    /// keeps the library mapped past `Symbol.dispose`; it does not make the
    /// library live again, because every JS-side dispatch reads
    /// [`Self::is_live`] first.
    fn share(&self, ctx: &Ctx<'_>) -> Result<Arc<dlopen2::raw::Library>> {
        self.handle
            .borrow()
            .clone()
            .ok_or_else(|| ErrorKind::Closed.throw_at(ctx, "library is closed", &self.path))
    }

    pub fn path(&self) -> &Path { &self.path }

    /// `Symbol.dispose`. Every later JS-side dispatch sees a `None` handle and
    /// throws `Closed`; a pointer C kept for itself is past den's reach (§5.1).
    fn close(&self) { drop(self.handle.borrow_mut().take()); }
}

/// A symbol bound to its signature: the address, the CIF built from the
/// schema, and the handle that keeps the address mapped.
struct BoundFn {
    /// Exposed at `open()`, so that the whole of a `nonblocking` call is plain
    /// `Send` data.
    address:   usize,
    /// Built once for the synchronous path. A `nonblocking` call rebuilds it on
    /// its worker from `signature`, because a `Cif` owns raw type arrays and is
    /// not worth making travel.
    cif:       Cif,
    /// What JS arguments are marshalled against.
    params:    Vec<ParamType>,
    /// What C sees: the same list with every buffer and callback as `pointer`.
    signature: Arc<FnSig>,
    /// `/path/to/lib.so::symbol` — the diagnostic a foreign-thread callback
    /// prints when it gives up, and the name in the "must be nonblocking"
    /// refusal.
    origin:    Arc<str>,
    mode:      CallMode,
    library:   Rc<LoadedLibrary>,
}

impl BoundFn {
    /// Check the call and marshal every argument, on the realm thread. This is
    /// the whole of the JS-touching half of a call: what it produces is plain
    /// data that a worker thread can use without a realm.
    fn prepare<'js>(&self, ctx: &Ctx<'js>, arguments: &[Value<'js>]) -> Result<Vec<ArgumentCell>> {
        if !self.library.is_live() {
            return Err(ErrorKind::Closed.throw_at(ctx, "library is closed", &self.library.path));
        }
        if arguments.len() != self.params.len() {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                format_args!(
                    "expected {} argument(s), got {}",
                    self.params.len(),
                    arguments.len()
                ),
            ));
        }
        let site = CallSite {
            mode:   self.mode,
            origin: &self.origin,
        };
        self.params
            .iter()
            .zip(arguments)
            .map(|(declared, value)| ArgumentCell::marshal(ctx, declared, value, site))
            .collect()
    }

    /// The synchronous call: marshal, call, read the result back — all on the
    /// realm thread, holding the runtime lock throughout.
    fn call<'js>(&self, ctx: &Ctx<'js>, arguments: &[Value<'js>]) -> Result<Value<'js>> {
        let cells = self.prepare(ctx, arguments)?;
        // SAFETY: the CIF and the argument cells were built from the same
        // `SymbolSpec`, so their count and types agree, and the address came
        // from `dlsym` on a handle this `BoundFn` still holds an `Rc` to,
        // checked live above.
        //
        // What no layer can check is that the schema matches the C
        // declaration; that is the caller's contract, stated in §5.1 and in
        // `types/den-ffi.d.ts`.
        let cell = unsafe {
            marshal::invoke(
                self.signature.result,
                &self.cif,
                CodePtr::from_ptr(std::ptr::with_exposed_provenance(self.address)),
                &cells,
            )
        };
        // SAFETY: `cell` holds exactly the bytes libffi wrote for the declared
        // result type.
        unsafe {
            marshal::read(
                ctx,
                self.signature.result,
                cell.as_ptr(),
                Some(&self.library),
            )
        }
    }

    /// Everything the worker thread needs, and nothing that belongs to a realm.
    fn plan<'js>(&self, ctx: &Ctx<'js>, arguments: &[Value<'js>]) -> Result<ForeignCall> {
        Ok(ForeignCall {
            address:   self.address,
            signature: Arc::clone(&self.signature),
            cells:     self.prepare(ctx, arguments)?,
            library:   self.library.share(ctx)?,
        })
    }
}

/// One `nonblocking` call in flight: plain data, plus the share of the library
/// handle that keeps the code mapped until it returns even if the script
/// disposes the library meanwhile.
struct ForeignCall {
    address:   usize,
    signature: Arc<FnSig>,
    cells:     Vec<ArgumentCell>,
    library:   Arc<dlopen2::raw::Library>,
}

impl ForeignCall {
    /// # Safety
    ///
    /// As [`marshal::invoke`]: the schema must be the symbol's real signature,
    /// which is the caller's contract (§5.1 item 1).
    unsafe fn run(self) -> Cell {
        let cif = self.signature.cif();
        // SAFETY: the caller's contract, restated on this function. The CIF is
        // rebuilt from the same signature the cells were marshalled against.
        let cell = unsafe {
            marshal::invoke(
                self.signature.result,
                &cif,
                CodePtr::from_ptr(std::ptr::with_exposed_provenance(self.address)),
                &self.cells,
            )
        };
        // Explicit, because this is the field's whole job: the `dlclose` a
        // concurrent `Symbol.dispose` asked for happens here, after the call,
        // rather than under it.
        drop(self.library);
        cell
    }
}

/// `open(path, schema, grant)` — the module's only entry point, and the only
/// capability check site in the crate.
pub fn open<'js>(
    ctx: Ctx<'js>, path: String, schema: Object<'js>, grant: Option<Value<'js>>,
) -> Result<Object<'js>> {
    let grant = grant
        .as_ref()
        .and_then(Value::as_object)
        .and_then(rquickjs::Class::<FfiGrant>::from_object)
        .ok_or_else(|| {
            ErrorKind::NotCapable.throw(
                &ctx,
                format_args!(
                    "loading `{path}` needs an FFI grant — run den with `--allow-ffi[=PATH]` and \
                     pass the value `grant()` returns"
                ),
            )
        })?;

    // Canonical before the check and before `dlopen`: a grant scoped to a
    // directory means nothing against a path with a `..` still in it.
    let path = Path::new(&path)
        .canonicalize()
        .map_err(|error| ErrorKind::Open.throw_at(&ctx, error, Path::new(&path)))?;
    if !grant.borrow().allows(&path) {
        return Err(ErrorKind::NotCapable.throw_at(
            &ctx,
            "the FFI grant does not cover this path",
            &path,
        ));
    }

    let library = Rc::new(LoadedLibrary::open(&ctx, path)?);
    let bound = Object::new(ctx.clone())?;
    for entry in schema.props::<String, Value<'js>>() {
        let (key, declared) = entry?;
        let spec = SymbolSpec::parse(&ctx, &key, &declared)?;
        let value = bind(&ctx, &library, &key, &spec)?;
        bound.set(key, value)?;
    }

    // Symbols hang off the handle itself, so the only reserved key is a
    // well-known symbol and a C function named `close` can never collide with
    // it (§4.8).
    let disposing = Rc::clone(&library);
    bound.set(
        dispose_key(&ctx)?,
        Function::new(ctx.clone(), move || disposing.close())?.with_name("[Symbol.dispose]")?,
    )?;
    Ok(bound)
}

/// One schema entry, resolved: a bound function, a static's value, or `null`
/// for an `optional` name the library does not export.
fn bind<'js>(
    ctx: &Ctx<'js>, library: &Rc<LoadedLibrary>, key: &str, spec: &SymbolSpec,
) -> Result<Value<'js>> {
    let Some(address) = library.address(ctx, &spec.symbol)? else {
        if spec.optional {
            return Ok(Value::new_null(ctx.clone()));
        }
        return Err(ErrorKind::Symbol.throw_for(
            ctx,
            format_args!("`{}` is not exported by this library", spec.symbol),
            key,
        ));
    };

    match &spec.kind {
        // A static is read once, here: its value is a property, not a getter,
        // which is also what keeps it readable after `Symbol.dispose`.
        SymbolKind::Static(declared) => {
            // SAFETY: `address` is the address of the exported object, and the
            // schema declares its type. That the declaration is true is the
            // caller's contract (§5.1) — a `.so` carries no type information.
            unsafe { marshal::read(ctx, *declared, address.cast::<u8>(), Some(library)) }
        }
        SymbolKind::Function {
            params,
            result,
            mode,
        } => {
            let signature = Arc::new(FnSig::of(params, *result));
            let call = Rc::new(BoundFn {
                cif: signature.cif(),
                address: address.expose_provenance(),
                params: params.clone(),
                signature,
                origin: format!("{}::{}", library.path().display(), spec.symbol).into(),
                mode: *mode,
                library: Rc::clone(library),
            });
            let bound = match mode {
                CallMode::Blocking => {
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                        call.call(&ctx, &args.0)
                    })?
                }
                CallMode::Nonblocking => nonblocking(ctx, call)?,
            };
            Ok(bound.with_name(key)?.into_value())
        }
    }
}

/// A `nonblocking` symbol: everything that needs the realm happens before the
/// `await`, the call itself happens on a `spawn_blocking` worker, and the
/// result is read back here. JS keeps running throughout — which is exactly
/// what makes a callback handed to this symbol serviceable (§4.7).
fn nonblocking<'js>(ctx: &Ctx<'js>, call: Rc<BoundFn>) -> Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        Async(move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
            // Marshalling is synchronous on purpose: it reads JS values, so it
            // must finish before this function yields and lets JS mutate them.
            //
            // Its throw is *caught* here rather than propagated. rquickjs
            // reports a JS throw as `Error::Exception` and leaves the value in
            // the realm, and the realm runs other code before this promise
            // settles — so by then the pending exception is somebody else's or
            // nobody's. `CaughtError` owns the value and re-raises it at the
            // moment the rejection is actually delivered.
            let planned = CaughtError::catch(&ctx, call.plan(&ctx, &args.0));
            let result = call.signature.result;
            let library = Rc::clone(&call.library);
            async move {
                let planned = planned.map_err(|caught| caught.throw(&ctx))?;
                // SAFETY: as the synchronous path — the CIF is rebuilt from the
                // signature the cells were marshalled against, and that the
                // schema is the symbol's real signature is the caller's
                // contract (§5.1 item 1).
                let returned = tokio::task::spawn_blocking(move || unsafe { planned.run() })
                    .await
                    .map_err(|joined| {
                        ErrorKind::BadArgument.throw(
                            &ctx,
                            format_args!("the foreign call did not finish: {joined}"),
                        )
                    })?;
                // SAFETY: `cell` holds exactly the bytes libffi wrote for the
                // declared result type.
                unsafe { marshal::read(&ctx, result, returned.as_ptr(), Some(&library)) }
            }
        }),
    )
}

/// `Symbol.dispose`, which both handles this crate hands out use as their one
/// reserved key.
pub fn dispose_key<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    ctx.globals()
        .get::<_, Object<'js>>("Symbol")?
        .get("dispose")
}
