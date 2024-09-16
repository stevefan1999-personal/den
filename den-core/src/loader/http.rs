use derivative::Derivative;
use mime::Mime;
use reqwest::header::CONTENT_TYPE;
use rquickjs::{loader::Loader, module::Declared, Ctx, Error, Module};
use tokio::runtime::Handle;

#[derive(Derivative)]
#[derivative(Default(new = "true"))]
pub struct HttpLoader {
    #[derivative(Default(value = "true"))]
    check_mime: bool,
}

impl Loader for HttpLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Declared>> {
        let task = async move {
            let body = reqwest::get(name)
                .await
                .map_err(|e| Error::new_loading_message(name, e.to_string()))?;
            if self.check_mime {
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
                                break 'check_mime;
                            }

                            #[cfg(feature = "typescript")]
                            if subtype == "typescript" {
                                break 'check_mime;
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
            }

            if let Ok(body) = body.text().await {
                Module::declare(ctx.clone(), name, body)
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
