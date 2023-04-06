use mime::Mime;
use reqwest::header::CONTENT_TYPE;
use rquickjs::{Ctx, Error, Loaded, Loader, Module};

#[derive(Debug)]
pub struct HttpLoader {
    check_mime: bool,
}

impl Default for HttpLoader {
    fn default() -> Self {
        Self { check_mime: true }
    }
}

impl Loader for HttpLoader {
    fn load<'js>(&mut self, ctx: Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Loaded>> {
        tokio::task::block_in_place(|| match reqwest::blocking::get(name) {
            Ok(body) => {
                if self.check_mime {
                    let mime_type = body
                        .headers()
                        .get(CONTENT_TYPE)
                        .and_then(|x| x.to_str().ok())
                        .and_then(|x| x.parse::<Mime>().ok());
                    'check_mime: loop {
                        return match mime_type {
                            Some(mime) => {
                                if let mime::TEXT | mime::APPLICATION = mime.type_() {
                                    let subtype = mime.subtype();
                                    if subtype == mime::JAVASCRIPT {
                                        break 'check_mime;
                                    }
                                    if subtype == "typescript" {
                                        break 'check_mime;
                                    }
                                }
                                Err(Error::new_loading_message(
                                    name,
                                    format!("{name} is not a valid script"),
                                ))
                            }
                            None => {
                                let msg = format!(
                                    "cannot determine whether the content of {name} is valid javascript"
                                );
                                Err(Error::new_loading_message(name, msg))
                            }
                        };
                    }
                }

                if let Ok(body) = body.text() {
                    Ok(Module::new(ctx, name, body)?.into_loaded())
                } else {
                    Err(Error::new_loading_message(
                        name,
                        format!("cannot load {name} as program text"),
                    ))
                }
            }
            Err(e) => Err(Error::new_loading_message(name, e.to_string())),
        })
    }
}
