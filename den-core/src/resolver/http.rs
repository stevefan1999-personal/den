use rquickjs::{
    Ctx, Error, Result,
    loader::{ImportAttributes, Resolver},
};
use url::{ParseError, Url};

pub struct HttpResolver;

impl Resolver for HttpResolver {
    fn resolve<'js>(
        &mut self, _ctx: &Ctx<'js>, base_path: &str, path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
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

        match name.scheme().to_ascii_lowercase().as_str() {
            "http" | "https" => Ok(name.into()),
            _ => Err(Error::new_resolving(base_path, path)),
        }
    }
}
