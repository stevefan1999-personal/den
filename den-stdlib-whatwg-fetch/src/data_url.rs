//! Fetch `data:` URL processor (`https://fetch.spec.whatwg.org/#data-url-processor`)
//! plus MIME Sniff parse/serialize (`https://mimesniff.spec.whatwg.org/`).

use indexmap::IndexMap;

pub(crate) struct DataUrl {
    pub mime: String,
    pub body: Vec<u8>,
}

pub(crate) fn parse(input: &str) -> Option<DataUrl> {
    let parsed = reqwest::Url::parse(input).ok()?;
    if parsed.scheme() != "data" {
        return None;
    }
    let mut serialized = parsed.clone();
    serialized.set_fragment(None);
    let rest = serialized.as_str().strip_prefix("data:")?;
    let comma = rest.find(',')?;
    let mut mime_type = rest[..comma]
        .trim_matches(|ch: char| ch.is_ascii_whitespace())
        .to_string();
    let encoded_body = &rest[comma + 1..];
    let mut body = percent_decode(encoded_body.as_bytes());
    if let Some(stripped) = strip_base64_flag(&mime_type) {
        mime_type = stripped;
        let text: String = body.iter().map(|byte| char::from(*byte)).collect();
        body = forgiving_base64(&text)?;
    }
    if mime_type.starts_with(';') {
        mime_type.insert_str(0, "text/plain");
    }
    let mime = match parse_mime(&mime_type) {
        Some(parsed) => serialize_mime(&parsed),
        None => "text/plain;charset=US-ASCII".to_string(),
    };
    Some(DataUrl { mime, body })
}

fn strip_base64_flag(mime: &str) -> Option<String> {
    if mime.len() < 6 || !mime[mime.len() - 6..].eq_ignore_ascii_case("base64") {
        return None;
    }
    let before = mime[..mime.len() - 6].trim_end_matches(' ');
    before.strip_suffix(';').map(str::to_string)
}

struct MimeType {
    essence_type: String,
    subtype:      String,
    parameters:   IndexMap<String, String>,
}

fn is_http_token(ch: char) -> bool {
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

fn is_http_quoted_string_token(ch: char) -> bool {
    matches!(ch, '\t' | '\u{20}'..='\u{7e}' | '\u{80}'..='\u{ff}')
}

fn is_http_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r' | ' ')
}

fn tokens_only(input: &str) -> bool {
    !input.is_empty() && input.chars().all(is_http_token)
}

fn quoted_string_tokens_only(input: &str) -> bool {
    input.chars().all(is_http_quoted_string_token)
}

fn parse_mime(input: &str) -> Option<MimeType> {
    let input = input.trim_matches(is_http_whitespace);
    let slash = input.find('/')?;
    let kind = &input[..slash];
    if !tokens_only(kind) {
        return None;
    }
    let after_slash = &input[slash + 1..];
    let subtype_rel = after_slash.find(';').unwrap_or(after_slash.len());
    let subtype = after_slash[..subtype_rel].trim_end_matches(is_http_whitespace);
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
        let name_end = input[pos..]
            .find(|ch| ch == ';' || ch == '=')
            .map_or(input.len(), |rel| pos + rel);
        let name = input[pos..name_end].to_ascii_lowercase();
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
            pos = match input[next..].find(';') {
                Some(rel) => next + rel,
                None => input.len(),
            };
            extracted
        } else {
            let value_end = input[pos..].find(';').map_or(input.len(), |rel| pos + rel);
            let value = input[pos..value_end].trim_end_matches(is_http_whitespace);
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
        let Some(ch) = input[pos..].chars().next() else {
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
            let Some(ch) = input[pos..].chars().next() else {
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

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' && index + 2 < input.len() {
            let hex = &input[index + 1..index + 3];
            if let Ok(text) = std::str::from_utf8(hex)
                && let Ok(byte) = u8::from_str_radix(text, 16)
            {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(input[index]);
        index += 1;
    }
    out
}

fn forgiving_base64(input: &str) -> Option<Vec<u8>> {
    let mut filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if filtered.len() % 4 == 0 {
        let trailing = filtered.iter().rev().take_while(|byte| **byte == b'=').count();
        if trailing > 2 {
            return None;
        }
        if trailing > 0 {
            filtered.truncate(filtered.len() - trailing);
        }
    }
    if filtered.iter().any(|byte| !is_base64_alphabet(*byte)) {
        return None;
    }
    if filtered.is_empty() {
        return Some(Vec::new());
    }
    if filtered.len() % 4 == 1 {
        return None;
    }
    while filtered.len() % 4 != 0 {
        filtered.push(b'=');
    }
    decode_base64(&filtered)
}

fn is_base64_alphabet(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/')
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
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
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for chunk in input.chunks(4) {
        if chunk.len() < 4 {
            return None;
        }
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

#[cfg(test)]
mod tests {
    use super::{
        collect_http_quoted_string, decode_base64, forgiving_base64, is_base64_alphabet,
        parse, parse_mime, percent_decode, quoted_string_tokens_only, serialize_mime,
        skip_http_whitespace, strip_base64_flag,
    };

    #[test]
    fn wpt_data_urls_processor() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../../vendor/wpt/fetch/data-urls/resources/data-urls.json"
        ))
        .expect("data-urls.json");
        let mut failures = Vec::new();
        for case in data.as_array().expect("array") {
            let row = case.as_array().expect("row");
            let input = row[0].as_str().expect("input");
            let expected_mime = row[1].as_str();
            let got = parse(input);
            match (expected_mime, got) {
                (None, None) => {}
                (Some(mime), Some(data)) => {
                    let expected_body: Vec<u8> = row[2]
                        .as_array()
                        .expect("body")
                        .iter()
                        .map(|number| number.as_u64().expect("byte") as u8)
                        .collect();
                    if data.mime != mime || data.body != expected_body {
                        failures.push(format!(
                            "{input:?}: mime {} vs {mime}, body {:?} vs {expected_body:?}",
                            data.mime, data.body
                        ));
                    }
                }
                (expected, got) => {
                    failures.push(format!(
                        "{input:?}: expected mime {expected:?}, got {:?}",
                        got.map(|data| data.mime)
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn mime_parse_keeps_leading_value_space_and_drops_name_space() {
        let spaced = parse_mime("text/plain;charset= x").expect("charset= x");
        assert_eq!(serialize_mime(&spaced), "text/plain;charset=\" x\"");
        assert!(parse_mime("text/plain;charset =x").is_some());
        assert_eq!(
            serialize_mime(&parse_mime("text/plain;charset =x").expect("name space")),
            "text/plain"
        );
        assert_eq!(strip_base64_flag(";charset=x;base64").as_deref(), Some(";charset=x"));
        assert_eq!(strip_base64_flag(";base64;").as_deref(), None);
        assert_eq!(percent_decode(b"%FF"), vec![255]);
        assert_eq!(percent_decode(b"X"), vec![b'X']);
        assert_eq!(forgiving_base64("WA").as_deref(), Some([b'X'].as_slice()));
        assert_eq!(forgiving_base64("W A").as_deref(), Some([b'X'].as_slice()));
        assert!(is_base64_alphabet(b'A'));
        assert!(!is_base64_alphabet(b'*'));
        assert_eq!(decode_base64(b"WA==").as_deref(), Some([b'X'].as_slice()));
        assert!(quoted_string_tokens_only(" x"));
        assert!(!quoted_string_tokens_only("\u{0008}"));
        assert_eq!(skip_http_whitespace("  x", 0), 2);
        assert_eq!(skip_http_whitespace("x", 0), 0);
        assert_eq!(collect_http_quoted_string("\"x\"", 0).0, "x");
        assert_eq!(collect_http_quoted_string("\"", 0).0, "");
    }
}
