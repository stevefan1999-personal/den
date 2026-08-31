use std::path::Path;

use super::{ImportMap, Mapping};

const BASE: &str = "den-import-map";

fn map(json: &str) -> ImportMap { ImportMap::parse(json, Path::new(BASE)).unwrap() }

fn at(path: &str) -> String {
    std::env::current_dir()
        .unwrap_or_else(|error| panic!("cannot resolve the test workspace: {error}"))
        .join(BASE)
        .join(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn exact_match_joins_a_relative_target() {
    let map = map(r#"{"imports":{"lib":"./lib.js"}}"#);
    assert_eq!(
        map.resolve("lib", &at("main.js")),
        Mapping::Target(at("lib.js"))
    );
}

#[test]
fn prefix_match_appends_the_remainder() {
    let map = map(r#"{"imports":{"lib/":"./vendor/"}}"#);
    assert_eq!(
        map.resolve("lib/util.js", &at("main.js")),
        Mapping::Target(at("vendor/util.js"))
    );
}

#[test]
fn longest_prefix_wins() {
    let map = map(r#"{"imports":{"lib/":"./a/","lib/sub/":"./b/"}}"#);
    assert_eq!(
        map.resolve("lib/sub/x.js", &at("main.js")),
        Mapping::Target(at("b/x.js"))
    );
}

#[test]
fn null_target_blocks_the_specifier() {
    let map = map(r#"{"imports":{"blocked":null}}"#);
    assert_eq!(map.resolve("blocked", &at("main.js")), Mapping::Blocked);
}

#[test]
fn unmatched_specifier_is_a_miss() {
    let map = map(r#"{"imports":{"lib":"./lib.js"}}"#);
    assert_eq!(map.resolve("other", &at("main.js")), Mapping::Miss);
}

#[test]
fn scopes_override_top_level_imports_for_matching_parents() {
    let map = map(include_str!(
        "../fixtures/import_map_engine/scopes/map.json"
    ));
    assert_eq!(
        map.resolve("pkg", &at("main.js")),
        Mapping::Target(at("top.js"))
    );
    assert_eq!(
        map.resolve("pkg", &at("nested/mod.js")),
        Mapping::Target(at("nested/inner.js"))
    );
}

#[test]
fn rejects_non_object_json() {
    assert!(ImportMap::parse("[]", Path::new(BASE)).is_err());
    assert!(ImportMap::parse("not json", Path::new(BASE)).is_err());
}
