use fmmap::{MmapFile, MmapFileExt};
use relative_path::RelativePath;
use rquickjs::{Ctx, Error, Loaded, Loader, Module};

#[derive(Debug)]
pub struct MmapScriptLoader {
    extensions: Vec<String>,
}

impl MmapScriptLoader {
    /// Add script file extension
    pub fn add_extension<X: Into<String>>(&mut self, extension: X) -> &mut Self {
        self.extensions.push(extension.into());
        self
    }

    /// Add script file extension
    #[must_use]
    pub fn with_extension<X: Into<String>>(mut self, extension: X) -> Self {
        self.add_extension(extension);
        self
    }
}

impl Default for MmapScriptLoader {
    fn default() -> Self {
        Self {
            extensions: vec!["js".into(), "ts".into()],
        }
    }
}

impl Loader for MmapScriptLoader {
    fn load<'js>(&mut self, ctx: Ctx<'js>, path: &str) -> rquickjs::Result<Module<'js, Loaded>> {
        if !check_extensions(path, &self.extensions) {
            return Err(Error::new_loading(path));
        }

        let source = MmapFile::open(path).map_err(|_| Error::new_loading(path))?;
        Ok(Module::new(ctx, path, source.as_slice())?.into_loaded())
    }
}

fn check_extensions(name: &str, extensions: &[String]) -> bool {
    let path = RelativePath::new(name);
    path.extension()
        .map(|extension| {
            extensions
                .iter()
                .any(|known_extension| known_extension == extension)
        })
        .unwrap_or(false)
}
