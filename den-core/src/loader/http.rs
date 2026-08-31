#[cfg(feature = "transpile")]
use den_transpiler_oxc::{infer_transpile_syntax_by_extension, transpile_with_source_map};
#[cfg(feature = "transpile")]
use reqwest::header::CONTENT_TYPE;
use rquickjs::{
    Ctx, Error, Module, Result,
    loader::{ImportAttributes, Loader},
    module::Declared,
};
use tokio::runtime::Handle;
use url::Url;

use crate::loader::typed::{declare_import_kind, import_kind};

#[cfg(feature = "ring")]
fn install_default_crypto_provider() {
    let provider = rustls::crypto::ring::default_provider();
    let _provider_installed = provider.install_default();
}

#[cfg(not(feature = "ring"))]
const fn install_default_crypto_provider() {}

#[derive(Debug)]
pub struct HttpLoader;

impl Loader for HttpLoader {
    fn load<'js>(
        &mut self, ctx: &Ctx<'js>, name: &str, attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let url = Url::parse(name).map_err(|_error| Error::new_loading(name))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::new_loading(name));
        }
        install_default_crypto_provider();
        let kind = import_kind(name, attributes.as_ref())?;
        let task = async move {
            let response = reqwest::get(url)
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
                let authored_map = load_source_map(name, &body).await;
                #[cfg(feature = "transpile")]
                {
                    let output =
                        transpile_with_source_map(&body, source_type, name).map_err(|e| {
                            Error::new_loading_message("cannot transpile", e.to_string())
                        })?;
                    let mut maps = vec![output.source_map.into_inner()];
                    maps.extend(authored_map);
                    den_util::stack::register_source(ctx, name, output.code.clone(), maps)?;
                    Module::declare(ctx.clone(), name, output.code)
                }
                #[cfg(not(feature = "transpile"))]
                {
                    den_util::stack::register_source(ctx, name, body.clone(), authored_map)?;
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

async fn load_source_map(name: &str, source: &str) -> Option<oxc_sourcemap::SourceMap<'static>> {
    let mapping = den_util::stack::source_mapping_url(source)?;
    let source_url = Url::parse(name).ok()?;
    if mapping.starts_with("data:") {
        return den_util::stack::inline_source_map(mapping, &source_url);
    }
    let map_url = source_url.join(mapping).ok()?;
    if !matches!(map_url.scheme(), "http" | "https") {
        return None;
    }
    let json = reqwest::get(map_url.clone())
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    den_util::stack::parse_source_map(&json, &map_url)
}
