use std::{fs, path::Path};

#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile};
use rquickjs::{
    Ctx, Error, Module, Result,
    loader::{ImportAttributes, Loader},
    module::Declared,
};

use crate::loader::typed::{declare_import_kind, import_kind};

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
        &mut self, ctx: &Ctx<'js>, path: &str, attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let kind = import_kind(path, attributes.as_ref())?;
        // Typed imports intentionally skip the script-extension gate.
        let extension = if kind.is_some() {
            None
        } else {
            let extension = Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .ok_or_else(|| Error::new_loading(path))?;
            Some(
                self.extensions
                    .iter()
                    .find(|candidate| extension == candidate.as_str())
                    .ok_or_else(|| Error::new_loading(path))?
                    .as_str(),
            )
        };
        let src = fs::read(path).map_err(|_error| Error::new_loading(path))?;

        if let Some(kind) = kind {
            return declare_import_kind(ctx, path, &src, kind);
        }

        #[cfg(feature = "transpile")]
        {
            let source_type = infer_transpile_syntax_by_extension(
                extension.ok_or_else(|| Error::new_loading(path))?,
            )
            .unwrap_or_default()
            .with_module(true);
            let src = transpile(std::str::from_utf8(&src)?, source_type)
                .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;
            Module::declare(ctx.clone(), path, src)
        }
        #[cfg(not(feature = "transpile"))]
        {
            let _ = extension;
            Module::declare(ctx.clone(), path, src)
        }
    }
}
