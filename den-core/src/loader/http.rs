use derivative::Derivative;
use mime::Mime;
use reqwest::header::CONTENT_TYPE;
use rquickjs::{loader::Loader, module::Declared, Ctx, Error, Module};
use tokio::runtime::Handle;
use typed_builder::TypedBuilder;

#[cfg(feature = "transpile")]
use {
    den_transpiler_swc::swc_core::base::config::IsModule, den_transpiler_swc::EasySwcTranspiler,
    den_utils::transpile::infer_transpile_syntax_by_extension, std::sync::Arc,
};


#[derive(Derivative, TypedBuilder)]
#[derivative(Default(new = "true"))]
pub struct HttpLoader {
    #[derivative(Default(value = "true"))]
    #[builder(default)]
    check_mime: bool,
    #[derivative(Debug = "ignore")]
    #[cfg(feature = "transpile")]
    transpiler: Arc<EasySwcTranspiler>,
}

impl Loader for HttpLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Declared>> {
        let task = async move {
            let body = reqwest::get(name)
                .await
                .map_err(|e| Error::new_loading_message(name, e.to_string()))?;
            let extension = if self.check_mime {
                let mime_type = body
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|x| x.to_str().ok())
                    .and_then(|x| x.parse::<Mime>().ok());
                // We need to check whether the MIME type is "text/javascript",
                // "text/typescript", "application/javascript", "application/typescript", ...
                'check_mime: loop {
                    match mime_type {
                        Some(ref mime)
                            if matches!(mime.type_(), mime::TEXT | mime::APPLICATION) =>
                        {
                            let subtype = mime.subtype();

                            if subtype == mime::JAVASCRIPT {
                                break 'check_mime Some("js");
                            }

                            #[cfg(feature = "typescript")]
                            if subtype == "typescript" {
                                break 'check_mime Some("ts");
                            }
                            return Err(Error::new_loading_message(
                                name,
                                format!("{name} is not a valid script"),
                            ));
                        }
                        Some(_) => {
                            return Err(Error::new_loading_message(
                                name,
                                format!("{name} is not a valid script"),
                            ))
                        }
                        None => {
                            let msg = format!(
                                "cannot determine whether the content of {name} is valid \
                                 javascript"
                            );
                            return Err(Error::new_loading_message(name, msg));
                        }
                    };
                }
            } else {
                None
            }.unwrap_or("js");

            if let Ok(body) = body.text().await {

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
                        .map_err(|e| Error::new_loading_message("cannot transpile", e.to_string()))?;
    
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
