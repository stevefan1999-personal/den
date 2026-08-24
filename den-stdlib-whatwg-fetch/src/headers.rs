use indexmap::IndexMap;
use rquickjs::{
    Array, Class, Coerced, Ctx, Exception, Filter, Function, IntoJs, Iterable, JsLifetime, Object,
    Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Guard {
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
    fn guard(&self) -> Guard {
        match self.guard {
            1 => Guard::Immutable,
            2 => Guard::Request,
            3 => Guard::RequestNoCors,
            4 => Guard::Response,
            _ => Guard::None,
        }
    }

    pub(crate) fn set_guard(&mut self, guard: Guard) {
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
        if let Some(init) = init.filter(|value| !value.is_null() && !value.is_undefined()) {
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
            return Ok(());
        };
        if let Some(other) = Class::<Headers>::from_object(object) {
            let other = other.borrow();
            self.map = other.map.clone();
            self.cookies = other.cookies.clone();
            return Ok(());
        }
        if object.is_array() {
            return self.fill_pairs(ctx, object);
        }
        for name in object.own_keys::<String>(Filter::new().string()) {
            let name = name?;
            let value = den_util::coerce_string(ctx, object.get(&name)?)?;
            self.append(ctx.clone(), Coerced(name), Coerced(value))?;
        }
        Ok(())
    }

    fn fill_pairs<'js>(&mut self, ctx: &Ctx<'js>, object: &Object<'js>) -> Result<()> {
        let length: i32 = object.get("length").unwrap_or(0);
        for index in 0..length {
            let entry: Value = object.get(index as u32)?;
            let Some(pair) = entry.as_object() else {
                return Err(Exception::throw_type(
                    ctx,
                    "Expected name/value pair to be length 2, found0",
                ));
            };
            let pair_len: i32 = pair.get("length").unwrap_or(0);
            if pair_len != 2 {
                return Err(Exception::throw_type(
                    ctx,
                    &format!("Expected name/value pair to be length 2, found{pair_len}"),
                ));
            }
            let name = den_util::coerce_string(ctx, pair.get(0)?)?;
            let value = den_util::coerce_string(ctx, pair.get(1)?)?;
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

    fn entries_iter<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let mut pairs = Vec::new();
        for (name, value) in self.sorted_pairs() {
            let pair = Array::new(ctx.clone())?;
            pair.set(0, name)?;
            pair.set(1, value)?;
            pairs.push(pair);
        }
        Iterable::from(pairs).into_js(ctx)
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
        if let Some(init) = init
            .0
            .filter(|value| !value.is_null() && !value.is_undefined())
        {
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
        let entries = this.0.borrow().sorted_pairs();
        let this_arg = this_arg.0.unwrap_or_else(|| Value::new_undefined(ctx));
        for (name, value) in entries {
            callback.call::<_, ()>((This(this_arg.clone()), value, name, this.0.clone()))?;
        }
        Ok(())
    }

    pub fn keys<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Iterable::from(
            self.sorted_pairs()
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
        )
        .into_js(&ctx)
    }

    pub fn values<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        Iterable::from(
            self.sorted_pairs()
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>(),
        )
        .into_js(&ctx)
    }

    pub fn entries<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> { self.entries_iter(&ctx) }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn iterate<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> { self.entries_iter(&ctx) }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Headers" }
}

fn strip_http_whitespace(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && matches!(bytes[start], b'\t' | b'\n' | b'\r' | b' ') {
        start += 1;
    }
    while end > start && matches!(bytes[end - 1], b'\t' | b'\n' | b'\r' | b' ') {
        end -= 1;
    }
    value[start..end].to_string()
}

pub(crate) fn is_forbidden_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "CONNECT" | "TRACE" | "TRACK"
    )
}

pub(crate) fn is_forbidden_request_header(name: &str, value: &str) -> bool {
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

fn is_cors_unsafe_byte(byte: u8) -> bool {
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
    let combined = match existing {
        Some(old) => format!("{old}, {value}"),
        None => value.to_string(),
    };
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
