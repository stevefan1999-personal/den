use std::sync::Arc;

use derivative::Derivative;
use fmmap::tokio::{AsyncMmapFile, AsyncMmapFileExt};
use relative_path::RelativePath;
use rquickjs::{loader::Loader, module::ModuleData, Ctx, Error};
use swc_core::{base::config::IsModule, ecma::parser::Syntax};
use tokio::runtime::Handle;

use crate::transpile::EasySwcTranspiler;

#[derive(Derivative)]
#[derivative(Debug)]
#[derivative(Default(new = "true"))]
pub struct MmapScriptLoader {
    extensions: Vec<String>,
    #[derivative(Debug = "ignore")]
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
    fn load<'js>(&mut self, _ctx: &Ctx<'js>, path: &str) -> rquickjs::Result<ModuleData> {
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
            let (src, _) = self
                .transpiler
                .transpile(
                    std::str::from_utf8(source.as_slice())?,
                    Syntax::Typescript(Default::default()),
                    IsModule::Bool(true),
                    false,
                )
                .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;

            Ok(ModuleData::source(path, src))
        };

        tokio::task::block_in_place(move || Handle::current().block_on(task))
    }
}
