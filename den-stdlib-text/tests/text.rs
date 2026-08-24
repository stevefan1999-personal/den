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
async fn text_encoder_and_decoder_round_trip_multibyte_text() -> eyre::Result<()> {
    run(include_str!("js/text.js")).await
}
