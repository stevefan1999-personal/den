use color_eyre::eyre;
use den_core::engine::Engine;

async fn run(source: &str) -> eyre::Result<()> {
    let _: String = Engine::new()
        .await
        .eval(&format!("{source}\n\"ok\""))
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn base64_round_trips_through_btoa_and_atob() -> eyre::Result<()> {
    run(include_str!("js/base64.js")).await
}
