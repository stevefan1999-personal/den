//! WHATWG `URL` and `URLSearchParams` on the rust-url crate (`url::quirks`).

use std::{cell::RefCell, rc::Rc};

use indexmap::IndexMap;
use rquickjs::{
    Array, Class, Ctx, Filter, FromJs as _, Function, IntoJs as _, JsIterator, JsLifetime, Object,
    Result, Symbol, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{Opt, This},
};
use url::{Url, form_urlencoded, quirks};

use crate::host::{Host, UsvString};

type Pair = (String, String);

const FALLBACK_HOST: &str = "den.invalid";

struct WebUrl {
    url:           Url,
    hostname:      Option<String>,
    pathname:      Option<String>,
    has_authority: bool,
}

impl WebUrl {
    fn from_url(url: Url) -> Self {
        let pathname = (!url.cannot_be_a_base() && url.path().contains('^'))
            .then(|| url.path().replace('^', "%5E"));
        let has_authority = url.has_authority();
        Self {
            url,
            hostname: None,
            pathname,
            has_authority,
        }
    }

    fn clean_input(input: &str) -> String {
        input
            .trim_matches(|character| character <= ' ')
            .chars()
            .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
            .collect()
    }

    fn scheme(input: &str) -> Option<&str> {
        let end = input.find(':')?;
        let scheme = input.get(..end)?;
        (scheme
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')))
        .then_some(scheme)
    }

    fn parse(input: &str, base: Option<&str>) -> std::result::Result<Self, ()> {
        let input = Self::clean_input(input);
        let base = base.map(|base| Self::parse(base, None)).transpose()?;
        let explicit_scheme = Self::scheme(&input).map(str::to_ascii_lowercase);
        if explicit_scheme.as_deref() == Some("file")
            || (explicit_scheme.is_none()
                && base
                    .as_ref()
                    .is_some_and(|base| base.url.scheme() == "file"))
        {
            return Self::parse_file(&input, base.as_ref());
        }

        let input = if explicit_scheme.is_none()
            && base.as_ref().is_some_and(|base| base.url.is_special())
            && input
                .chars()
                .take_while(|character| matches!(character, '/' | '\\'))
                .count()
                >= 2
        {
            format!("//{}", input.trim_start_matches(['/', '\\']))
        } else {
            input
        };

        if explicit_scheme.is_none()
            && base
                .as_ref()
                .is_some_and(|base| !base.url.is_special() && base.url.has_authority())
            && input.starts_with('\\')
        {
            let base = base.as_ref().ok_or(())?;
            let mut parsed = Self::from_url(base.url.clone());
            parsed.hostname.clone_from(&base.hostname);
            parsed.url.set_query(None);
            parsed.url.set_fragment(None);
            parsed.pathname = Some(Self::resolve_path(base.pathname(), &input, false));
            return Ok(parsed);
        }

        let mut parsed = Self::parse_rust(&input, base.as_ref())?;
        if explicit_scheme.is_none()
            && input == ".."
            && base.as_ref().is_some_and(|base| {
                !base.url.is_special()
                    && base
                        .pathname()
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .is_some_and(Self::is_drive)
            })
        {
            let base = base.as_ref().ok_or(())?;
            parsed.pathname = Some(Self::resolve_path(base.pathname(), &input, false));
        }
        Ok(parsed)
    }

    fn parse_rust(input: &str, base: Option<&Self>) -> std::result::Result<Self, ()> {
        let parsed = base.map_or_else(|| Url::parse(input), |base| base.url.join(input));
        if let Ok(url) = parsed {
            let mut parsed = Self::from_url(normalize_opaque_path(url));
            if parsed.url.host_str() == Some(FALLBACK_HOST) {
                parsed.hostname = base.and_then(|base| base.hostname.clone());
            }
            return Ok(parsed);
        }

        let absolute = if Self::scheme(input).is_some() {
            input.to_owned()
        } else if input.starts_with("//") {
            format!("{}:{input}", base.ok_or(())?.url.scheme())
        } else {
            return Err(());
        };
        let range = Self::hostname_range(&absolute).ok_or(())?;
        let hostname = Self::normalize_hostname(absolute.get(range.clone()).ok_or(())?).ok_or(())?;
        let mut rewritten = absolute;
        rewritten.replace_range(range, FALLBACK_HOST);
        let url = Url::parse(&rewritten).map_err(|_error| ())?;
        let mut parsed = Self::from_url(normalize_opaque_path(url));
        parsed.hostname = Some(hostname);
        Ok(parsed)
    }

    fn parse_file(input: &str, base: Option<&Self>) -> std::result::Result<Self, ()> {
        let mut parsed = Self::parse_rust(input, base).or_else(|()| {
            Url::parse("file:///")
                .map(normalize_opaque_path)
                .map(Self::from_url)
                .map_err(|_error| ())
        })?;
        let explicit = Self::scheme(input).is_some();
        let source = input
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .replace('\\', "/");
        let rest = if explicit {
            source.split_once(':').map_or("", |(_, rest)| rest)
        } else {
            source.as_str()
        };
        let file_base = base.filter(|base| base.url.scheme() == "file");
        let base_hostname = file_base.map_or("", Self::hostname);
        let base_pathname = file_base.map_or("/", Self::pathname);

        let (hostname, pathname) = if let Some(authority) = rest.strip_prefix("//") {
            if authority.is_empty() {
                (String::new(), "/".to_owned())
            } else if authority.starts_with('/') {
                (String::new(), Self::normalize_path(authority, true))
            } else {
                let (hostname, pathname) =
                    authority
                        .split_once('/')
                        .map_or((authority, ""), |(hostname, _pathname)| {
                            (hostname, authority.get(hostname.len()..).unwrap_or(""))
                        });
                if Self::is_drive(hostname) {
                    (
                        String::new(),
                        Self::normalize_path(&format!("/{hostname}{pathname}"), true),
                    )
                } else {
                    let hostname = Self::normalize_hostname(hostname).ok_or(())?;
                    let hostname = if hostname.eq_ignore_ascii_case("localhost") {
                        String::new()
                    } else {
                        hostname
                    };
                    (
                        hostname,
                        if pathname.is_empty() {
                            "/".to_owned()
                        } else {
                            Self::normalize_path(pathname, true)
                        },
                    )
                }
            }
        } else if rest.is_empty() {
            (
                base_hostname.to_owned(),
                if file_base.is_some() {
                    base_pathname.to_owned()
                } else {
                    "/".to_owned()
                },
            )
        } else if Self::starts_with_drive(rest.trim_start_matches('/')) {
            (
                base_hostname.to_owned(),
                Self::normalize_path(&format!("/{}", rest.trim_start_matches('/')), true),
            )
        } else if rest.starts_with('/') {
            let pathname = if rest == "/" {
                Self::drive_root(base_pathname).unwrap_or(rest).to_owned()
            } else {
                Self::normalize_path(rest, true)
            };
            (base_hostname.to_owned(), pathname)
        } else if file_base.is_some() {
            (
                base_hostname.to_owned(),
                Self::resolve_path(base_pathname, rest, true),
            )
        } else {
            (
                String::new(),
                Self::normalize_path(&format!("/{rest}"), true),
            )
        };

        parsed.hostname = Some(hostname);
        parsed.pathname = Some(pathname);
        parsed.has_authority = true;
        Ok(parsed)
    }

    fn hostname_range(input: &str) -> Option<std::ops::Range<usize>> {
        let scheme_end = input.find(':')?;
        let authority_start = scheme_end + 1;
        let slashes = input.get(authority_start..authority_start + 2)?;
        if slashes != "//" {
            return None;
        }
        let authority_start = authority_start + 2;
        let authority_end = input
            .get(authority_start..)?
            .find(['/', '?', '#'])
            .map_or(input.len(), |end| authority_start + end);
        let host_start = input
            .get(authority_start..authority_end)?
            .rfind('@')
            .map_or(authority_start, |at| authority_start + at + 1);
        let host_port = input.get(host_start..authority_end)?;
        if host_port.starts_with('[') {
            return host_port
                .find(']')
                .map(|end| host_start..host_start + end + 1);
        }
        let host_end = host_port
            .rfind(':')
            .filter(|colon| {
                host_port
                    .get(colon + 1..)
                    .unwrap_or("")
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            })
            .map_or(authority_end, |colon| host_start + colon);
        (host_start < host_end).then_some(host_start..host_end)
    }

    fn normalize_hostname(input: &str) -> Option<String> {
        if let Ok(host) = url::Host::parse(input) {
            return Some(host.to_string());
        }
        if input.is_empty()
            || input.chars().any(|character| {
                character <= ' '
                    || matches!(
                        character,
                        '#' | '%'
                            | '/'
                            | ':'
                            | '<'
                            | '>'
                            | '?'
                            | '@'
                            | '['
                            | '\\'
                            | ']'
                            | '^'
                            | '|'
                    )
                    || character == '\u{7f}'
            })
        {
            return None;
        }
        if input.is_ascii() {
            let last = input
                .trim_end_matches('.')
                .rsplit('.')
                .next()
                .unwrap_or(input);
            if last.bytes().all(|byte| byte.is_ascii_digit())
                || last
                    .get(..2)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("0x"))
            {
                return None;
            }
            return Some(input.to_ascii_lowercase());
        }

        None
    }

    const fn is_drive(segment: &str) -> bool {
        match segment.as_bytes() {
            [drive, separator] => drive.is_ascii_alphabetic() && matches!(separator, b':' | b'|'),
            _ => false,
        }
    }

    fn starts_with_drive(path: &str) -> bool { path.split('/').next().is_some_and(Self::is_drive) }

    fn drive_root(pathname: &str) -> Option<&str> {
        let drive = pathname.strip_prefix('/')?.split('/').next()?;
        Self::is_drive(drive)
            .then(|| pathname.get(..drive.len() + 2))
            .flatten()
    }

    fn resolve_path(base: &str, input: &str, special: bool) -> String {
        if input.starts_with('/') || (special && input.starts_with('\\')) {
            return Self::normalize_path(input, special);
        }
        let directory = base
            .rfind('/')
            .and_then(|slash| base.get(..=slash))
            .unwrap_or("/");
        Self::normalize_path(&format!("{directory}{input}"), special)
    }

    fn normalize_path(input: &str, special: bool) -> String {
        let mut input: String = input
            .chars()
            .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
            .map(|character| {
                if special && character == '\\' {
                    '/'
                } else {
                    character
                }
            })
            .collect();
        if !input.starts_with('/') {
            input.insert(0, '/');
        }
        let raw_segments: Vec<_> = input
            .strip_prefix('/')
            .unwrap_or(&input)
            .split('/')
            .collect();
        let mut segments = Vec::new();
        for (index, segment) in raw_segments.iter().enumerate() {
            let last = index + 1 == raw_segments.len();
            if segment.eq_ignore_ascii_case(".") || segment.eq_ignore_ascii_case("%2e") {
                if last {
                    segments.push(String::new());
                }
                continue;
            }
            let double_dot = *segment == ".."
                || segment.eq_ignore_ascii_case(".%2e")
                || segment.eq_ignore_ascii_case("%2e.")
                || segment.eq_ignore_ascii_case("%2e%2e");
            if double_dot {
                if !segments
                    .last()
                    .is_some_and(|segment| special && Self::is_drive(segment))
                {
                    segments.pop();
                }
                if last {
                    segments.push(String::new());
                }
                continue;
            }
            let segment = if special && segments.is_empty() && Self::is_drive(segment) {
                format!("{}:", segment.chars().next().unwrap_or_default())
            } else {
                Self::encode_path_segment(segment)
            };
            segments.push(segment);
        }
        format!("/{}", segments.join("/"))
    }

    fn encode_path_segment(segment: &str) -> String {
        let mut output = String::new();
        for byte in segment.bytes() {
            if byte <= 0x20
                || byte >= 0x7f
                || matches!(
                    byte,
                    b'"' | b'#' | b'<' | b'>' | b'?' | b'^' | b'`' | b'{' | b'}'
                )
            {
                output.push('%');
                output.push(Self::hex_digit(byte >> 4));
                output.push(Self::hex_digit(byte & 0x0f));
            } else {
                output.push(char::from(byte));
            }
        }
        output
    }

    const fn hex_digit(nibble: u8) -> char {
        match nibble {
            0 => '0',
            1 => '1',
            2 => '2',
            3 => '3',
            4 => '4',
            5 => '5',
            6 => '6',
            7 => '7',
            8 => '8',
            9 => '9',
            10 => 'A',
            11 => 'B',
            12 => 'C',
            13 => 'D',
            14 => 'E',
            _ => 'F',
        }
    }

    fn hostname(&self) -> &str {
        self.hostname
            .as_deref()
            .unwrap_or_else(|| self.url.host_str().unwrap_or(""))
    }

    fn host(&self) -> String {
        self.url.port().map_or_else(
            || self.hostname().to_owned(),
            |port| format!("{}:{port}", self.hostname()),
        )
    }

    fn pathname(&self) -> &str { self.pathname.as_deref().unwrap_or_else(|| self.url.path()) }

    fn href(&self) -> String {
        let mut output = format!("{}:", self.url.scheme());
        if self.has_authority {
            output.push_str("//");
            if !self.url.username().is_empty() || self.url.password().is_some() {
                output.push_str(self.url.username());
                if let Some(password) = self.url.password() {
                    output.push(':');
                    output.push_str(password);
                }
                output.push('@');
            }
            output.push_str(self.hostname());
            if let Some(port) = self.url.port() {
                output.push(':');
                output.push_str(&port.to_string());
            }
        } else if !self.url.cannot_be_a_base() && self.pathname().starts_with("//") {
            output.push_str("/.");
        }
        output.push_str(self.pathname());
        if let Some(query) = self.url.query() {
            output.push('?');
            output.push_str(query);
        }
        if let Some(fragment) = self.url.fragment() {
            output.push('#');
            output.push_str(fragment);
        }
        output
    }

    fn origin(&self) -> String {
        match self.url.scheme() {
            "http" | "https" | "ftp" | "ws" | "wss" => {
                format!("{}://{}", self.url.scheme(), self.host())
            }
            "blob" => {
                Url::parse(self.pathname()).map_or_else(
                    |_| "null".to_owned(),
                    |url| {
                        match url.scheme() {
                            "http" | "https" => quirks::origin(&url),
                            _ => "null".to_owned(),
                        }
                    },
                )
            }
            _ => "null".to_owned(),
        }
    }

    fn set_host(&mut self, input: &str, hostname_only: bool) {
        if self.url.cannot_be_a_base()
            || (input.is_empty()
                && (!self.url.username().is_empty()
                    || self.url.password().is_some()
                    || self.url.port().is_some()))
        {
            return;
        }
        let mut url = self.url.clone();
        let result = if hostname_only {
            quirks::set_hostname(&mut url, input)
        } else {
            quirks::set_host(&mut url, input)
        };
        if result.is_ok() && (!url.is_special() || input.is_empty() || url.host_str().is_some()) {
            let file_localhost = url.scheme() == "file"
                && (url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
                    || Self::normalize_hostname(input)
                        .is_some_and(|host| host.eq_ignore_ascii_case("localhost")));
            self.url = url;
            self.hostname = file_localhost.then(String::new);
            self.has_authority = self.url.has_authority();
            return;
        }
        let opaque_input: String = input
            .chars()
            .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
            .collect();
        if result.is_err()
            && !self.url.is_special()
            && opaque_input.starts_with(['#', '/', '?'])
            && self.url.username().is_empty()
            && self.url.password().is_none()
            && self.url.port().is_none()
        {
            let _ = self.url.set_host(Some(""));
            self.hostname = None;
            self.has_authority = true;
            return;
        }

        let (hostname, suffix) = if hostname_only {
            (input, "")
        } else {
            input.rfind(':').map_or((input, ""), |colon| {
                if input
                    .get(colon + 1..)
                    .is_some_and(|tail| tail.bytes().all(|byte| byte.is_ascii_digit()))
                {
                    (
                        input.get(..colon).unwrap_or(input),
                        input.get(colon..).unwrap_or(""),
                    )
                } else {
                    (input, "")
                }
            })
        };
        let Some(hostname) = Self::normalize_hostname(hostname) else {
            return;
        };
        if self.url.scheme() == "file" && hostname.eq_ignore_ascii_case("localhost") {
            self.hostname = Some(String::new());
            return;
        }
        let placeholder = format!("{FALLBACK_HOST}{suffix}");
        let result = if hostname_only {
            quirks::set_hostname(&mut self.url, &placeholder)
        } else {
            quirks::set_host(&mut self.url, &placeholder)
        };
        if result.is_ok() {
            self.hostname = Some(hostname);
            self.has_authority = self.url.has_authority();
        }
    }

    fn set_pathname(&mut self, input: &str) {
        if self.url.cannot_be_a_base() {
            return;
        }
        quirks::set_pathname(&mut self.url, input);
        self.pathname = Some(
            if !self.url.is_special() && self.has_authority && input.is_empty() {
                String::new()
            } else {
                Self::normalize_path(input, self.url.is_special())
            },
        );
    }
}

fn pairs_of(url: &WebUrl) -> Vec<Pair> {
    form_urlencoded::parse(url.url.query().unwrap_or("").as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn serialize_pairs(pairs: &[Pair]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

fn write_query(url: &mut WebUrl, pairs: &[Pair]) {
    if pairs.is_empty() {
        url.url.set_query(None);
    } else {
        url.url.set_query(Some(&serialize_pairs(pairs)));
    }
}

fn parse_urlencoded(input: &str) -> Vec<Pair> {
    let input = input.strip_prefix('?').unwrap_or(input);
    form_urlencoded::parse(input.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn normalize_opaque_path(mut url: Url) -> Url {
    if url.cannot_be_a_base()
        && (url.query().is_some() || url.fragment().is_some())
        && let Some(path) = url.path().strip_suffix(' ')
    {
        // ponytail: rust-url 2.5.8 predates the current opaque-path trailing-space
        // rule.
        url.set_path(&format!("{path}%20"));
    }
    url
}

fn coerce_url_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    if let Some(object) = value.as_object()
        && ctx
            .globals()
            .get::<_, Value>("location")
            .is_ok_and(|location| object.as_ref() == &location)
    {
        return Host::coerce_usv_string(ctx, object.get("href")?);
    }
    Host::coerce_usv_string(ctx, value)
}

fn optional_usv<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<Option<String>> {
    let Some(value) = value.0 else {
        return Ok(None);
    };
    if value.is_undefined() {
        return Ok(None);
    }
    Host::coerce_usv_string(ctx, value.clone()).map(Some)
}

fn js_iterator<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<JsIterator<'js>>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let method: Value = object.get(PredefinedAtom::SymbolIterator)?;
    if method.is_null() || method.is_undefined() {
        return Ok(None);
    }
    let iterator: Value = Function::from_js(ctx, method)?.call((This(object.clone()),))?;
    JsIterator::from_js(ctx, iterator).map(Some)
}

fn pair_from_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Pair> {
    let Some(iterator) = js_iterator(ctx, &value)? else {
        return Err(Host::throw_type(
            ctx,
            "Expected name/value pair to be iterable",
        ));
    };
    let items = iterator.collect::<Result<Vec<_>>>()?;
    if items.len() != 2 {
        return Err(Host::throw_type(
            ctx,
            &format!(
                "Expected name/value pair to be length 2, found {}",
                items.len()
            ),
        ));
    }
    let [name, value] = items.as_slice() else {
        return Err(Host::throw_type(
            ctx,
            "Expected name/value pair to be length 2",
        ));
    };
    Ok((
        Host::coerce_usv_string(ctx, name.clone())?,
        Host::coerce_usv_string(ctx, value.clone())?,
    ))
}

fn is_dom_exception_prototype<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> bool {
    ctx.globals()
        .get::<_, Object>("DOMException")
        .ok()
        .and_then(|constructor| constructor.get::<_, Value>("prototype").ok())
        .is_some_and(|prototype| object.as_ref() == &prototype)
}

fn pairs_from_init<'js>(ctx: &Ctx<'js>, init: Option<Value<'js>>) -> Result<Vec<Pair>> {
    let Some(value) = init else {
        return Ok(Vec::new());
    };
    if value.is_undefined() {
        return Ok(Vec::new());
    }
    if value.as_string().is_some() || !value.is_object() {
        return Ok(parse_urlencoded(&Host::coerce_usv_string(ctx, value)?));
    }
    if let Some(iterator) = js_iterator(ctx, &value)? {
        return iterator.map(|item| pair_from_value(ctx, item?)).collect();
    }
    let Some(object) = value.as_object() else {
        return Ok(parse_urlencoded(&Host::coerce_usv_string(ctx, value)?));
    };
    if is_dom_exception_prototype(ctx, object) {
        // QuickJS marks DOMException's branded accessors non-enumerable; browsers do
        // not.
        return Err(Host::throw_type(ctx, "Illegal invocation"));
    }
    let mut record = IndexMap::new();
    for key in object.own_keys::<Value>(Filter::default()) {
        let key = key?;
        let name = Host::coerce_usv_string(ctx, key.clone())?;
        let value = Host::coerce_usv_string(ctx, object.get(key)?)?;
        record.insert(name, value);
    }
    Ok(record.into_iter().collect())
}

fn parse_ctor_args<'js>(
    ctx: &Ctx<'js>, input: Value<'js>, base: Option<Value<'js>>,
) -> Result<std::result::Result<WebUrl, ()>> {
    let input = coerce_url_string(ctx, input)?;
    let base = base
        .as_ref()
        .filter(|value| !value.is_undefined())
        .map_or(Ok::<_, rquickjs::Error>(None), |value| {
            coerce_url_string(ctx, value.clone()).map(Some)
        })?;
    Ok(WebUrl::parse(&input, base.as_deref()))
}

fn search_slot<'js>(ctx: &Ctx<'js>) -> Result<Symbol<'js>> {
    Symbol::new_global(ctx.clone(), "den.url.searchParams")
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct URL {
    #[qjs(skip_trace)]
    inner: Rc<RefCell<WebUrl>>,
    #[qjs(skip_trace)]
    query: Rc<RefCell<Vec<Pair>>>,
}

impl URL {
    fn from_url(url: WebUrl) -> Self {
        let query = Rc::new(RefCell::new(pairs_of(&url)));
        Self {
            inner: Rc::new(RefCell::new(url)),
            query,
        }
    }

    fn reload_query(&self) { *self.query.borrow_mut() = pairs_of(&self.inner.borrow()); }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl URL {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, input: Value<'js>, base: Opt<Value<'js>>) -> Result<Self> {
        match parse_ctor_args(&ctx, input, base.0)? {
            Ok(url) => Ok(Self::from_url(url)),
            Err(()) => {
                Err(Host::throw_type(
                    &ctx,
                    "Failed to construct 'URL': Invalid URL",
                ))
            }
        }
    }

    #[qjs(static)]
    pub fn parse<'js>(
        ctx: Ctx<'js>, input: Value<'js>, base: Opt<Value<'js>>,
    ) -> Result<Value<'js>> {
        match parse_ctor_args(&ctx, input, base.0)? {
            Ok(url) => Class::instance(ctx, Self::from_url(url)).map(rquickjs::Class::into_value),
            Err(()) => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(static, rename = "canParse")]
    pub fn can_parse<'js>(ctx: Ctx<'js>, input: Value<'js>, base: Opt<Value<'js>>) -> Result<bool> {
        Ok(parse_ctor_args(&ctx, input, base.0)?.is_ok())
    }

    #[qjs(get)]
    pub fn href(&self) -> String { self.inner.borrow().href() }

    #[qjs(set, rename = "href")]
    pub fn set_href<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        let parsed = WebUrl::parse(&text, None).map_err(|()| {
            Host::throw_type(
                &ctx,
                "Failed to set the 'href' property on 'URL': Invalid URL",
            )
        })?;
        *self.inner.borrow_mut() = parsed;
        self.reload_query();
        Ok(())
    }

    #[qjs(get)]
    pub fn origin(&self) -> String { self.inner.borrow().origin() }

    #[qjs(get)]
    pub fn protocol(&self) -> String { quirks::protocol(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "protocol")]
    pub fn set_protocol<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        let _ = quirks::set_protocol(&mut self.inner.borrow_mut().url, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn username(&self) -> String { quirks::username(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "username")]
    pub fn set_username<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        let _ = quirks::set_username(&mut self.inner.borrow_mut().url, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn password(&self) -> String { quirks::password(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "password")]
    pub fn set_password<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        let _ = quirks::set_password(&mut self.inner.borrow_mut().url, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn host(&self) -> String { self.inner.borrow().host() }

    #[qjs(set, rename = "host")]
    pub fn set_host<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        self.inner.borrow_mut().set_host(&text, false);
        Ok(())
    }

    #[qjs(get)]
    pub fn hostname(&self) -> String { self.inner.borrow().hostname().to_owned() }

    #[qjs(set, rename = "hostname")]
    pub fn set_hostname<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        self.inner.borrow_mut().set_host(&text, true);
        Ok(())
    }

    #[qjs(get)]
    pub fn port(&self) -> String { quirks::port(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "port")]
    pub fn set_port<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        if !text.is_empty() && text.chars().all(char::is_whitespace) {
            return Ok(());
        }
        let _ = quirks::set_port(&mut self.inner.borrow_mut().url, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn pathname(&self) -> String { self.inner.borrow().pathname().to_owned() }

    #[qjs(set, rename = "pathname")]
    pub fn set_pathname<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        self.inner.borrow_mut().set_pathname(&text);
        Ok(())
    }

    #[qjs(get)]
    pub fn search(&self) -> String { quirks::search(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "search")]
    pub fn set_search<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        quirks::set_search(&mut self.inner.borrow_mut().url, &text);
        self.reload_query();
        Ok(())
    }

    #[qjs(get)]
    pub fn hash(&self) -> String { quirks::hash(&self.inner.borrow().url).to_string() }

    #[qjs(set, rename = "hash")]
    pub fn set_hash<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = Host::coerce_usv_string(&ctx, value)?;
        quirks::set_hash(&mut self.inner.borrow_mut().url, &text);
        Ok(())
    }

    #[qjs(get, rename = "searchParams")]
    pub fn search_params<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>,
    ) -> Result<Class<'js, URLSearchParams>> {
        let slot = search_slot(&ctx)?;
        let existing: Value = this.0.get(slot.clone())?;
        if let Ok(params) = Class::<URLSearchParams>::from_value(&existing) {
            return Ok(params);
        }
        let url = this.0.borrow();
        let params = Class::instance(ctx, URLSearchParams {
            query: Rc::clone(&url.query),
            owner: Some(Rc::clone(&url.inner)),
        })?;
        this.0.set(slot, params.clone())?;
        Ok(params)
    }

    #[qjs(rename = "toString")]
    pub fn to_string_js(&self) -> String { self.href() }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.href() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "URL" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct URLSearchParams {
    #[qjs(skip_trace)]
    query: Rc<RefCell<Vec<Pair>>>,
    #[qjs(skip_trace)]
    owner: Option<Rc<RefCell<WebUrl>>>,
}

impl URLSearchParams {
    fn sync(&self) {
        if let Some(owner) = &self.owner {
            write_query(&mut owner.borrow_mut(), &self.query.borrow());
        }
    }

    fn iterator<'js>(&self, ctx: Ctx<'js>, kind: u8) -> Result<Class<'js, UrlSearchIterator>> {
        Class::instance(ctx, UrlSearchIterator {
            query: Rc::clone(&self.query),
            index: 0,
            kind,
        })
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl URLSearchParams {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, init: Opt<Value<'js>>) -> Result<Self> {
        Ok(Self {
            query: Rc::new(RefCell::new(pairs_from_init(&ctx, init.0)?)),
            owner: None,
        })
    }

    pub fn append(&self, name: UsvString, value: UsvString) {
        self.query.borrow_mut().push((name.0, value.0));
        self.sync();
    }

    pub fn delete<'js>(
        &self, ctx: Ctx<'js>, name: UsvString, value: Opt<Value<'js>>,
    ) -> Result<()> {
        let name = name.0;
        let value = optional_usv(&ctx, value)?;
        self.query
            .borrow_mut()
            .retain(|(existing, existing_value)| {
                if existing != &name {
                    return true;
                }
                value
                    .as_ref()
                    .is_some_and(|wanted| existing_value != wanted)
            });
        self.sync();
        Ok(())
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, name: UsvString) -> Result<Value<'js>> {
        for (existing, value) in self.query.borrow().iter() {
            if existing == &name.0 {
                return value.clone().into_js(&ctx);
            }
        }
        Ok(Value::new_null(ctx))
    }

    pub fn get_all(&self, name: UsvString) -> Vec<String> {
        self.query
            .borrow()
            .iter()
            .filter(|(existing, _)| existing == &name.0)
            .map(|(_, value)| value.clone())
            .collect()
    }

    pub fn has<'js>(&self, ctx: Ctx<'js>, name: UsvString, value: Opt<Value<'js>>) -> Result<bool> {
        let name = name.0;
        let value = optional_usv(&ctx, value)?;
        Ok(self
            .query
            .borrow()
            .iter()
            .any(|(existing, existing_value)| {
                existing == &name && value.as_ref().is_none_or(|wanted| existing_value == wanted)
            }))
    }

    pub fn set(&self, name: UsvString, value: UsvString) {
        let name = name.0;
        let value = value.0;
        let mut pairs = self.query.borrow_mut();
        let mut replaced = false;
        let mut result = Vec::new();
        for pair in pairs.drain(..) {
            if pair.0 == name {
                if !replaced {
                    result.push((name.clone(), value.clone()));
                    replaced = true;
                }
            } else {
                result.push(pair);
            }
        }
        if !replaced {
            result.push((name, value));
        }
        *pairs = result;
        drop(pairs);
        self.sync();
    }

    pub fn sort(&self) {
        self.query
            .borrow_mut()
            .sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
        self.sync();
    }

    #[qjs(get)]
    pub fn size(&self) -> usize { self.query.borrow().len() }

    pub fn keys<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, UrlSearchIterator>> {
        self.iterator(ctx, 0)
    }

    pub fn values<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, UrlSearchIterator>> {
        self.iterator(ctx, 1)
    }

    pub fn entries<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, UrlSearchIterator>> {
        self.iterator(ctx, 2)
    }

    pub fn for_each<'js>(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, callback: Function<'js>,
        this_arg: Opt<Value<'js>>,
    ) -> Result<()> {
        let this_arg = this_arg
            .0
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let snapshot = this.0.borrow().query.borrow().clone();
        for (name, value) in snapshot {
            callback.call::<_, ()>((This(this_arg.clone()), value, name, this.0.clone()))?;
        }
        Ok(())
    }

    #[qjs(rename = "toString")]
    pub fn to_string_js(&self) -> String { serialize_pairs(&self.query.borrow()) }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn js_iterator<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, UrlSearchIterator>> {
        self.iterator(ctx, 2)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "URLSearchParams" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct UrlSearchIterator {
    #[qjs(skip_trace)]
    query: Rc<RefCell<Vec<Pair>>>,
    index: usize,
    kind:  u8,
}

#[rquickjs::methods]
impl UrlSearchIterator {
    pub fn next<'js>(&mut self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let result = Object::new(ctx.clone())?;
        let pair = self.query.borrow().get(self.index).cloned();
        let Some((name, value)) = pair else {
            result.set("done", true)?;
            result.set("value", Value::new_undefined(ctx))?;
            return Ok(result);
        };
        self.index += 1;
        let value = match self.kind {
            0 => name.into_js(&ctx)?,
            1 => value.into_js(&ctx)?,
            _ => {
                let array = Array::new(ctx.clone())?;
                array.set(0, name)?;
                array.set(1, value)?;
                array.into_value()
            }
        };
        result.set("done", false)?;
        result.set("value", value)?;
        Ok(result)
    }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn iter(this: This<Class<'_, Self>>) -> Class<'_, Self> { this.0 }
}
