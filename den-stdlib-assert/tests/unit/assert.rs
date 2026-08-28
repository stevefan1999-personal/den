use super::assert_insta_snapshot;

#[test]
fn snapshot_names_are_validated() {
    assert!(
        assert_insta_snapshot("../escape", "value").is_err(),
        "path-like names must be rejected"
    );
}
