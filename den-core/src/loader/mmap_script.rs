use derivative::Derivative;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt};
use relative_path::RelativePath;
use rquickjs::{loader::Loader, module::Declared, Ctx, Error, Module};
use tokio::runtime::Handle;
use typed_builder::TypedBuilder;
#[cfg(feature = "transpile")]
use {
    den_transpiler_swc::{infer_transpile_syntax_by_extension, EasySwcTranspiler, IsModule},
    std::sync::Arc,
};

#[derive(Derivative, TypedBuilder)]
#[derivative(Debug)]
#[derivative(Default(new = "true"))]
pub struct MmapScriptLoader {
    #[builder(default)]
    extensions: Vec<String>,
    #[derivative(Debug = "ignore")]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
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
    fn load<'js>(&mut self, ctx: &Ctx<'js>, path: &str) -> rquickjs::Result<Module<'js, Declared>> {
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

            let src = AsyncMmapFile::open(path)
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

                Module::declare(ctx.clone(), path, src)
            }
            #[cfg(not(feature = "transpile"))]
            {
                Module::declare(ctx.clone(), path, src.as_slice())
            }
        };

        tokio::task::block_in_place(move || Handle::current().block_on(task))
    }
}
