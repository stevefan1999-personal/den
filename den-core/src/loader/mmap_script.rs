use derive_more::Debug;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt};
use relative_path::RelativePath;
use rquickjs::{
    Ctx, Error, Module, Result,
    loader::{ImportAttributes, Loader},
    module::Declared,
};
use tokio::runtime::Handle;
use typed_builder::TypedBuilder;
#[cfg(feature = "transpile")]
use {
    den_transpiler_oxc::{EasyOxcTranspiler, IsModule, infer_transpile_syntax_by_extension},
    std::sync::Arc,
};

#[derive(Debug, Default, TypedBuilder)]
pub struct MmapScriptLoader {
    #[builder(default)]
    extensions: Vec<String>,
    #[debug(ignore)]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasyOxcTranspiler>,
}

impl MmapScriptLoader {
    pub fn new() -> Self {
        Self::default()
    }

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
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let task = async move {
            let extension = RelativePath::new(path)
                .extension()
                .ok_or(Error::new_loading(path))?;

            #[allow(unused_variables)]
            let extension = self
                .extensions
                .iter()
                .find(|&e| extension == e)
                .ok_or(Error::new_loading(path))?;

            // SAFETY: fmmap 0.5 marks every file-backed constructor unsafe because an
            // external writer truncating the file while the mapping is live is
            // UB (SIGBUS on read). den maps the script read-only for the
            // duration of this load and fmmap takes a shared advisory flock;
            // same exposure as before the bump, now spelled out.
            let src = unsafe { AsyncMmapFile::open(path) }
                .await
                .map_err(|_| Error::new_loading(path))?;

            #[cfg(feature = "transpile")]
            {
                let (src, _) = self
                    .transpiler
                    .transpile(
                        std::str::from_utf8(src.as_slice())?,
                        infer_transpile_syntax_by_extension(extension).unwrap_or_default(),
                        IsModule::Bool(true),
                        false,
                    )
                    .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;

                let module = Module::declare(ctx.clone(), path, src)?;
                Ok(module)
            }
            #[cfg(not(feature = "transpile"))]
            {
                Module::declare(ctx.clone(), path, src.as_slice())
            }
        };

        tokio::task::block_in_place(move || Handle::current().block_on(task))
    }
}
