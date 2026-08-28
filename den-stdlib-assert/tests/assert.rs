use std::path::PathBuf;

use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cases")
        .join(name)
}

async fn run(name: &str) -> Result<(), String> {
    Engine::new()
        .await
        .run_file(case(name))
        .await
        .map_err(|error| error.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn equals_cases() { run("equals.js").await.expect("equals"); }

#[tokio::test(flavor = "multi_thread")]
async fn throws_cases() { run("throws.js").await.expect("throws"); }

#[tokio::test(flavor = "multi_thread")]
async fn match_cases() { run("match.js").await.expect("match"); }

#[tokio::test(flavor = "multi_thread")]
async fn assert_equals_failure_message_snapshots() {
    run("equals_failure.js").await.expect("failure message");
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluate_def_exports_the_jsr_names() { run("export_names.js").await.expect("exports"); }

#[tokio::test(flavor = "multi_thread")]
async fn equality_edge_cases() { run("equality_edges.js").await.expect("equality edges"); }

#[tokio::test(flavor = "multi_thread")]
async fn assertion_failure_messages_snapshot() {
    run("failure_messages.js").await.expect("failure messages");
}

#[tokio::test(flavor = "multi_thread")]
async fn javascript_values_snapshot_through_insta() { run("snapshot.js").await.expect("snapshot"); }

#[tokio::test(flavor = "multi_thread")]
async fn fail_unimplemented_unreachable_and_is_error() { run("fail.js").await.expect("fail"); }

#[test]
fn insta_is_reexported_for_rust_consumers() { drop(den_stdlib_assert::insta::Settings::new()); }
