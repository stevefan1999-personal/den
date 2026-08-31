#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile};
#[cfg(feature = "transpile")]
use reqwest::header::CONTENT_TYPE;
use rquickjs::{
    Ctx, Error, Module, Result,
    loader::{ImportAttributes, Loader},
    module::Declared,
};
use tokio::runtime::Handle;

use crate::loader::typed::{declare_import_kind, import_kind};

#[derive(Debug)]
pub struct HttpLoader;

impl Loader for HttpLoader {
    fn load<'js>(
        &mut self, ctx: &Ctx<'js>, name: &str, attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let kind = import_kind(name, attributes.as_ref())?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let task = async move {
            let response = reqwest::get(name)
                .await
                .map_err(|e| Error::new_loading_message(name, e.to_string()))?;

            // A `type` attribute declares how to interpret the body instead of
            // treating it as JavaScript.
            if let Some(kind) = kind {
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| Error::new_loading_message(name, e.to_string()))?;
                return declare_import_kind(ctx, name, &body, kind);
            }

            #[cfg(feature = "transpile")]
            let source_type = {
                let extension = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .and_then(|value| value.rsplit('/').next())
                    .filter(|subtype| subtype.eq_ignore_ascii_case("typescript"))
                    .map_or("js", |_| "ts");
                infer_transpile_syntax_by_extension(extension)
                    .unwrap_or_default()
                    .with_module(true)
            };

            if let Ok(body) = response.text().await {
                #[cfg(feature = "transpile")]
                {
                    let src = transpile(&body, source_type).map_err(|e| {
                        Error::new_loading_message("cannot transpile", e.to_string())
                    })?;

                    Module::declare(ctx.clone(), name, src)
                }
                #[cfg(not(feature = "transpile"))]
                {
                    Module::declare(ctx.clone(), name, body)
                }
            } else {
                Err(Error::new_loading_message(
                    name,
                    format!("cannot load {name} as program text"),
                ))
            }
        };

        tokio::task::block_in_place(move || Handle::current().block_on(task))
    }
}
