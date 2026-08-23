use rquickjs::{
    AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Object, Promise,
    context::EvalOptions,
};

/// Evaluate `source` in a fresh realm with `den:temporal` installed.
async fn eval<T>(source: &str) -> Result<T, String>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    let runtime = AsyncRuntime::new().expect("runtime");
    let context = AsyncContext::full(&runtime).await.expect("context");
    context
        .async_with(async |ctx| {
            let run = async {
                let (_module, evaluated) = Module::evaluate_def::<
                    den_stdlib_temporal::js_temporal,
                    _,
                >(ctx.clone(), "den:temporal")?;
                evaluated.into_future::<()>().await?;
                let mut options = EvalOptions::default();
                options.global = true;
                options.promise = true;
                options.strict = true;
                ctx.eval_with_options::<Promise, _>(source, options)?
                    .into_future::<Object>()
                    .await?
                    .get::<_, T>("value")
            };
            run.await.catch(&ctx).map_err(|error| error.to_string())
        })
        .await
}

#[tokio::test]
async fn now_instant_returns_an_instant() {
    let is_instant: bool = eval(
        r#"
          const instant = Temporal.Now.instant();
          instant instanceof Temporal.Instant
        "#,
    )
    .await
    .expect("eval");
    assert!(is_instant);
}

#[tokio::test]
async fn instant_from_unix_epoch_has_zero_nanoseconds() {
    let is_zero: bool = eval(
        r#"
          Temporal.Instant.from("1970-01-01T00:00:00Z").epochNanoseconds === 0n
        "#,
    )
    .await
    .expect("eval");
    assert!(is_zero);
}

#[tokio::test]
async fn plain_date_from_iso_string_exposes_year() {
    let year: i32 = eval(r#"Temporal.PlainDate.from("2025-03-03").year"#)
        .await
        .expect("eval");
    assert_eq!(year, 2025);
}

#[tokio::test]
async fn duration_constructor_and_to_string() {
    let text: String = eval(r#"new Temporal.Duration(1, 2, 3, 4).toString()"#)
        .await
        .expect("eval");
    assert_eq!(text, "P1Y2M3W4D");
}

#[tokio::test]
async fn instant_value_of_throws() {
    let threw: bool = eval(
        r#"
          let threw = false;
          try { new Temporal.Instant(0n).valueOf(); } catch (error) {
            threw = error instanceof TypeError;
          }
          threw
        "#,
    )
    .await
    .expect("eval");
    assert!(threw);
}

#[tokio::test]
async fn instant_constructor_requires_new() {
    let name: String = eval(
        r#"
          let name = "no throw";
          try { Temporal.Instant(0n); } catch (error) { name = error.name; }
          name
        "#,
    )
    .await
    .expect("eval");
    assert_eq!(name, "TypeError");
}

#[tokio::test]
async fn duration_from_property_bag() {
    let days: i32 = eval(r#"Temporal.Duration.from({ days: 5, hours: 2 }).days"#)
        .await
        .expect("eval");
    assert_eq!(days, 5);
}
