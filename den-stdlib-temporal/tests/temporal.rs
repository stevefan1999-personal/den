use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;

async fn eval<T>(source: &str) -> eyre::Result<T>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    Ok(Engine::new().await.eval(source).await?)
}

async fn run(source: &str) -> eyre::Result<()> {
    let _: String = eval(&format!("{source}\n\"ok\"")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn temporal_is_installed_as_a_global() -> eyre::Result<()> {
    run(include_str!("js/now.js")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn now_instant_returns_an_instant() -> eyre::Result<()> {
    let is_instant: bool = eval(
        r#"
          const instant = Temporal.Now.instant();
          instant instanceof Temporal.Instant
        "#,
    )
    .await?;
    assert!(is_instant);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn instant_from_unix_epoch_has_zero_nanoseconds() -> eyre::Result<()> {
    let is_zero: bool = eval(
        r#"
          Temporal.Instant.from("1970-01-01T00:00:00Z").epochNanoseconds === 0n
        "#,
    )
    .await?;
    assert!(is_zero);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_date_from_iso_string_exposes_year() -> eyre::Result<()> {
    let year: i32 = eval(r#"Temporal.PlainDate.from("2025-03-03").year"#).await?;
    assert_eq!(year, 2025);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn duration_constructor_and_to_string() -> eyre::Result<()> {
    let text: String = eval(r#"new Temporal.Duration(1, 2, 3, 4).toString()"#).await?;
    assert_eq!(text, "P1Y2M3W4D");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn instant_value_of_throws() -> eyre::Result<()> {
    let threw: bool = eval(
        r#"
          let threw = false;
          try { new Temporal.Instant(0n).valueOf(); } catch (error) {
            threw = error instanceof TypeError;
          }
          threw
        "#,
    )
    .await?;
    assert!(threw);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn instant_constructor_requires_new() -> eyre::Result<()> {
    let name: String = eval(
        r#"
          let name = "no throw";
          try { Temporal.Instant(0n); } catch (error) { name = error.name; }
          name
        "#,
    )
    .await?;
    assert_eq!(name, "TypeError");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn duration_from_property_bag() -> eyre::Result<()> {
    let days: i32 = eval(r#"Temporal.Duration.from({ days: 5, hours: 2 }).days"#).await?;
    assert_eq!(days, 5);
    Ok(())
}
