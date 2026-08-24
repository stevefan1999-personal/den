#![expect(clippy::expect_used, reason = "test harness panics are the report")]

use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt as _, FromJs, Module, Object, Promise,
    context::EvalOptions,
    loader::{BuiltinResolver, ModuleLoader},
};

async fn realm() -> (AsyncRuntime, AsyncContext) {
    let runtime = AsyncRuntime::new().expect("runtime");
    runtime
        .set_loader(
            BuiltinResolver::default().with_module("den:assert"),
            ModuleLoader::default().with_module("den:assert", den_stdlib_assert::js_assert),
        )
        .await;
    let context = AsyncContext::full(&runtime).await.expect("context");
    (runtime, context)
}

async fn eval_js<T>(source: &str) -> Result<T, String>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    let (_runtime, context) = realm().await;
    context
        .async_with(async |ctx| {
            let run = async {
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                ctx.eval_with_options::<Promise, _>(source, options)?
                    .into_future::<Object>()
                    .await?
                    .get("value")
            };
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await
}

async fn run(source: &str) -> Result<(), String> {
    eval_js::<bool>(&format!("{source}\ntrue")).await.map(|_| ())
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

#[tokio::test]
async fn equals_cases() { run(include_str!("cases/equals.js")).await.expect("equals"); }

#[tokio::test]
async fn throws_cases() { run(include_str!("cases/throws.js")).await.expect("throws"); }

#[tokio::test]
async fn match_cases() { run(include_str!("cases/match.js")).await.expect("match"); }

#[tokio::test]
async fn assert_equals_failure_message_snapshots() {
    let message = thrown_message(
        r#"
          const { assertEquals } = await import("den:assert");
          assertEquals(1, 2);
        "#,
    )
    .await;
    assert!(
        message.contains('1') && message.contains('2'),
        "{message}"
    );
    insta::assert_snapshot!(message);
}

#[tokio::test]
async fn evaluate_def_exports_the_jsr_names() {
    let runtime = AsyncRuntime::new().expect("runtime");
    let context = AsyncContext::full(&runtime).await.expect("context");
    let names: String = context
        .async_with(async |ctx| {
            let run = async {
                let (module, evaluated) = Module::evaluate_def::<den_stdlib_assert::js_assert, _>(
                    ctx.clone(),
                    "den:assert",
                )?;
                evaluated.into_future::<()>().await?;
                let namespace = module.namespace()?;
                let mut names = namespace
                    .keys::<String>()
                    .collect::<rquickjs::Result<Vec<_>>>()?;
                names.sort();
                Ok(names.join(","))
            };
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await
        .expect("exports");
    insta::assert_snapshot!(names);
}
