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
async fn image_data_constructor_overloads_and_data_aliasing() -> eyre::Result<()> {
    run("image_data.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn image_data_constructor_reports_the_spec_errors() -> eyre::Result<()> {
    run("image_data_errors.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn image_bitmap_round_trips_and_detaches_on_close() -> eyre::Result<()> {
    run("bitmap.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn crop_rectangle_pads_out_of_bounds_with_transparent_black() -> eyre::Result<()> {
    run("crop.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_create_image_bitmap_option_is_honoured() -> eyre::Result<()> {
    run("options.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn den_canvas_module_exports_the_installed_globals() -> eyre::Result<()> {
    run("module.js").await
}
