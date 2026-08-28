use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    Engine::new().await.run_file(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn text_encoder_and_decoder_round_trip_multibyte_text() -> eyre::Result<()> {
    run("text.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn encode_into_stops_on_a_character_boundary() -> eyre::Result<()> {
    run("encode_into.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fatal_decoder_throws_on_malformed_bytes() -> eyre::Result<()> { run("fatal.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn unknown_encoding_is_a_range_error() -> eyre::Result<()> {
    run("unknown_encoding.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn utf8_bom_is_stripped_unless_ignore_bom() -> eyre::Result<()> { run("bom.js").await }
