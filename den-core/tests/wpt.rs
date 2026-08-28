//! Official WPT testharness runner (`vendor/wpt`).
//!
//! `vendor/wpt` is a shallow git submodule with a cone sparse-checkout.
//! Materialized trees: `resources/` (testharness.js), `common/`,
//! `websockets/`, `wasm/jsapi/`, `url/`, `fetch/`, `FileAPI/`.
//!
//! Each official testharness source file is one cargo/nextest test. The file
//! is `fs::read` from the submodule. This harness never rewrites files on
//! disk. `{{host}}` / `{{ports[ws][0]}}` substitution is harness-side, the
//! same job wpt-serve does, and does not edit the vendor tree.
//!
//! testharness subtests are discovered only after the file runs, so nextest
//! lists the official file. The file fails if any testharness case FAILs.
//!
//! ```text
//! cargo nextest run -p den-core --test wpt --features stdlib,wasm,jit
//! cargo nextest run -p den-core --test wpt --features stdlib -E 'test(FileAPI)'
//! ```

use std::{
    any::Any,
    collections::HashMap,
    fs,
    io::{self, ErrorKind},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use den_core::engine::Engine;
use futures::{SinkExt, StreamExt};
use libtest_mimic::{Arguments, Completion, Failed, Trial};
use rquickjs::{CatchResultExt, Object, Promise, context::EvalOptions};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{Request, Response},
};

const ADAPTER: &str = include_str!("wpt_adapter.js");
const BOOTSTRAP: &str = include_str!("wpt_bootstrap.js");

const STATUS_PASS: i32 = 0;
const STATUS_FAIL: i32 = 1;
const STATUS_TIMEOUT: i32 = 2;
const HARNESS_ERROR: i32 = 1;
const HARNESS_TIMEOUT: i32 = 2;

const WPT_TREES: &[&str] = &[
    "websockets",
    "FileAPI",
    "url",
    "fetch",
    "wasm/jsapi",
    "streams",
];

fn wpt_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let Some(workspace) = manifest.parent() else {
        panic!("den-core lives in the workspace");
    };
    workspace.join("vendor/wpt")
}

fn relative_to(root: &Path, case_path: &Path) -> String {
    case_path
        .strip_prefix(root)
        .unwrap_or(case_path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_cases(dir: &Path) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return collected;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collected.extend(collect_cases(&entry_path));
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let under_constructor = entry_path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|parent| parent.to_str())
            == Some("constructor");
        let runnable = name.ends_with(".any.js")
            || name.ends_with(".window.js")
            || ((name.ends_with(".html") || name.ends_with(".htm")) && under_constructor);
        if runnable {
            collected.push(entry_path);
        }
    }
    collected
}

fn skip_reason(relative: &str) -> Option<&'static str> {
    if relative.starts_with("wasm/") && !cfg!(feature = "wasm") {
        return Some("needs-wasm-feature");
    }
    if relative.starts_with("url/") && relative.contains(".window.") {
        return Some("needs-document");
    }
    if relative.starts_with("fetch/") {
        const FETCH_ONLY: &[(&str, &str)] = &[
            ("fetch-later", "needs-fetch-later"),
            ("mediasource", "needs-mediasource"),
            ("response-clone-iframe", "needs-document"),
            ("response-arraybuffer-realm", "needs-document"),
            ("response-blob-realm", "needs-document"),
            ("data-urls/navigate", "needs-navigation"),
            ("local-network-access", "needs-lna"),
            ("scheme-blob", "needs-blob-url"),
            ("range/blob", "needs-blob-url"),
            ("nosniff/parsing-nosniff", "needs-document"),
            ("content-type/script", "needs-document"),
            ("content-type/response", "needs-document"),
            ("content-type/multipart.window", "needs-document"),
            ("h1-parsing/", "needs-raw-http-document"),
            ("origin/assorted", "needs-document"),
            ("range/general.window", "needs-document"),
            ("redirects/data.window", "needs-navigation"),
            ("keepalive.any.js", "needs-document"),
            ("cors-keepalive", "needs-document"),
            ("redirect-keepalive", "needs-document"),
            (".h2.any.js", "needs-http2"),
            ("request-headers-case", "needs-http-header-name-case"),
            ("stale-while-revalidate/", "needs-swr-timing"),
            ("http-cache/no-vary-search", "needs-no-vary-search"),
            ("http-cache/partial", "needs-http-range-cache"),
            ("orb/tentative/", "needs-orb"),
            ("header-values.any.js", "needs-http-ctl-header-bytes"),
            ("header-values-normalize", "needs-http-ctl-header-bytes"),
            ("header-value-combining", "needs-raw-response-headers"),
            ("content-length/parsing", "needs-raw-http-content-length"),
            ("api-and-duplicate-headers", "needs-raw-http-content-length"),
        ];
        for (needle, reason) in FETCH_ONLY {
            if relative.contains(needle) {
                return Some(*reason);
            }
        }
    }
    const RULES: &[(&str, &str)] = &[
        ("idlharness", "needs-idlharness"),
        ("/stream/", "needs-websocketstream"),
        (".https.", "needs-http-wss"),
        (".wss.", "needs-http-wss"),
        ("mixed-content", "needs-http-wss"),
        ("back-forward-cache", "needs-bfcache"),
        ("basic-auth", "needs-wpt-serve"),
        ("/cookies/", "needs-wpt-serve"),
        ("opening-handshake", "needs-wpt-serve"),
        ("/handlers/", "needs-wpt-serve"),
        ("referrer", "needs-wpt-serve"),
        ("keeping-connection-open", "needs-wpt-serve"),
        ("/security/", "needs-wpt-serve"),
        ("Create-on-worker-shutdown", "needs-worker-shutdown"),
        ("bufferedAmount-unchanged", "needs-sync-xhr"),
        ("Create-http-urls", "needs-url"),
        ("Create-non-absolute-url", "needs-url"),
        ("target-address-space", "needs-private-network"),
        ("multi-globals", "needs-multi-global"),
        ("/functions/", "needs-multi-global"),
        ("/jspi/", "needs-jspi"),
        ("esm-integration", "needs-wasm-esm"),
        ("/gc/", "needs-wasm-gc"),
        ("/js-string/", "needs-wasm-string-builtins"),
        ("/exception/", "needs-wasm-proposal"),
        ("/tag/", "needs-wasm-proposal"),
        ("wasm/jsapi/function/", "needs-wasm-proposal"),
        ("moduleSource", "needs-wasm-module-source"),
        ("resizable", "needs-shared-or-resizable-memory"),
        ("constructor-shared", "needs-shared-or-resizable-memory"),
        (
            "to-fixed-length-buffer-shared",
            "needs-shared-or-resizable-memory",
        ),
        ("FileAPI/url/", "needs-blob-url"),
        ("send-file-formdata", "needs-form-post"),
        ("Blob-stream", "needs-blob-stream"),
        ("Blob-textStream", "needs-blob-stream"),
        (".worker.", "needs-worker-harness"),
        ("proto-from-ctor-realm", "needs-document"),
        ("remove-own-iframe", "needs-document"),
        ("Close-server-initiated-close", "needs-wpt-serve"),
        ("Close-delayed", "needs-wpt-serve"),
        ("send-many-64K-messages", "needs-backpressure"),
        ("close-connecting-async", "needs-wpt-serve"),
    ];
    for (needle, reason) in RULES {
        if relative.contains(needle) {
            return Some(*reason);
        }
    }
    if (relative.ends_with(".html") || relative.ends_with(".htm"))
        && !relative.ends_with("constructor/001.html")
    {
        return Some("legacy-html");
    }
    None
}

#[derive(Clone, Copy)]
struct WptPorts {
    http0: u16,
    http1: u16,
    ws:    u16,
}

fn rewrite_tokens(source: &str, ports: WptPorts, host: &str) -> String {
    source
        .replace("{{host}}", host)
        .replace("{{hosts[][www]}}", host)
        .replace("{{hosts[alt][www]}}", "127.0.0.1")
        .replace("{{hosts[alt][]}}", "127.0.0.1")
        .replace("{{hosts[alt][www2]}}", "127.0.0.1")
        .replace("{{domains[www1]}}", "127.0.0.1")
        .replace("{{domains[www2]}}", "127.0.0.1")
        .replace("{{ports[http][0]}}", &ports.http0.to_string())
        .replace("{{ports[http][1]}}", &ports.http1.to_string())
        .replace("{{ports[https][0]}}", &ports.http0.to_string())
        .replace("{{ports[https][1]}}", &ports.http1.to_string())
        .replace("{{ports[ws][0]}}", &ports.ws.to_string())
        .replace("{{ports[wss][0]}}", &ports.ws.to_string())
        .replace("{{ports[h2][0]}}", &ports.http0.to_string())
}

fn location_js(origin: &str, relative: &str) -> String {
    let path = format!("/{}", relative.replace('\\', "/"));
    let href = format!("{origin}{path}");
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let (hostname, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, port),
        None => (host, ""),
    };
    format!(
        r#"
globalThis.location = {{
  href: {href:?},
  protocol: "http:",
  host: {host:?},
  hostname: {hostname:?},
  port: {port:?},
  pathname: {path:?},
  search: "",
  hash: "",
  origin: {origin:?},
}};
"#
    )
}

fn timeout_ms(body: &str) -> u64 {
    if body.contains("timeout=long") || body.contains("name=\"timeout\" content=\"long\"") {
        60_000
    } else {
        15_000
    }
}

fn read_optional(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => String::new(),
    }
}

fn resolve_script(wpt: &Path, case_path: &Path, spec: &str) -> Option<PathBuf> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let no_query = match trimmed.split_once('?') {
        Some((head, _)) => head,
        None => trimmed,
    };
    if no_query.ends_with("testharness.js") || no_query.ends_with("testharnessreport.js") {
        return None;
    }
    if let Some(absolute) = no_query.strip_prefix('/') {
        return Some(wpt.join(absolute));
    }
    let Some(parent) = case_path.parent() else {
        return Some(wpt.join(no_query));
    };
    Some(parent.join(no_query))
}

fn script_src_attr(tag: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    let Some(src_at) = lowered.find("src=") else {
        return None;
    };
    let Some(after) = tag.get(src_at + 4..) else {
        return None;
    };
    let trimmed = after.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return None;
    };
    if first == '"' || first == '\'' {
        let Some(rest) = trimmed.get(1..) else {
            return None;
        };
        let end = rest.find(first).unwrap_or(rest.len());
        return rest.get(..end).map(str::to_owned);
    }
    let end = trimmed
        .find(|ch: char| ch.is_whitespace() || ch == '>')
        .unwrap_or(trimmed.len());
    trimmed.get(..end).map(str::to_owned)
}

fn expand_html(wpt: &Path, case_path: &Path, html: &str) -> String {
    let mut bundle = String::new();
    let mut cursor = 0;
    while let Some(rel) = html.get(cursor..).and_then(|rest| rest.find("<script")) {
        let start = cursor + rel;
        let Some(after_open) = html.get(start..) else {
            break;
        };
        let Some(gt) = after_open.find('>') else {
            break;
        };
        let Some(tag) = after_open.get(..gt) else {
            break;
        };
        let inner_at = start + gt + 1;
        let Some(after_inner) = html.get(inner_at..) else {
            break;
        };
        let Some(close) = after_inner.find("</script>") else {
            break;
        };
        let Some(inner) = after_inner.get(..close) else {
            break;
        };
        if let Some(src) = script_src_attr(tag) {
            if let Some(resolved) = resolve_script(wpt, case_path, &src) {
                bundle.push_str(&read_optional(&resolved));
                bundle.push('\n');
            }
        } else {
            bundle.push_str(inner);
            bundle.push('\n');
        }
        cursor = inner_at + close + 9;
    }
    if bundle.is_empty() {
        html.to_owned()
    } else {
        bundle
    }
}

fn expand_source(wpt: &Path, case_path: &Path, body: &str) -> String {
    let Some(name) = case_path.file_name().and_then(|name| name.to_str()) else {
        return body.to_owned();
    };
    if name.ends_with(".html") || name.ends_with(".htm") {
        return expand_html(wpt, case_path, body);
    }
    let mut bundle = String::new();
    for line in body.lines() {
        let Some(meta) = line.trim().strip_prefix("// META:") else {
            continue;
        };
        let Some(spec) = meta.trim().strip_prefix("script=") else {
            continue;
        };
        let Some(resolved) = resolve_script(wpt, case_path, spec) else {
            continue;
        };
        bundle.push_str(&read_optional(&resolved));
        bundle.push('\n');
    }
    bundle.push_str(body);
    bundle
}

struct Row {
    status:  i32,
    name:    String,
    message: String,
}

struct Report {
    timed_out: bool,
    harness:   i32,
    rows:      Vec<Row>,
}

fn parse_report(encoded: &str) -> Report {
    let mut timed_out = false;
    let mut harness = 0;
    let mut rows = Vec::new();
    for line in encoded.lines() {
        if line == "TIMEOUT" {
            timed_out = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("HARNESS\t") {
            harness = rest.parse().unwrap_or(HARNESS_ERROR);
            continue;
        }
        let mut parts = line.split('\t');
        let Some(status_text) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let message = parts.next().unwrap_or("");
        let Ok(status) = status_text.parse::<i32>() else {
            continue;
        };
        rows.push(Row {
            status,
            name: name.to_owned(),
            message: message.to_owned(),
        });
    }
    Report {
        timed_out,
        harness,
        rows,
    }
}

enum Verdict {
    Pass,
    Skip,
    Fail,
}

fn classify_verdict(report: &Report) -> Verdict {
    if report.timed_out {
        return Verdict::Fail;
    }
    if report.harness == HARNESS_ERROR || report.harness == HARNESS_TIMEOUT {
        return Verdict::Fail;
    }
    if report.rows.is_empty() {
        return Verdict::Fail;
    }
    let any_fail = report
        .rows
        .iter()
        .any(|row| row.status == STATUS_FAIL || row.status == STATUS_TIMEOUT);
    if any_fail {
        return Verdict::Fail;
    }
    let any_pass = report.rows.iter().any(|row| row.status == STATUS_PASS);
    if any_pass {
        Verdict::Pass
    } else {
        Verdict::Skip
    }
}

fn fail_detail(relative: &str, report: &Report) -> String {
    let fails: Vec<String> = report
        .rows
        .iter()
        .filter(|row| row.status == STATUS_FAIL || row.status == STATUS_TIMEOUT)
        .map(|row| format!("{} — {}", row.name, row.message))
        .collect();
    if fails.is_empty() {
        if report.timed_out {
            return format!("{relative}: harness timeout");
        }
        return format!("{relative}: harness status {}", report.harness);
    }
    format!("{relative}: {}", fails.join(" | "))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    "unknown panic".to_owned()
}

async fn bind_echo() -> Result<u16, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("wpt echo bind failed: {error}"))?;
    let echo_port = listener
        .local_addr()
        .map_err(|error| format!("wpt echo addr failed: {error}"))?
        .port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let greet = Arc::new(AtomicBool::new(false));
                let mark_greeting = Arc::clone(&greet);
                let accepted = tokio_tungstenite::accept_hdr_async(
                    stream,
                    move |request: &Request, mut response: Response| {
                        mark_greeting
                            .store(request.uri().path() == "/protocol_array", Ordering::Relaxed);
                        if let Some(header) = request.headers().get("Sec-WebSocket-Protocol") {
                            if let Ok(text) = header.to_str() {
                                if let Some(first) = text
                                    .split(',')
                                    .map(str::trim)
                                    .find(|token| !token.is_empty())
                                {
                                    if let Ok(value) = first.parse() {
                                        response
                                            .headers_mut()
                                            .insert("Sec-WebSocket-Protocol", value);
                                    }
                                }
                            }
                        }
                        Ok(response)
                    },
                )
                .await;
                let Ok(mut socket) = accepted else {
                    return;
                };
                if greet.load(Ordering::Relaxed) {
                    let _ = socket.send(Message::Text("foobar".into())).await;
                }
                while let Some(message) = socket.next().await {
                    let Ok(message) = message else {
                        break;
                    };
                    if message.is_close() {
                        break;
                    }
                    if message.is_text() || message.is_binary() {
                        let _ = socket.send(message).await;
                    }
                }
            });
        }
    });
    Ok(echo_port)
}

struct HttpState {
    root:  PathBuf,
    stash: Mutex<HashMap<String, serde_json::Value>>,
}

struct HttpOut {
    status:      u16,
    status_text: String,
    headers:     Vec<(String, String)>,
    body:        Vec<u8>,
    kind:        OutKind,
}

#[derive(Clone)]
enum OutKind {
    Bytes,
    Raw,
    Huge,
    Trickle {
        delay_ms: u64,
        count:    u32,
    },
    Infinite {
        state_key: String,
        abort_key: String,
    },
    BadChunk {
        delay_ms: u64,
        count:    u32,
    },
}

impl HttpOut {
    fn ok(body: impl Into<Vec<u8>>, content_type: &str) -> Self {
        Self {
            status:      200,
            status_text: "OK".into(),
            headers:     vec![("Content-Type".into(), content_type.into())],
            body:        body.into(),
            kind:        OutKind::Bytes,
        }
    }
}

fn query_map(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        map.insert(percent_decode(key), percent_decode(value));
    }
    map
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    out.into_iter().map(char::from).collect()
}

fn latin1_bytes(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| u32::from(ch).min(0xff) as u8)
        .collect()
}

fn header_get<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn cors_echo(headers: &HashMap<String, String>, out: &mut Vec<(String, String)>) {
    match header_get(headers, "origin") {
        Some(origin) => out.push(("Access-Control-Allow-Origin".into(), origin.into())),
        None => out.push(("Access-Control-Allow-Origin".into(), "*".into())),
    }
    out.push(("Access-Control-Allow-Credentials".into(), "true".into()));
}

fn apply_pipes(query: &str, headers: &mut Vec<(String, String)>) {
    let Some(pipe) = query_map(query).get("pipe").cloned() else {
        return;
    };
    let mut rest = pipe.as_str();
    while let Some(start) = rest.find("header(") {
        rest = &rest[start + 7..];
        let Some(comma) = rest.find(',') else {
            break;
        };
        let name = rest[..comma].trim().to_string();
        rest = &rest[comma + 1..];
        let mut depth = 1usize;
        let mut end = 0usize;
        let chars: Vec<char> = rest.chars().collect();
        while end < chars.len() {
            match chars[end] {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        if depth != 0 {
            break;
        }
        let inside: String = chars[..end].iter().collect();
        rest = &rest[rest
            .char_indices()
            .nth(end)
            .map(|(i, _)| i + 1)
            .unwrap_or(rest.len())..];
        let (value, append) = if let Some(stripped) = inside.strip_suffix(",True") {
            (stripped.trim().to_string(), true)
        } else {
            (inside.trim().to_string(), false)
        };
        if name.eq_ignore_ascii_case("set-cookie") || append {
            headers.push((name, value));
        } else {
            headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
            headers.push((name, value));
        }
    }
}

fn b64decode(input: &str) -> Option<Vec<u8>> {
    let mut filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .map(|byte| {
            match byte {
                b'-' => b'+',
                b'_' => b'/',
                other => other,
            }
        })
        .collect();
    while filtered.len() % 4 != 0 {
        filtered.push(b'=');
    }
    let table = |byte: u8| -> Option<u8> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 0,
            _ => return None,
        })
    };
    let mut out = Vec::new();
    for chunk in filtered.chunks(4) {
        let a = table(chunk[0])?;
        let b = table(chunk[1])?;
        let c = table(chunk[2])?;
        let d = table(chunk[3])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

fn http_date(delta_secs: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        + delta_secs;
    let now = now.max(0) as u64;
    let days = now / 86400;
    let rem = now % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let weekdays = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let months = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        weekdays[(days % 7) as usize],
        d,
        months[m as usize],
        y,
        hour,
        min,
        sec
    )
}

fn stash_key(path: &str, token: &str) -> String {
    let dir = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(path);
    format!("{dir}\0{token}")
}

fn handle_py(
    state: &HttpState, method: &str, path: &str, query: &str, headers: &HashMap<String, String>,
    body: &[u8],
) -> Option<HttpOut> {
    let q = query_map(query);
    let file = path.rsplit('/').next().unwrap_or(path);
    match file {
        "inspect-headers.py" => {
            let mut out = Vec::new();
            if let Some(list) = q.get("headers") {
                for header in list.split('|') {
                    if let Some(value) = header_get(headers, header) {
                        out.push((format!("x-request-{header}"), value.to_string()));
                    }
                }
            }
            if q.contains_key("cors") {
                cors_echo(headers, &mut out);
                out.push(("Access-Control-Allow-Credentials".into(), "true".into()));
                out.push((
                    "Access-Control-Allow-Methods".into(),
                    "GET, POST, HEAD, OPTIONS".into(),
                ));
                if let Some(list) = q.get("headers") {
                    let exposed = list
                        .split('|')
                        .map(|header| format!("x-request-{header}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push(("Access-Control-Expose-Headers".into(), exposed));
                }
                if let Some(allow) = q.get("allow_headers") {
                    out.push(("Access-Control-Allow-Headers".into(), allow.clone()));
                } else if let Some(acrh) = header_get(headers, "access-control-request-headers") {
                    out.push(("Access-Control-Allow-Headers".into(), acrh.into()));
                }
            }
            out.push(("Content-Type".into(), "text/plain".into()));
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        Vec::new(),
                kind:        OutKind::Bytes,
            })
        }
        "echo-content.py" | "echo-content.h2.py" => {
            let mut out = vec![
                ("X-Request-Method".into(), method.into()),
                (
                    "X-Request-Content-Length".into(),
                    header_get(headers, "content-length").unwrap_or("NO").into(),
                ),
                (
                    "X-Request-Content-Type".into(),
                    header_get(headers, "content-type").unwrap_or("NO").into(),
                ),
                ("Content-Type".into(), "text/plain".into()),
            ];
            if q.contains_key("cors") || header_get(headers, "origin").is_some() {
                cors_echo(headers, &mut out);
            }
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        body.to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "method.py" => {
            let mut out = vec![
                ("x-request-method".into(), method.into()),
                (
                    "x-request-content-type".into(),
                    header_get(headers, "content-type").unwrap_or("NO").into(),
                ),
                (
                    "x-request-content-length".into(),
                    header_get(headers, "content-length").unwrap_or("NO").into(),
                ),
                (
                    "x-request-content-encoding".into(),
                    header_get(headers, "content-encoding")
                        .unwrap_or("NO")
                        .into(),
                ),
                (
                    "x-request-content-language".into(),
                    header_get(headers, "content-language")
                        .unwrap_or("NO")
                        .into(),
                ),
                (
                    "x-request-content-location".into(),
                    header_get(headers, "content-location")
                        .unwrap_or("NO")
                        .into(),
                ),
            ];
            if q.contains_key("cors") {
                cors_echo(headers, &mut out);
                out.push((
                    "Access-Control-Allow-Methods".into(),
                    "GET, POST, PUT, FOO".into(),
                ));
                out.push((
                    "Access-Control-Allow-Headers".into(),
                    "x-test, x-foo".into(),
                ));
                out.push((
                    "Access-Control-Expose-Headers".into(),
                    "x-request-method".into(),
                ));
            }
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        body.to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "status.py" => {
            let code = q.get("code").and_then(|v| v.parse().ok()).unwrap_or(200);
            let text = q.get("text").cloned().unwrap_or_else(|| "OMG".into());
            let content = q.get("content").cloned().unwrap_or_default();
            let ctype = q.get("type").cloned().unwrap_or_default();
            Some(HttpOut {
                status:      code,
                status_text: text,
                headers:     vec![
                    ("Content-Type".into(), ctype),
                    ("X-Request-Method".into(), method.into()),
                ],
                body:        latin1_bytes(&content),
                kind:        OutKind::Bytes,
            })
        }
        "redirect.py" | "redirect.h2.py" => Some(handle_redirect(state, method, path, &q, headers)),
        "redirect-empty-location.py" => {
            Some(HttpOut {
                status:      302,
                status_text: "Found".into(),
                headers:     vec![("Location".into(), String::new())],
                body:        Vec::new(),
                kind:        OutKind::Bytes,
            })
        }
        "preflight.py" => Some(handle_preflight(state, method, path, &q, headers)),
        "cache.py" if path.contains("/request/resources/") => {
            Some(handle_request_cache(state, path, &q, headers))
        }
        "cache.py" => {
            Some(HttpOut {
                status:      if header_get(headers, "if-none-match") == Some("\"123abc\"") {
                    304
                } else {
                    200
                },
                status_text: if header_get(headers, "if-none-match") == Some("\"123abc\"") {
                    "Not Modified".into()
                } else {
                    "OK".into()
                },
                headers:     {
                    let not_modified = header_get(headers, "if-none-match") == Some("\"123abc\"");
                    let mut outgoing = vec![
                        ("ETag".into(), "\"123abc\"".into()),
                        ("Content-Type".into(), "text/plain".into()),
                    ];
                    if not_modified {
                        outgoing.push(("X-HTTP-STATUS".into(), "304".into()));
                    }
                    outgoing
                },
                body:        if header_get(headers, "if-none-match") == Some("\"123abc\"") {
                    Vec::new()
                } else {
                    b"lorem ipsum dolor sit amet".to_vec()
                },
                kind:        OutKind::Bytes,
            })
        }
        "authentication.py" => {
            let auth = header_get(headers, "authorization").unwrap_or("");
            if auth == "Basic dXNlcjpwYXNzd29yZA==" {
                Some(HttpOut::ok(b"Authentication done".to_vec(), "text/plain"))
            } else {
                let realm = q.get("realm").cloned().unwrap_or_else(|| "test".into());
                Some(HttpOut {
                    status:      401,
                    status_text: "Unauthorized".into(),
                    headers:     vec![(
                        "WWW-Authenticate".into(),
                        format!("Basic realm=\"{realm}\""),
                    )],
                    body:        b"Please login with credentials 'user' and 'password'".to_vec(),
                    kind:        OutKind::Bytes,
                })
            }
        }
        "dump-authorization-header.py" => {
            let mut out = vec![
                ("Content-Type".into(), "text/html".into()),
                ("Cache-Control".into(), "no-cache".into()),
                (
                    "Access-Control-Allow-Headers".into(),
                    "Authorization".into(),
                ),
            ];
            cors_echo(headers, &mut out);
            let body = header_get(headers, "authorization").unwrap_or("none");
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        body.as_bytes().to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "stash-put.py" => {
            if method == "OPTIONS" {
                return Some(HttpOut {
                    status:      200,
                    status_text: "OK".into(),
                    headers:     vec![
                        ("Access-Control-Allow-Origin".into(), "*".into()),
                        ("Access-Control-Allow-Methods".into(), "*".into()),
                        ("Access-Control-Allow-Headers".into(), "*".into()),
                    ],
                    body:        b"done".to_vec(),
                    kind:        OutKind::Bytes,
                });
            }
            let key = q.get("key").cloned().unwrap_or_default();
            let value = q.get("value").cloned().unwrap_or_default();
            if let Ok(mut stash) = state.stash.lock() {
                stash.insert(stash_key(path, &key), serde_json::Value::String(value));
            }
            let mut headers_out = Vec::new();
            if !q.contains_key("disallow_cross_origin") {
                headers_out.push(("Access-Control-Allow-Origin".into(), "*".into()));
            }
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     headers_out,
                body:        b"done".to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "stash-take.py" => {
            let key = q.get("key").cloned().unwrap_or_default();
            let taken = state
                .stash
                .lock()
                .ok()
                .and_then(|mut stash| stash.remove(&stash_key(path, &key)));
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![
                    ("Access-Control-Allow-Origin".into(), "*".into()),
                    ("Content-Type".into(), "application/json".into()),
                ],
                body:        serde_json::to_vec(&taken).unwrap_or_else(|_| b"null".to_vec()),
                kind:        OutKind::Bytes,
            })
        }
        "clean-stash.py" => {
            let token = q.get("token").cloned().unwrap_or_default();
            let found = state
                .stash
                .lock()
                .ok()
                .and_then(|mut stash| stash.remove(&stash_key(path, &token)))
                .is_some();
            Some(HttpOut::ok(
                if found { b"1".to_vec() } else { b"0".to_vec() },
                "text/plain",
            ))
        }
        "trickle.py" => {
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     if q.contains_key("notype") {
                    vec![("Transfer-Encoding".into(), "chunked".into())]
                } else {
                    vec![
                        ("Content-Type".into(), "text/plain".into()),
                        ("Transfer-Encoding".into(), "chunked".into()),
                    ]
                },
                body:        Vec::new(),
                kind:        OutKind::Trickle {
                    delay_ms: q.get("ms").and_then(|v| v.parse().ok()).unwrap_or(500),
                    count:    q.get("count").and_then(|v| v.parse().ok()).unwrap_or(50),
                },
            })
        }
        "huge-response.py" => {
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![
                    ("Content-Type".into(), "text/plain".into()),
                    (
                        "Content-Length".into(),
                        (8u64 * 1024 * 1024 * 1024).to_string(),
                    ),
                    ("Cache-Control".into(), "max-age=86400".into()),
                ],
                body:        Vec::new(),
                kind:        OutKind::Huge,
            })
        }
        "infinite-slow-response.py" => {
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![("Content-Type".into(), "text/plain".into())],
                body:        Vec::new(),
                kind:        OutKind::Infinite {
                    state_key: q.get("stateKey").cloned().unwrap_or_default(),
                    abort_key: q.get("abortKey").cloned().unwrap_or_default(),
                },
            })
        }
        "bad-chunk-encoding.py" => {
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![("Transfer-Encoding".into(), "chunked".into())],
                body:        Vec::new(),
                kind:        OutKind::BadChunk {
                    delay_ms: q.get("ms").and_then(|v| v.parse().ok()).unwrap_or(1000),
                    count:    q.get("count").and_then(|v| v.parse().ok()).unwrap_or(50),
                },
            })
        }
        "bad-gzip-body.py" => {
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![("Content-Encoding".into(), "gzip".into())],
                body:        b"not actually gzip".to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "hello.py" => {
            let mut out = Vec::new();
            if let Some(corp) = q.get("corp") {
                out.push(("Cross-Origin-Resource-Policy".into(), corp.clone()));
            }
            if let Some(origin) = header_get(headers, "origin") {
                out.push(("Access-Control-Allow-Origin".into(), origin.into()));
            }
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        b"hello".to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        "http-cache.py" => Some(handle_http_cache(state, method, path, query, &q, headers)),
        "content-length.py" => {
            let extra = q.get("length").cloned().unwrap_or_default();
            let mut raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain;charset=UTF-8\r\nConnection: \
                 close\r\n{extra}\r\n\r\n"
            )
            .into_bytes();
            raw.extend_from_slice(b"Fact: this is really forty-two bytes long.");
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     Vec::new(),
                body:        raw,
                kind:        OutKind::Raw,
            })
        }
        "echo-headers.py" => {
            let mut body = String::new();
            for (name, value) in headers {
                body.push_str(name);
                body.push_str(": ");
                body.push_str(value);
                body.push('\n');
            }
            Some(HttpOut::ok(body.into_bytes(), "text/plain"))
        }
        "parse-headers.py" => {
            let mut out = Vec::new();
            for (name, value) in &q {
                if name != "pipe" {
                    out.push((name.clone(), value.clone()));
                }
            }
            Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     out,
                body:        Vec::new(),
                kind:        OutKind::Bytes,
            })
        }
        "network-partition-key.py" => {
            Some(HttpOut {
                status:      q.get("status").and_then(|v| v.parse().ok()).unwrap_or(421),
                status_text: "Misdirected Request".into(),
                headers:     vec![("Content-Type".into(), "text/plain".into())],
                body:        b"ok. Request was sent 1 times. 1 connections were created.".to_vec(),
                kind:        OutKind::Bytes,
            })
        }
        _ => None,
    }
}

fn handle_redirect(
    state: &HttpState, method: &str, path: &str, q: &HashMap<String, String>,
    headers: &HashMap<String, String>,
) -> HttpOut {
    let token = q.get("token").cloned();
    let key = token.as_deref().map(|token| stash_key(path, token));
    let mut count = 0u64;
    if let Some(key) = &key
        && let Ok(stash) = state.stash.lock()
        && let Some(serde_json::Value::Number(number)) = stash.get(key)
    {
        count = number.as_u64().unwrap_or(0);
    }
    let mut out = vec![
        ("Content-Type".into(), "text/plain".into()),
        ("Cache-Control".into(), "no-cache".into()),
        ("Pragma".into(), "no-cache".into()),
    ];
    if !path.contains("/common/redirect") {
        cors_echo(headers, &mut out);
    }
    if let Some(allow) = q.get("allow_headers") {
        out.push(("Access-Control-Allow-Headers".into(), allow.clone()));
    }
    if method == "OPTIONS" && !q.contains_key("redirect_preflight") {
        return HttpOut {
            status:      200,
            status_text: "OK".into(),
            headers:     out,
            body:        Vec::new(),
            kind:        OutKind::Bytes,
        };
    }
    count += 1;
    if let Some(key) = &key
        && let Ok(mut stash) = state.stash.lock()
    {
        stash.insert(key.clone(), serde_json::json!(count));
    }
    if let Some(max) = q.get("max_count").and_then(|v| v.parse::<u64>().ok())
        && count > max
    {
        return HttpOut::ok((count - 1).to_string().into_bytes(), "text/plain");
    }
    let status = q
        .get("redirect_status")
        .and_then(|v| v.parse().ok())
        .unwrap_or(302);
    if let Some(corp) = q.get("corp") {
        out.push(("Cross-Origin-Resource-Policy".into(), corp.clone()));
    }
    if let Some(location) = q.get("location").or_else(|| q.get("redirectTo")) {
        let mut location = location.clone();
        if !q.contains_key("simple") {
            location.push(if location.contains('?') { '&' } else { '?' });
            let mut first = true;
            for (key, value) in q {
                if key == "allow_headers" || key == "allow_methods" {
                    continue;
                }
                if !first {
                    location.push('&');
                }
                first = false;
                location.push_str(&percent_encode(key));
                location.push('=');
                location.push_str(&percent_encode(value));
            }
            location.push_str(&format!("&count={count}"));
        }
        out.push(("Location".into(), location));
    }
    HttpOut {
        status,
        status_text: "Redirect".into(),
        headers: out,
        body: Vec::new(),
        kind: OutKind::Bytes,
    }
}

fn handle_preflight(
    state: &HttpState, method: &str, path: &str, q: &HashMap<String, String>,
    headers: &HashMap<String, String>,
) -> HttpOut {
    let mut out = vec![("Content-Type".into(), "text/plain".into())];
    if let Some(origin) = q.get("origin") {
        out.push(("Access-Control-Allow-Origin".into(), origin.clone()));
    } else {
        out.push(("Access-Control-Allow-Origin".into(), "*".into()));
    }
    if q.contains_key("credentials") {
        out.push(("Access-Control-Allow-Credentials".into(), "true".into()));
    }
    let token = q.get("token").cloned();
    let key = token.as_deref().map(|token| stash_key(path, token));
    if q.contains_key("clear-stash") {
        let found = key
            .as_ref()
            .and_then(|key| state.stash.lock().ok()?.remove(key))
            .is_some();
        return HttpOut {
            status:      200,
            status_text: "OK".into(),
            headers:     out,
            body:        if found { b"1".to_vec() } else { b"0".to_vec() },
            kind:        OutKind::Bytes,
        };
    }
    if method == "OPTIONS" {
        if let Some(max_age) = q.get("max_age") {
            out.push(("Access-Control-Max-Age".into(), max_age.clone()));
        }
        if let Some(allow) = q.get("allow_headers") {
            out.push(("Access-Control-Allow-Headers".into(), allow.clone()));
        }
        if let Some(allow) = q.get("allow_methods") {
            out.push(("Access-Control-Allow-Methods".into(), allow.clone()));
        }
        if let Some(key) = &key
            && let Ok(mut stash) = state.stash.lock()
        {
            let mut stored = serde_json::json!({
                "preflight": "1",
                "referrer": header_get(headers, "referer").unwrap_or(""),
                "user_agent": header_get(headers, "user-agent").unwrap_or(""),
            });
            if let Some(acrh) = header_get(headers, "access-control-request-headers") {
                stored["acrh"] = serde_json::Value::String(acrh.to_string());
            }
            stash.insert(key.clone(), stored);
        }
        let status = q
            .get("preflight_status")
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        return HttpOut {
            status,
            status_text: "OK".into(),
            headers: out,
            body: Vec::new(),
            kind: OutKind::Bytes,
        };
    }
    let stored = key
        .as_ref()
        .and_then(|key| state.stash.lock().ok()?.get(key).cloned())
        .unwrap_or(serde_json::json!({}));
    if q.contains_key("checkUserAgentHeaderInPreflight")
        && header_get(headers, "user-agent") != stored.get("user_agent").and_then(|v| v.as_str())
    {
        return HttpOut {
            status:      400,
            status_text: "Bad Request".into(),
            headers:     out,
            body:        b"ERROR: No user-agent header in preflight".to_vec(),
            kind:        OutKind::Bytes,
        };
    }
    out.push((
        "Access-Control-Expose-Headers".into(),
        "x-did-preflight, x-control-request-headers, x-referrer, x-preflight-referrer, x-origin"
            .into(),
    ));
    out.push((
        "x-did-preflight".into(),
        stored
            .get("preflight")
            .and_then(|v| v.as_str())
            .unwrap_or("0")
            .into(),
    ));
    if let Some(acrh) = stored.get("acrh").and_then(|v| v.as_str()) {
        out.push(("x-control-request-headers".into(), acrh.into()));
    }
    out.push((
        "x-preflight-referrer".into(),
        stored
            .get("referrer")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
    ));
    out.push((
        "x-referrer".into(),
        header_get(headers, "referer").unwrap_or("").into(),
    ));
    out.push((
        "x-origin".into(),
        header_get(headers, "origin").unwrap_or("").into(),
    ));
    HttpOut {
        status:      200,
        status_text: "OK".into(),
        headers:     out,
        body:        Vec::new(),
        kind:        OutKind::Bytes,
    }
}

fn handle_request_cache(
    state: &HttpState, path: &str, q: &HashMap<String, String>, headers: &HashMap<String, String>,
) -> HttpOut {
    let token = q.get("token").cloned().unwrap_or_default();
    let key = stash_key(path, &token);
    if q.contains_key("querystate") {
        let taken = state
            .stash
            .lock()
            .ok()
            .and_then(|mut stash| stash.remove(&key))
            .unwrap_or(serde_json::Value::Null);
        return HttpOut::ok(
            serde_json::to_vec(&taken).unwrap_or_else(|_| b"null".to_vec()),
            "text/plain",
        );
    }
    let mut state_entry = serde_json::Map::new();
    if !q.contains_key("ignore") {
        if let Some(value) = header_get(headers, "if-none-match") {
            state_entry.insert(
                "If-None-Match".into(),
                serde_json::Value::String(value.into()),
            );
        }
        if let Some(value) = header_get(headers, "if-modified-since") {
            state_entry.insert(
                "If-Modified-Since".into(),
                serde_json::Value::String(value.into()),
            );
        }
        if let Some(value) = header_get(headers, "pragma") {
            state_entry.insert("Pragma".into(), serde_json::Value::String(value.into()));
        }
        if let Some(value) = header_get(headers, "cache-control") {
            state_entry.insert(
                "Cache-Control".into(),
                serde_json::Value::String(value.into()),
            );
        }
    }
    if let Ok(mut stash) = state.stash.lock() {
        let list = stash.entry(key).or_insert_with(|| serde_json::json!([]));
        if let serde_json::Value::Array(items) = list {
            items.push(serde_json::Value::Object(state_entry));
        }
    }
    let mut out = vec![("Access-Control-Allow-Origin".into(), "*".into())];
    if let Some(tag) = q.get("tag") {
        out.push(("ETag".into(), format!("\"{tag}\"")));
    }
    if let Some(date) = q.get("date") {
        out.push(("Last-Modified".into(), date.clone()));
    }
    if let Some(expires) = q.get("expires") {
        out.push(("Expires".into(), expires.clone()));
    }
    if let Some(vary) = q.get("vary") {
        out.push(("Vary".into(), vary.clone()));
    }
    if let Some(cc) = q.get("cache_control") {
        out.push(("Cache-Control".into(), cc.clone()));
    }
    if let Some(redirect) = q.get("redirect") {
        out.push(("Location".into(), redirect.clone()));
        return HttpOut {
            status:      302,
            status_text: "Redirect".into(),
            headers:     out,
            body:        Vec::new(),
            kind:        OutKind::Bytes,
        };
    }
    let tag = q.get("tag").map(|tag| format!("\"{tag}\""));
    let not_modified = tag
        .as_deref()
        .is_some_and(|tag| header_get(headers, "if-none-match") == Some(tag))
        || q.get("date")
            .is_some_and(|date| header_get(headers, "if-modified-since") == Some(date.as_str()));
    if not_modified {
        return HttpOut {
            status:      304,
            status_text: "Not Modified".into(),
            headers:     out,
            body:        Vec::new(),
            kind:        OutKind::Bytes,
        };
    }
    out.push(("Content-Type".into(), "text/plain".into()));
    HttpOut {
        status:      200,
        status_text: "OK".into(),
        headers:     out,
        body:        latin1_bytes(&q.get("content").cloned().unwrap_or_default()),
        kind:        OutKind::Bytes,
    }
}

fn handle_http_cache(
    state: &HttpState, method: &str, path: &str, query: &str, q: &HashMap<String, String>,
    headers: &HashMap<String, String>,
) -> HttpOut {
    let mut base = vec![("Access-Control-Allow-Credentials".into(), "true".into())];
    if method == "OPTIONS" {
        cors_echo(headers, &mut base);
        base.push((
            "Access-Control-Allow-Methods".into(),
            "GET, HEAD, POST, PUT, DELETE, PATCH, FOO, OPTIONS".into(),
        ));
        base.push((
            "Access-Control-Allow-Headers".into(),
            header_get(headers, "access-control-request-headers")
                .unwrap_or("*")
                .into(),
        ));
        base.push(("Access-Control-Max-Age".into(), "86400".into()));
        return HttpOut {
            status:      200,
            status_text: "OK".into(),
            headers:     base,
            body:        b"Preflight request".to_vec(),
            kind:        OutKind::Bytes,
        };
    }
    let uuid = q.get("uuid").cloned().unwrap_or_default();
    if uuid.is_empty() {
        return HttpOut {
            status:      404,
            status_text: "Not Found".into(),
            headers:     vec![("Content-Type".into(), "text/plain".into())],
            body:        b"UUID not found".to_vec(),
            kind:        OutKind::Bytes,
        };
    }
    let key = format!("http-cache:{uuid}");
    if q.get("dispatch").map(String::as_str) == Some("state") {
        let taken = state
            .stash
            .lock()
            .ok()
            .and_then(|mut stash| stash.remove(&key))
            .unwrap_or(serde_json::json!([]));
        return HttpOut::ok(
            serde_json::to_vec(&taken).unwrap_or_else(|_| b"[]".to_vec()),
            "text/plain",
        );
    }
    let requests = header_get(headers, "test-requests")
        .and_then(b64decode)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or(serde_json::json!([]));
    let mut server_state = state
        .stash
        .lock()
        .ok()
        .and_then(|stash| stash.get(&key).cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let config = requests
        .as_array()
        .and_then(|items| items.get(server_state.len()))
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let mut noted = serde_json::Map::new();
    let mut out = base;
    if let Some(response_headers) = config.get("response_headers").and_then(|v| v.as_array()) {
        for header in response_headers {
            let Some(pair) = header.as_array() else {
                continue;
            };
            let name = pair.first().and_then(|v| v.as_str()).unwrap_or("");
            let mut value = match pair.get(1) {
                Some(serde_json::Value::Number(number)) => http_date(number.as_i64().unwrap_or(0)),
                Some(serde_json::Value::String(text)) => text.clone(),
                _ => String::new(),
            };
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "location" | "content-location"
            ) {
                let host = header_get(headers, "host").unwrap_or("localhost");
                let request_url = if query.is_empty() {
                    format!("http://{host}{path}")
                } else {
                    format!("http://{host}{path}?{query}")
                };
                value = if value.is_empty() {
                    request_url
                } else {
                    format!("{request_url}&target={value}")
                };
            }
            out.push((name.to_string(), value.clone()));
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "content-type" | "access-control-allow-origin" | "last-modified" | "etag"
            ) {
                noted.insert(name.to_ascii_lowercase(), serde_json::Value::String(value));
            }
        }
    }
    if !noted.contains_key("access-control-allow-origin") {
        out.push(("Access-Control-Allow-Origin".into(), "*".into()));
    }
    if !noted.contains_key("content-type") {
        out.push(("Content-Type".into(), "text/plain".into()));
    }
    let mut req_headers = serde_json::Map::new();
    for (name, value) in headers {
        req_headers.insert(
            name.to_ascii_lowercase(),
            serde_json::Value::String(value.clone()),
        );
    }
    server_state.push(serde_json::json!({
        "request_method": method,
        "request_headers": req_headers,
        "response_headers": noted,
    }));
    out.push((
        "Server-Request-Count".into(),
        server_state.len().to_string(),
    ));
    if let Ok(mut stash) = state.stash.lock() {
        stash.insert(key, serde_json::Value::Array(server_state));
    }
    let (code, phrase) = config
        .get("response_status")
        .and_then(|v| v.as_array())
        .map(|pair| {
            (
                pair.first().and_then(|v| v.as_u64()).unwrap_or(200) as u16,
                pair.get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("OK")
                    .to_string(),
            )
        })
        .unwrap_or((200, "OK".into()));
    let body = if matches!(code, 204 | 304) {
        Vec::new()
    } else {
        config
            .get("response_body")
            .and_then(|v| v.as_str())
            .unwrap_or(&uuid)
            .as_bytes()
            .to_vec()
    };
    HttpOut {
        status: code,
        status_text: phrase,
        headers: out,
        body,
        kind: OutKind::Bytes,
    }
}

fn guess_type(path: &str) -> &'static str {
    if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".js") {
        "text/javascript"
    } else if path.ends_with(".txt") {
        "text/plain"
    } else if path.ends_with(".png") {
        "image/png"
    } else {
        "application/octet-stream"
    }
}

fn load_sidecar(path: &Path) -> Vec<(String, String)> {
    let Some(text) = fs::read_to_string(path.with_extension(format!(
        "{}.headers",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    )))
    .ok()
    .or_else(|| {
        let name = path.file_name()?.to_str()?;
        fs::read_to_string(path.with_file_name(format!("{name}.headers"))).ok()
    }) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn synthetic_xhr(path: &str) -> Option<HttpOut> {
    let headers = match path {
        "/xhr/resources/header-content-length.asis" => {
            vec![("Content-Length".into(), "0".into())]
        }
        "/xhr/resources/header-content-length-twice.asis" => {
            vec![
                ("Content-Length".into(), "0".into()),
                ("Content-Length".into(), "0".into()),
            ]
        }
        "/xhr/resources/headers-double-empty.asis" => {
            vec![
                ("double-trouble".into(), String::new()),
                ("double-trouble".into(), String::new()),
            ]
        }
        "/xhr/resources/headers-basic.asis" => {
            vec![
                ("foo-test".into(), "1".into()),
                ("foo-test".into(), "2".into()),
                ("foo-test".into(), "3".into()),
            ]
        }
        "/xhr/resources/headers-some-are-empty.asis" => {
            vec![
                ("heya".into(), String::new()),
                ("heya".into(), "\u{000B}\u{000C}".into()),
                ("heya".into(), "1".into()),
                ("heya".into(), String::new()),
                ("heya".into(), String::new()),
                ("heya".into(), "2".into()),
            ]
        }
        "/xhr/resources/headers-www-authenticate.asis" => {
            vec![
                ("www-authenticate".into(), "1".into()),
                ("www-authenticate".into(), "2".into()),
                ("www-authenticate".into(), "3".into()),
                ("www-authenticate".into(), "4".into()),
            ]
        }
        _ => return None,
    };
    Some(HttpOut {
        status: 200,
        status_text: "OK".into(),
        headers,
        body: Vec::new(),
        kind: OutKind::Bytes,
    })
}

fn handle_static(state: &HttpState, path: &str) -> Option<HttpOut> {
    let relative = path.trim_start_matches('/');
    let file = state.root.join(relative);
    if !file.is_file() {
        if path == "/xhr/resources/utf16-bom.json" {
            return Some(HttpOut {
                status:      200,
                status_text: "OK".into(),
                headers:     vec![("Content-Type".into(), "application/json".into())],
                body:        [0xff, 0xfe]
                    .into_iter()
                    .chain("{\"a\":1}".encode_utf16().flat_map(|u| u.to_le_bytes()))
                    .collect(),
                kind:        OutKind::Bytes,
            });
        }
        return synthetic_xhr(path);
    }
    if path.ends_with(".asis") {
        return Some(HttpOut {
            status:      200,
            status_text: "OK".into(),
            headers:     vec![("Content-Type".into(), "application/octet-stream".into())],
            body:        fs::read(&file).ok()?,
            kind:        OutKind::Bytes,
        });
    }
    let mut headers = load_sidecar(&file);
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".into(), guess_type(path).into()));
    }
    Some(HttpOut {
        status: 200,
        status_text: "OK".into(),
        headers,
        body: fs::read(&file).ok()?,
        kind: OutKind::Bytes,
    })
}

fn dispatch(
    state: &HttpState, method: &str, target: &str, headers: &HashMap<String, String>, body: &[u8],
) -> HttpOut {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut outgoing = handle_py(state, method, path, query, headers, body)
        .or_else(|| handle_static(state, path))
        .unwrap_or_else(|| {
            HttpOut {
                status:      404,
                status_text: "Not Found".into(),
                headers:     vec![("Content-Type".into(), "text/plain".into())],
                body:        format!("missing {path}").into_bytes(),
                kind:        OutKind::Bytes,
            }
        });
    apply_pipes(query, &mut outgoing.headers);
    outgoing
}

async fn read_more(
    stream: &mut tokio::net::TcpStream, buf: &mut Vec<u8>, need: usize,
) -> io::Result<()> {
    let mut tmp = [0u8; 4096];
    while buf.len() < need {
        stream.readable().await?;
        match stream.try_read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn write_all(stream: &mut tokio::net::TcpStream, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        stream.writable().await?;
        match stream.try_write(data) {
            Ok(0) => break,
            Ok(n) => data = &data[n..],
            Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn read_http(
    stream: &mut tokio::net::TcpStream,
) -> io::Result<(String, String, HashMap<String, String>, Vec<u8>)> {
    let mut buf = Vec::new();
    loop {
        let before = buf.len();
        read_more(stream, &mut buf, before + 1).await?;
        if buf.len() == before {
            break;
        }
        if let Some(at) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let head: String = buf[..at].iter().map(|byte| *byte as char).collect();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or("");
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or("GET").to_string();
            let target = parts.next().unwrap_or("/").to_string();
            let mut headers = HashMap::new();
            for line in lines {
                if let Some((name, value)) = line.split_once(':') {
                    let key = name.trim().to_ascii_lowercase();
                    let raw = value.trim();
                    let value = if raw.bytes().all(|byte| byte.is_ascii()) {
                        raw.to_string()
                    } else if let Ok(utf8) = {
                        let latin1: Vec<u8> = raw.chars().map(|ch| ch as u8).collect();
                        String::from_utf8(latin1)
                    } {
                        utf8
                    } else {
                        raw.to_string()
                    };
                    headers
                        .entry(key)
                        .and_modify(|existing: &mut String| {
                            existing.push_str(", ");
                            existing.push_str(&value);
                        })
                        .or_insert(value);
                }
            }
            let rest = buf[at + 4..].to_vec();
            let length = headers
                .get("content-length")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = rest;
            if body.len() < length {
                read_more(stream, &mut body, length).await?;
            }
            body.truncate(length);
            return Ok((method, target, headers, body));
        }
        if buf.len() > 1024 * 1024 {
            break;
        }
    }
    Err(io::Error::new(
        ErrorKind::UnexpectedEof,
        "incomplete HTTP request",
    ))
}

fn format_status(outgoing: &HttpOut) -> Vec<u8> {
    let mut bytes =
        format!("HTTP/1.1 {} {}\r\n", outgoing.status, outgoing.status_text).into_bytes();
    let mut has_length = false;
    for (name, value) in &outgoing.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_length = true;
        }
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(b": ");
        for ch in value.chars() {
            let code = ch as u32;
            if code <= 0xff {
                bytes.push(code as u8);
            } else {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
        bytes.extend_from_slice(b"\r\n");
    }
    if matches!(outgoing.kind, OutKind::Bytes) && !has_length {
        bytes.extend_from_slice(format!("Content-Length: {}\r\n", outgoing.body.len()).as_bytes());
    }
    bytes.extend_from_slice(b"Connection: close\r\n\r\n");
    if matches!(outgoing.kind, OutKind::Bytes) {
        bytes.extend_from_slice(&outgoing.body);
    }
    bytes
}

async fn write_outgoing(
    stream: &mut tokio::net::TcpStream, outgoing: HttpOut, state: &HttpState, path: &str,
) -> io::Result<()> {
    match outgoing.kind {
        OutKind::Raw => write_all(stream, &outgoing.body).await,
        OutKind::Bytes => write_all(stream, &format_status(&outgoing)).await,
        OutKind::Huge => {
            write_all(stream, &format_status(&outgoing)).await?;
            let chunk = vec![0u8; 1024 * 1024];
            for _ in 0..64 {
                write_all(stream, &chunk).await?;
            }
            Ok(())
        }
        OutKind::Trickle { delay_ms, count } => {
            write_all(stream, &format_status(&outgoing)).await?;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            for _ in 0..count {
                write_all(stream, b"d\r\nTEST_TRICKLE\n\r\n").await?;
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            write_all(stream, b"0\r\n\r\n").await
        }
        OutKind::Infinite {
            ref state_key,
            ref abort_key,
        } => {
            if !state_key.is_empty()
                && let Ok(mut stash) = state.stash.lock()
            {
                stash.insert(
                    stash_key(path, state_key),
                    serde_json::Value::String("open".into()),
                );
            }
            write_all(stream, &format_status(&outgoing)).await?;
            write_all(stream, &[b'.'; 2048]).await?;
            loop {
                if !abort_key.is_empty()
                    && let Ok(mut stash) = state.stash.lock()
                    && stash.remove(&stash_key(path, &abort_key)).is_some()
                {
                    break;
                }
                if write_all(stream, b".").await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if !state_key.is_empty()
                && let Ok(mut stash) = state.stash.lock()
            {
                stash.insert(
                    stash_key(path, &state_key),
                    serde_json::Value::String("closed".into()),
                );
            }
            Ok(())
        }
        OutKind::BadChunk { delay_ms, count } => {
            write_all(stream, &format_status(&outgoing)).await?;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            for _ in 0..count {
                write_all(stream, b"a\r\nTEST_CHUNK\r\n").await?;
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            write_all(stream, b"garbage").await
        }
    }
}

async fn serve_http(listener: tokio::net::TcpListener, state: Arc<HttpState>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            break;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let Ok((method, target, headers, body)) = read_http(&mut stream).await else {
                return;
            };
            let outgoing = dispatch(&state, &method, &target, &headers, &body);
            let path = target
                .split_once('?')
                .map_or(target.as_str(), |(path, _)| path);
            let _ = write_outgoing(&mut stream, outgoing, &state, path).await;
        });
    }
}

async fn bind_http(root: PathBuf) -> Result<(u16, u16), String> {
    let first = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("wpt http bind failed: {error}"))?;
    let second = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("wpt http bind2 failed: {error}"))?;
    let http0 = first
        .local_addr()
        .map_err(|error| format!("wpt http addr failed: {error}"))?
        .port();
    let http1 = second
        .local_addr()
        .map_err(|error| format!("wpt http addr2 failed: {error}"))?
        .port();
    let state = Arc::new(HttpState {
        root,
        stash: Mutex::new(HashMap::new()),
    });
    let replica = Arc::clone(&state);
    tokio::spawn(serve_http(first, state));
    tokio::spawn(serve_http(second, replica));
    Ok((http0, http1))
}

async fn run_case(
    wpt: &Path, testharness_src: &str, case_path: &Path, relative: &str,
) -> Result<Completion, Failed> {
    let ws = if relative.starts_with("websockets/") {
        bind_echo().await?
    } else {
        0
    };
    let needs_http = relative.starts_with("fetch/") || relative.starts_with("url/");
    let (http0, http1) = if needs_http {
        bind_http(wpt.to_path_buf()).await?
    } else {
        (0, 0)
    };
    let ports = WptPorts { http0, http1, ws };
    let host = if needs_http { "localhost" } else { "127.0.0.1" };
    let body =
        fs::read_to_string(case_path).map_err(|error| format!("{relative}: read: {error}"))?;
    let expanded = rewrite_tokens(&expand_source(wpt, case_path, &body), ports, host);
    let wait_ms = timeout_ms(&body);
    let mut bundle = String::from(BOOTSTRAP);
    bundle.push_str(testharness_src);
    bundle.push('\n');
    if needs_http {
        bundle.push_str(&location_js(&format!("http://localhost:{http0}"), relative));
    }
    bundle.push_str(ADAPTER);
    bundle.push_str("\nawait Promise.resolve();\n");
    if !needs_http {
        bundle.push_str("globalThis.location.pathname = \"");
        bundle.push_str(&relative.replace('\\', "/"));
        bundle.push_str("\";\n");
    }
    bundle.push_str(&expanded);
    bundle.push_str("\ndone();\n");
    bundle.push_str("\nawait __denWptWait(");
    bundle.push_str(&wait_ms.to_string());
    bundle.push_str(");\n");
    let engine = Engine::new().await;
    let filename = relative.to_owned();
    let encoded = engine
        .context
        .async_with(async move |ctx| {
            let mut options = EvalOptions::default();
            options.global = true;
            options.promise = true;
            // testharness.js is browser sloppy-mode; Engine::eval_prepared is always
            // strict.
            options.strict = false;
            options.filename = Some(filename);
            let run = async {
                let promise: Promise = ctx.eval_with_options(bundle.as_str(), options)?;
                let object = promise.into_future::<Object>().await?;
                object.get::<_, String>("value")
            };
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await;
    engine.shutdown().await;
    match encoded {
        Ok(text) => {
            let report = parse_report(&text);
            match classify_verdict(&report) {
                Verdict::Pass => Ok(Completion::Completed),
                Verdict::Skip => Ok(Completion::ignored_with("testharness precondition")),
                Verdict::Fail => Err(fail_detail(relative, &report).into()),
            }
        }
        Err(error) => Err(format!("{relative}: {error}").into()),
    }
}

fn run_case_caught(
    wpt: &Path, testharness_src: &str, case_path: &Path, relative: &str,
) -> Result<Completion, Failed> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    match panic::catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(run_case(wpt, testharness_src, case_path, relative))
    })) {
        Ok(result) => result,
        Err(payload) => Err(format!("host panic: {}", panic_message(payload.as_ref())).into()),
    }
}

fn harness_classify() -> Result<(), Failed> {
    assert!(
        wpt_root().ends_with("vendor/wpt"),
        "wpt root is the vendored submodule"
    );
    assert_eq!(
        skip_reason("websockets/idlharness.any.js"),
        Some("needs-idlharness"),
        "idlharness needs the WPT IDL fixtures"
    );
    assert_eq!(
        skip_reason("websockets/constructor/001.html"),
        None,
        "constructor no-args is runnable without wpt serve"
    );
    assert_eq!(
        skip_reason("websockets/constructor/006.html"),
        Some("legacy-html"),
        "HTML constructor tests other than 001 stay skipped"
    );
    assert_eq!(
        skip_reason("FileAPI/url/url-format.any.js"),
        Some("needs-blob-url"),
        "blob: URLs are not walked yet"
    );
    assert_eq!(
        skip_reason("websockets/Send-data.any.js"),
        None,
        "live send is an official testharness file and is executed"
    );
    assert_eq!(
        skip_reason("url/javascript-urls.window.js"),
        Some("needs-document"),
        "url window tests need HTML navigation or <a>/<area>"
    );
    assert_eq!(
        rewrite_tokens(
            "ws://{{host}}:{{ports[ws][0]}}/echo",
            WptPorts {
                http0: 1,
                http1: 2,
                ws:    9,
            },
            "127.0.0.1",
        ),
        "ws://127.0.0.1:9/echo",
        "wpt tokens rewrite onto the echo listener"
    );
    assert_eq!(timeout_ms("// META: timeout=long\n"), 60_000, "long META");
    assert_eq!(timeout_ms("test(() => {});"), 15_000, "default timeout");
    let report = parse_report("HARNESS\t0\n0\tok\t\n");
    assert!(!report.timed_out, "sample report is not a timeout");
    assert_eq!(report.rows.len(), 1, "sample report has one row");
    assert!(
        matches!(classify_verdict(&report), Verdict::Pass),
        "a single PASS row is a file pass"
    );
    let html =
        "<script src=/resources/testharness.js></script><script>test(function(){});</script>";
    let expanded = expand_html(
        Path::new("/tmp"),
        Path::new("/tmp/constructor/001.html"),
        html,
    );
    assert!(
        expanded.contains("test(function(){})"),
        "inline HTML script is kept"
    );
    assert_eq!(
        script_src_attr("<script src=/resources/testharness.js>"),
        Some("/resources/testharness.js".to_owned()),
        "src= without quotes"
    );
    assert_eq!(
        resolve_script(
            Path::new("/wpt"),
            Path::new("/wpt/websockets/foo.any.js"),
            "/resources/testharness.js"
        ),
        None,
        "testharness.js is already loaded by the runner"
    );
    assert_eq!(
        fail_detail(
            "websockets/x.any.js",
            &parse_report("HARNESS\t0\n1\tcase\tboom\n")
        ),
        "websockets/x.any.js: case — boom",
        "first failing testharness case is the detail"
    );
    let payload = "AlreadyBorrowed".to_owned();
    assert_eq!(
        panic_message(&payload),
        "AlreadyBorrowed",
        "String panic payloads keep their message"
    );
    assert!(
        collect_cases(Path::new("/den-wpt-no-such-dir")).is_empty(),
        "missing tree walks to empty"
    );
    assert!(
        collect_official(Path::new("/den-wpt-no-such")).is_empty(),
        "missing official trees walk to empty"
    );
    let missing = run_case_caught(
        Path::new("/den-wpt-no-such"),
        "",
        Path::new("/den-wpt-no-such/x.any.js"),
        "x.any.js",
    );
    assert!(missing.is_err(), "missing official file is a fail");
    Ok(())
}

fn collect_official(wpt: &Path) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    for tree in WPT_TREES {
        let dir = wpt.join(tree);
        if dir.is_dir() {
            collected.extend(collect_cases(&dir));
        }
    }
    collected.sort();
    collected
}

fn main() {
    let mut tests = vec![Trial::ignorable_test("harness::classify", || {
        harness_classify().map(|()| Completion::Completed)
    })];

    let wpt = wpt_root();
    let testharness = wpt.join("resources").join("testharness.js");
    if !testharness.is_file() {
        tests.push(Trial::ignorable_test("vendor/wpt", || {
            Err("missing testharness.js; run `git submodule update --init vendor/wpt`".into())
        }));
        libtest_mimic::run(&Arguments::from_args(), tests).exit();
    }
    let testharness_src = match fs::read_to_string(&testharness) {
        Ok(text) => Arc::new(text),
        Err(error) => {
            tests.push(Trial::ignorable_test("vendor/wpt", move || {
                Err(format!("read testharness.js: {error}").into())
            }));
            libtest_mimic::run(&Arguments::from_args(), tests).exit();
        }
    };
    let wpt = Arc::new(wpt);
    let collected = collect_official(&wpt);
    if collected.is_empty() {
        tests.push(Trial::ignorable_test("vendor/wpt", || {
            Err("walker found no official testharness files under vendor/wpt".into())
        }));
    }
    for case_path in collected {
        let relative = relative_to(&wpt, &case_path);
        let ignored = skip_reason(&relative).is_some();
        let wpt = Arc::clone(&wpt);
        let testharness_src = Arc::clone(&testharness_src);
        tests.push(
            Trial::ignorable_test(relative.clone(), move || {
                run_case_caught(&wpt, &testharness_src, &case_path, &relative)
            })
            .with_ignored_flag(ignored),
        );
    }
    libtest_mimic::run(&Arguments::from_args(), tests).exit();
}
