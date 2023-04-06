use matchit::{MatchError, Router};
use rquickjs::{Ctx, Error, Resolver};
use url::{ParseError, Url};

pub struct HttpResolver {
    pub(crate) allowlist: Option<Router<String>>,
    pub(crate) denylist: Option<Router<String>>,
}

impl Default for HttpResolver {
    fn default() -> Self {
        Self {
            allowlist: None,
            denylist: None,
        }
    }
}

fn get_http_url(base_path: &str) -> Result<Option<Url>, ParseError> {
    match Url::parse(base_path) {
        Ok(x) => match x.scheme() {
            "http" | "https" => Ok(Some(x)),
            _ => Ok(None),
        },
        e => e.map(|_| None),
    }
}

impl Resolver for HttpResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: Ctx<'js>,
        base_path: &str,
        path: &str,
    ) -> rquickjs::Result<String> {
        let base_path_url = get_http_url(base_path);
        let path_url = get_http_url(path);

        let name = match (base_path_url, path_url) {
            (Ok(Some(base_path)), Ok(Some(path))) => base_path.join(&path.to_string()),
            (Ok(Some(base_path)), Err(ParseError::RelativeUrlWithoutBase)) => {
                base_path.join(&path.to_string()).or(Ok(base_path))
            }
            (_, Ok(Some(path))) => Ok(path),
            (Ok(Some(base_path)), _) => Ok(base_path),
            // placeholder
            _ => Err(ParseError::EmptyHost),
        }
        .map_err(|_| Error::new_resolving_message(base_path, path, "path is invalid"))?
        .to_string();

        if let Some(allow) = &self.allowlist {
            if let Err(MatchError::NotFound) = allow.at(&name) {
                let msg = format!("{name} is not allowed");
                return Err(Error::new_resolving_message(base_path, path, msg));
            }
        }

        if let Some(deny) = &self.denylist {
            if let Ok(_) = deny.at(&name) {
                let msg = format!("{name} is denied");
                return Err(Error::new_resolving_message(base_path, path, msg));
            }
        }

        if name.starts_with("http") {
            Ok(name.to_string())
        } else {
            Err(Error::new_resolving(base_path, path))
        }
    }
}
