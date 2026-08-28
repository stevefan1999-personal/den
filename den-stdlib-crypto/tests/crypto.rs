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
async fn crypto_random_uuid_has_the_version_4_shape() -> eyre::Result<()> { run("uuid.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn crypto_get_random_values_fills_the_array_in_place() -> eyre::Result<()> {
    run("random_values.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn crypto_subtle_digest_sha256_of_abc_matches_the_well_known_hex() -> eyre::Result<()> {
    run("digest.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn digest_of_abc_matches_the_well_known_hexes() -> eyre::Result<()> {
    run("digest_vectors.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn digest_rejects_an_unknown_algorithm_with_not_supported_error() -> eyre::Result<()> {
    run("digest_unknown.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn digest_accepts_algorithm_objects_and_buffer_source_views() -> eyre::Result<()> {
    run("digest_views.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_random_values_accepts_wider_typed_arrays() -> eyre::Result<()> {
    run("random_values_types.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_random_values_fills_only_the_typed_array_view() -> eyre::Result<()> {
    run("random_values_offset.js").await
}
