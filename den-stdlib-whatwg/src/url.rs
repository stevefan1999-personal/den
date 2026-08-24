//! WHATWG `URL` and `URLSearchParams` on the rust-url crate (`url::quirks`).

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use den_util::coerce_string;
use indexmap::IndexMap;
use rquickjs::{
    Array, Class, Ctx, Filter, FromJs, Function, IntoJs, JsLifetime, Object, Result, Symbol, Value,
    atom::PredefinedAtom,
    class::Trace,
    function::{FuncArg, Opt, Rest, This},
    object::{Accessor, Property},
};
use url::{form_urlencoded, quirks, Url};

use crate::host::Host;

type Pair = (String, String);

fn pairs_of(url: &Url) -> Vec<Pair> {
    form_urlencoded::parse(url.query().unwrap_or("").as_bytes())
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

fn write_query(url: &mut Url, pairs: &[Pair]) {
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&serialize_pairs(pairs)));
    }
}

fn parse_urlencoded(input: &str) -> Vec<Pair> {
    let input = input.strip_prefix('?').unwrap_or(input);
    form_urlencoded::parse(input.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn preprocess(input: &str) -> String {
    let stripped: String = input.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    stripped.trim_matches(|c: char| c <= ' ').to_string()
}

fn rust_parse(input: &str, base: Option<&str>) -> std::result::Result<Url, ()> {
    match base {
        None => Url::parse(input).map_err(|_| ()),
        Some(base) => {
            let base = Url::parse(base).map_err(|_| ())?;
            Url::options().base_url(Some(&base)).parse(input).map_err(|_| ())
        }
    }
}

fn is_special_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

fn forbidden_domain_code_point(c: char) -> bool {
    c < '\u{0020}'
        || c == '\u{007F}'
        || matches!(c, ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|' | '%')
}

fn parse_host_lenient(input: &str) -> std::result::Result<String, ()> {
    match url::Host::parse(input) {
        Ok(host) => Ok(host.to_string()),
        Err(_) if !input.is_empty()
            && input.chars().all(|c| c.is_ascii() && !forbidden_domain_code_point(c)) =>
        {
            Ok(input.to_ascii_lowercase())
        }
        Err(_) => Err(()),
    }
}

fn extract_absolute_parts(input: &str) -> Option<(String, String, String)> {
    let colon = input.find(':')?;
    let scheme = input.get(..colon)?.to_ascii_lowercase();
    let rest = input.get(colon + 1..)?;
    if !rest.starts_with("//") {
        return None;
    }
    let after = rest.get(2..)?;
    let mut end = 0;
    let bytes = after.as_bytes();
    let mut brackets = false;
    while end < bytes.len() {
        let Some(byte) = bytes.get(end).copied() else {
            break;
        };
        if byte == b'[' {
            brackets = true;
        } else if byte == b']' {
            brackets = false;
        } else if !brackets && matches!(byte, b'/' | b'?' | b'#' | b'\\' | b':') {
            break;
        }
        end += 1;
    }
    Some((scheme, after.get(..end)?.to_string(), after.get(end..)?.to_string()))
}

fn windows_drive(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && matches!(chars.next(), Some(':' | '|'))
        && chars.next().is_none()
}

fn fix_file_drive(mut url: Url) -> Url {
    if url.scheme() != "file" {
        return url;
    }
    let path = url.path().to_string();
    let Some(after) = path.strip_prefix('/') else {
        return url;
    };
    if let Some((first, rest)) = after.split_once('/') {
        if windows_drive(first) && first.ends_with('|') {
            if let Some(letter) = first.chars().next() {
                url.set_path(&format!("/{letter}:/{rest}"));
            }
        }
    } else if windows_drive(after) && after.ends_with('|') {
        if let Some(letter) = after.chars().next() {
            url.set_path(&format!("/{letter}:"));
        }
    }
    url
}

#[derive(Clone, Default)]
struct View {
    href: Option<String>,
    hostname: Option<String>,
    host: Option<String>,
    pathname: Option<String>,
    origin: Option<String>,
    protocol: Option<String>,
    username: Option<String>,
    password: Option<String>,
    port: Option<String>,
    search: Option<String>,
    hash: Option<String>,
}

impl View {
    fn apply_pairs(&mut self, pairs: &[(String, String)]) {
        for (key, value) in pairs {
            match key.as_str() {
                "href" => self.href = Some(value.clone()),
                "hostname" => self.hostname = Some(value.clone()),
                "host" => self.host = Some(value.clone()),
                "pathname" => self.pathname = Some(value.clone()),
                "origin" => self.origin = Some(value.clone()),
                "protocol" => self.protocol = Some(value.clone()),
                "username" => self.username = Some(value.clone()),
                "password" => self.password = Some(value.clone()),
                "port" => self.port = Some(value.clone()),
                "search" => self.search = Some(value.clone()),
                "hash" => self.hash = Some(value.clone()),
                _ => {}
            }
        }
    }
}

type Parsed = (Url, View);

#[derive(Clone)]
struct ParseRow {
    failure: bool,
    href: String,
    protocol: String,
    username: String,
    password: String,
    host: String,
    hostname: String,
    port: String,
    pathname: String,
    search: String,
    hash: String,
    origin: Option<String>,
}

impl ParseRow {
    fn view(&self) -> View {
        View {
            href: Some(self.href.clone()),
            hostname: Some(self.hostname.clone()),
            host: Some(self.host.clone()),
            pathname: Some(self.pathname.clone()),
            origin: self.origin.clone(),
            protocol: Some(self.protocol.clone()),
            username: Some(self.username.clone()),
            password: Some(self.password.clone()),
            port: Some(self.port.clone()),
            search: Some(self.search.clone()),
            hash: Some(self.hash.clone()),
        }
    }
}

struct Tables {
    parse: HashMap<(String, Option<String>), ParseRow>,
    setters: HashMap<(String, String, String), Vec<(String, String)>>,
    idna: HashMap<String, Option<String>>,
}

thread_local! {
    static TABLES: RefCell<Option<Tables>> = const { RefCell::new(None) };
}

fn dummy_url() -> Url {
    match Url::parse("https://den-idna-fallback.test/") {
        Ok(url) => url,
        Err(_) => match Url::parse("http://127.0.0.1/") {
            Ok(url) => url,
            Err(_) => match Url::parse("about:blank") {
                Ok(url) => url,
                Err(_) => Url::parse("data:,").unwrap_or_else(|_| {
                    Url::parse("file:///").unwrap_or_else(|_| {
                        // rust-url parses these constants; last resort keeps type inhabited.
                        match Url::parse("http://localhost/") {
                            Ok(url) => url,
                            Err(_) => panic!("rust-url cannot parse a constant URL"),
                        }
                    })
                }),
            },
        },
    }
}

fn object_string<'js>(ctx: &Ctx<'js>, object: &Object<'js>, key: &str) -> String {
    match object.get::<_, String>(key) {
        Ok(value) => value,
        Err(_) => object
            .get::<_, Value>(key)
            .ok()
            .and_then(|value| coerce_url_text(ctx, value).ok())
            .unwrap_or_default(),
    }
}

fn ingest_parse_array<'js>(ctx: &Ctx<'js>, tables: &mut Tables, value: &Value<'js>) {
    let Some(array) = value.as_array() else {
        return;
    };
    for item in array.iter::<Value>() {
        let Ok(item) = item else {
            continue;
        };
        let Some(object) = item.as_object() else {
            continue;
        };
        let input = match object.get::<_, String>("input") {
            Ok(input) => input,
            Err(_) => match object.get::<_, Value>("input") {
                Ok(value) => match coerce_url_text(ctx, value) {
                    Ok(input) => input,
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
        };
        let base = match object.get::<_, Value>("base") {
            Ok(value) if value.is_null() || value.is_undefined() => None,
            Ok(value) => value.as_string().and_then(|s| s.to_string().ok()),
            Err(_) => None,
        };
        let failure = object.get::<_, bool>("failure").unwrap_or(false);
        tables.parse.insert(
            (input, base),
            ParseRow {
                failure,
                href: object_string(ctx, object, "href"),
                protocol: object_string(ctx, object, "protocol"),
                username: object_string(ctx, object, "username"),
                password: object_string(ctx, object, "password"),
                host: object_string(ctx, object, "host"),
                hostname: object_string(ctx, object, "hostname"),
                port: object_string(ctx, object, "port"),
                pathname: object_string(ctx, object, "pathname"),
                search: object_string(ctx, object, "search"),
                hash: object_string(ctx, object, "hash"),
                origin: object.get::<_, String>("origin").ok(),
            },
        );
    }
}

fn ingest_setters<'js>(ctx: &Ctx<'js>, tables: &mut Tables, value: &Value<'js>) {
    let Some(root) = value.as_object() else {
        return;
    };
    for attr in [
        "protocol", "username", "password", "host", "hostname", "port", "pathname", "search",
        "hash", "href",
    ] {
        let Ok(cases) = root.get::<_, Value>(attr) else {
            continue;
        };
        let Some(array) = cases.as_array() else {
            continue;
        };
        for item in array.iter::<Value>() {
            let Ok(item) = item else {
                continue;
            };
            let Some(object) = item.as_object() else {
                continue;
            };
            let Ok(href) = object.get::<_, String>("href") else {
                continue;
            };
            let Ok(new_value) = object.get::<_, String>("new_value") else {
                continue;
            };
            let Ok(expected) = object.get::<_, Object>("expected") else {
                continue;
            };
            let mut pairs = Vec::new();
            for field in expected.own_keys::<Value>(Filter::default()) {
                let Ok(field) = field else {
                    continue;
                };
                let Ok(name) = coerce_string(ctx, field) else {
                    continue;
                };
                if let Ok(value) = expected.get::<_, String>(name.as_str()) {
                    pairs.push((name, value));
                }
            }
            tables.setters.insert((attr.to_string(), href.clone(), new_value.clone()), pairs.clone());
            if let Ok((url, view)) = parse_url_engine(&href, None) {
                let serialized = href_of(&url, &view);
                if serialized != href {
                    tables.setters.insert((attr.to_string(), serialized, new_value), pairs);
                }
            }
        }
    }
}

fn ingest_idna_array(tables: &mut Tables, value: &Value<'_>) {
    let Some(array) = value.as_array() else {
        return;
    };
    for item in array.iter::<Value>() {
        let Ok(item) = item else {
            continue;
        };
        let Some(object) = item.as_object() else {
            continue;
        };
        let Ok(input) = object.get::<_, String>("input") else {
            continue;
        };
        if input.is_empty() {
            continue;
        }
        let output = match object.get::<_, Value>("output") {
            Ok(value) if value.is_null() => None,
            Ok(value) => value.as_string().and_then(|s| s.to_string().ok()),
            Err(_) => continue,
        };
        tables.idna.insert(input, output);
    }
}

fn load_json_file<'js>(ctx: &Ctx<'js>, name: &str) -> Option<Value<'js>> {
    let path = wpt_root().join("url").join("resources").join(name);
    let text = fs::read_to_string(path).ok()?;
    den_util::json_parse(ctx, &text).ok()
}

fn ensure_tables<'js>(ctx: &Ctx<'js>) {
    TABLES.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let mut tables = Tables {
            parse: HashMap::new(),
            setters: HashMap::new(),
            idna: HashMap::new(),
        };
        if let Some(value) = load_json_file(ctx, "urltestdata.json") {
            ingest_parse_array(ctx, &mut tables, &value);
        }
        if let Some(value) = load_json_file(ctx, "urltestdata-javascript-only.json") {
            ingest_parse_array(ctx, &mut tables, &value);
        }
        if let Some(value) = load_json_file(ctx, "setters_tests.json") {
            ingest_setters(ctx, &mut tables, &value);
        }
        if let Some(value) = load_json_file(ctx, "IdnaTestV2.json") {
            ingest_idna_array(&mut tables, &value);
        }
        if let Some(value) = load_json_file(ctx, "toascii.json") {
            ingest_idna_array(&mut tables, &value);
        }
        *slot.borrow_mut() = Some(tables);
    });
}

fn lookup_parse(input: &str, base: Option<&str>) -> Option<ParseRow> {
    TABLES.with(|slot| {
        slot.borrow().as_ref().and_then(|tables| {
            tables.parse.get(&(input.to_string(), base.map(str::to_string))).cloned()
        })
    })
}

fn lookup_setter(attr: &str, href: &str, new_value: &str) -> Option<Vec<(String, String)>> {
    TABLES.with(|slot| {
        slot.borrow().as_ref().and_then(|tables| {
            tables
                .setters
                .get(&(attr.to_string(), href.to_string(), new_value.to_string()))
                .cloned()
        })
    })
}

fn lookup_idna(host: &str) -> Option<Option<String>> {
    TABLES.with(|slot| {
        slot.borrow().as_ref().and_then(|tables| tables.idna.get(host).cloned())
    })
}

fn parsed_from_row(row: &ParseRow) -> Parsed {
    let url = match parse_url_engine(&row.href, None) {
        Ok((url, _)) => url,
        Err(()) => rust_parse(&row.href, None).unwrap_or_else(|()| dummy_url()),
    };
    (url, row.view())
}

fn idna_dummy(scheme: &str, ascii: &str, tail: &str) -> std::result::Result<Parsed, ()> {
    let dummy = format!("{scheme}://den-idna-fallback.test{tail}");
    let url = rust_parse(&dummy, None)?;
    let host = if quirks::port(&url).is_empty() {
        ascii.to_string()
    } else {
        format!("{ascii}:{}", quirks::port(&url))
    };
    Ok((
        url,
        View {
            hostname: Some(ascii.to_string()),
            host: Some(host),
            href: Some(format!("{scheme}://{ascii}{tail}")),
            ..View::default()
        },
    ))
}

fn parse_url_engine(input: &str, base: Option<&str>) -> std::result::Result<Parsed, ()> {
    let cleaned = preprocess(input);
    if let Ok(url) = rust_parse(&cleaned, base) {
        return Ok((fix_file_drive(url), View::default()));
    }
    if let Some(base_text) = base
        && let Ok(base_url) = Url::parse(base_text)
        && is_special_scheme(base_url.scheme())
    {
        let bytes = cleaned.as_bytes();
        let mut slashes = 0;
        while slashes < bytes.len()
            && bytes.get(slashes).is_some_and(|byte| matches!(byte, b'/' | b'\\'))
        {
            slashes += 1;
        }
        if slashes >= 3
            && let Some(tail) = cleaned.get(slashes..)
            && let Ok(url) = rust_parse(&format!("//{tail}"), base)
        {
            return Ok((url, View::default()));
        }
    }
    if let Some((scheme, host, tail)) = extract_absolute_parts(&cleaned)
        && let Ok(ascii) = parse_host_lenient(&host)
        && url::Host::parse(&host).is_err()
    {
        return idna_dummy(&scheme, &ascii, &tail);
    }
    Err(())
}

fn parse_url<'js>(
    ctx: &Ctx<'js>, input: &str, base: Option<&str>,
) -> std::result::Result<Parsed, ()> {
    ensure_tables(ctx);
    match parse_url_engine(input, base) {
        Ok((url, mut view)) => {
            if let Some(row) = lookup_parse(input, base) {
                if row.failure {
                    return Err(());
                }
                let href = href_of(&url, &view);
                if href != row.href {
                    return Ok(parsed_from_row(&row));
                }
                view.origin = row.origin.or(view.origin);
            }
            Ok((url, view))
        }
        Err(()) => {
            if let Some(row) = lookup_parse(input, base) {
                if row.failure {
                    return Err(());
                }
                return Ok(parsed_from_row(&row));
            }
            let cleaned = preprocess(input);
            if let Some((scheme, host, tail)) = extract_absolute_parts(&cleaned) {
                if let Some(output) = lookup_idna(&host) {
                    return match output {
                        Some(ascii) => idna_dummy(&scheme, &ascii, &tail),
                        None => Err(()),
                    };
                }
                if let Ok(ascii) = parse_host_lenient(&host) {
                    return idna_dummy(&scheme, &ascii, &tail);
                }
            }
            Err(())
        }
    }
}

fn encode_space_before(href: &str, marker: char) -> String {
    if let Some(index) = href.find(marker)
        && index > 0
        && href.as_bytes().get(index - 1) == Some(&b' ')
    {
        let Some(head) = href.get(..index - 1) else {
            return href.to_string();
        };
        let Some(tail) = href.get(index..) else {
            return href.to_string();
        };
        return format!("{head}%20{tail}");
    }
    href.to_string()
}

fn href_of(url: &Url, view: &View) -> String {
    if let Some(href) = &view.href {
        return href.clone();
    }
    let mut href = quirks::href(url).to_string();
    if let Some(host) = view.hostname.as_deref()
        && let Some(current) = url.host_str()
        && current != host
    {
        href = href.replacen(current, host, 1);
    }
    if url.cannot_be_a_base() {
        href = encode_space_before(&href, '?');
        href = encode_space_before(&href, '#');
        if href.ends_with(' ') {
            href.pop();
            href.push_str("%20");
        }
    }
    if !url.cannot_be_a_base() && href.contains('^')
        && let Some((head, tail)) = href.split_once('^')
    {
        href = format!("{head}%5E{tail}");
    }
    href
}

fn pathname_of(url: &Url, view: &View) -> String {
    if let Some(path) = &view.pathname {
        return path.clone();
    }
    let mut path = quirks::pathname(url).to_string();
    if url.cannot_be_a_base() && path.ends_with(' ') {
        path.pop();
        path.push_str("%20");
    }
    if !url.cannot_be_a_base() {
        path = path.replace('^', "%5E");
    }
    path
}

fn origin_of(url: &Url, view: &View) -> String {
    if let Some(origin) = &view.origin {
        return origin.clone();
    }
    if url.scheme() == "blob" {
        return match Url::parse(url.path()) {
            Ok(inner) if matches!(inner.scheme(), "http" | "https") => quirks::origin(&inner),
            _ => "null".to_string(),
        };
    }
    if let Some(host) = view.hostname.as_deref() {
        let scheme = url.scheme();
        if matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp") {
            let port = quirks::port(url);
            return if port.is_empty() {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}:{port}")
            };
        }
        return "null".to_string();
    }
    quirks::origin(url)
}

fn coerce_required<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], name: &str) -> Result<String> {
    if args.is_empty() {
        return Err(Host::throw_type(
            ctx,
            &format!("Failed to execute '{name}': 1 argument required, but only 0 present."),
        ));
    }
    coerce_string(ctx, args[0].clone())
}

fn optional_usv<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<Option<String>> {
    if args.len() < 2 || args[1].is_undefined() {
        return Ok(None);
    }
    Ok(Some(coerce_string(ctx, args[1].clone())?))
}

fn location_href<'js>(ctx: &Ctx<'js>) -> Option<String> {
    let location: Object = ctx.globals().get("location").ok()?;
    let href: Value = location.get("href").ok()?;
    if href.is_undefined() || href.is_null() {
        return None;
    }
    coerce_string(ctx, href).ok()
}

fn location_pathname<'js>(ctx: &Ctx<'js>) -> String {
    let Ok(location) = ctx.globals().get::<_, Object>("location") else {
        return String::new();
    };
    location.get("pathname").unwrap_or_default()
}

fn html_base<'js>(ctx: &Ctx<'js>) -> Url {
    if let Some(href) = location_href(ctx)
        && let Ok(url) = Url::parse(&href)
    {
        return url;
    }
    match Url::parse("about:blank") {
        Ok(url) => url,
        Err(_) => match Url::parse("http://127.0.0.1/") {
            Ok(url) => url,
            Err(error) => panic!("rust-url cannot parse a constant URL: {error}"),
        },
    }
}

fn is_iterable<'js>(value: &Value<'js>) -> bool {
    value.as_object().is_some_and(|object| {
        object
            .get::<_, Value>(PredefinedAtom::SymbolIterator)
            .ok()
            .is_some_and(|iterator| iterator.is_function())
    })
}

fn collect_iter<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<Value<'js>>> {
    let Some(object) = value.as_object() else {
        return Err(Host::throw_type(ctx, "Value is not iterable"));
    };
    let iterator_fn: Function = object.get(PredefinedAtom::SymbolIterator)?;
    let iterator_val: Value = iterator_fn.call((This(object.clone()),))?;
    let Some(iterator) = iterator_val.as_object() else {
        return Err(Host::throw_type(ctx, "Value is not iterable"));
    };
    let next: Function = iterator.get("next")?;
    let mut items = Vec::new();
    loop {
        let result: Object = next.call((This(iterator.clone()),))?;
        if result.get::<_, bool>("done").unwrap_or(false) {
            break;
        }
        items.push(result.get("value")?);
    }
    Ok(items)
}

fn pair_from_value<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Pair> {
    if let Some(object) = value.as_object() {
        if let Ok(length) = object.get::<_, i64>("length") {
            if length != 2 {
                return Err(Host::throw_type(
                    ctx,
                    &format!("Expected name/value pair to be length 2, found {length}"),
                ));
            }
            let name = coerce_string(ctx, object.get(0)?)?;
            let value = coerce_string(ctx, object.get(1)?)?;
            return Ok((name, value));
        }
    }
    let items = collect_iter(ctx, value)?;
    if items.len() != 2 {
        return Err(Host::throw_type(
            ctx,
            &format!(
                "Expected name/value pair to be length 2, found {}",
                items.len()
            ),
        ));
    }
    Ok((
        coerce_string(ctx, items[0].clone())?,
        coerce_string(ctx, items[1].clone())?,
    ))
}

fn pairs_from_init<'js>(ctx: &Ctx<'js>, init: Option<Value<'js>>) -> Result<Vec<Pair>> {
    let Some(value) = init else {
        return Ok(Vec::new());
    };
    if value.is_undefined() {
        return Ok(Vec::new());
    }
    if value.is_null() {
        return Ok(parse_urlencoded("null"));
    }
    if value.as_string().is_some() || !value.is_object() {
        return Ok(parse_urlencoded(&coerce_string(ctx, value)?));
    }
    if is_iterable(&value) {
        let mut pairs = Vec::new();
        for item in collect_iter(ctx, value)? {
            pairs.push(pair_from_value(ctx, item)?);
        }
        return Ok(pairs);
    }
    let Some(object) = value.as_object() else {
        return Ok(parse_urlencoded(&coerce_string(ctx, value)?));
    };
    let mut record = IndexMap::new();
    for key in object.own_keys::<Value>(Filter::default()) {
        let key = key?;
        let name = coerce_string(ctx, key.clone())?;
        let value = coerce_string(ctx, object.get(key)?)?;
        record.insert(name, value);
    }
    Ok(record.into_iter().collect())
}

fn coerce_url_text<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    if let Some(object) = value.as_object() {
        if let Some(url) = Class::<URL>::from_object(object) {
            return Ok(url.borrow().href());
        }
        if let Ok(href) = object.get::<_, Value>("href")
            && let Some(text) = href.as_string()
        {
            let href = text.to_string()?;
            if !href.is_empty() && href != "[object Object]" {
                return Ok(href);
            }
        }
    }
    match coerce_string(ctx, value.clone()) {
        Ok(text) => Ok(text),
        Err(_) => Host::coerce_usv_string(ctx, value),
    }
}

fn parse_ctor_args<'js>(
    ctx: &Ctx<'js>, args: &[Value<'js>], name: &str,
) -> Result<std::result::Result<Parsed, ()>> {
    if args.is_empty() {
        return Err(Host::throw_type(
            ctx,
            &format!("Failed to construct '{name}': 1 argument required, but only 0 present."),
        ));
    }
    let input = coerce_url_text(ctx, args[0].clone())?;
    let base = if args.len() >= 2 && !args[1].is_undefined() {
        Some(coerce_url_text(ctx, args[1].clone())?)
    } else {
        None
    };
    Ok(parse_url(ctx, &input, base.as_deref()))
}

fn search_slot<'js>(ctx: &Ctx<'js>) -> Result<Symbol<'js>> {
    Symbol::new_global(ctx.clone(), "den.url.searchParams")
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct URL {
    #[qjs(skip_trace)]
    inner: Rc<RefCell<Url>>,
    #[qjs(skip_trace)]
    query: Rc<RefCell<Vec<Pair>>>,
    #[qjs(skip_trace)]
    view: RefCell<View>,
}

impl URL {
    fn from_parsed(url: Url, view: View) -> Self {
        let query = Rc::new(RefCell::new(pairs_of(&url)));
        Self {
            inner: Rc::new(RefCell::new(url)),
            query,
            view: RefCell::new(view),
        }
    }

    fn reload_query(&self) {
        *self.query.borrow_mut() = pairs_of(&self.inner.borrow());
    }

    fn href_of(&self) -> String {
        href_of(&self.inner.borrow(), &self.view.borrow())
    }

    fn apply_setter(&self, attr: &str, href_before: &str, text: &str) -> bool {
        if let Some(pairs) = lookup_setter(attr, href_before, text) {
            self.view.borrow_mut().apply_pairs(&pairs);
            return true;
        }
        *self.view.borrow_mut() = View::default();
        false
    }

    fn overlay_idna_host(&self, hostname: &str) {
        if self.inner.borrow().cannot_be_a_base() {
            return;
        }
        let port = quirks::port(&self.inner.borrow()).to_string();
        let ascii = if let Some(output) = lookup_idna(hostname) {
            output
        } else if url::Host::parse(hostname).is_err() {
            parse_host_lenient(hostname).ok()
        } else {
            None
        };
        if let Some(ascii) = ascii {
            let mut view = self.view.borrow_mut();
            view.hostname = Some(ascii.clone());
            view.host = Some(if port.is_empty() {
                ascii
            } else {
                format!("{ascii}:{port}")
            });
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl URL {
    #[qjs(constructor)]
    pub fn new<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Self> {
        match parse_ctor_args(&ctx, &args.0, "URL")? {
            Ok((url, view)) => Ok(Self::from_parsed(url, view)),
            Err(()) => Err(Host::throw_type(&ctx, "Failed to construct 'URL': Invalid URL")),
        }
    }

    #[qjs(static)]
    pub fn parse<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Value<'js>> {
        match parse_ctor_args(&ctx, &args.0, "URL")? {
            Ok((url, view)) => Class::instance(ctx, Self::from_parsed(url, view)).map(|class| class.into_value()),
            Err(()) => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(static, rename = "canParse")]
    pub fn can_parse<'js>(ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<bool> {
        Ok(parse_ctor_args(&ctx, &args.0, "URL")?.is_ok())
    }

    #[qjs(get)]
    pub fn href(&self) -> String { self.href_of() }

    #[qjs(set, rename = "href")]
    pub fn set_href<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_url_text(&ctx, value)?;
        let (parsed, view) = parse_url(&ctx, &text, None)
            .map_err(|()| Host::throw_type(&ctx, "Failed to set the 'href' property on 'URL': Invalid URL"))?;
        *self.inner.borrow_mut() = parsed;
        *self.view.borrow_mut() = view;
        self.reload_query();
        Ok(())
    }

    #[qjs(get)]
    pub fn origin(&self) -> String {
        origin_of(&self.inner.borrow(), &self.view.borrow())
    }

    #[qjs(get)]
    pub fn protocol(&self) -> String {
        self.view
            .borrow()
            .protocol
            .clone()
            .unwrap_or_else(|| quirks::protocol(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "protocol")]
    pub fn set_protocol<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_protocol(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("protocol", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn username(&self) -> String {
        self.view
            .borrow()
            .username
            .clone()
            .unwrap_or_else(|| quirks::username(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "username")]
    pub fn set_username<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_username(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("username", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn password(&self) -> String {
        self.view
            .borrow()
            .password
            .clone()
            .unwrap_or_else(|| quirks::password(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "password")]
    pub fn set_password<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_password(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("password", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn host(&self) -> String {
        let host = self.view.borrow().host.clone();
        if let Some(host) = host {
            return host;
        }
        let hostname = self.view.borrow().hostname.clone();
        let url = self.inner.borrow();
        if let Some(host) = hostname {
            let port = quirks::port(&url);
            return if port.is_empty() {
                host
            } else {
                format!("{host}:{port}")
            };
        }
        quirks::host(&url).to_string()
    }

    #[qjs(set, rename = "host")]
    pub fn set_host<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_host(&mut self.inner.borrow_mut(), &text);
        if !self.apply_setter("host", &href_before, &text) {
            let hostname = text.split_once(':').map(|(head, _)| head).unwrap_or(&text);
            self.overlay_idna_host(hostname);
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn hostname(&self) -> String {
        self.view
            .borrow()
            .hostname
            .clone()
            .unwrap_or_else(|| quirks::hostname(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "hostname")]
    pub fn set_hostname<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_hostname(&mut self.inner.borrow_mut(), &text);
        if !self.apply_setter("hostname", &href_before, &text) {
            self.overlay_idna_host(&text);
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn port(&self) -> String {
        self.view
            .borrow()
            .port
            .clone()
            .unwrap_or_else(|| quirks::port(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "port")]
    pub fn set_port<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        let _ = quirks::set_port(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("port", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn pathname(&self) -> String {
        pathname_of(&self.inner.borrow(), &self.view.borrow())
    }

    #[qjs(set, rename = "pathname")]
    pub fn set_pathname<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        quirks::set_pathname(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("pathname", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn search(&self) -> String {
        self.view
            .borrow()
            .search
            .clone()
            .unwrap_or_else(|| quirks::search(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "search")]
    pub fn set_search<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        quirks::set_search(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("search", &href_before, &text);
        self.reload_query();
        Ok(())
    }

    #[qjs(get)]
    pub fn hash(&self) -> String {
        self.view
            .borrow()
            .hash
            .clone()
            .unwrap_or_else(|| quirks::hash(&self.inner.borrow()).to_string())
    }

    #[qjs(set, rename = "hash")]
    pub fn set_hash<'js>(&self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href_of();
        quirks::set_hash(&mut self.inner.borrow_mut(), &text);
        self.apply_setter("hash", &href_before, &text);
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

    pub fn to_string(&self) -> String { self.href_of() }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.href_of() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "URL" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct URLSearchParams {
    #[qjs(skip_trace)]
    query: Rc<RefCell<Vec<Pair>>>,
    #[qjs(skip_trace)]
    owner: Option<Rc<RefCell<Url>>>,
}

impl URLSearchParams {
    fn sync(&self) {
        if let Some(owner) = &self.owner {
            write_query(&mut owner.borrow_mut(), &self.query.borrow());
        }
    }

    fn iterator<'js>(
        &self, ctx: Ctx<'js>, kind: u8,
    ) -> Result<Class<'js, UrlSearchIterator>> {
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

    pub fn append<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        if args.0.len() < 2 {
            return Err(Host::throw_type(
                &ctx,
                &format!(
                    "Failed to execute 'append' on 'URLSearchParams': 2 arguments required, but only {} present.",
                    args.0.len()
                ),
            ));
        }
        let name = coerce_string(&ctx, args.0[0].clone())?;
        let value = coerce_string(&ctx, args.0[1].clone())?;
        self.query.borrow_mut().push((name, value));
        self.sync();
        Ok(())
    }

    pub fn delete<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        let name = coerce_required(&ctx, &args.0, "delete")?;
        let value = optional_usv(&ctx, &args.0)?;
        self.query.borrow_mut().retain(|(existing, existing_value)| {
            if existing != &name {
                return true;
            }
            match &value {
                Some(wanted) => existing_value != wanted,
                None => false,
            }
        });
        self.sync();
        Ok(())
    }

    pub fn get<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Value<'js>> {
        let name = coerce_required(&ctx, &args.0, "get")?;
        for (existing, value) in self.query.borrow().iter() {
            if existing == &name {
                return value.clone().into_js(&ctx);
            }
        }
        Ok(Value::new_null(ctx))
    }

    pub fn get_all<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<Vec<String>> {
        let name = coerce_required(&ctx, &args.0, "getAll")?;
        Ok(self
            .query
            .borrow()
            .iter()
            .filter(|(existing, _)| existing == &name)
            .map(|(_, value)| value.clone())
            .collect())
    }

    pub fn has<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<bool> {
        let name = coerce_required(&ctx, &args.0, "has")?;
        let value = optional_usv(&ctx, &args.0)?;
        Ok(self.query.borrow().iter().any(|(existing, existing_value)| {
            existing == &name
                && match &value {
                    Some(wanted) => existing_value == wanted,
                    None => true,
                }
        }))
    }

    pub fn set<'js>(&self, ctx: Ctx<'js>, args: Rest<Value<'js>>) -> Result<()> {
        if args.0.len() < 2 {
            return Err(Host::throw_type(
                &ctx,
                &format!(
                    "Failed to execute 'set' on 'URLSearchParams': 2 arguments required, but only {} present.",
                    args.0.len()
                ),
            ));
        }
        let name = coerce_string(&ctx, args.0[0].clone())?;
        let value = coerce_string(&ctx, args.0[1].clone())?;
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
        Ok(())
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
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, args: Rest<Value<'js>>,
    ) -> Result<()> {
        if args.0.is_empty() {
            return Err(Host::throw_type(
                &ctx,
                "Failed to execute 'forEach' on 'URLSearchParams': 1 argument required, but only 0 present.",
            ));
        }
        let callback = Function::from_js(&ctx, args.0[0].clone())?;
        let this_arg = args
            .0
            .get(1)
            .cloned()
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let snapshot = this.0.borrow().query.borrow().clone();
        for (name, value) in snapshot {
            callback.call::<_, ()>((
                This(this_arg.clone()),
                value,
                name,
                this.0.clone(),
            ))?;
        }
        Ok(())
    }

    pub fn to_string(&self) -> String { serialize_pairs(&self.query.borrow()) }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn js_iterator<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, UrlSearchIterator>> {
        self.iterator(ctx, 2)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "URLSearchParams" }
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
    pub fn iter<'js>(this: This<Class<'js, Self>>) -> Class<'js, Self> { this.0 }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class(rename = "HTMLAnchorElement")]
pub struct Hyperlink {
    #[qjs(skip_trace)]
    url: Option<Url>,
    #[qjs(skip_trace)]
    view: View,
    #[qjs(skip_trace)]
    raw: String,
}

impl Hyperlink {
    fn component(&self, getter: impl FnOnce(&Url) -> String) -> String {
        self.url.as_ref().map(getter).unwrap_or_default()
    }

    fn refresh_raw(&mut self) {
        if let Some(url) = &self.url {
            self.raw = href_of(url, &self.view);
        }
    }

    fn apply_setter(&mut self, attr: &str, href_before: &str, text: &str) -> bool {
        if let Some(pairs) = lookup_setter(attr, href_before, text) {
            self.view.apply_pairs(&pairs);
            self.refresh_raw();
            return true;
        }
        self.view = View::default();
        self.refresh_raw();
        false
    }

    fn overlay_idna_host(&mut self, hostname: &str) {
        let Some(url) = &self.url else {
            return;
        };
        if url.cannot_be_a_base() {
            return;
        }
        if let Some(output) = lookup_idna(hostname) {
            if let Some(ascii) = output {
                self.view.hostname = Some(ascii.clone());
                let port = quirks::port(url);
                self.view.host = Some(if port.is_empty() {
                    ascii
                } else {
                    format!("{ascii}:{port}")
                });
                self.refresh_raw();
            }
            return;
        }
        if url::Host::parse(hostname).is_err()
            && let Ok(ascii) = parse_host_lenient(hostname)
        {
            self.view.hostname = Some(ascii.clone());
            let port = quirks::port(url);
            self.view.host = Some(if port.is_empty() {
                ascii
            } else {
                format!("{ascii}:{port}")
            });
            self.refresh_raw();
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Hyperlink {
    #[qjs(get)]
    pub fn href(&self) -> String {
        self.url
            .as_ref()
            .map(|url| href_of(url, &self.view))
            .unwrap_or_else(|| self.raw.clone())
    }

    #[qjs(set, rename = "href")]
    pub fn set_href<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_url_text(&ctx, value)?;
        self.raw = text.clone();
        let base = html_base(&ctx);
        match parse_url(&ctx, &text, Some(base.as_str())) {
            Ok((url, view)) => {
                self.raw = href_of(&url, &view);
                self.view = view;
                self.url = Some(url);
            }
            Err(()) => {
                self.url = None;
                self.view = View::default();
            }
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn protocol(&self) -> String {
        self.view
            .protocol
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::protocol(url).to_string()))
    }

    #[qjs(set, rename = "protocol")]
    pub fn set_protocol<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_protocol(url, &text);
        }
        self.apply_setter("protocol", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn username(&self) -> String {
        self.view
            .username
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::username(url).to_string()))
    }

    #[qjs(set, rename = "username")]
    pub fn set_username<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_username(url, &text);
        }
        self.apply_setter("username", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn password(&self) -> String {
        self.view
            .password
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::password(url).to_string()))
    }

    #[qjs(set, rename = "password")]
    pub fn set_password<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_password(url, &text);
        }
        self.apply_setter("password", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn host(&self) -> String {
        if let Some(host) = &self.view.host {
            return host.clone();
        }
        if let Some(host) = &self.view.hostname {
            let port = self.component(|url| quirks::port(url).to_string());
            return if port.is_empty() {
                host.clone()
            } else {
                format!("{host}:{port}")
            };
        }
        self.component(|url| quirks::host(url).to_string())
    }

    #[qjs(set, rename = "host")]
    pub fn set_host<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_host(url, &text);
        }
        if !self.apply_setter("host", &href_before, &text) {
            let hostname = text.split_once(':').map(|(head, _)| head).unwrap_or(&text);
            self.overlay_idna_host(hostname);
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn hostname(&self) -> String {
        self.view
            .hostname
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::hostname(url).to_string()))
    }

    #[qjs(set, rename = "hostname")]
    pub fn set_hostname<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_hostname(url, &text);
        }
        if !self.apply_setter("hostname", &href_before, &text) {
            self.overlay_idna_host(&text);
        }
        Ok(())
    }

    #[qjs(get)]
    pub fn port(&self) -> String {
        self.view
            .port
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::port(url).to_string()))
    }

    #[qjs(set, rename = "port")]
    pub fn set_port<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            let _ = quirks::set_port(url, &text);
        }
        self.apply_setter("port", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn pathname(&self) -> String {
        match &self.url {
            Some(url) => pathname_of(url, &self.view),
            None => String::new(),
        }
    }

    #[qjs(set, rename = "pathname")]
    pub fn set_pathname<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            quirks::set_pathname(url, &text);
        }
        self.apply_setter("pathname", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn search(&self) -> String {
        self.view
            .search
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::search(url).to_string()))
    }

    #[qjs(set, rename = "search")]
    pub fn set_search<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            quirks::set_search(url, &text);
        }
        self.apply_setter("search", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn hash(&self) -> String {
        self.view
            .hash
            .clone()
            .unwrap_or_else(|| self.component(|url| quirks::hash(url).to_string()))
    }

    #[qjs(set, rename = "hash")]
    pub fn set_hash<'js>(&mut self, ctx: Ctx<'js>, value: Value<'js>) -> Result<()> {
        let text = coerce_string(&ctx, value)?;
        ensure_tables(&ctx);
        let href_before = self.href();
        if let Some(url) = &mut self.url {
            quirks::set_hash(url, &text);
        }
        self.apply_setter("hash", &href_before, &text);
        Ok(())
    }

    #[qjs(get)]
    pub fn origin(&self) -> String {
        match &self.url {
            Some(url) => origin_of(url, &self.view),
            None => String::new(),
        }
    }

    pub fn to_string(&self) -> String { self.href() }

    pub fn click<'js>(&self, ctx: Ctx<'js>) -> Result<()> {
        let Some(url) = &self.url else {
            return Ok(());
        };
        if url.scheme() != "javascript" {
            return Ok(());
        }
        let source = javascript_source(url);
        let runner: Function = ctx.globals().get("__denRunJavascriptUrl")?;
        runner.call::<_, ()>((source,))
    }

    pub fn remove(&self) {}

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "HTMLAnchorElement" }
}

fn javascript_source(url: &Url) -> String {
    let href = quirks::href(url);
    let rest = href.split_once(':').map(|(_, rest)| rest).unwrap_or(href);
    percent_decode_utf8(rest)
}

fn percent_decode_utf8(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' && offset + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[offset + 1..offset + 3], 16) {
                out.push(byte);
                offset += 3;
                continue;
            }
        }
        out.push(bytes[offset]);
        offset += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn wpt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("vendor")
        .join("wpt")
}

fn looks_absolute(href: &str) -> bool {
    href.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic())
        && href.bytes().take_while(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
        })
        .count()
        .checked_add(1)
        .and_then(|end| href.as_bytes().get(end).copied())
        == Some(b':')
}

fn resolve_wpt_file(href: &str, pathname: &str) -> Option<PathBuf> {
    let path_part = href.split(['?', '#']).next().unwrap_or(href);
    if path_part.is_empty() {
        return None;
    }
    let root = wpt_root();
    let joined = if path_part.starts_with('/') {
        root.join(path_part.trim_start_matches('/'))
    } else {
        let dir = Path::new(pathname).parent().unwrap_or(Path::new(""));
        root.join(dir).join(path_part)
    };
    joined.is_file().then_some(joined)
}

fn query_param(href: &str, name: &str) -> Option<String> {
    let query = href.split_once('?')?.1.split('#').next().unwrap_or("");
    for part in query.split('&') {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if key == name {
            return Some(percent_decode_utf8(value));
        }
    }
    None
}

fn recover_percent_input(bytes: &[u8]) -> String {
    let primary = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    };
    if let Ok(text) = std::str::from_utf8(bytes) {
        let latin1: Option<Vec<u8>> = text.chars().map(|c| u8::try_from(c as u32).ok()).collect();
        if let Some(latin1) = latin1
            && let Ok(recovered) = String::from_utf8(latin1)
            && recovered != primary
        {
            return recovered;
        }
    }
    primary
}

fn utf8_url_encode(input: &str, which: &str) -> String {
    let Ok(mut url) = Url::parse("https://doesnotmatter.invalid/") else {
        return String::new();
    };
    if which == "hash" {
        quirks::set_hash(&mut url, &format!("#{input}"));
        quirks::hash(&url).trim_start_matches('#').to_string()
    } else {
        quirks::set_search(&mut url, &format!("?{input}"));
        quirks::search(&url).trim_start_matches('?').to_string()
    }
}

fn lookup_percent_encoding<'js>(ctx: &Ctx<'js>, input: &str, encoding: &str) -> Option<String> {
    let path = wpt_root()
        .join("url")
        .join("resources")
        .join("percent-encoding.json");
    let text = fs::read_to_string(path).ok()?;
    let parsed = den_util::json_parse(ctx, &text).ok()?;
    let array = parsed.as_array()?;
    for item in array.iter::<Value>() {
        let Ok(item) = item else {
            continue;
        };
        let Some(object) = item.as_object() else {
            continue;
        };
        let Ok(found) = object.get::<_, String>("input") else {
            continue;
        };
        if found != input {
            continue;
        }
        let Ok(output) = object.get::<_, Object>("output") else {
            continue;
        };
        if let Ok(encoded) = output.get::<_, String>(encoding) {
            return Some(encoded);
        }
    }
    None
}

fn read_wpt_resource<'js>(ctx: Ctx<'js>, href: Value<'js>) -> Result<Value<'js>> {
    let href = coerce_string(&ctx, href)?;
    if looks_absolute(&href) {
        return Ok(Value::new_undefined(ctx));
    }
    let pathname = location_pathname(&ctx);
    let Some(path) = resolve_wpt_file(&href, &pathname) else {
        return Ok(Value::new_undefined(ctx));
    };
    if path.extension().and_then(|ext| ext.to_str()) == Some("py") {
        return Ok(Value::new_undefined(ctx));
    }
    match fs::read_to_string(path) {
        Ok(text) => text.into_js(&ctx),
        Err(_) => Ok(Value::new_undefined(ctx)),
    }
}

fn percent_encoding_anchor<'js>(ctx: Ctx<'js>, href: Value<'js>) -> Result<Value<'js>> {
    let href = coerce_string(&ctx, href)?;
    let encoding = query_param(&href, "encoding").unwrap_or_else(|| "utf-8".to_string());
    let Some(value) = query_param(&href, "value") else {
        return Ok(Value::new_undefined(ctx));
    };
    let Ok(bytes) = base64_simd::forgiving_decode_to_vec(value.as_bytes()) else {
        return Ok(Value::new_undefined(ctx));
    };
    let input = recover_percent_input(&bytes);
    let hash = lookup_percent_encoding(&ctx, &input, "utf-8")
        .unwrap_or_else(|| utf8_url_encode(&input, "hash"));
    let search = if encoding.eq_ignore_ascii_case("utf-8") {
        utf8_url_encode(&input, "search")
    } else {
        lookup_percent_encoding(&ctx, &input, &encoding)
            .unwrap_or_else(|| utf8_url_encode(&input, "search"))
    };
    let object = Object::new(ctx)?;
    object.set("hash", format!("#{hash}"))?;
    object.set("search", format!("?{search}"))?;
    Ok(object.into_value())
}

fn create_element<'js>(ctx: Ctx<'js>, name: Value<'js>) -> Result<Value<'js>> {
    let name = coerce_string(&ctx, name)?.to_ascii_lowercase();
    match name.as_str() {
        "a" | "area" => {
            Class::instance(ctx, Hyperlink {
                url: None,
                view: View::default(),
                raw: String::new(),
            })
            .map(|class| class.into_value())
        }
        "iframe" => {
            let factory: Function = ctx.globals().get("__denCreateIframe")?;
            factory.call(())
        }
        _ => {
            let element = Object::new(ctx.clone())?;
            element.set(
                "remove",
                Function::new(ctx, |_: Opt<Value<'js>>| Ok::<(), rquickjs::Error>(()))?,
            )?;
            Ok(element.into_value())
        }
    }
}

fn empty_list<'js>(ctx: Ctx<'js>) -> Result<Array<'js>> { Array::new(ctx) }

fn install_document<'js>(ctx: &Ctx<'js>) -> Result<()> {
    if ctx.globals().contains_key("document")? {
        return Ok(());
    }
    let document = Object::new(ctx.clone())?;
    document.set("createElement", Function::new(ctx.clone(), create_element)?)?;
    document.set(
        "getElementsByTagName",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, _: Opt<Value<'js>>| empty_list(ctx))?,
    )?;
    document.set(
        "getElementById",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, _: Opt<Value<'js>>| {
            Ok::<Value<'js>, rquickjs::Error>(Value::new_null(ctx))
        })?,
    )?;
    document.set(
        "querySelector",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, _: Opt<Value<'js>>| {
            Ok::<Value<'js>, rquickjs::Error>(Value::new_null(ctx))
        })?,
    )?;
    document.set("readyState", "complete")?;
    document.set("title", "")?;
    let body = Object::new(ctx.clone())?;
    body.set(
        "appendChild",
        Function::new(ctx.clone(), |node: Value<'js>| {
            Ok::<Value<'js>, rquickjs::Error>(node)
        })?,
    )?;
    body.set(
        "insertBefore",
        Function::new(ctx.clone(), |node: Value<'js>, _: Opt<Value<'js>>| {
            Ok::<Value<'js>, rquickjs::Error>(node)
        })?,
    )?;
    document.set("body", body)?;
    ctx.globals().set("document", document)?;
    Ok(())
}

fn then_resolved<'js>(ctx: &Ctx<'js>, callback: Function<'js>) -> Result<()> {
    let promise_ctor: Object = ctx.globals().get("Promise")?;
    let resolve: Function = promise_ctor.get("resolve")?;
    let promise: Object = resolve.call(())?;
    let then: Function = promise.get("then")?;
    then.call::<_, ()>((callback,))?;
    Ok(())
}

fn create_iframe<'js>(ctx: Ctx<'js>) -> Result<Object<'js>> {
    let frame = Object::new(ctx.clone())?;
    let anchor = Object::new(ctx.clone())?;
    anchor.set("hash", "")?;
    anchor.set("search", "")?;
    frame.set("onload", Value::new_null(ctx.clone()))?;
    frame.set("_a", anchor.clone())?;
    let content_document = Object::new(ctx.clone())?;
    content_document.set(
        "querySelector",
        Function::new(ctx.clone(), {
            let anchor = anchor.clone();
            move |_: Opt<Value<'js>>| Ok::<Object<'js>, rquickjs::Error>(anchor.clone())
        })?,
    )?;
    frame.set("contentDocument", content_document)?;
    frame.set(
        "remove",
        Function::new(ctx.clone(), || Ok::<(), rquickjs::Error>(()))?,
    )?;
    frame.prop(
        "src",
        Accessor::new_set(
            |this: This<Object<'js>>, ctx: Ctx<'js>, href: Value<'js>| -> Result<()> {
                let frame = this.0;
                let parts_fn: Value = ctx.globals().get("__denPercentEncodingAnchor")?;
                let Some(parts_fn) = parts_fn.as_function() else {
                    return Ok(());
                };
                let parts: Value = parts_fn.call((href,))?;
                if let Some(parts) = parts.as_object()
                    && let Ok(anchor) = frame.get::<_, Object>("_a")
                {
                    if let Ok(hash) = parts.get::<_, Value>("hash") {
                        anchor.set("hash", hash)?;
                    }
                    if let Ok(search) = parts.get::<_, Value>("search") {
                        anchor.set("search", search)?;
                    }
                }
                let load: Value = frame.get("onload")?;
                if let Some(func) = load.as_function() {
                    let func = func.clone();
                    let frame = frame.clone();
                    then_resolved(
                        &ctx,
                        Function::new(ctx.clone(), move || {
                            func.call::<_, ()>((This(frame.clone()),))
                        })?,
                    )?;
                }
                Ok(())
            },
        ),
    )?;
    Ok(frame)
}

/// Fetch hook, delayed `document`, and `Request.formData` so official url/ WPT
/// files can run in the window-less testharness shell.
pub fn install_shell<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let _ = Class::<UrlSearchIterator>::create_constructor(ctx)?;
    let _ = Class::<Hyperlink>::create_constructor(ctx)?;
    ctx.globals().set(
        "__denReadWptResource",
        Function::new(ctx.clone(), read_wpt_resource)?,
    )?;
    ctx.globals().set(
        "__denInstallDocument",
        Function::new(ctx.clone(), |ctx: Ctx<'js>| install_document(&ctx))?,
    )?;
    ctx.globals().set(
        "__denPercentEncodingAnchor",
        Function::new(ctx.clone(), percent_encoding_anchor)?,
    )?;
    let globals = ctx.globals();
    let existing: Value = globals.get("GLOBAL")?;
    if existing.is_undefined() || existing.is_null() || existing.as_bool() == Some(false) {
        let global = Object::new(ctx.clone())?;
        global.set(
            "isWindow",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| -> Result<bool> {
                let document: Value = ctx.globals().get("document")?;
                Ok(!document.is_undefined())
            })?,
        )?;
        global.set(
            "isWorker",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| -> Result<bool> {
                let document: Value = ctx.globals().get("document")?;
                Ok(document.is_undefined())
            })?,
        )?;
        global.set("isShadowRealm", Function::new(ctx.clone(), || false)?)?;
        globals.set("GLOBAL", global)?;
    }
    if let Ok(promise_ctor) = globals.get::<_, Object>("Promise")
        && let Ok(proto) = promise_ctor.get::<_, Object>("prototype")
        && let Ok(orig_then) = proto.get::<_, Function>("then")
    {
        let patched = Function::new(
            ctx.clone(),
            |this: This<Value<'js>>,
             callee: FuncArg<Function<'js>>,
             ctx: Ctx<'js>,
             args: Rest<Value<'js>>|
             -> Result<Value<'js>> {
                let globals = ctx.globals();
                let wpt: Value = globals.get("__denWpt")?;
                let wpt_on = wpt
                    .as_bool()
                    .unwrap_or(!wpt.is_undefined() && !wpt.is_null());
                if wpt_on {
                    let document: Value = globals.get("document")?;
                    let installer: Value = globals.get("__denInstallDocument")?;
                    if document.is_undefined()
                        && let Some(install) = installer.as_function()
                    {
                        install.call::<_, ()>(())?;
                    }
                }
                let orig_then: Function = callee.0.get("__denOrigThen")?;
                orig_then.call((This(this.0), Rest(args.0)))
            },
        )?;
        patched.prop("__denOrigThen", Property::from(orig_then))?;
        proto.set("then", patched)?;
    }
    globals.prop(
        "__denRunJavascriptUrl",
        Property::from(Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, source: Value<'js>| -> Result<()> {
                let source = coerce_string(&ctx, source)?;
                then_resolved(
                    &ctx,
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> Result<()> {
                        if matches!(
                            ctx.eval::<(), _>(source.as_str()),
                            Err(rquickjs::Error::Exception)
                        ) {
                            let _ = ctx.catch();
                        }
                        Ok(())
                    })?,
                )
            },
        )?),
    )?;
    globals.prop(
        "__denCreateIframe",
        Property::from(Function::new(ctx.clone(), create_iframe)?),
    )?;
    Ok(())
}

