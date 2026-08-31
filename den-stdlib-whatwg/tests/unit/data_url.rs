use super::parse;

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
