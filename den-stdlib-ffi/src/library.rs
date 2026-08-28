//! `dlopen` + libffi: turning a schema entry into a callable JS function.

use std::{
    cell::RefCell,
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
};

use libffi::middle::{Arg, Cif, CodePtr, Ret};
use rquickjs::{Ctx, Function, Object, Result, Value, function::Rest};

use crate::{
    error::ErrorKind,
    grant::FfiGrant,
    schema::{FnSpec, NativeType},
};

/// The loaded library. Every bound symbol holds an `Rc` of this, which is what
/// keeps the pages mapped for as long as JS can still reach an address into
/// them: `dlopen2::raw::Library::symbol` hands back a bare pointer with no
/// borrow tying it to the handle.
///
/// `Option<Library>` *is* the liveness flag — dropping the handle is the
/// `dlclose`, so there is one state, not a state plus a bool that can drift
/// from it.
struct LoadedLibrary {
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
    /// surprise at the first call.
    fn address(&self, ctx: &Ctx<'_>, symbol: &str) -> Result<CodePtr> {
        let handle = self.handle.borrow();
        let handle = handle
            .as_ref()
            .ok_or_else(|| ErrorKind::Closed.throw_at(ctx, "library is closed", &self.path))?;
        // SAFETY: `symbol::<T>` only transmutes the `dlsym` result into a
        // pointer-sized `T`, and `*const c_void` is exactly that; it reads
        // nothing through the address. Whether that address is really the
        // function the schema describes is the caller's contract (§5.1) and
        // cannot be checked at any layer.
        let address: *const c_void = unsafe { handle.symbol(symbol) }
            .map_err(|error| ErrorKind::Symbol.throw_for(ctx, error, symbol))?;
        Ok(CodePtr::from_ptr(address))
    }

    fn is_live(&self) -> bool { self.handle.borrow().is_some() }

    /// `Symbol.dispose`. Every later JS-side dispatch sees a `None` handle and
    /// throws `Closed`; a pointer C kept for itself is past den's reach (§5.1).
    fn close(&self) { drop(self.handle.borrow_mut().take()); }
}

/// One argument, owned for exactly the duration of one call: `Arg::new` takes
/// the cell's address, and libffi may rewrite the argument array in place, so
/// cells are built fresh per call and never cached (§4.3).
enum ArgumentCell {
    I32(i32),
    F64(f64),
}

impl ArgumentCell {
    fn marshal(ctx: &Ctx<'_>, declared: NativeType, value: &Value<'_>) -> Result<Self> {
        let number = value.as_number().ok_or_else(|| {
            ErrorKind::BadArgument.throw(
                ctx,
                format_args!("expected a number for `{}`", declared.name()),
            )
        })?;
        match declared {
            NativeType::F64 => Ok(Self::F64(number)),
            // A silent truncation is a wrong answer C cannot see; `4.5` and
            // `2**31` are both refusals, not roundings.
            NativeType::I32 if number.fract() == 0.0 && i32::try_from(number as i64).is_ok() => {
                Ok(Self::I32(number as i32))
            }
            NativeType::I32 => {
                Err(ErrorKind::Range
                    .throw(ctx, format_args!("{number} is not representable as an i32")))
            }
            NativeType::Void => {
                Err(ErrorKind::Schema.throw(ctx, "`void` is a result type, not a parameter type"))
            }
        }
    }

    fn as_arg(&self) -> Arg<'_> {
        match self {
            Self::I32(value) => Arg::new(value),
            Self::F64(value) => Arg::new(value),
        }
    }
}

/// A symbol bound to its signature: the address, the CIF built from the
/// schema, and the handle that keeps the address mapped.
struct BoundFn {
    address: CodePtr,
    cif:     Cif,
    spec:    FnSpec,
    library: Rc<LoadedLibrary>,
}

impl BoundFn {
    fn bind(
        ctx: &Ctx<'_>, library: &Rc<LoadedLibrary>, symbol: &str, spec: FnSpec,
    ) -> Result<Rc<Self>> {
        Ok(Rc::new(Self {
            address: library.address(ctx, symbol)?,
            cif: Cif::new(
                spec.params.iter().map(|declared| declared.ffi_type()),
                spec.result.ffi_type(),
            ),
            spec,
            library: Rc::clone(library),
        }))
    }

    fn call<'js>(&self, ctx: &Ctx<'js>, arguments: &[Value<'js>]) -> Result<Value<'js>> {
        if !self.library.is_live() {
            return Err(ErrorKind::Closed.throw_at(ctx, "library is closed", &self.library.path));
        }
        if arguments.len() != self.spec.params.len() {
            return Err(ErrorKind::BadArgument.throw(
                ctx,
                format_args!(
                    "expected {} argument(s), got {}",
                    self.spec.params.len(),
                    arguments.len()
                ),
            ));
        }
        let cells = self
            .spec
            .params
            .iter()
            .zip(arguments)
            .map(|(declared, value)| ArgumentCell::marshal(ctx, *declared, value))
            .collect::<Result<Vec<_>>>()?;
        let cells: Vec<Arg<'_>> = cells.iter().map(ArgumentCell::as_arg).collect();

        // SAFETY: the CIF and the argument cells were built from the same
        // `FnSpec`, so their count and types agree; the address came from
        // `dlsym` on a handle this `BoundFn` still holds an `Rc` to, checked
        // live above. The return cell is exactly the declared type, which is
        // what `call_return_into` writes — it corrects sub-register returns
        // itself and never writes past `type.size()` (§0 fact 3).
        //
        // What no layer can check is that the schema matches the C
        // declaration; that is the caller's contract, stated in §5.1 and in
        // `types/den-ffi.d.ts`.
        unsafe {
            match self.spec.result {
                NativeType::Void => {
                    self.cif.call_return_into(self.address, &cells, Ret::void());
                    Ok(Value::new_undefined(ctx.clone()))
                }
                NativeType::I32 => {
                    let mut returned = 0_i32;
                    self.cif
                        .call_return_into(self.address, &cells, Ret::new(&mut returned));
                    Ok(Value::new_int(ctx.clone(), returned))
                }
                NativeType::F64 => {
                    let mut returned = 0.0_f64;
                    self.cif
                        .call_return_into(self.address, &cells, Ret::new(&mut returned));
                    Ok(Value::new_float(ctx.clone(), returned))
                }
            }
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
        let (symbol, declared) = entry?;
        let spec = FnSpec::parse(&ctx, &symbol, &declared)?;
        let call = BoundFn::bind(&ctx, &library, &symbol, spec)?;
        let function = Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
            call.call(&ctx, &args.0)
        })?
        .with_name(&symbol)?;
        bound.set(symbol, function)?;
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

fn dispose_key<'js>(ctx: &Ctx<'js>) -> Result<Value<'js>> {
    ctx.globals()
        .get::<_, Object<'js>>("Symbol")?
        .get("dispose")
}
