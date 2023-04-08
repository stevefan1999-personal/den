use derivative::Derivative;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt};
use relative_path::RelativePath;
use rquickjs::{Ctx, Error, Loaded, Loader, Module};
use tokio::runtime::Handle;

#[derive(Debug, Derivative)]
#[derivative(Default(new = "true"))]
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

impl Loader for MmapScriptLoader {
    fn load<'js>(&mut self, ctx: Ctx<'js>, path: &str) -> rquickjs::Result<Module<'js, Loaded>> {
        let task = async move {
            let extension = RelativePath::new(path)
                .extension()
                .ok_or(Error::new_loading(path))?;
            self.extensions
                .iter()
                .find(|&e| extension == e)
                .ok_or(Error::new_loading(path))?;

            let source = AsyncMmapFile::open(path)
                .await
                .map_err(|_| Error::new_loading(path))?;

            Ok(Module::new(ctx, path, source.as_slice())?.into_loaded())
        };

        tokio::task::block_in_place(move || Handle::current().block_on(task))
    }
}
