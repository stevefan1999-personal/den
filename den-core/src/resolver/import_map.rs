//! Import maps ([WICG](https://wicg.github.io/import-maps/)), stored as
//! context userdata and applied by a resolver that sits first in the chain.
//!
//! Resolution follows txiki's subset: exact key, then longest prefix key
//! ending in `/`; `scopes` whose key prefixes the parent URL, longer first;
//! mapping to `null` blocks the import; relative `./` / `../` targets join
//! against the map's base directory. A miss is a resolving error so the rest
//! of the chain still runs.

use std::path::Path;

use derive_more::{Display, Error, From};
use rquickjs::{
    Ctx, Error, JsLifetime, Result,
    loader::{ImportAttributes, Resolver},
};
use url::Url;

/// A parsed import map, already joined against its base directory.
#[derive(Clone, Debug)]
pub struct ImportMap {
    imports: SpecifierMap,
    /// Longest scope prefix first.
    scopes:  Vec<(String, SpecifierMap)>,
}

// SAFETY: `ImportMap` borrows no `'js` lifetime.
unsafe impl<'js> JsLifetime<'js> for ImportMap {
    type Changed<'to> = ImportMap;
}

#[derive(Clone, Debug, Default)]
struct SpecifierMap {
    /// Longest key first so a prefix match hits the most specific mapping.
    entries: Vec<(String, Option<String>)>,
}

#[derive(Debug, Display, Error, From)]
pub enum ImportMapError {
    #[display("import map is not valid JSON: {_0}")]
    #[from]
    Json(serde_json::Error),
    #[display("import map must be a JSON object")]
    NotAnObject,
    #[display("import map `{_0}` must be an object")]
    ExpectedObject(#[error(not(source))] String),
    #[display("import map target for `{_0}` must be a string or null")]
    InvalidTarget(#[error(not(source))] String),
    #[display("import map base directory cannot be used as a URL")]
    BaseDirectory,
    #[display("cannot resolve import map target `{_0}` against the base directory")]
    Join(#[error(not(source))] String),
}

impl ImportMap {
    /// Parse `json` and join relative targets against `base_dir`.
    pub fn parse(json: &str, base_dir: &Path) -> std::result::Result<Self, ImportMapError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let object = value.as_object().ok_or(ImportMapError::NotAnObject)?;
        let base = directory_url(base_dir)?;

        let imports = match object.get("imports") {
            None => SpecifierMap::default(),
            Some(value) => SpecifierMap::parse(value, &base, "imports")?,
        };

        let mut scopes = Vec::new();
        if let Some(value) = object.get("scopes") {
            let scopes_object = value
                .as_object()
                .ok_or_else(|| ImportMapError::ExpectedObject("scopes".into()))?;
            for (scope_key, scope_map) in scopes_object {
                let resolved_key = resolve_against_base(scope_key, &base)?;
                scopes.push((
                    resolved_key,
                    SpecifierMap::parse(scope_map, &base, "scopes")?,
                ));
            }
            scopes.sort_by(|left, right| {
                right
                    .0
                    .len()
                    .cmp(&left.0.len())
                    .then_with(|| left.0.cmp(&right.0))
            });
        }

        Ok(Self { imports, scopes })
    }

    /// Remap `specifier` as imported from `parent_url`.
    ///
    /// `None` — no mapping. `Some(None)` — blocked. `Some(Some(target))` —
    /// load `target` instead.
    pub fn resolve(&self, specifier: &str, parent_url: &str) -> Option<Option<String>> {
        for (scope_key, scope_map) in &self.scopes {
            if parent_url.starts_with(scope_key.as_str())
                && let Some(mapped) = scope_map.resolve(specifier)
            {
                return Some(mapped);
            }
        }
        self.imports.resolve(specifier)
    }
}

impl SpecifierMap {
    fn parse(
        value: &serde_json::Value, base: &Url, field: &str,
    ) -> std::result::Result<Self, ImportMapError> {
        let object = value
            .as_object()
            .ok_or_else(|| ImportMapError::ExpectedObject(field.into()))?;
        let mut entries = object
            .iter()
            .map(|(key, target)| {
                let resolved = match target {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(target) => Some(resolve_against_base(target, base)?),
                    _ => return Err(ImportMapError::InvalidTarget(key.clone())),
                };
                Ok((key.clone(), resolved))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then_with(|| left.0.cmp(&right.0))
        });
        Ok(Self { entries })
    }

    fn resolve(&self, specifier: &str) -> Option<Option<String>> {
        if let Some((_, target)) = self.entries.iter().find(|(key, _)| key == specifier) {
            return Some(canonicalize_target(target.clone()));
        }
        for (key, target) in &self.entries {
            if key.ends_with('/') && specifier.starts_with(key) {
                let mapped = target
                    .as_ref()
                    .map(|prefix| format!("{}{}", prefix, &specifier[key.len()..]));
                return Some(canonicalize_target(mapped));
            }
        }
        None
    }
}

/// Remaps a specifier when a map is installed on the context; otherwise a
/// resolving error so `BuiltinResolver` / files / HTTP still run.
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
            None => Err(Error::new_resolving(base, name)),
            Some(None) => {
                Err(Error::new_resolving_message(
                    base,
                    name,
                    format!("import of '{name}' was blocked by import map"),
                ))
            }
            Some(Some(target)) => Ok(target),
        }
    }
}

fn directory_url(base_dir: &Path) -> std::result::Result<Url, ImportMapError> {
    let absolute = if base_dir.is_absolute() {
        base_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| ImportMapError::BaseDirectory)?
            .join(base_dir)
    };
    Url::from_directory_path(&absolute).map_err(|()| ImportMapError::BaseDirectory)
}

fn resolve_against_base(value: &str, base: &Url) -> std::result::Result<String, ImportMapError> {
    if value.starts_with("./") || value.starts_with("../") {
        let joined = base
            .join(value)
            .map_err(|_| ImportMapError::Join(value.into()))?;
        Ok(url_to_specifier(&joined))
    } else {
        Ok(value.to_string())
    }
}

fn url_to_specifier(url: &Url) -> String {
    if url.scheme() != "file" {
        return url.to_string();
    }
    let mut path = url
        .to_file_path()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| url.to_string());
    // Prefix mappings end in `/`; `Url::to_file_path` drops that slash.
    if url.path().ends_with('/') && !path.ends_with('/') {
        path.push('/');
    }
    path
}

fn canonicalize_target(target: Option<String>) -> Option<String> {
    target.map(canonicalize_file_specifier)
}

fn canonicalize_file_specifier(target: String) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target;
    }
    match Path::new(&target).canonicalize() {
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
