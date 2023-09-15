use matchit::{MatchError, Router};
use rquickjs::{loader::Resolver, Ctx, Error};

use url::{ParseError, Url};

#[derive(Default)]
pub struct HttpResolver {
    pub(crate) allowlist: Option<Router<String>>,
    pub(crate) denylist:  Option<Router<String>>,
}

impl Resolver for HttpResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base_path: &str, path: &str) -> rquickjs::Result<String> {
        let base_path_url = Url::parse(base_path);
        let path_url = Url::parse(path);

        let name = match (base_path_url, path_url) {
            // If both paths are okay, join them together. Usually it will take the current path
            (Ok(base_path), Ok(path)) => base_path.join(path.as_str()).map_err(|_| ()),
            // Try to join the path, and if that's not okay we will just use the base path
            // instead
            (Ok(base_path), Err(ParseError::RelativeUrlWithoutBase)) => {
                base_path.join(path).or(Ok(base_path))
            }
            // Only the current path
            (_, Ok(path)) => Ok(path),
            // Only base path
            (Ok(base_path), _) => Ok(base_path),
            // Placeholder
            _ => Err(()),
        }
        .map_err(|_| Error::new_resolving_message(base_path, path, "path is invalid"))?;

        // If an allow list exists and the current path is not in it, deny
        if let Some(allow) = &self.allowlist {
            if let Err(MatchError::NotFound) = allow.at(name.as_str()) {
                let msg = format!("{name} is not allowed");
                return Err(Error::new_resolving_message(base_path, path, msg));
            }
        }

        // If a deny list exists and the current path is in it, deny
        if let Some(deny) = &self.denylist {
            if deny.at(name.as_str()).is_ok() {
                let msg = format!("{name} is denied");
                return Err(Error::new_resolving_message(base_path, path, msg));
            }
        }

        match name.scheme().to_ascii_lowercase().as_str() {
            "http" | "https" => Ok(name.into()),
            _ => Err(Error::new_resolving(base_path, path)),
        }
    }
}
