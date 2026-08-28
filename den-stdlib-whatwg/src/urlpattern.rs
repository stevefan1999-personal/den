//! JS `URLPattern` wrapping the denoland `urlpattern` crate.

use std::borrow::Cow;

use indexmap::IndexMap;
use rquickjs::{
    Array, Coerced, Ctx, Exception, FromJs, IntoJs, JsLifetime, Object, Result, Value,
    atom::PredefinedAtom, class::Trace, function::Opt,
};
use urlpattern::{
    UrlPattern as Inner, UrlPatternComponentResult, UrlPatternMatchInput, UrlPatternOptions,
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
        Self::optional_arg(ctx, Opt(obj.get(name)?))
    }

    /// A missing argument and an explicit `undefined`/`null` are the same
    /// absence; anything else is stringified as the IDL would.
    fn optional_arg<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<Option<String>> {
        match value.0 {
            None => Ok(None),
            Some(value) if value.is_undefined() || value.is_null() => Ok(None),
            Some(value) => Ok(Some(Coerced::<String>::from_js(ctx, value)?.0)),
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
    fn string_or_init<'js>(
        ctx: &Ctx<'js>, input: Opt<Value<'js>>,
    ) -> Result<StringOrInit<'static>> {
        let Some(input) = input.0 else {
            return Ok(StringOrInit::Init(UrlPatternInit::default()));
        };
        if input.is_undefined() || input.is_null() {
            return Ok(StringOrInit::Init(UrlPatternInit::default()));
        }
        match input.as_object() {
            Some(obj) => Ok(StringOrInit::Init(Self::init_from_object(ctx, obj)?)),
            None => {
                Ok(StringOrInit::String(Cow::Owned(
                    Coerced::<String>::from_js(ctx, input)?.0,
                )))
            }
        }
    }

    /// `None` means the input could not be parsed as a URL at all, which the
    /// spec reports as "no match" rather than as an error.
    fn match_input<'js>(
        ctx: &Ctx<'js>, input: Value<'js>, base_url: Option<&str>,
    ) -> Result<Option<UrlPatternMatchInput>> {
        let input = Self::string_or_init(ctx, Opt(Some(input)))?;
        quirks::process_match_input(input, base_url)
            .map(|matched| matched.map(|(input, _)| input))
            .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))
    }

    fn component_to_js<'js>(
        ctx: &Ctx<'js>, component: &UrlPatternComponentResult,
    ) -> Result<Value<'js>> {
        let mut groups = IndexMap::new();
        for (name, value) in &component.groups {
            groups.insert(name.clone(), match value {
                Some(value) => value.clone().into_js(ctx)?,
                None => Value::new_undefined(ctx.clone()),
            });
        }
        let entry = Object::new(ctx.clone())?;
        entry.set("input", component.input.as_str())?;
        entry.set("groups", groups)?;
        Ok(entry.into_value())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl URLPattern {
    /// `new URLPattern(input, baseURL?, options?)` and the two-argument
    /// `new URLPattern(input, options?)` overload, as implemented by Deno.
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>, input: Opt<Value<'js>>, base_or_options: Opt<Value<'js>>,
        trailing_options: Opt<Object<'js>>,
    ) -> Result<Self> {
        let (base_url, options) = match base_or_options.0 {
            // The two-argument overload passes the options bag in the baseURL slot.
            Some(value) if value.is_object() => (None, value.into_object()),
            other => (Self::optional_arg(&ctx, Opt(other))?, trailing_options.0),
        };
        let options = match options {
            None => UrlPatternOptions::default(),
            Some(options) => {
                UrlPatternOptions {
                    ignore_case: options
                        .get::<_, Option<Coerced<bool>>>("ignoreCase")?
                        .is_some_and(|flag| flag.0),
                    ..UrlPatternOptions::default()
                }
            }
        };

        let input = Self::string_or_init(&ctx, input)?;
        let init = quirks::process_construct_pattern_input(input, base_url.as_deref())
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?;
        let inner = Inner::parse(init, options)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?;
        Ok(Self { inner })
    }

    pub fn test<'js>(
        &self, ctx: Ctx<'js>, input: Value<'js>, base_url: Opt<Value<'js>>,
    ) -> Result<bool> {
        let base_url = Self::optional_arg(&ctx, base_url)?;
        let Some(input) = Self::match_input(&ctx, input, base_url.as_deref())? else {
            return Ok(false);
        };
        self.inner
            .test(input)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))
    }

    pub fn exec<'js>(
        &self, ctx: Ctx<'js>, input: Value<'js>, base_url: Opt<Value<'js>>,
    ) -> Result<Value<'js>> {
        let inputs = Array::new(ctx.clone())?;
        inputs.set(0, input.clone())?;
        if let Some(base_url) = base_url.0.as_ref() {
            inputs.set(1, base_url.clone())?;
        }

        let base_url = Self::optional_arg(&ctx, base_url)?;
        let Some(input) = Self::match_input(&ctx, input, base_url.as_deref())? else {
            return Ok(Value::new_null(ctx));
        };
        let Some(result) = self
            .inner
            .exec(input)
            .map_err(|err| Exception::throw_type(&ctx, &format!("{err}")))?
        else {
            return Ok(Value::new_null(ctx));
        };

        let matched = Object::new(ctx.clone())?;
        matched.set("inputs", inputs)?;
        for (name, component) in [
            ("protocol", &result.protocol),
            ("username", &result.username),
            ("password", &result.password),
            ("hostname", &result.hostname),
            ("port", &result.port),
            ("pathname", &result.pathname),
            ("search", &result.search),
            ("hash", &result.hash),
        ] {
            matched.set(name, Self::component_to_js(&ctx, component)?)?;
        }
        Ok(matched.into_value())
    }

    #[qjs(get)]
    pub fn protocol(&self) -> String { self.inner.protocol().to_string() }

    #[qjs(get)]
    pub fn username(&self) -> String { self.inner.username().to_string() }

    #[qjs(get)]
    pub fn password(&self) -> String { self.inner.password().to_string() }

    #[qjs(get)]
    pub fn hostname(&self) -> String { self.inner.hostname().to_string() }

    #[qjs(get)]
    pub fn port(&self) -> String { self.inner.port().to_string() }

    #[qjs(get)]
    pub fn pathname(&self) -> String { self.inner.pathname().to_string() }

    #[qjs(get)]
    pub fn search(&self) -> String { self.inner.search().to_string() }

    #[qjs(get)]
    pub fn hash(&self) -> String { self.inner.hash().to_string() }

    #[qjs(get)]
    pub fn has_reg_exp_groups(&self) -> bool { self.inner.has_regexp_groups() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "URLPattern" }
}
