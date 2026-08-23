//! JS `URLPattern` wrapping the denoland `urlpattern` crate.

use indexmap::{IndexMap, indexmap};
use rquickjs::{
    Ctx, Exception, FromJs, IntoJs, JsLifetime, Object, Result, Value, class::Trace, function::Opt,
};
use url::Url;
use urlpattern::{UrlPattern as Inner, UrlPatternInit, UrlPatternMatchInput, UrlPatternOptions};

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

    fn parse_base(ctx: &Ctx<'_>, value: Option<String>) -> Result<Option<Url>> {
        match value {
            None => Ok(None),
            Some(href) => {
                Url::parse(&href)
                    .map(Some)
                    .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))
            }
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
            base_url: Self::parse_base(ctx, Self::optional_string(ctx, obj, "baseURL")?)?,
        })
    }

    fn parse_init<'js>(
        ctx: &Ctx<'js>, input: Value<'js>, base_url: Option<String>,
    ) -> Result<UrlPatternInit> {
        let base = Self::parse_base(ctx, base_url)?;
        if let Some(string) = input.as_string() {
            let href = string.to_string()?;
            return Ok(UrlPatternInit {
                pathname: Some(href),
                base_url: base,
                ..Default::default()
            });
        }
        let Some(obj) = input.as_object() else {
            return Err(Exception::throw_type(
                ctx,
                "URLPattern input must be a string or object",
            ));
        };
        let mut init = Self::init_from_object(ctx, obj)?;
        if init.base_url.is_none() {
            init.base_url = base;
        }
        Ok(init)
    }

    fn match_input<'js>(ctx: &Ctx<'js>, input: Value<'js>) -> Result<UrlPatternMatchInput> {
        if let Some(string) = input.as_string() {
            let href = string.to_string()?;
            let url = Url::parse(&href)
                .map_err(|err| Exception::throw_type(ctx, &format!("invalid URL: {err}")))?;
            return Ok(UrlPatternMatchInput::Url(url));
        }
        if let Some(obj) = input.as_object() {
            return Ok(UrlPatternMatchInput::Init(Self::init_from_object(
                ctx, obj,
            )?));
        }
        Err(Exception::throw_type(
            ctx,
            "URLPattern input must be a string or object",
        ))
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
        let init = Self::parse_init(&ctx, input, base_url.0)?;
        let inner = Inner::parse(init, UrlPatternOptions::default())
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?;
        Ok(Self { inner })
    }

    pub fn test<'js>(&self, ctx: Ctx<'js>, input: Value<'js>) -> Result<bool> {
        let input = Self::match_input(&ctx, input)?;
        self.inner
            .test(input)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))
    }

    pub fn exec<'js>(&self, ctx: Ctx<'js>, input: Value<'js>) -> Result<Value<'js>> {
        let input = Self::match_input(&ctx, input)?;
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
