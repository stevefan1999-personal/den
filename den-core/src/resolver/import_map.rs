//! Import maps ([WICG](https://wicg.github.io/import-maps/)), stored as
//! context userdata and applied by the first resolver in the chain.

use std::path::Path;

use derive_more::{Display, Error, From};
use import_map::{ImportMapDiagnostic, ImportMapErrorKind, parse_from_json};
use rquickjs::{
    Ctx, Error, JsLifetime, Result,
    loader::{ImportAttributes, Resolver},
};
use url::Url;

/// A parsed import map and its base URL.
#[derive(Clone, Debug)]
pub struct ImportMap(import_map::ImportMap);

// SAFETY: `ImportMap` owns its strings and URLs and borrows no JavaScript data.
unsafe impl JsLifetime<'_> for ImportMap {
    type Changed<'to> = ImportMap;
}

#[expect(
    clippy::module_name_repetitions,
    reason = "ImportMapError is the public error paired with ImportMap"
)]
#[derive(Debug, Display, Error, From)]
pub enum ImportMapError {
    #[display("{_0}")]
    #[from]
    Parse(import_map::ImportMapError),
    #[display("{_0}")]
    Diagnostic(#[error(not(source))] String),
    #[display("import map base directory cannot be used as a URL")]
    BaseDirectory,
}

impl ImportMap {
    /// Parse `json` against `base_dir` using the WICG import-map
    /// implementation.
    pub fn parse(json: &str, base_dir: &Path) -> std::result::Result<Self, ImportMapError> {
        let parsed = parse_from_json(directory_url(base_dir)?, json)?;
        if let Some(diagnostic) = parsed.diagnostics.iter().find(|diagnostic| {
            matches!(
                diagnostic,
                ImportMapDiagnostic::InvalidAddressNotString(value, _) if value != "null"
            )
        }) {
            return Err(ImportMapError::Diagnostic(diagnostic.to_string()));
        }
        Ok(Self(parsed.import_map))
    }

    /// Resolve to a target, block the import, or leave it to the next resolver.
    fn resolve(&self, specifier: &str, parent: &str) -> Mapping {
        let referrer = specifier_url(parent).unwrap_or_else(|| self.0.base_url().clone());
        if referrer.scheme() == "den-pkg" {
            return Mapping::Miss;
        }
        let fallback = url_like_specifier(specifier, &referrer);
        match self.0.resolve(specifier, &referrer) {
            Ok(target) if fallback.as_ref() == Some(&target) => Mapping::Miss,
            Ok(target) => Mapping::Target(canonicalize_file_specifier(url_to_specifier(&target))),
            Err(error) => {
                match error.0.as_ref() {
                    ImportMapErrorKind::UnmappedBareSpecifier(..) => Mapping::Miss,
                    ImportMapErrorKind::BlockedByNullEntry(..) => Mapping::Blocked,
                    _ => Mapping::Invalid(error.to_string()),
                }
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Mapping {
    Miss,
    Blocked,
    Target(String),
    Invalid(String),
}

/// Remaps a specifier when a map is installed on the context; otherwise a
/// resolving error lets the builtin, HTTP, and file resolvers continue.
#[expect(
    clippy::module_name_repetitions,
    reason = "the qualified name distinguishes this resolver from other resolvers"
)]
#[derive(Debug, Default)]
pub struct ImportMapResolver;

impl Resolver for ImportMapResolver {
    fn resolve<'js>(
        &mut self, ctx: &Ctx<'js>, base: &str, name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        let Some(map) = ctx.userdata::<ImportMap>() else {
            return Err(Error::new_resolving(base, name));
        };
        match map.resolve(name, base) {
            Mapping::Miss => Err(Error::new_resolving(base, name)),
            Mapping::Blocked => {
                Err(Error::new_resolving_message(
                    base,
                    name,
                    format!("import of '{name}' was blocked by import map"),
                ))
            }
            Mapping::Target(target) => Ok(target),
            Mapping::Invalid(message) => Err(Error::new_resolving_message(base, name, message)),
        }
    }
}

fn directory_url(base_dir: &Path) -> std::result::Result<Url, ImportMapError> {
    let absolute = if base_dir.is_absolute() {
        base_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_error| ImportMapError::BaseDirectory)?
            .join(base_dir)
    };
    Url::from_directory_path(&absolute).map_err(|()| ImportMapError::BaseDirectory)
}

fn specifier_url(specifier: &str) -> Option<Url> {
    let path = Path::new(specifier);
    if path.is_absolute() {
        Url::from_file_path(path).ok()
    } else {
        Url::parse(specifier).ok()
    }
}

/// The import-map crate returns ordinary URL-like specifiers unchanged. Those
/// are misses here so the rest of den's resolver chain still handles extension
/// patterns, canonical paths, and supported network schemes.
fn url_like_specifier(specifier: &str, base: &Url) -> Option<Url> {
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        return base.join(specifier).ok();
    }
    Url::parse(specifier).ok()
}

fn url_to_specifier(url: &Url) -> String {
    if url.scheme() != "file" {
        return url.to_string();
    }
    let mut path = url.to_file_path().map_or_else(
        |()| url.to_string(),
        |path| path.to_string_lossy().replace('\\', "/"),
    );
    if url.path().ends_with('/') && !path.ends_with('/') {
        path.push('/');
    }
    path
}

fn canonicalize_file_specifier(target: String) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target;
    }
    match dunce::canonicalize(&target) {
        Ok(path) => {
            let mut name = path.to_string_lossy().replace('\\', "/");
            if target.ends_with('/') && !name.ends_with('/') {
                name.push('/');
            }
            name
        }
        Err(_) => target,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/import_map_parser.rs"]
mod tests;
