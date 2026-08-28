//! HTTP(S) fetch: data URLs, CORS, redirects, cookies, cache, SRI.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::future::Either;
use rquickjs::{
    ArrayBuffer, Class, Ctx, Error, Exception, Function, Object, Result, TypedArray,
    Value as JsValue,
    function::{Constructor, This},
    promise::MaybePromise,
};
use tokio::sync::Notify;

use crate::{
    Request, Response,
    body::is_blocked_port,
    data_url,
    headers::{self, Headers, is_forbidden_request_header},
};

struct AbortWatch {
    aborted: Arc<AtomicBool>,
    notify:  Arc<Notify>,
}

impl AbortWatch {
    fn from_js<'js>(_ctx: &Ctx<'js>, value: JsValue<'js>) -> Result<Option<Self>> {
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        let Some(obj) = value.as_object() else {
            return Ok(None);
        };
        let already = obj.get::<_, JsValue>("aborted")?.as_bool().unwrap_or(false);
        let watch = Self {
            aborted: Arc::new(AtomicBool::new(already)),
            notify:  Arc::new(Notify::new()),
        };
        if already {
            watch.notify.notify_one();
        } else if let Ok(add) = obj.get::<_, Function>("addEventListener") {
            let aborted = Arc::clone(&watch.aborted);
            let notify = Arc::clone(&watch.notify);
            let listener = Function::new(_ctx.clone(), move || {
                aborted.store(true, Ordering::SeqCst);
                notify.notify_waiters();
                notify.notify_one();
                Ok::<(), Error>(())
            })?;
            let _ = add.call::<_, ()>((This(value.clone()), "abort", listener));
        }
        Ok(Some(watch))
    }

    fn refresh(&self, signal: &JsValue<'_>) {
        if let Some(object) = signal.as_object()
            && object.get::<_, bool>("aborted").ok().unwrap_or(false)
        {
            self.aborted.store(true, Ordering::SeqCst);
            self.notify.notify_waiters();
        }
    }
}

pub(crate) fn abort_error<'js>(ctx: &Ctx<'js>, signal: &JsValue<'js>) -> Error {
    if let Some(object) = signal.as_object()
        && let Ok(reason) = object.get::<_, JsValue>("reason")
        && !reason.is_undefined()
        && !reason.is_null()
    {
        return ctx.throw(reason);
    }
    if let Ok(exc) = den_util::new_dom_exception(ctx, "The operation was aborted.", "AbortError") {
        return ctx.throw(exc);
    }
    // Without `DOMException` (realms lacking `den:worker`) keep the spec's
    // error name on a plain `Error` instead of degrading to `TypeError`.
    let plain = ctx
        .globals()
        .get::<_, Constructor>("Error")
        .and_then(|ctor| ctor.construct::<_, JsValue>(("The operation was aborted.",)))
        .and_then(|exc| {
            if let Some(object) = exc.as_object() {
                object.set("name", "AbortError")?;
            }
            Ok(exc)
        });
    match plain {
        Ok(exc) => ctx.throw(exc),
        Err(_) => Exception::throw_type(ctx, "The operation was aborted."),
    }
}

fn network_error(ctx: &Ctx<'_>, message: &str) -> Error { Exception::throw_type(ctx, message) }

/// A request body on its way to the wire.
///
/// `Bytes` has a source and can be replayed, which redirects and retries need.
/// `Stream` does not: it is consumed the moment it is sent, so a second send
/// has to be a network error rather than a silently empty body.
pub(crate) enum Outgoing {
    None,
    Bytes(Vec<u8>),
    Stream(reqwest::Body),
    Spent,
}

impl Outgoing {
    fn is_none(&self) -> bool { matches!(self, Self::None) }

    fn is_stream(&self) -> bool { matches!(self, Self::Stream(_)) }

    fn take_for_send(&mut self) -> Self {
        match self {
            Self::None | Self::Spent => Self::None,
            Self::Bytes(bytes) => Self::Bytes(bytes.clone()),
            Self::Stream(_) => std::mem::replace(self, Self::Spent),
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    status:        u16,
    status_text:   String,
    headers:       Vec<(String, String)>,
    body:          Vec<u8>,
    stored_at:     Instant,
    max_age:       Option<Duration>,
    expires:       Option<SystemTime>,
    etag:          Option<String>,
    last_modified: Option<String>,
    vary:          Option<String>,
    vary_values:   Vec<(String, String)>,
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn cookie_jar() -> &'static reqwest::cookie::Jar {
    static JAR: OnceLock<reqwest::cookie::Jar> = OnceLock::new();
    JAR.get_or_init(reqwest::cookie::Jar::default)
}

fn cache() -> &'static Mutex<HashMap<String, Vec<CacheEntry>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<CacheEntry>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn origin_of(ctx: &Ctx<'_>) -> String {
    if let Ok(location) = ctx.globals().get::<_, Object>("location")
        && let Ok(origin) = location.get::<_, String>("origin")
        && !origin.is_empty()
    {
        return origin;
    }
    "http://127.0.0.1".to_string()
}

fn url_origin(url: &reqwest::Url) -> String { format!("{}://{}", url.scheme(), url.authority()) }

fn same_origin(a: &str, b: &str) -> bool { a.eq_ignore_ascii_case(b) }

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]"
}

fn connect_url(url: &reqwest::Url) -> reqwest::Url {
    let mut connected = url.clone();
    if connected.host_str().is_some_and(is_loopback_host) {
        let _ = connected.set_host(Some("127.0.0.1"));
        if connected.scheme() == "https" {
            let _ = connected.set_scheme("http");
        }
    }
    connected
}

fn cors_safelisted_request_header(name: &str, value: &str) -> bool {
    match name {
        "accept" | "accept-language" | "content-language" => {
            value.len() <= 128
                && !value.bytes().any(|byte| {
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
                })
        }
        "content-type" => {
            let mime = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            matches!(
                mime.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            ) && value.len() <= 128
        }
        _ => false,
    }
}

fn is_simple_method(method: &str) -> bool { matches!(method, "GET" | "HEAD" | "POST") }

fn needs_preflight(method: &str, headers: &[(String, String)]) -> bool {
    if !is_simple_method(method) {
        return true;
    }
    headers.iter().any(|(name, value)| {
        !cors_safelisted_request_header(name, value) && !skip_preflight_header(name)
    })
}

fn cors_safelisted_response_header(name: &str) -> bool {
    matches!(
        name,
        "cache-control"
            | "content-language"
            | "content-length"
            | "content-type"
            | "expires"
            | "last-modified"
            | "pragma"
    )
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn parse_max_age(headers: &[(String, String)]) -> Option<Duration> {
    let cc = header_value(headers, "cache-control")?;
    for part in cc.split(',') {
        let part = part.trim();
        if let Some(rest) = part.to_ascii_lowercase().strip_prefix("max-age=")
            && let Ok(secs) = rest.parse::<u64>()
        {
            return Some(Duration::from_secs(secs));
        }
    }
    None
}

fn parse_http_date(value: &str) -> Option<SystemTime> {
    let parsed = chrono_http_date(value)?;
    Some(UNIX_EPOCH + Duration::from_secs(parsed))
}

fn chrono_http_date(value: &str) -> Option<u64> {
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Vec<&str> = value.split([',', ' ']).filter(|p| !p.is_empty()).collect();
    if parts.len() < 5 {
        return None;
    }
    let day: u64 = parts[1].parse().ok()?;
    let month = months.iter().position(|m| *m == parts[2])? as u64 + 1;
    let year: u64 = parts[3].parse().ok()?;
    let time: Vec<u64> = parts[4].split(':').filter_map(|p| p.parse().ok()).collect();
    if time.len() != 3 {
        return None;
    }
    let days = year.saturating_sub(1970) * 365
        + (year.saturating_sub(1969) / 4)
        + [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][(month as usize).min(12) - 1]
        + day.saturating_sub(1);
    Some(days * 86400 + time[0] * 3600 + time[1] * 60 + time[2])
}

fn cache_directive(headers: &[(String, String)], name: &str) -> bool {
    header_value(headers, "cache-control").is_some_and(|value| {
        value
            .to_ascii_lowercase()
            .split(',')
            .any(|part| part.trim() == name || part.trim().starts_with(&format!("{name}=")))
    })
}

fn cache_fresh(entry: &CacheEntry) -> bool {
    let age_header = header_value(&entry.headers, "age")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let elapsed = entry
        .stored_at
        .elapsed()
        .as_secs()
        .saturating_add(age_header);
    if cache_directive(&entry.headers, "no-cache")
        || cache_directive(&entry.headers, "must-revalidate")
    {
        return false;
    }
    if let Some(max_age) = entry.max_age {
        return elapsed < max_age.as_secs();
    }
    if header_value(&entry.headers, "expires").is_some() {
        return entry
            .expires
            .is_some_and(|expires| SystemTime::now() < expires);
    }
    let heuristic = matches!(
        entry.status,
        200 | 203 | 204 | 206 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
    ) || cache_directive(&entry.headers, "public");
    if heuristic
        && let Some(last_modified) = &entry.last_modified
        && let Some(modified) = parse_http_date(last_modified)
        && let Ok(age) = SystemTime::now().duration_since(modified)
    {
        return Duration::from_secs(elapsed) < age / 10;
    }
    false
}

fn request_min_fresh(headers: &[(String, String)]) -> Option<u64> {
    let cc = header_value(headers, "cache-control")?;
    for part in cc.split(',') {
        if let Some(rest) = part.trim().to_ascii_lowercase().strip_prefix("min-fresh=")
            && let Ok(secs) = rest.parse::<u64>()
        {
            return Some(secs);
        }
    }
    None
}

fn request_only_if_cached(headers: &[(String, String)]) -> bool {
    cache_directive(headers, "only-if-cached")
}

fn user_has_conditional(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, _)| {
        matches!(
            name.as_str(),
            "if-match" | "if-none-match" | "if-modified-since" | "if-unmodified-since" | "if-range"
        )
    })
}

fn is_redirect_status(status: u16) -> bool { matches!(status, 301 | 302 | 303 | 307 | 308) }

fn is_unsafe_method(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE")
}

fn encode_location(location: &str) -> String {
    let mut out = String::with_capacity(location.len());
    for ch in location.chars() {
        let code = ch as u32;
        if code < 0x80 {
            out.push(ch);
        } else if code <= 0xff {
            out.push_str(&format!("%{code:02X}"));
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

fn request_max_age(headers: &[(String, String)]) -> Option<u64> {
    let cc = header_value(headers, "cache-control")?;
    for part in cc.split(',') {
        if let Some(rest) = part.trim().to_ascii_lowercase().strip_prefix("max-age=")
            && let Ok(secs) = rest.parse::<u64>()
        {
            return Some(secs);
        }
    }
    None
}

fn request_max_stale(headers: &[(String, String)]) -> Option<u64> {
    let cc = header_value(headers, "cache-control")?;
    for part in cc.split(',') {
        let part = part.trim().to_ascii_lowercase();
        if part == "max-stale" {
            return Some(u64::MAX);
        }
        if let Some(rest) = part.strip_prefix("max-stale=")
            && let Ok(secs) = rest.parse::<u64>()
        {
            return Some(secs);
        }
    }
    None
}

fn request_no_store(headers: &[(String, String)]) -> bool {
    header_value(headers, "cache-control").is_some_and(|value| {
        value
            .to_ascii_lowercase()
            .split(',')
            .any(|part| part.trim() == "no-store")
    })
}

fn is_http_token(name: &str) -> bool {
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

fn parse_cors_token_list(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    if trimmed == "*" {
        return Some(vec!["*".to_string()]);
    }
    let mut out = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if !is_http_token(part) {
            return None;
        }
        out.push(part.to_string());
    }
    Some(out)
}

fn cache_no_store(headers: &[(String, String)]) -> bool {
    header_value(headers, "cache-control")
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"))
}

fn cache_key(url: &str, credentials: &str) -> String { format!("{credentials}\0{url}") }

fn lookup_cache(
    url: &str, credentials: &str, request_headers: &[(String, String)],
) -> Option<CacheEntry> {
    let Ok(map) = cache().lock() else {
        return None;
    };
    let entries = map.get(&cache_key(url, credentials))?;
    entries
        .iter()
        .rev()
        .find(|entry| {
            if let Some(vary) = &entry.vary {
                if vary.split(',').any(|name| name.trim() == "*") {
                    return false;
                }
                vary.split(',').all(|name| {
                    let name = name.trim();
                    header_value(&entry.vary_values, name).unwrap_or("")
                        == header_value(request_headers, name).unwrap_or("")
                })
            } else {
                true
            }
        })
        .cloned()
}

fn invalidate_cache(url: &str) {
    if let Ok(mut map) = cache().lock() {
        let suffix = format!("\0{url}");
        map.retain(|key, _| key != url && !key.ends_with(&suffix));
    }
}

fn store_cache(
    url: String, credentials: &str, status: u16, status_text: String,
    headers: Vec<(String, String)>, body: Vec<u8>, request_headers: &[(String, String)],
) {
    if cache_no_store(&headers) {
        return;
    }
    let entry = CacheEntry {
        status,
        status_text,
        etag: header_value(&headers, "etag").map(ToString::to_string),
        last_modified: header_value(&headers, "last-modified").map(ToString::to_string),
        vary: header_value(&headers, "vary").map(ToString::to_string),
        vary_values: request_headers.to_vec(),
        max_age: parse_max_age(&headers),
        expires: header_value(&headers, "expires").and_then(parse_http_date),
        headers,
        body,
        stored_at: Instant::now(),
    };
    if let Ok(mut map) = cache().lock() {
        map.entry(cache_key(&url, credentials))
            .or_default()
            .push(entry);
    }
}

/// A cache entry whose body is not known yet.
///
/// Buffering a whole response just so the HTTP cache can copy it is what the
/// old size heuristic was working around. Instead the response streams and the
/// bytes are mirrored here as the consumer reads them; the entry is written
/// only when the body ends cleanly, so a body nobody reads is never buffered
/// and a truncated one is never cached.
pub(crate) struct PendingCacheWrite {
    url:             String,
    credentials:     String,
    status:          u16,
    status_text:     String,
    headers:         Vec<(String, String)>,
    request_headers: Vec<(String, String)>,
    /// Set when the entry's body is not the response body (the
    /// `content-location` case keys the entry off the request's `uuid`).
    body_override:   Option<Vec<u8>>,
}

#[derive(Default)]
pub(crate) struct CacheFill {
    writes: Vec<PendingCacheWrite>,
    body:   RefCell<Vec<u8>>,
    done:   Cell<bool>,
}

impl CacheFill {
    /// `None` when there is nothing to cache, so the streamed body pays for no
    /// mirroring at all in the common case.
    pub(crate) fn pending(writes: Vec<PendingCacheWrite>) -> Option<Rc<Self>> {
        (!writes.is_empty()).then(|| {
            Rc::new(Self {
                writes,
                ..Self::default()
            })
        })
    }

    /// The buffered path already holds the whole body.
    pub(crate) fn commit_now(writes: Vec<PendingCacheWrite>, body: &[u8]) {
        if let Some(fill) = Self::pending(writes) {
            fill.push(body);
            fill.commit();
        }
    }

    pub(crate) fn push(&self, chunk: &[u8]) {
        if !self.done.get() {
            self.body.borrow_mut().extend_from_slice(chunk);
        }
    }

    pub(crate) fn commit(&self) {
        if self.done.replace(true) {
            return;
        }
        let body = self.body.take();
        for write in &self.writes {
            store_cache(
                write.url.clone(),
                &write.credentials,
                write.status,
                write.status_text.clone(),
                write.headers.clone(),
                write.body_override.clone().unwrap_or_else(|| body.clone()),
                &write.request_headers,
            );
        }
    }
}

fn filter_cors_headers(
    headers: Vec<(String, String)>, expose: Option<&str>, credentials: bool,
) -> Vec<(String, String)> {
    let listed = expose
        .unwrap_or("")
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    let star = listed.iter().any(|name| name == "*");
    headers
        .into_iter()
        .filter(|(name, _)| {
            if name == "set-cookie" || name == "set-cookie2" {
                return false;
            }
            if cors_safelisted_response_header(name) {
                return true;
            }
            if star && !credentials {
                return true;
            }
            listed.iter().any(|exposed| exposed == name)
        })
        .collect()
}

fn check_acao(acao: Option<&str>, origin: &str, credentials: bool) -> bool {
    let Some(acao) = acao else {
        return false;
    };
    if acao.contains(',') {
        return false;
    }
    if acao == "*" {
        return !credentials;
    }
    acao.trim() == origin
}

fn skip_preflight_header(name: &str) -> bool {
    matches!(name, "origin" | "referer" | "user-agent" | "cookie")
        || is_forbidden_request_header(name, "")
}

fn orb_blocks(content_type: Option<&str>, _nosniff: bool, status: u16) -> bool {
    if status == 206 {
        return true;
    }
    let essence = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if matches!(
        essence.as_str(),
        "text/html"
            | "application/json"
            | "text/json"
            | "text/javascript"
            | "application/javascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "application/xml"
            | "text/xml"
            | "font/ttf"
            | "font/woff"
            | "font/woff2"
            | "application/gzip"
    ) {
        return true;
    }
    false
}

fn corp_blocks(corp: Option<&str>, same_origin: bool, same_site: bool) -> bool {
    match corp.map(str::trim) {
        Some("same-origin") => !same_origin,
        Some("same-site") => !same_site,
        _ => false,
    }
}

fn same_site(request_origin: &str, response_origin: &str) -> bool {
    let Ok(a) = reqwest::Url::parse(request_origin) else {
        return false;
    };
    let Ok(b) = reqwest::Url::parse(response_origin) else {
        return false;
    };
    a.host_str() == b.host_str() && a.scheme() == b.scheme()
}

fn normalize_sri_b64(input: &str) -> String {
    let mut filtered: String = input
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .map(|ch| {
            match ch {
                '-' => '+',
                '_' => '/',
                other => other,
            }
        })
        .collect();
    while filtered.len() % 4 != 0 {
        filtered.push('=');
    }
    filtered
}

async fn digest_b64<'js>(ctx: &Ctx<'js>, algorithm: &str, bytes: &[u8]) -> Result<String> {
    let crypto: Object = ctx
        .globals()
        .get("crypto")
        .map_err(|_| Exception::throw_type(ctx, "crypto is not defined"))?;
    let subtle: Object = crypto.get("subtle")?;
    let digest: Function = subtle.get("digest")?;
    let view = TypedArray::<u8>::new_copy(ctx.clone(), bytes)?;
    let produced: JsValue = digest.call((algorithm, view))?;
    let resolved = MaybePromise::from_value(produced)
        .into_future::<ArrayBuffer>()
        .await?;
    Ok(base64_simd::STANDARD.encode_to_string(resolved.as_bytes().unwrap_or(&[])))
}

async fn check_integrity<'js>(ctx: &Ctx<'js>, integrity: &str, body: &[u8]) -> Result<()> {
    let integrity = integrity.trim();
    if integrity.is_empty() {
        return Ok(());
    }
    let mut options = Vec::new();
    for part in integrity.split_whitespace() {
        let Some((algo, b64)) = part.split_once('-') else {
            continue;
        };
        let algo = algo.to_ascii_lowercase();
        let name = match algo.as_str() {
            "sha256" => "SHA-256",
            "sha384" => "SHA-384",
            "sha512" => "SHA-512",
            _ => continue,
        };
        let rank = match name {
            "SHA-512" => 3,
            "SHA-384" => 2,
            _ => 1,
        };
        options.push((rank, name, normalize_sri_b64(b64)));
    }
    if options.is_empty() {
        return Err(network_error(ctx, "Invalid integrity metadata"));
    }
    let strongest = options.iter().map(|opt| opt.0).max().unwrap_or(0);
    let strongest: Vec<_> = options
        .into_iter()
        .filter(|opt| opt.0 == strongest)
        .collect();
    let hashed = digest_b64(ctx, strongest[0].1, body).await?;
    if strongest.iter().any(|opt| opt.2 == hashed) {
        Ok(())
    } else {
        Err(network_error(ctx, "Integrity check failed"))
    }
}

async fn send_http<'js>(
    ctx: &Ctx<'js>, method: reqwest::Method, url: &reqwest::Url, headers: &[(String, String)],
    body: Outgoing, watch: Option<&AbortWatch>, send_cookies: bool,
) -> Result<reqwest::Response> {
    use reqwest::{
        cookie::CookieStore as _,
        header::{COOKIE, SET_COOKIE},
    };

    let mut builder = client().request(method, connect_url(url));
    if send_cookies && let Some(cookies) = cookie_jar().cookies(url) {
        builder = builder.header(COOKIE, cookies);
    }
    let mut saw_host = false;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("host") {
            saw_host = true;
        }
        let Ok(header_name) = name.parse::<reqwest::header::HeaderName>() else {
            continue;
        };
        let bytes: Vec<u8> = value.chars().map(|ch| ch as u8).collect();
        let Ok(header_value) = reqwest::header::HeaderValue::from_bytes(&bytes) else {
            continue;
        };
        builder = builder.header(header_name, header_value);
    }
    if !saw_host && let Some(host) = url.host_str() {
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        builder = builder.header("host", host);
    }
    match body {
        Outgoing::Bytes(bytes) => builder = builder.body(bytes),
        Outgoing::Stream(stream) => builder = builder.body(stream),
        Outgoing::None | Outgoing::Spent => {}
    }
    let request = builder.send();
    let response = if let Some(signal) = watch {
        let abort = signal.notify.notified();
        futures::pin_mut!(request);
        futures::pin_mut!(abort);
        match futures::future::select(request, abort).await {
            Either::Left((result, _)) => {
                result.map_err(|err| network_error(ctx, &format!("{err}")))?
            }
            Either::Right(_) => return Err(Exception::throw_type(ctx, "aborted")),
        }
    } else {
        request
            .await
            .map_err(|err| network_error(ctx, &format!("{err}")))?
    };
    if send_cookies {
        cookie_jar().set_cookies(&mut response.headers().get_all(SET_COOKIE).iter(), url);
    }
    Ok(response)
}

fn response_header_pairs(response: &reqwest::Response) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (name, value) in response.headers() {
        let text = value
            .to_str()
            .map(ToString::to_string)
            .unwrap_or_else(|_| value.as_bytes().iter().map(|byte| *byte as char).collect());
        if name.as_str() == "set-cookie" {
            pairs.push(("set-cookie".to_string(), text));
        } else {
            pairs.push((name.as_str().to_string(), text));
        }
    }
    pairs
}

fn collect_request_headers(
    headers: Vec<(String, String)>, origin: &str, _method: &str, cross_origin: bool,
    referrer: &str, redirected: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut has_accept = false;
    let mut has_accept_language = false;
    let mut has_user_agent = false;
    for (name, value) in headers {
        if is_forbidden_request_header(&name, &value) {
            continue;
        }
        if name == "accept" {
            has_accept = true;
        }
        if name == "accept-language" {
            has_accept_language = true;
        }
        if name == "user-agent" {
            has_user_agent = true;
        }
        out.push((name, value));
    }
    if !has_accept {
        out.push(("accept".to_string(), "*/*".to_string()));
    }
    if !has_accept_language {
        out.push(("accept-language".to_string(), "*".to_string()));
    }
    if !has_user_agent {
        out.push(("user-agent".to_string(), "den/0.4".to_string()));
    }
    let send_origin =
        cross_origin || origin == "null" || (!matches!(_method, "GET" | "HEAD") && !redirected);
    if send_origin {
        out.push(("origin".to_string(), origin.to_string()));
    }
    if referrer != "no-referrer" && referrer != "about:client" {
        out.push(("referer".to_string(), referrer.to_string()));
    } else if referrer == "about:client" {
        out.push(("referer".to_string(), origin.to_string()));
    }
    out
}

struct PreflightEntry {
    origin:  String,
    url:     String,
    method:  String,
    headers: Vec<String>,
    expires: Instant,
}

fn preflight_cache() -> &'static Mutex<Vec<PreflightEntry>> {
    static CACHE: OnceLock<Mutex<Vec<PreflightEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn preflight_cached(origin: &str, url: &str, method: &str, extra: &[String]) -> bool {
    let Ok(mut cache) = preflight_cache().lock() else {
        return false;
    };
    let now = Instant::now();
    cache.retain(|entry| entry.expires > now);
    cache.iter().any(|entry| {
        entry.origin == origin
            && entry.url == url
            && entry.method.eq_ignore_ascii_case(method)
            && extra.iter().all(|name| {
                entry
                    .headers
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(name))
            })
    })
}

fn store_preflight(
    origin: String, url: String, method: String, headers: Vec<String>, max_age: u64,
) {
    if max_age == 0 {
        return;
    }
    if let Ok(mut cache) = preflight_cache().lock() {
        cache.push(PreflightEntry {
            origin,
            url,
            method,
            headers,
            expires: Instant::now() + Duration::from_secs(max_age),
        });
    }
}

async fn preflight<'js>(
    ctx: &Ctx<'js>, url: &reqwest::Url, method: &str, headers: &[(String, String)], origin: &str,
    credentials: &str, watch: Option<&AbortWatch>,
) -> Result<()> {
    let mut extra: Vec<String> = headers
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .filter(|(name, value)| {
            !cors_safelisted_request_header(name, value) && !skip_preflight_header(name)
        })
        .map(|(name, _)| name)
        .collect();
    extra.sort();
    extra.dedup();
    if preflight_cached(origin, url.as_str(), method, &extra) {
        return Ok(());
    }
    let mut preflight_headers = vec![
        ("origin".to_string(), origin.to_string()),
        (
            "access-control-request-method".to_string(),
            method.to_string(),
        ),
        ("accept".to_string(), "*/*".to_string()),
        ("user-agent".to_string(), "den/0.4".to_string()),
        ("referer".to_string(), origin.to_string()),
    ];
    if !extra.is_empty() {
        preflight_headers.push((
            "access-control-request-headers".to_string(),
            extra.join(","),
        ));
    }
    let response = send_http(
        ctx,
        reqwest::Method::OPTIONS,
        url,
        &preflight_headers,
        Outgoing::None,
        watch,
        false,
    )
    .await?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(network_error(ctx, "CORS preflight redirect"));
    }
    if !(200..300).contains(&status) {
        return Err(network_error(ctx, "CORS preflight status"));
    }
    let pairs = response_header_pairs(&response);
    let include = credentials == "include";
    if !check_acao(
        header_value(&pairs, "access-control-allow-origin"),
        origin,
        include,
    ) {
        return Err(network_error(ctx, "CORS preflight failed"));
    }
    if include
        && header_value(&pairs, "access-control-allow-credentials")
            .is_none_or(|value| value != "true")
    {
        return Err(network_error(ctx, "CORS preflight credentials failed"));
    }
    let allow_methods = header_value(&pairs, "access-control-allow-methods");
    match allow_methods {
        Some(allow_methods) => {
            let Some(allowed) = parse_cors_token_list(allow_methods) else {
                return Err(network_error(ctx, "CORS method list invalid"));
            };
            if allowed.iter().any(|name| name == "*") {
                if include && method != "*" {
                    return Err(network_error(ctx, "CORS method not allowed"));
                }
            } else if !allowed.iter().any(|name| name == method) && !is_simple_method(method) {
                return Err(network_error(ctx, "CORS method not allowed"));
            }
        }
        None if !is_simple_method(method) => {
            return Err(network_error(ctx, "CORS method not allowed"));
        }
        _ => {}
    }
    let allow_headers = header_value(&pairs, "access-control-allow-headers");
    if let Some(allow_headers) = allow_headers
        && parse_cors_token_list(allow_headers).is_none()
    {
        return Err(network_error(ctx, "CORS header list invalid"));
    }
    if !extra.is_empty() {
        match allow_headers {
            Some("*") if include => {
                if extra.iter().any(|name| name != "*") {
                    return Err(network_error(ctx, "CORS header not allowed"));
                }
            }
            Some("*") => {
                if extra
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case("authorization"))
                {
                    return Err(network_error(ctx, "CORS header not allowed"));
                }
            }
            Some(allow) => {
                let Some(allowed) = parse_cors_token_list(allow) else {
                    return Err(network_error(ctx, "CORS header list invalid"));
                };
                for name in &extra {
                    if !allowed.iter().any(|part| part.eq_ignore_ascii_case(name)) {
                        return Err(network_error(ctx, "CORS header not allowed"));
                    }
                }
            }
            None => return Err(network_error(ctx, "CORS header not allowed")),
        }
    }
    let max_age = header_value(&pairs, "access-control-max-age")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    store_preflight(
        origin.to_string(),
        url.to_string(),
        method.to_string(),
        extra,
        max_age,
    );
    Ok(())
}

pub(crate) async fn run<'js>(
    ctx: Ctx<'js>, input: JsValue<'js>, init: Option<Object<'js>>,
) -> Result<Response<'js>> {
    let request = Request::wrap_input(ctx.clone(), input, init)?;
    let abort = AbortWatch::from_js(&ctx, request.borrow().signal.clone())?;
    let signal = request.borrow().signal.clone();
    if let Some(watch) = &abort
        && watch.aborted.load(Ordering::SeqCst)
    {
        return Err(abort_error(&ctx, &signal));
    }
    let snapshot = {
        let request = request.borrow();
        (
            request.url.clone(),
            request.method.clone(),
            request.mode.clone(),
            request.credentials.clone(),
            request.redirect.clone(),
            request.cache.clone(),
            request.integrity.clone(),
            request.referrer.clone(),
            request.headers.borrow().pairs(),
            request.body.is_some() || request.body_stream.is_some(),
        )
    };
    let (
        url,
        method,
        mode,
        credentials,
        redirect,
        cache_mode,
        integrity,
        referrer,
        headers,
        has_body,
    ) = snapshot;
    let parsed = reqwest::Url::parse(&url)
        .map_err(|error| Exception::throw_type(&ctx, &format!("Invalid URL: {error}")))?;
    if let Some(port) = parsed.port_or_known_default()
        && is_blocked_port(port)
    {
        return Err(network_error(&ctx, "Port is blocked"));
    }
    match parsed.scheme() {
        "data" => {
            let Some(data) = data_url::parse(&url) else {
                return Err(network_error(&ctx, "Invalid data URL"));
            };
            let mut header_obj = Headers::empty_with(headers::Guard::Immutable);
            header_obj.map.insert("content-type".to_string(), data.mime);
            let headers = Class::instance(ctx.clone(), header_obj)?;
            let body = if method == "HEAD" {
                None
            } else {
                Some(data.body)
            };
            return Response::from_bytes(&ctx, 200, "OK".to_string(), url, "basic", headers, body);
        }
        "about" | "blob" | "javascript" | "file" => {
            return Err(network_error(&ctx, "Scheme is not supported"));
        }
        "http" | "https" => {}
        _ => return Err(network_error(&ctx, "Scheme is not supported")),
    }

    // `duplex: "half"` is only honest if the body actually leaves as it is
    // produced, so a `ReadableStream` body is piped straight to the transport;
    // everything else has a byte source and is collected as before.
    let body = if has_body {
        let taken = request.borrow_mut().take_body(&ctx)?;
        match taken
            .as_ref()
            .and_then(JsValue::as_object)
            .and_then(Class::<den_stdlib_whatwg::streams::ReadableStream>::from_object)
        {
            Some(stream) => Outgoing::Stream(crate::upload::stream_request_body(&ctx, &stream)?),
            None => Outgoing::Bytes(crate::body::value_to_bytes(&ctx, taken).await?),
        }
    } else {
        Outgoing::None
    };
    if let Some(watch) = &abort
        && watch.aborted.load(Ordering::SeqCst)
    {
        if let Some(stream) = &request.borrow().body_stream
            && let Some(object) = stream.as_object()
            && let Ok(cancel) = object.get::<_, Function>("cancel")
        {
            let _ = cancel.call::<_, JsValue>((This(object.clone()), signal.clone()));
        }
        return Err(abort_error(&ctx, &signal));
    }

    let mut response = http_fetch(
        &ctx,
        parsed,
        method,
        mode,
        credentials,
        redirect,
        cache_mode,
        integrity,
        referrer,
        headers,
        body,
        abort.as_ref(),
        &signal,
        0,
        false,
        false,
        false,
        false,
    )
    .await?;
    response.abort_signal = signal.clone();
    if let Some(watch) = &abort {
        response.abort_notify = Some(Arc::clone(&watch.notify));
    }
    Ok(response)
}

async fn http_fetch<'js>(
    ctx: &Ctx<'js>, mut url: reqwest::Url, mut method: String, mode: String, credentials: String,
    redirect: String, cache_mode: String, integrity: String, referrer: String,
    mut headers: Vec<(String, String)>, mut body: Outgoing, watch: Option<&AbortWatch>,
    signal: &JsValue<'js>, hops: u8, mut redirected: bool, streamed_upload: bool,
    mut tainted_origin: bool, mut saw_cross: bool,
) -> Result<Response<'js>> {
    if hops > 20 {
        return Err(network_error(ctx, "Too many redirects"));
    }
    // The specification's "request's body's source is null" case: a stream body
    // was already handed to the transport and there is nothing to replay.
    if matches!(body, Outgoing::Spent) {
        return Err(network_error(
            ctx,
            "a streaming request body cannot be sent twice",
        ));
    }
    if let Some(watch) = watch {
        watch.refresh(signal);
        if watch.aborted.load(Ordering::SeqCst) {
            return Err(abort_error(ctx, signal));
        }
    }
    if let Some(port) = url.port_or_known_default()
        && is_blocked_port(port)
    {
        return Err(network_error(ctx, "Port is blocked"));
    }
    let origin = origin_of(ctx);
    let response_origin = url_origin(&url);
    let cross_origin = !same_origin(&origin, &response_origin);
    if cross_origin {
        saw_cross = true;
    }
    if mode == "same-origin" && cross_origin {
        return Err(network_error(
            ctx,
            "same-origin mode cannot fetch cross-origin",
        ));
    }
    if mode == "no-cors" && redirect != "follow" && cross_origin {
        return Err(network_error(
            ctx,
            "no-cors non-follow redirect is cross-origin",
        ));
    }
    if cache_mode == "only-if-cached" && cross_origin {
        return Err(network_error(ctx, "only-if-cached requires same-origin"));
    }

    let origin_header = if tainted_origin {
        "null".to_string()
    } else {
        origin.clone()
    };
    let mut request_headers = collect_request_headers(
        headers.clone(),
        &origin_header,
        &method,
        cross_origin || tainted_origin,
        &referrer,
        redirected,
    );
    if matches!(method.as_str(), "POST" | "PUT" | "PATCH") && body.is_none() {
        request_headers.push(("content-length".to_string(), "0".to_string()));
    }

    let cors_mode = mode == "cors";
    let preflighted =
        cors_mode && needs_preflight(&method, &request_headers) && (cross_origin || saw_cross);
    let user_conditional = user_has_conditional(&headers);
    let cacheable_method = matches!(method.as_str(), "GET" | "HEAD");
    let skip_reuse = user_conditional
        || request_no_store(&request_headers)
        || cache_directive(&request_headers, "no-cache");

    if cacheable_method
        && let Some(cached) = lookup_cache(url.as_str(), &credentials, &request_headers)
    {
        let fresh = cache_fresh(&cached);
        let age = header_value(&cached.headers, "age")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            + cached.stored_at.elapsed().as_secs();
        let freshness = cached.max_age.map(|max| max.as_secs());
        let remaining = freshness.unwrap_or(0).saturating_sub(age);
        let min_fresh_ok = request_min_fresh(&request_headers).is_none_or(|min| remaining >= min);
        let stale_ok = request_max_stale(&request_headers).is_some_and(|stale| {
            let lifetime = freshness.unwrap_or(0);
            age <= lifetime.saturating_add(stale)
        });
        let reuse = !skip_reuse
            && min_fresh_ok
            && !header_value(&request_headers, "cache-control")
                .is_some_and(|value| value.to_ascii_lowercase().contains("max-age=0"))
            && request_max_age(&request_headers).is_none_or(|max| age <= max);
        if is_redirect_status(cached.status)
            && redirect == "follow"
            && (cache_mode == "only-if-cached"
                || cache_mode == "force-cache"
                || (cache_mode == "default" && reuse && (fresh || stale_ok)))
            && let Some(location) = header_value(&cached.headers, "location")
        {
            let next = url
                .join(&encode_location(location))
                .map_err(|error| network_error(ctx, &format!("{error}")))?;
            return Box::pin(http_fetch(
                ctx,
                next,
                method,
                mode,
                credentials,
                redirect,
                cache_mode,
                integrity,
                referrer,
                headers,
                body,
                watch,
                signal,
                hops + 1,
                true,
                false,
                tainted_origin,
                saw_cross,
            ))
            .await;
        }
        match cache_mode.as_str() {
            "only-if-cached" | "force-cache" => {
                return cached_response(
                    ctx,
                    &url,
                    cached,
                    redirected,
                    &mode,
                    &origin,
                    &credentials,
                );
            }
            "default" if reuse && (fresh || stale_ok) => {
                return cached_response(
                    ctx,
                    &url,
                    cached,
                    redirected,
                    &mode,
                    &origin,
                    &credentials,
                );
            }
            "no-cache" | "default" if !fresh && !user_conditional => {
                if let Some(etag) = &cached.etag {
                    request_headers.push(("if-none-match".to_string(), etag.clone()));
                }
                if let Some(last_modified) = &cached.last_modified {
                    request_headers.push(("if-modified-since".to_string(), last_modified.clone()));
                }
            }
            _ => {}
        }
    } else if cache_mode == "only-if-cached" {
        return Err(network_error(ctx, "only-if-cached miss"));
    } else if request_only_if_cached(&request_headers) {
        let headers = Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
        return Response::from_bytes(
            ctx,
            504,
            "Gateway Timeout".to_string(),
            url.to_string(),
            "basic",
            headers,
            None,
        );
    }

    match cache_mode.as_str() {
        "no-cache" => {
            if !request_headers
                .iter()
                .any(|(name, _)| name == "cache-control")
            {
                request_headers.push(("cache-control".to_string(), "max-age=0".to_string()));
            }
        }
        "reload" | "no-store" => {
            if !request_headers.iter().any(|(name, _)| name == "pragma") {
                request_headers.push(("pragma".to_string(), "no-cache".to_string()));
            }
            if !request_headers
                .iter()
                .any(|(name, _)| name == "cache-control")
            {
                request_headers.push(("cache-control".to_string(), "no-cache".to_string()));
            }
        }
        _ if user_conditional => {
            if !request_headers.iter().any(|(name, _)| name == "pragma") {
                request_headers.push(("pragma".to_string(), "no-cache".to_string()));
            }
            if !request_headers
                .iter()
                .any(|(name, _)| name == "cache-control")
            {
                request_headers.push(("cache-control".to_string(), "no-cache".to_string()));
            }
        }
        _ => {}
    }

    if preflighted {
        preflight(
            ctx,
            &url,
            &method,
            &request_headers,
            &origin,
            &credentials,
            watch,
        )
        .await?;
    }

    let http_method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|err| Exception::throw_type(ctx, &format!("{err}")))?;
    let send_body = if matches!(method.as_str(), "GET" | "HEAD") {
        Outgoing::None
    } else {
        body.take_for_send()
    };
    let streamed_upload = streamed_upload || send_body.is_stream();
    let send_cookies = credentials == "include" || (credentials == "same-origin" && !cross_origin);
    let response = match send_http(
        ctx,
        http_method,
        &url,
        &request_headers,
        send_body,
        watch,
        send_cookies,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            if watch.is_some_and(|watch| watch.aborted.load(Ordering::SeqCst)) {
                return Err(abort_error(ctx, signal));
            }
            return Err(error);
        }
    };

    if streamed_upload && response.status().as_u16() == 401 {
        return Err(network_error(ctx, "Streaming upload rejected by 401"));
    }

    let status = response.status().as_u16();
    if is_redirect_status(status) {
        if redirect == "error" {
            return Err(network_error(ctx, "Redirect not allowed"));
        }
        if redirect == "manual" {
            if cors_mode && cross_origin {
                let pairs = response_header_pairs(&response);
                let include = credentials == "include";
                if !check_acao(
                    header_value(&pairs, "access-control-allow-origin"),
                    &origin,
                    include,
                ) {
                    return Err(network_error(ctx, "CORS failed"));
                }
            }
            let headers =
                Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
            return Response::from_bytes(
                ctx,
                0,
                String::new(),
                url.to_string(),
                "opaqueredirect",
                headers,
                None,
            );
        }
        let location = response.headers().get("location").map(|value| {
            value
                .to_str()
                .map(ToString::to_string)
                .unwrap_or_else(|_| value.as_bytes().iter().map(|byte| *byte as char).collect())
        });
        match location.as_deref() {
            Some("") => return Err(network_error(ctx, "Redirect with empty location")),
            None => {}
            Some(location) => {
                let redirect_pairs = response_header_pairs(&response);
                if mode == "no-cors"
                    && corp_blocks(
                        header_value(&redirect_pairs, "cross-origin-resource-policy"),
                        !cross_origin,
                        same_site(&origin, &response_origin),
                    )
                {
                    return Err(network_error(ctx, "CORP blocked"));
                }
                if cacheable_method && !cache_no_store(&redirect_pairs) && cache_mode != "no-store"
                {
                    store_cache(
                        url.to_string(),
                        &credentials,
                        status,
                        response
                            .status()
                            .canonical_reason()
                            .unwrap_or("")
                            .to_string(),
                        redirect_pairs.clone(),
                        Vec::new(),
                        &request_headers,
                    );
                }
                let next = url
                    .join(&encode_location(location))
                    .map_err(|error| network_error(ctx, &format!("{error}")))?;
                if next.scheme() == "data" || next.scheme() == "blob" {
                    return Err(network_error(ctx, "Redirect to data: or blob:"));
                }
                if !next.username().is_empty()
                    || next.password().is_some_and(|password| !password.is_empty())
                {
                    return Err(network_error(ctx, "Redirect URL has credentials"));
                }
                if (matches!(status, 301 | 302) && method == "POST")
                    || (status == 303 && !matches!(method.as_str(), "GET" | "HEAD"))
                {
                    method = "GET".to_string();
                    body = Outgoing::None;
                    headers.retain(|(name, _)| {
                        !matches!(
                            name.as_str(),
                            "content-type"
                                | "content-length"
                                | "content-encoding"
                                | "content-language"
                                | "content-location"
                        )
                    });
                }
                if url_origin(&next) != url_origin(&url) {
                    headers.retain(|(name, _)| name != "authorization");
                    if cross_origin {
                        tainted_origin = true;
                    }
                }
                redirected = true;
                url = next;
                return Box::pin(http_fetch(
                    ctx,
                    url,
                    method,
                    mode,
                    credentials,
                    redirect,
                    cache_mode,
                    integrity,
                    referrer,
                    headers,
                    body,
                    watch,
                    signal,
                    hops + 1,
                    redirected,
                    false,
                    tainted_origin,
                    saw_cross,
                ))
                .await;
            }
        }
    }

    let mut pairs = response_header_pairs(&response);
    if pairs.iter().any(|(_, value)| value.contains('\0')) {
        return Err(network_error(ctx, "NUL in header"));
    }
    if cors_mode && cross_origin {
        let include = credentials == "include";
        // Validate against the origin that was actually sent: a cross-origin
        // redirect taints it to "null", and the server opts back in with
        // `Access-Control-Allow-Origin: null`.
        if !check_acao(
            header_value(&pairs, "access-control-allow-origin"),
            &origin_header,
            include,
        ) {
            return Err(network_error(ctx, "CORS failed"));
        }
        if include
            && header_value(&pairs, "access-control-allow-credentials")
                .is_none_or(|value| value != "true")
        {
            return Err(network_error(ctx, "CORS credentials failed"));
        }
        let expose = header_value(&pairs, "access-control-expose-headers").map(str::to_owned);
        pairs = filter_cors_headers(pairs, expose.as_deref(), include);
    }

    if mode == "no-cors" && (cross_origin || saw_cross) {
        if !integrity.is_empty() {
            return Err(network_error(
                ctx,
                "Integrity cannot be used with opaque response",
            ));
        }
        if corp_blocks(
            header_value(&pairs, "cross-origin-resource-policy"),
            !cross_origin,
            same_site(&origin, &response_origin),
        ) {
            return Err(network_error(ctx, "CORP blocked"));
        }
        if orb_blocks(
            header_value(&pairs, "content-type"),
            header_value(&pairs, "x-content-type-options")
                .is_some_and(|value| value.eq_ignore_ascii_case("nosniff")),
            status,
        ) {
            return Err(network_error(ctx, "ORB blocked"));
        }
        let headers = Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
        return Response::from_bytes(
            ctx,
            0,
            String::new(),
            String::new(),
            "opaque",
            headers,
            None,
        );
    }

    if mode != "no-cors"
        && corp_blocks(
            header_value(&pairs, "cross-origin-resource-policy"),
            !cross_origin,
            same_site(&origin, &response_origin),
        )
    {
        return Err(network_error(ctx, "CORP blocked"));
    }

    if is_unsafe_method(&method) && (200..300).contains(&status) {
        invalidate_cache(url.as_str());
        for name in ["location", "content-location"] {
            if let Some(related) = header_value(&pairs, name)
                && let Ok(next) = url.join(&encode_location(related))
            {
                invalidate_cache(next.as_str());
            }
        }
    }

    let kind = if (cross_origin || saw_cross) && cors_mode {
        "cors"
    } else {
        "basic"
    };
    let mut header_obj = Headers::from_pairs(pairs.clone());
    header_obj.set_guard(headers::Guard::Immutable);
    let headers = Class::instance(ctx.clone(), header_obj)?;
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let null_body = crate::body::null_body_status(status) || method == "HEAD";
    if !integrity.is_empty() && null_body {
        return Err(network_error(ctx, "Integrity check on null body"));
    }

    if status == 304
        && !user_conditional
        && let Some(mut cached) = lookup_cache(url.as_str(), &credentials, &request_headers)
    {
        for (name, value) in &pairs {
            if let Some(existing) = cached
                .headers
                .iter_mut()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
            {
                existing.1.clone_from(value);
            } else {
                cached.headers.push((name.clone(), value.clone()));
            }
        }
        store_cache(
            url.to_string(),
            &credentials,
            cached.status,
            cached.status_text.clone(),
            cached.headers.clone(),
            cached.body.clone(),
            &request_headers,
        );
        return cached_response(ctx, &url, cached, redirected, &mode, &origin, &credentials);
    }

    let can_store = !cache_no_store(&pairs)
        && !matches!(cache_mode.as_str(), "no-store")
        && !user_conditional
        && !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");

    if null_body {
        if can_store {
            store_cache(
                url.to_string(),
                &credentials,
                status,
                status_text.clone(),
                pairs,
                Vec::new(),
                &request_headers,
            );
        }
        let mut produced = Response::from_bytes(
            ctx,
            status,
            status_text,
            url.to_string(),
            kind,
            headers,
            None,
        )?;
        produced.redirected = redirected;
        return Ok(produced);
    }

    // Every cache entry this response would populate, described up front so the
    // body can be streamed and mirrored into them instead of pre-buffered.
    let mut cache_writes = Vec::new();
    if matches!(method.as_str(), "POST" | "PATCH")
        && (200..300).contains(&status)
        && let Some(cl) = header_value(&pairs, "content-location")
        && (parse_max_age(&pairs).is_some()
            || header_value(&pairs, "expires").is_some()
            || cache_directive(&pairs, "public"))
        && let Ok(cl_url) = url.join(&encode_location(cl))
    {
        cache_writes.push(PendingCacheWrite {
            url: cl_url.to_string(),
            credentials: credentials.clone(),
            status,
            status_text: status_text.clone(),
            headers: pairs.clone(),
            request_headers: request_headers.clone(),
            body_override: url
                .query_pairs()
                .find(|(key, _)| key == "uuid")
                .map(|(_, value)| value.into_owned().into_bytes())
                .filter(|body| !body.is_empty()),
        });
    }
    if can_store {
        cache_writes.push(PendingCacheWrite {
            url: url.to_string(),
            credentials: credentials.clone(),
            status,
            status_text: status_text.clone(),
            headers: pairs.clone(),
            request_headers: request_headers.clone(),
            body_override: None,
        });
    }

    // Stream unless the whole body is needed before any of it can be handed
    // over, which is only subresource integrity: it has to hash the body to
    // decide whether the response exists at all. Size is not a reason — a body
    // that fits in memory is still one the consumer wants the first chunk of
    // now — and neither is caching, which the fill mirrors as bytes flow.
    let content_len = response.content_length();
    if integrity.is_empty() {
        let mut produced = Response::from_reqwest(ctx, response, kind)?;
        produced.expected_length = content_len;
        produced.cache_fill = CacheFill::pending(cache_writes);
        produced.redirected = redirected;
        produced.headers = headers;
        produced.url = url.to_string();
        produced.status = status;
        return Ok(produced);
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        Err(error) => {
            let mut produced = Response::from_failed(
                ctx,
                status,
                status_text,
                url.to_string(),
                kind,
                headers,
                error.to_string(),
            )?;
            produced.redirected = redirected;
            return Ok(produced);
        }
    };
    if let Some(expected) = content_len
        && expected as usize > bytes.len()
    {
        let mut produced = Response::from_failed(
            ctx,
            status,
            status_text,
            url.to_string(),
            kind,
            headers,
            "response body shorter than Content-Length".to_string(),
        )?;
        produced.redirected = redirected;
        return Ok(produced);
    }
    check_integrity(ctx, &integrity, &bytes).await?;
    CacheFill::commit_now(cache_writes, &bytes);
    let mut produced = Response::from_bytes(
        ctx,
        status,
        status_text,
        url.to_string(),
        kind,
        headers,
        Some(bytes),
    )?;
    produced.redirected = redirected;
    Ok(produced)
}

fn cached_response<'js>(
    ctx: &Ctx<'js>, url: &reqwest::Url, entry: CacheEntry, redirected: bool, mode: &str,
    origin: &str, credentials: &str,
) -> Result<Response<'js>> {
    let cross_origin = !same_origin(origin, &url_origin(url));
    if mode == "no-cors" && cross_origin {
        let headers = Class::instance(ctx.clone(), Headers::empty_with(headers::Guard::Immutable))?;
        return Response::from_bytes(
            ctx,
            0,
            String::new(),
            String::new(),
            "opaque",
            headers,
            None,
        );
    }
    let kind = if cross_origin && mode == "cors" {
        "cors"
    } else {
        "basic"
    };
    let mut pairs = entry.headers;
    if mode == "cors" && cross_origin {
        let include = credentials == "include";
        if !check_acao(
            header_value(&pairs, "access-control-allow-origin"),
            origin,
            include,
        ) {
            return Err(network_error(ctx, "CORS failed"));
        }
        let expose = header_value(&pairs, "access-control-expose-headers").map(str::to_owned);
        pairs = filter_cors_headers(pairs, expose.as_deref(), include);
    }
    let mut header_obj = Headers::from_pairs(pairs);
    header_obj.set_guard(headers::Guard::Immutable);
    let mut response = Response::from_bytes(
        ctx,
        entry.status,
        entry.status_text,
        url.to_string(),
        kind,
        Class::instance(ctx.clone(), header_obj)?,
        Some(entry.body),
    )?;
    response.redirected = redirected;
    Ok(response)
}
