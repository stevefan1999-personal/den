use std::path::{Path, PathBuf};

use rquickjs::{
    Ctx, Error, Result,
    loader::{ImportAttributes, Resolver},
};
use url::Url;

/// Resolves the specifiers [`rquickjs::loader::FileResolver`] cannot: one that
/// is already an absolute path, one that is a `file:` URL, and — the half that
/// makes the other two useful — one that is relative *to* such a name.
///
/// `FileResolver` hands every path to [`relative_path::RelativePath`], which
/// has no notion of a root and interprets what it is given against the working
/// directory: `den /home/me/app.js` asks for `./home/me/app.js`, and `./lib.js`
/// imported from `/home/me/app.js` comes out as `./home/me/lib.js` relative to
/// wherever den happens to have been started (ARCHITECTURE §6).
///
/// Names this resolver cannot prove name an existing file are left to the rest
/// of the chain, so a bare specifier still resolves against `./` as before.
#[derive(Debug, Default)]
pub struct AbsolutePathResolver {
    /// The same file name patterns `FileResolver` is given, so that leaving the
    /// extension out works the same either side of the root.
    patterns: Vec<String>,
}

impl AbsolutePathResolver {
    pub fn new<P: Into<String>>(patterns: impl IntoIterator<Item = P>) -> Self {
        Self {
            patterns: patterns.into_iter().map(Into::into).collect(),
        }
    }

    /// The path a specifier denotes on its own, without a base.
    ///
    /// The absolute-path case is tried first because a Windows path (`C:/x.js`)
    /// *parses* as a URL whose scheme is the drive letter.
    fn denoted_path(specifier: &str) -> Option<PathBuf> {
        let direct = Path::new(specifier);
        if direct.is_absolute() {
            return Some(direct.to_path_buf());
        }
        match Url::parse(specifier) {
            Ok(url) if url.scheme() == "file" => url.to_file_path().ok(),
            _ => None,
        }
    }

    /// `name` read as a sibling of `base` — only for a base this resolver could
    /// have produced itself, and only for the specifiers the spec calls
    /// relative.
    fn sibling_of(base: &str, name: &str) -> Option<PathBuf> {
        name.starts_with('.')
            .then(|| Self::denoted_path(base))
            .flatten()
            .and_then(|base| Some(base.parent()?.join(name)))
    }

    /// The file a specifier ends up naming, extension patterns included.
    fn file_for(&self, base: &str, name: &str) -> Option<PathBuf> {
        let candidate = Self::denoted_path(name).or_else(|| Self::sibling_of(base, name))?;
        if candidate.is_file() {
            return Some(candidate);
        }
        let file_name = candidate.file_name()?.to_str()?;
        self.patterns
            .iter()
            .map(|pattern| candidate.with_file_name(pattern.replace("{}", file_name)))
            .find(|path| path.is_file())
    }
}

impl Resolver for AbsolutePathResolver {
    fn resolve<'js>(
        &mut self, _ctx: &Ctx<'js>, base: &str, name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        self.file_for(base, name)
            // Canonical, so that one file is one module however it was reached
            // — `./lib.js` and `./sub/../lib.js` are the same script — and so
            // that the loaders below get a path rather than the `file:` URL a
            // specifier may have been written as.
            .and_then(|path| path.canonicalize().ok())
            .and_then(|path| path.to_str().map(str::to_owned))
            .ok_or_else(|| Error::new_resolving(base, name))
    }
}
