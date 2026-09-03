use std::{ffi::OsString, path::PathBuf};

use color_eyre::eyre;
use den_core::engine::Engine;

static GPU_ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvGuard(Option<OsString>);

impl EnvGuard {
    fn set(value: &str) -> Self {
        let previous = std::env::var_os("DEN_WEBGPU_BACKEND");
        // SAFETY: this test binary serializes every WebGPU test with GPU_ENV.
        unsafe { std::env::set_var("DEN_WEBGPU_BACKEND", value) };
        Self(previous)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: this test binary serializes every WebGPU test with GPU_ENV.
        unsafe {
            match self.0.take() {
                Some(value) => std::env::set_var("DEN_WEBGPU_BACKEND", value),
                None => std::env::remove_var("DEN_WEBGPU_BACKEND"),
            }
        }
    }
}

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    let engine = Engine::new().await;
    let result = engine.run_file(case(name)).await;
    engine.shutdown().await;
    result?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn constructors_and_navigator_gpu_are_installed_on_the_realm() -> eyre::Result<()> {
    let _lock = GPU_ENV.lock().await;
    let _env = EnvGuard::set("noop");
    run("globals.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn noop_adapter_exposes_buffer_api() -> eyre::Result<()> {
    let _lock = GPU_ENV.lock().await;
    let _env = EnvGuard::set("noop");
    run("noop.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_pipeline_round_trips_a_buffer_when_an_adapter_exists() -> eyre::Result<()> {
    let _lock = GPU_ENV.lock().await;
    run("compute.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_script_visible_surface_of_the_native_slice_is_stable() -> eyre::Result<()> {
    let _lock = GPU_ENV.lock().await;
    let _env = EnvGuard::set("noop");
    run("surface.js").await
}
