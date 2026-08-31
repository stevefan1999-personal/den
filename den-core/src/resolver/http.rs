use rquickjs::{
    Ctx, Error, Result,
    loader::{ImportAttributes, Resolver},
};
use url::{ParseError, Url};

#[expect(
    clippy::module_name_repetitions,
    reason = "the qualified name distinguishes this resolver from file resolvers"
)]
pub struct HttpResolver;

impl Resolver for HttpResolver {
    fn resolve<'js>(
        &mut self, _ctx: &Ctx<'js>, base: &str, name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        let resolved = Url::parse(name)
            .or_else(|error| {
                match error {
                    ParseError::RelativeUrlWithoutBase => Url::parse(base)?.join(name),
                    error => Err(error),
                }
            })
            .map_err(|_error| Error::new_resolving_message(base, name, "path is invalid"))?;

        match resolved.scheme().to_ascii_lowercase().as_str() {
            "http" | "https" => Ok(resolved.into()),
            _ => Err(Error::new_resolving(base, name)),
        }
    }
}
