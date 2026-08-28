//! The capability gate: which libraries this realm may load.
//!
//! Not a permission system — `docs/research/19-den-ffi.md` §5.3. One value in
//! context userdata, minted by the composition root (`den --allow-ffi`), read
//! at exactly one check site ([`FfiGrant::allows`], called from
//! `Library::open`). A module that imports `den:ffi` in a realm nobody granted
//! anything to cannot bind a single symbol.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rquickjs::{JsLifetime, class::Trace};

/// The paths a grant covers. `Any` is `--allow-ffi` with no value, the way
/// Deno's bare permission flags read; `Under` is `--allow-ffi=PATH,PATH`.
#[derive(Clone, Debug)]
enum Roots {
    Any,
    Under(Arc<[PathBuf]>),
}

/// A capability value. Unforgeable from JS — the class declares no usable
/// constructor, so the only instance a script can hold is the one `grant()`
/// mints from userdata.
#[derive(Clone, Debug, Trace, JsLifetime)]
#[rquickjs::class(rename = "FfiGrant")]
pub struct FfiGrant {
    #[qjs(skip_trace)]
    roots: Roots,
}

impl FfiGrant {
    /// Every path. `den --allow-ffi`.
    pub const fn any() -> Self { Self { roots: Roots::Any } }

    /// Only paths under one of `roots`, which may name a directory or a single
    /// library. Roots are resolved once here so that the check site compares
    /// two canonical paths; a root that does not exist stays as written and
    /// therefore matches nothing.
    pub fn under<R: IntoIterator<Item = PathBuf>>(roots: R) -> Self {
        Self {
            roots: Roots::Under(
                roots
                    .into_iter()
                    .map(|root| root.canonicalize().unwrap_or(root))
                    .collect(),
            ),
        }
    }

    /// The single capability check. `path` must already be canonical.
    pub fn allows(&self, path: &Path) -> bool {
        match &self.roots {
            Roots::Any => true,
            Roots::Under(roots) => roots.iter().any(|root| path.starts_with(root)),
        }
    }
}

#[rquickjs::methods]
impl FfiGrant {
    // rquickjs exports a class only if it declares a constructor, and a `()`
    // return makes `new FfiGrant()` throw — which is the point: a forgeable
    // grant is not a capability.
    #[expect(
        clippy::new_ret_no_self,
        reason = "`#[qjs(constructor)]` marker; not constructible from JS"
    )]
    #[qjs(constructor)]
    pub const fn new() {}
}
