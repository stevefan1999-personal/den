use std::{fs, path::Path};

#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile_with_source_map};
use rquickjs::{
    Ctx, Error, Module, Result,
    loader::{ImportAttributes, Loader},
    module::Declared,
};
use url::Url;

use crate::loader::typed::{declare_import_kind, import_kind};

#[expect(
    clippy::module_name_repetitions,
    reason = "the qualified name distinguishes this loader from other loaders"
)]
#[derive(Debug, Default)]
pub struct MmapScriptLoader {
    extensions: Vec<String>,
}

impl MmapScriptLoader {
    /// Add script file extension
    #[must_use]
    pub fn with_extension<X: Into<String>>(mut self, extension: X) -> Self {
        self.extensions.push(extension.into());
        self
    }
}

impl Loader for MmapScriptLoader {
    fn load<'js>(
        &mut self, ctx: &Ctx<'js>, name: &str, attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let kind = import_kind(name, attributes.as_ref())?;
        // Typed imports intentionally skip the script-extension gate.
        let extension = if kind.is_some() {
            None
        } else {
            let extension = Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .ok_or_else(|| Error::new_loading(name))?;
            Some(
                self.extensions
                    .iter()
                    .find(|candidate| extension == candidate.as_str())
                    .ok_or_else(|| Error::new_loading(name))?
                    .as_str(),
            )
        };
        let src = fs::read(name).map_err(|_error| Error::new_loading(name))?;

        if let Some(kind) = kind {
            return declare_import_kind(ctx, name, &src, kind);
        }

        #[cfg(feature = "transpile")]
        {
            let authored = std::str::from_utf8(&src)?;
            let authored_map = load_source_map(name, authored);
            let source_type = infer_transpile_syntax_by_extension(
                extension.ok_or_else(|| Error::new_loading(name))?,
            )
            .unwrap_or_default()
            .with_module(true);
            let output = transpile_with_source_map(authored, source_type, name)
                .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;
            let mut maps = vec![output.source_map.into_inner()];
            maps.extend(authored_map);
            den_util::stack::register_source(ctx, name, output.code.clone(), maps)?;
            Module::declare(ctx.clone(), name, output.code)
        }
        #[cfg(not(feature = "transpile"))]
        {
            let _ = extension;
            let src = std::str::from_utf8(&src)?.to_owned();
            let maps = load_source_map(name, &src).into_iter();
            den_util::stack::register_source(ctx, name, src.clone(), maps)?;
            Module::declare(ctx.clone(), name, src)
        }
    }
}

fn load_source_map(name: &str, source: &str) -> Option<oxc_sourcemap::SourceMap<'static>> {
    let mapping = den_util::stack::source_mapping_url(source)?;
    let source_url = Url::from_file_path(name).ok()?;
    if mapping.starts_with("data:") {
        return den_util::stack::inline_source_map(mapping, &source_url);
    }
    let map_url = source_url.join(mapping).ok()?;
    if map_url.scheme() != "file" {
        return None;
    }
    let json = fs::read_to_string(map_url.to_file_path().ok()?).ok()?;
    den_util::stack::parse_source_map(&json, &map_url)
}
