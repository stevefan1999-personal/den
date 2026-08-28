//! JS `URLPattern` wrapping the denoland `urlpattern` crate.

use std::borrow::Cow;

use indexmap::{IndexMap, indexmap};
use rquickjs::{
    Ctx, Exception, FromJs, IntoJs, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};
use urlpattern::{
    UrlPattern as Inner, UrlPatternMatchInput, UrlPatternOptions,
    quirks::{self, StringOrInit, UrlPatternInit},
};

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct URLPattern {
    #[qjs(skip_trace)]
    inner: Inner,
}

impl URLPattern {
    fn optional_string<'js>(
        ctx: &Ctx<'js>, obj: &Object<'js>, name: &str,
    ) -> Result<Option<String>> {
        let value: Value = obj.get(name)?;
        if value.is_undefined() || value.is_null() {
            Ok(None)
        } else {
            Ok(Some(String::from_js(ctx, value)?))
        }
    }

    fn init_from_object<'js>(ctx: &Ctx<'js>, obj: &Object<'js>) -> Result<UrlPatternInit> {
        Ok(UrlPatternInit {
            protocol: Self::optional_string(ctx, obj, "protocol")?,
            username: Self::optional_string(ctx, obj, "username")?,
            password: Self::optional_string(ctx, obj, "password")?,
            hostname: Self::optional_string(ctx, obj, "hostname")?,
            port:     Self::optional_string(ctx, obj, "port")?,
            pathname: Self::optional_string(ctx, obj, "pathname")?,
            search:   Self::optional_string(ctx, obj, "search")?,
            hash:     Self::optional_string(ctx, obj, "hash")?,
            base_url: Self::optional_string(ctx, obj, "baseURL")?,
        })
    }

    /// A pattern or a match target is either a full URL-shaped string — which
    /// the crate decomposes into all eight components — or an init dictionary.
    /// A missing argument is an empty init, i.e. every component wildcards.
    fn string_or_init<'js>(ctx: &Ctx<'js>, input: &Value<'js>) -> Result<StringOrInit<'static>> {
        if input.is_undefined() || input.is_null() {
            return Ok(StringOrInit::Init(UrlPatternInit::default()));
        }
        if let Some(string) = input.as_string() {
            return Ok(StringOrInit::String(Cow::Owned(string.to_string()?)));
        }
        let Some(obj) = input.as_object() else {
            return Err(Exception::throw_type(
                ctx,
                "URLPattern input must be a string or object",
            ));
        };
        Ok(StringOrInit::Init(Self::init_from_object(ctx, obj)?))
    }

    /// `None` means the input could not be parsed as a URL at all, which the
    /// spec reports as "no match" rather than as an error.
    fn match_input<'js>(
        ctx: &Ctx<'js>, input: Value<'js>, base_url: Option<&str>,
    ) -> Result<Option<UrlPatternMatchInput>> {
        let input = Self::string_or_init(ctx, &input)?;
        quirks::process_match_input(input, base_url)
            .map(|matched| matched.map(|(input, _)| input))
            .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))
    }

    fn component_to_js<'js>(
        ctx: &Ctx<'js>, input: &str, groups: &std::collections::HashMap<String, Option<String>>,
    ) -> Result<Value<'js>> {
        let mut group_map = IndexMap::new();
        for (name, value) in groups {
            group_map.insert(name.clone(), match value {
                Some(value) => value.clone().into_js(ctx)?,
                None => Value::new_undefined(ctx.clone()),
            });
        }
        indexmap! {
          "input" => input.into_js(ctx)?,
          "groups" => group_map.into_js(ctx)?,
        }
        .into_js(ctx)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl URLPattern {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, input: Value<'js>, base_url: Opt<String>) -> Result<Self> {
        let input = Self::string_or_init(&ctx, &input)?;
        let init = quirks::process_construct_pattern_input(input, base_url.0.as_deref())
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?;
        let inner = Inner::parse(init, UrlPatternOptions::default())
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?;
        Ok(Self { inner })
    }

    pub fn test<'js>(
        &self, ctx: Ctx<'js>, input: Value<'js>, base_url: Opt<String>,
    ) -> Result<bool> {
        let Some(input) = Self::match_input(&ctx, input, base_url.0.as_deref())? else {
            return Ok(false);
        };
        self.inner
            .test(input)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))
    }

    pub fn exec<'js>(
        &self, ctx: Ctx<'js>, input: Value<'js>, base_url: Opt<String>,
    ) -> Result<Value<'js>> {
        let Some(input) = Self::match_input(&ctx, input, base_url.0.as_deref())? else {
            return Ok(Value::new_null(ctx));
        };
        match self
            .inner
            .exec(input)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?
        {
            None => Ok(Value::new_null(ctx)),
            Some(result) => {
                indexmap! {
                  "pathname" => Self::component_to_js(
                    &ctx,
                    &result.pathname.input,
                    &result.pathname.groups,
                  )?,
                  "protocol" => Self::component_to_js(
                    &ctx,
                    &result.protocol.input,
                    &result.protocol.groups,
                  )?,
                  "hostname" => Self::component_to_js(
                    &ctx,
                    &result.hostname.input,
                    &result.hostname.groups,
                  )?,
                }
                .into_js(&ctx)
            }
        }
    }

    #[qjs(get)]
    pub fn pathname(&self) -> String { self.inner.pathname().to_string() }

    #[qjs(get)]
    pub fn protocol(&self) -> String { self.inner.protocol().to_string() }

    #[qjs(get)]
    pub fn hostname(&self) -> String { self.inner.hostname().to_string() }
}
