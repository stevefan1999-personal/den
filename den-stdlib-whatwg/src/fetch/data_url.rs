//! Fetch `data:` URL processor (`https://fetch.spec.whatwg.org/#data-url-processor`)
//! plus MIME Sniff parse/serialize (`https://mimesniff.spec.whatwg.org/`).

use indexmap::IndexMap;
pub struct DataUrl {
    pub mime: String,
    pub body: Vec<u8>,
}

pub fn parse(input: &str) -> Option<DataUrl> {
    let parsed = reqwest::Url::parse(input).ok()?;
    if parsed.scheme() != "data" {
        return None;
    }
    let mut serialized = parsed;
    serialized.set_fragment(None);
    let rest = serialized.as_str().strip_prefix("data:")?;
    let (raw_mime, encoded_body) = rest.split_once(',')?;
    let mut mime_type = raw_mime
        .trim_matches(|ch: char| ch.is_ascii_whitespace())
        .to_string();
    let mut body = percent_decode(encoded_body.as_bytes());
    if let Some(stripped) = strip_base64_flag(&mime_type) {
        mime_type = stripped;
        let text: String = body.iter().map(|byte| char::from(*byte)).collect();
        body = base64_simd::forgiving_decode_to_vec(text.as_bytes()).ok()?;
    }
    if mime_type.starts_with(';') {
        mime_type.insert_str(0, "text/plain");
    }
    let mime = parse_mime(&mime_type).map_or_else(
        || "text/plain;charset=US-ASCII".to_string(),
        |parsed| serialize_mime(&parsed),
    );
    Some(DataUrl { mime, body })
}

fn strip_base64_flag(mime: &str) -> Option<String> {
    let split = mime.len().checked_sub(6)?;
    if !mime.get(split..)?.eq_ignore_ascii_case("base64") {
        return None;
    }
    let before = mime.get(..split)?.trim_end_matches(' ');
    before.strip_suffix(';').map(str::to_string)
}

struct MimeType {
    essence_type: String,
    subtype:      String,
    parameters:   IndexMap<String, String>,
}

const fn is_http_token(ch: char) -> bool {
    matches!(
        ch,
        '0'..='9'
            | 'a'..='z'
            | 'A'..='Z'
            | '!'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '*'
            | '+'
            | '-'
            | '.'
            | '^'
            | '_'
            | '`'
            | '|'
            | '~'
    )
}

const fn is_http_quoted_string_token(ch: char) -> bool {
    matches!(ch, '\t' | '\u{20}'..='\u{7e}' | '\u{80}'..='\u{ff}')
}

const fn is_http_whitespace(ch: char) -> bool { matches!(ch, '\t' | '\n' | '\r' | ' ') }

fn tokens_only(input: &str) -> bool { !input.is_empty() && input.chars().all(is_http_token) }

fn quoted_string_tokens_only(input: &str) -> bool { input.chars().all(is_http_quoted_string_token) }

fn parse_mime(input: &str) -> Option<MimeType> {
    let input = input.trim_matches(is_http_whitespace);
    let slash = input.find('/')?;
    let (kind, after_slash) = input.split_at(slash);
    if !tokens_only(kind) {
        return None;
    }
    let after_slash = after_slash.strip_prefix('/')?;
    let subtype_rel = after_slash.find(';').unwrap_or(after_slash.len());
    let subtype = after_slash
        .get(..subtype_rel)?
        .trim_end_matches(is_http_whitespace);
    if !tokens_only(subtype) {
        return None;
    }
    let mut pos = slash + 1 + subtype_rel;
    let mut parsed = MimeType {
        essence_type: kind.to_ascii_lowercase(),
        subtype:      subtype.to_ascii_lowercase(),
        parameters:   IndexMap::new(),
    };
    while pos < input.len() {
        if input.as_bytes().get(pos).copied() != Some(b';') {
            return None;
        }
        pos += 1;
        pos = skip_http_whitespace(input, pos);
        let name_end = input
            .get(pos..)?
            .find([';', '='])
            .map_or(input.len(), |rel| pos + rel);
        let name = input.get(pos..name_end)?.to_ascii_lowercase();
        pos = name_end;
        if input.as_bytes().get(pos).copied() == Some(b';') {
            continue;
        }
        if pos >= input.len() {
            break;
        }
        pos += 1;
        if pos >= input.len() {
            break;
        }
        let value = if input.as_bytes().get(pos).copied() == Some(b'"') {
            let (extracted, next) = collect_http_quoted_string(input, pos);
            pos = input
                .get(next..)
                .and_then(|tail| tail.find(';'))
                .map_or(input.len(), |rel| next + rel);
            extracted
        } else {
            let value_end = input
                .get(pos..)
                .and_then(|tail| tail.find(';'))
                .map_or(input.len(), |rel| pos + rel);
            let value = input
                .get(pos..value_end)?
                .trim_end_matches(is_http_whitespace);
            pos = value_end;
            if value.is_empty() {
                continue;
            }
            value.to_string()
        };
        if !name.is_empty()
            && name.chars().all(is_http_token)
            && quoted_string_tokens_only(&value)
            && !parsed.parameters.contains_key(&name)
        {
            parsed.parameters.insert(name, value);
        }
    }
    Some(parsed)
}

fn skip_http_whitespace(input: &str, mut pos: usize) -> usize {
    while pos < input.len() {
        let Some(ch) = input.get(pos..).and_then(|tail| tail.chars().next()) else {
            break;
        };
        if !is_http_whitespace(ch) {
            break;
        }
        pos += ch.len_utf8();
    }
    pos
}

fn collect_http_quoted_string(input: &str, mut pos: usize) -> (String, usize) {
    pos += 1;
    let mut value = String::new();
    loop {
        while pos < input.len() {
            let Some(ch) = input.get(pos..).and_then(|tail| tail.chars().next()) else {
                break;
            };
            if ch == '"' || ch == '\\' {
                break;
            }
            value.push(ch);
            pos += ch.len_utf8();
        }
        let Some(quote_or_backslash) = input.get(pos..).and_then(|rest| rest.chars().next()) else {
            break;
        };
        pos += quote_or_backslash.len_utf8();
        if quote_or_backslash == '"' {
            break;
        }
        let Some(escaped) = input.get(pos..).and_then(|rest| rest.chars().next()) else {
            value.push('\\');
            break;
        };
        value.push(escaped);
        pos += escaped.len_utf8();
    }
    (value, pos)
}

fn serialize_mime(mime: &MimeType) -> String {
    let mut out = format!("{}/{}", mime.essence_type, mime.subtype);
    for (name, value) in &mime.parameters {
        out.push(';');
        out.push_str(name);
        out.push('=');
        if value.is_empty() || !value.chars().all(is_http_token) {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            out.push('"');
            out.push_str(&escaped);
            out.push('"');
        } else {
            out.push_str(value);
        }
    }
    out
}

fn percent_decode(input: &[u8]) -> Vec<u8> { percent_encoding::percent_decode(input).collect() }

#[cfg(test)]
#[path = "../../tests/unit/data_url.rs"]
mod tests;
