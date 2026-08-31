use indexmap::IndexMap;
use rquickjs::{
    Array, Class, Coerced, Ctx, Exception, FromJs as _, Function, IntoJs, Iterable, JsIterator,
    JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Guard {
    None,
    Immutable,
    Request,
    RequestNoCors,
    Response,
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct Headers {
    #[qjs(skip_trace)]
    pub(crate) map:     IndexMap<String, String>,
    #[qjs(skip_trace)]
    pub(crate) cookies: Vec<String>,
    #[qjs(skip_trace)]
    pub(crate) guard:   u8,
}

impl Headers {
    pub(crate) const fn guard(&self) -> Guard {
        match self.guard {
            1 => Guard::Immutable,
            2 => Guard::Request,
            3 => Guard::RequestNoCors,
            4 => Guard::Response,
            _ => Guard::None,
        }
    }

    pub(crate) const fn set_guard(&mut self, guard: Guard) {
        self.guard = match guard {
            Guard::None => 0,
            Guard::Immutable => 1,
            Guard::Request => 2,
            Guard::RequestNoCors => 3,
            Guard::Response => 4,
        };
    }

    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        self.map
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    pub(crate) fn from_pairs(
        pairs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
    ) -> Self {
        let mut headers = Self {
            map:     IndexMap::new(),
            cookies: Vec::new(),
            guard:   0,
        };
        for (name, value) in pairs {
            headers.append_combined(
                name.as_ref().to_ascii_lowercase(),
                value.as_ref().to_string(),
            );
        }
        headers
    }

    pub(crate) fn empty_with(guard: Guard) -> Self {
        let mut headers = Self {
            map:     IndexMap::new(),
            cookies: Vec::new(),
            guard:   0,
        };
        headers.set_guard(guard);
        headers
    }

    pub(crate) fn from_init<'js>(
        ctx: Ctx<'js>, init: Option<Value<'js>>, guard: Guard,
    ) -> Result<Self> {
        let mut headers = Self::empty_with(guard);
        if let Some(init) = init.filter(|value| !value.is_undefined()) {
            headers.fill(&ctx, init)?;
        }
        Ok(headers)
    }

    fn is_valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.bytes().all(|byte| {
                matches!(
                    byte,
                    b'0'..=b'9'
                        | b'a'..=b'z'
                        | b'A'..=b'Z'
                        | b'!'
                        | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
            })
    }

    fn normalize_name(ctx: &Ctx<'_>, name: &str) -> Result<String> {
        if !Self::is_valid_name(name) {
            return Err(Exception::throw_type(
                ctx,
                &format!("Invalid character in header field name: \"{name}\""),
            ));
        }
        Ok(name.to_ascii_lowercase())
    }

    pub(crate) fn normalize_value(ctx: &Ctx<'_>, value: &str) -> Result<String> {
        if value.chars().any(|ch| (ch as u32) > 0xff) {
            return Err(Exception::throw_type(
                ctx,
                "Header value is not a ByteString",
            ));
        }
        let stripped = strip_http_whitespace(value);
        if stripped
            .bytes()
            .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
        {
            return Err(Exception::throw_type(ctx, "Invalid header value"));
        }
        Ok(stripped)
    }

    fn append_combined(&mut self, name: String, value: String) {
        if name == "set-cookie" {
            self.cookies.push(value);
            return;
        }
        match self.map.get(&name) {
            Some(old) => {
                let combined = format!("{old}, {value}");
                self.map.insert(name, combined);
            }
            None => {
                self.map.insert(name, value);
            }
        }
    }

    fn fill<'js>(&mut self, ctx: &Ctx<'js>, init: Value<'js>) -> Result<()> {
        let Some(object) = init.as_object() else {
            return Err(Exception::throw_type(ctx, "HeadersInit must be an object"));
        };

        let iterator: Value = object.get(PredefinedAtom::SymbolIterator)?;
        if !iterator.is_null() && !iterator.is_undefined() {
            let iterator =
                Function::from_js(ctx, iterator)?.call::<_, Value>((This(object.clone()),))?;
            return self.fill_pairs(ctx, JsIterator::from_js(ctx, iterator)?);
        }

        let reflect: Object = ctx.globals().get("Reflect")?;
        let own_keys: Function = reflect.get("ownKeys")?;
        let keys: Array = own_keys.call((object.clone(),))?;
        let object_ctor: Object = ctx.globals().get(PredefinedAtom::Object)?;
        let descriptor: Function = object_ctor.get(PredefinedAtom::GetOwnPropertyDescriptor)?;
        for index in 0..keys.len() {
            let key: Value = keys.get(index)?;
            let property: Value = descriptor.call((object.clone(), key.clone()))?;
            let Some(property) = property.as_object() else {
                continue;
            };
            if !property.get::<_, bool>("enumerable").unwrap_or(false) {
                continue;
            }
            let name = den_util::coerce_string(ctx, key.clone())?;
            if name.chars().any(|ch| (ch as u32) > 0xff) {
                return Err(Exception::throw_type(
                    ctx,
                    "Header name is not a ByteString",
                ));
            }
            let value = den_util::coerce_string(ctx, object.get::<_, Value>(key)?)?;
            self.append(ctx.clone(), Coerced(name), Coerced(value))?;
        }
        Ok(())
    }

    fn fill_pairs<'js>(
        &mut self, ctx: &Ctx<'js>, entries: impl Iterator<Item = Result<Value<'js>>>,
    ) -> Result<()> {
        for entry in entries {
            let mut pair = JsIterator::<Value>::from_js(ctx, entry?)?;
            let Some(name) = pair.next().transpose()? else {
                return Err(Exception::throw_type(
                    ctx,
                    "Expected name/value pair to be length 2, found 0",
                ));
            };
            let Some(value) = pair.next().transpose()? else {
                return Err(Exception::throw_type(
                    ctx,
                    "Expected name/value pair to be length 2, found 1",
                ));
            };
            if pair.next().transpose()?.is_some() {
                return Err(Exception::throw_type(
                    ctx,
                    "Expected name/value pair to be length 2, found more than 2",
                ));
            }
            let name = den_util::coerce_string(ctx, name)?;
            let value = den_util::coerce_string(ctx, value)?;
            self.append(ctx.clone(), Coerced(name), Coerced(value))?;
        }
        Ok(())
    }

    fn sorted_pairs(&self) -> Vec<(String, String)> {
        let mut names = self.map.keys().cloned().collect::<Vec<_>>();
        if !self.cookies.is_empty() && !names.iter().any(|name| name == "set-cookie") {
            names.push("set-cookie".to_string());
        }
        names.sort();
        let mut pairs = Vec::new();
        for name in names {
            if name == "set-cookie" {
                for cookie in &self.cookies {
                    pairs.push((name.clone(), cookie.clone()));
                }
            } else if let Some(value) = self.map.get(&name) {
                pairs.push((name, value.clone()));
            }
        }
        pairs
    }

    fn live_iter<'js, T: IntoJs<'js> + 'js>(
        ctx: &Ctx<'js>, headers: Class<'js, Self>, map: impl Fn((String, String)) -> T + 'js,
    ) -> Result<Value<'js>> {
        let mut index = 0;
        Iterable::from_fn(move || {
            let pair = headers.borrow().sorted_pairs().get(index).cloned();
            index += usize::from(pair.is_some());
            pair.map(&map)
        })
        .into_js(ctx)
    }

    fn check_guard(&self, ctx: &Ctx<'_>, name: &str, value: &str, combine: bool) -> Result<bool> {
        match self.guard() {
            Guard::None => Ok(true),
            Guard::Immutable => Err(Exception::throw_type(ctx, "Headers are immutable")),
            Guard::Request => Ok(!is_forbidden_request_header(name, value)),
            Guard::RequestNoCors => {
                if value.is_empty() && !combine {
                    return Ok(matches!(
                        name,
                        "accept" | "accept-language" | "content-language" | "content-type"
                    ));
                }
                let existing = if combine { self.map.get(name) } else { None };
                Ok(is_no_cors_safelisted(name, value, existing))
            }
            Guard::Response => Ok(!is_forbidden_response_header(name)),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Headers {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
        let mut headers = Self {
            map:     IndexMap::new(),
            cookies: Vec::new(),
            guard:   0,
        };
        if let Some(init) = init.0.filter(|value| !value.is_undefined()) {
            headers.fill(&ctx, init)?;
        }
        Ok(headers)
    }

    pub fn append(
        &mut self, ctx: Ctx<'_>, name: Coerced<String>, value: Coerced<String>,
    ) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        let value = Self::normalize_value(&ctx, &value.0)?;
        if !self.check_guard(&ctx, &name, &value, true)? {
            return Ok(());
        }
        self.append_combined(name, value);
        Ok(())
    }

    #[qjs(rename = "delete")]
    pub fn r#delete(&mut self, ctx: Ctx<'_>, name: Coerced<String>) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        if !self.check_guard(&ctx, &name, "", false)? {
            return Ok(());
        }
        if name == "set-cookie" {
            self.cookies.clear();
        }
        self.map.shift_remove(&name);
        Ok(())
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, name: Coerced<String>) -> Result<Value<'js>> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        if name == "set-cookie" {
            return if self.cookies.is_empty() {
                Ok(Value::new_null(ctx))
            } else {
                self.cookies.join(", ").into_js(&ctx)
            };
        }
        match self.map.get(&name) {
            Some(value) => value.clone().into_js(&ctx),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn get_set_cookie(&self) -> Vec<String> { self.cookies.clone() }

    pub fn has(&self, ctx: Ctx<'_>, name: Coerced<String>) -> Result<bool> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        if name == "set-cookie" {
            return Ok(!self.cookies.is_empty());
        }
        Ok(self.map.contains_key(&name))
    }

    pub fn set(
        &mut self, ctx: Ctx<'_>, name: Coerced<String>, value: Coerced<String>,
    ) -> Result<()> {
        let name = Self::normalize_name(&ctx, &name.0)?;
        let value = Self::normalize_value(&ctx, &value.0)?;
        if !self.check_guard(&ctx, &name, &value, false)? {
            return Ok(());
        }
        if name == "set-cookie" {
            self.cookies.clear();
            self.cookies.push(value);
            return Ok(());
        }
        self.map.insert(name, value);
        Ok(())
    }

    pub fn for_each<'js>(
        this: This<Class<'js, Headers>>, callback: Function<'js>, this_arg: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<()> {
        let this_arg = this_arg.0.unwrap_or_else(|| Value::new_undefined(ctx));
        let mut index = 0;
        while let Some((name, value)) = this.0.borrow().sorted_pairs().get(index).cloned() {
            callback.call::<_, ()>((This(this_arg.clone()), value, name, this.0.clone()))?;
            index += 1;
        }
        Ok(())
    }

    pub fn keys<'js>(this: This<Class<'js, Headers>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::live_iter(&ctx, this.0, |(name, _)| name)
    }

    pub fn values<'js>(this: This<Class<'js, Headers>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::live_iter(&ctx, this.0, |(_, value)| value)
    }

    pub fn entries<'js>(this: This<Class<'js, Headers>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::live_iter(&ctx, this.0, |(name, value)| vec![name, value])
    }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn iterate<'js>(this: This<Class<'js, Headers>>, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Self::live_iter(&ctx, this.0, |(name, value)| vec![name, value])
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Headers" }
}

fn strip_http_whitespace(value: &str) -> String {
    value
        .trim_matches(|byte| matches!(byte, '\t' | '\n' | '\r' | ' '))
        .to_string()
}

pub fn is_forbidden_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "CONNECT" | "TRACE" | "TRACK"
    )
}

pub fn is_forbidden_request_header(name: &str, value: &str) -> bool {
    if matches!(
        name,
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
    {
        return true;
    }
    if matches!(
        name,
        "x-http-method" | "x-http-method-override" | "x-method-override"
    ) {
        return value
            .split(',')
            .any(|part| is_forbidden_method(part.trim()));
    }
    false
}

const fn is_cors_unsafe_byte(byte: u8) -> bool {
    matches!(
        byte,
        0x00..=0x08
            | 0x0A..=0x1F
            | 0x22
            | 0x28
            | 0x29
            | 0x3A
            | 0x3C
            | 0x3E
            | 0x3F
            | 0x40
            | 0x5B
            | 0x5C
            | 0x5D
            | 0x7B
            | 0x7D
            | 0x7F
    )
}

fn is_forbidden_response_header(name: &str) -> bool { matches!(name, "set-cookie" | "set-cookie2") }

fn is_no_cors_safelisted(name: &str, value: &str, existing: Option<&String>) -> bool {
    let combined = existing.map_or_else(|| value.to_string(), |old| format!("{old}, {value}"));
    match name {
        "accept" | "accept-language" | "content-language" => {
            combined.len() <= 128 && !combined.bytes().any(is_cors_unsafe_byte)
        }
        "content-type" => {
            let mime = combined
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            matches!(
                mime.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            ) && combined.len() <= 128
        }
        _ => false,
    }
}
