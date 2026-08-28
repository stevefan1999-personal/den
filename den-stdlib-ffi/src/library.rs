//! `dlopen` + libffi: turning a schema entry into a callable JS function.

use std::{
    cell::RefCell,
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
};

use libffi::middle::{Arg, Cif, CodePtr};
use rquickjs::{Ctx, Function, Object, Result, Value, function::Rest};

use crate::{
    error::ErrorKind,
    grant::FfiGrant,
    marshal::{self, ArgumentCell},
    schema::{NativeType, SymbolKind, SymbolSpec},
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
    handle: RefCell<Option<dlopen2::raw::Library>>,
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
            handle: RefCell::new(Some(handle)),
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

    pub fn path(&self) -> &Path { &self.path }

    /// `Symbol.dispose`. Every later JS-side dispatch sees a `None` handle and
    /// throws `Closed`; a pointer C kept for itself is past den's reach (§5.1).
    fn close(&self) { drop(self.handle.borrow_mut().take()); }
}

/// A symbol bound to its signature: the address, the CIF built from the
/// schema, and the handle that keeps the address mapped.
struct BoundFn {
    address: CodePtr,
    cif:     Cif,
    params:  Vec<NativeType>,
    result:  NativeType,
    library: Rc<LoadedLibrary>,
}

impl BoundFn {
    fn call<'js>(&self, ctx: &Ctx<'js>, arguments: &[Value<'js>]) -> Result<Value<'js>> {
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
        let cells = self
            .params
            .iter()
            .zip(arguments)
            .map(|(declared, value)| ArgumentCell::marshal(ctx, *declared, value))
            .collect::<Result<Vec<_>>>()?;
        let cells: Vec<Arg<'_>> = cells.iter().map(ArgumentCell::as_arg).collect();

        // SAFETY: the CIF and the argument cells were built from the same
        // `SymbolSpec`, so their count and types agree, and the address came
        // from `dlsym` on a handle this `BoundFn` still holds an `Rc` to,
        // checked live above.
        //
        // What no layer can check is that the schema matches the C
        // declaration; that is the caller's contract, stated in §5.1 and in
        // `types/den-ffi.d.ts`.
        unsafe {
            marshal::call(
                ctx,
                self.result,
                &self.cif,
                self.address,
                &cells,
                &self.library,
            )
        }
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
            unsafe { marshal::read(ctx, *declared, address.cast::<u8>(), library) }
        }
        SymbolKind::Function { params, result } => {
            let call = Rc::new(BoundFn {
                address: CodePtr::from_ptr(address),
                cif:     Cif::new(
                    params.iter().map(|declared| declared.ffi_type()),
                    result.ffi_type(),
                ),
                params:  params.clone(),
                result:  *result,
                library: Rc::clone(library),
            });
            Ok(
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                    call.call(&ctx, &args.0)
                })?
                .with_name(key)?
                .into_value(),
            )
        }
    }
}

fn dispose_key<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    ctx.globals()
        .get::<_, Object<'js>>("Symbol")?
        .get("dispose")
}
