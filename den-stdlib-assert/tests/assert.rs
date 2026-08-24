#![expect(clippy::expect_used, reason = "test harness panics are the report")]

use den_core::engine::Engine;
use rquickjs::FromJs;

async fn eval_js<T>(source: &str) -> Result<T, String>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    Engine::new()
        .await
        .eval(source)
        .await
        .map_err(|error| error.to_string())
}

async fn run(source: &str) -> Result<(), String> {
    eval_js::<bool>(&format!("{source}\ntrue"))
        .await
        .map(|_| ())
}

async fn thrown_message(source: &str) -> String {
    eval_js::<String>(&format!(
        r#"
          try {{
            {source}
            throw new Error("expected a throw");
          }} catch (error) {{
            String(error && error.message ? error.message : error);
          }}
        "#
    ))
    .await
    .expect("catch the assertion")
}

#[tokio::test(flavor = "multi_thread")]
async fn equals_cases() { run(include_str!("cases/equals.js")).await.expect("equals"); }

#[tokio::test(flavor = "multi_thread")]
async fn throws_cases() { run(include_str!("cases/throws.js")).await.expect("throws"); }

#[tokio::test(flavor = "multi_thread")]
async fn match_cases() { run(include_str!("cases/match.js")).await.expect("match"); }

#[tokio::test(flavor = "multi_thread")]
async fn assert_equals_failure_message_snapshots() {
    let message = thrown_message(
        r#"
          const { assertEquals } = await import("den:assert");
          assertEquals(1, 2);
        "#,
    )
    .await;
    assert!(message.contains('1') && message.contains('2'), "{message}");
    insta::assert_snapshot!(message);
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluate_def_exports_the_jsr_names() {
    let names: String = eval_js(
        r#"
          const ns = await import("den:assert");
          Object.keys(ns).sort().join(",")
        "#,
    )
    .await
    .expect("exports");
    insta::assert_snapshot!(names);
}
