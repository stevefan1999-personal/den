use super::{
    collect_http_quoted_string, parse, parse_mime, percent_decode, quoted_string_tokens_only,
    serialize_mime, skip_http_whitespace, strip_base64_flag,
};

#[test]
fn wpt_data_urls_processor() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/wpt/fetch/data-urls/resources/data-urls.json");
    let Ok(source) = std::fs::read_to_string(&path) else {
        eprintln!("skipping WPT data-URL corpus; {} is absent", path.display());
        return;
    };
    let data: serde_json::Value = serde_json::from_str(&source).expect("data-urls.json");
    let mut failures = Vec::new();
    for case in data.as_array().expect("array") {
        let row = case.as_array().expect("row");
        let input = row
            .first()
            .and_then(serde_json::Value::as_str)
            .expect("input");
        let expected_mime = row.get(1).and_then(serde_json::Value::as_str);
        let got = parse(input);
        match (expected_mime, got) {
            (None, None) => {}
            (Some(mime), Some(data)) => {
                let expected_body: Vec<u8> = row
                    .get(2)
                    .and_then(serde_json::Value::as_array)
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
    assert_eq!(
        strip_base64_flag(";charset=x;base64").as_deref(),
        Some(";charset=x")
    );
    assert_eq!(strip_base64_flag(";base64;").as_deref(), None);
    assert_eq!(percent_decode(b"%FF"), vec![255]);
    assert_eq!(percent_decode(b"X"), vec![b'X']);
    assert_eq!(
        base64_simd::forgiving_decode_to_vec(b"WA").ok().as_deref(),
        Some(b"X" as &[u8])
    );
    assert_eq!(
        base64_simd::forgiving_decode_to_vec(b"W A").ok().as_deref(),
        Some(b"X" as &[u8])
    );
    assert_eq!(base64_simd::forgiving_decode_to_vec(b"ab").unwrap(), [0x69]);
    assert_eq!(base64_simd::forgiving_decode_to_vec(b"ab==").unwrap(), [
        0x69
    ]);
    assert!(base64_simd::forgiving_decode_to_vec(b"abcde").is_err());
    assert!(base64_simd::forgiving_decode_to_vec(b"a$").is_err());
    assert!(quoted_string_tokens_only(" x"));
    assert!(!quoted_string_tokens_only("\u{0008}"));
    assert_eq!(skip_http_whitespace("  x", 0), 2);
    assert_eq!(skip_http_whitespace("x", 0), 0);
    assert_eq!(collect_http_quoted_string("\"x\"", 0).0, "x");
    assert_eq!(collect_http_quoted_string("\"", 0).0, "");
}
