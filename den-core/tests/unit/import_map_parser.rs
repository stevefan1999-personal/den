use std::path::Path;

use super::ImportMap;

const BASE: &str = "/tmp/den-import-map";

fn map(json: &str) -> ImportMap { ImportMap::parse(json, Path::new(BASE)).unwrap() }

#[test]
fn exact_match_joins_a_relative_target() {
    let map = map(r#"{"imports":{"lib":"./lib.js"}}"#);
    assert_eq!(
        map.resolve("lib", &format!("{BASE}/main.js")),
        Some(Some(format!("{BASE}/lib.js")))
    );
}

#[test]
fn prefix_match_appends_the_remainder() {
    let map = map(r#"{"imports":{"lib/":"./vendor/"}}"#);
    assert_eq!(
        map.resolve("lib/util.js", &format!("{BASE}/main.js")),
        Some(Some(format!("{BASE}/vendor/util.js")))
    );
}

#[test]
fn longest_prefix_wins() {
    let map = map(r#"{"imports":{"lib/":"./a/","lib/sub/":"./b/"}}"#);
    assert_eq!(
        map.resolve("lib/sub/x.js", &format!("{BASE}/main.js")),
        Some(Some(format!("{BASE}/b/x.js")))
    );
}

#[test]
fn null_target_blocks_the_specifier() {
    let map = map(r#"{"imports":{"blocked":null}}"#);
    assert_eq!(
        map.resolve("blocked", &format!("{BASE}/main.js")),
        Some(None)
    );
}

#[test]
fn unmatched_specifier_is_a_miss() {
    let map = map(r#"{"imports":{"lib":"./lib.js"}}"#);
    assert_eq!(map.resolve("other", &format!("{BASE}/main.js")), None);
}

#[test]
fn scopes_override_top_level_imports_for_matching_parents() {
    let map = map(include_str!(
        "../fixtures/import_map_engine/scopes/map.json"
    ));
    assert_eq!(
        map.resolve("pkg", &format!("{BASE}/main.js")),
        Some(Some(format!("{BASE}/top.js")))
    );
    assert_eq!(
        map.resolve("pkg", &format!("{BASE}/nested/mod.js")),
        Some(Some(format!("{BASE}/nested/inner.js")))
    );
}

#[test]
fn rejects_non_object_json() {
    assert!(ImportMap::parse("[]", Path::new(BASE)).is_err());
    assert!(ImportMap::parse("not json", Path::new(BASE)).is_err());
}
