use derive_more::Debug;
use mime::Mime;
use reqwest::header::CONTENT_TYPE;
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

use crate::loader::typed::{declare_import_kind, import_kind};

#[derive(Debug, TypedBuilder)]
pub struct HttpLoader {
    #[builder(default)]
    check_mime: bool,
    #[debug(ignore)]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasyOxcTranspiler>,
}

/// What a response is treated as when its flavour cannot be narrowed further.
const DEFAULT_SCRIPT_EXTENSION: &str = "js";

impl Default for HttpLoader {
    fn default() -> Self {
        Self {
            check_mime:                               true,
            #[cfg(feature = "transpile")]
            transpiler:                               Arc::default(),
        }
    }
}

impl HttpLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Derive the script extension from the response's `Content-Type`.
    ///
    /// This is a gate, not a hint: a remote import is only ever executed as
    /// script, so anything that is not recognisably script is refused rather
    /// than being guessed at.
    fn sniff_extension(&self, name: &str, response: &reqwest::Response) -> Result<&'static str> {
        if !self.check_mime {
            return Ok(DEFAULT_SCRIPT_EXTENSION);
        }

        let Some(mime) = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<Mime>().ok())
        else {
            let msg = format!("cannot determine whether the content of {name} is valid javascript");
            return Err(Error::new_loading_message(name, msg));
        };

        let not_a_script =
            || Error::new_loading_message(name, format!("{name} is not a valid script"));

        if !matches!(mime.type_(), mime::TEXT | mime::APPLICATION) {
            return Err(not_a_script());
        }

        let subtype = mime.subtype();
        if subtype == mime::JAVASCRIPT {
            return Ok(DEFAULT_SCRIPT_EXTENSION);
        }
        #[cfg(feature = "typescript")]
        if subtype == "typescript" {
            return Ok("ts");
        }
        Err(not_a_script())
    }
}

impl Loader for HttpLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let kind = import_kind(name, attributes.as_ref())?;
        let task = async move {
            let response = reqwest::get(name)
                .await
                .map_err(|e| Error::new_loading_message(name, e.to_string()))?;

            // The MIME sniff is a script gate. A `type` attribute is the
            // caller's declaration of what the body is, including
            // `application/json` / `application/octet-stream` that would
            // otherwise be refused.
            if let Some(kind) = kind {
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| Error::new_loading_message(name, e.to_string()))?;
                return declare_import_kind(ctx, name, &body, kind);
            }

            // Without the transpiler the sniffed extension is unused, but the sniffing
            // itself still rejects responses that are not script.
            #[allow(unused_variables)]
            let extension = self.sniff_extension(name, &response)?;

            if let Ok(body) = response.text().await {
                #[cfg(feature = "transpile")]
                {
                    let (src, _) = self
                        .transpiler
                        .transpile(
                            &body,
                            infer_transpile_syntax_by_extension(extension).unwrap_or_default(),
                            IsModule::Bool(true),
                            false,
                        )
                        .map_err(|e| {
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
