//! The WebAssembly JS API driven through the real [`Engine`].

use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/webassembly")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    Engine::new().await.run_file::<()>(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rust_built_buffer_survives_being_transferred() -> eyre::Result<()> {
    run("transfer.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn wat2wasm_assembles_a_module_with_the_wasm_magic() -> eyre::Result<()> {
    run("wat2wasm_magic.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn validate_answers_true_for_a_real_module_and_false_for_garbage() -> eyre::Result<()> {
    run("validate.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn instantiating_a_buffer_source_yields_a_module_instance_pair() -> eyre::Result<()> {
    run("instantiate_buffer.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn instantiating_a_compiled_module_yields_a_bare_instance() -> eyre::Result<()> {
    run("instantiate_module.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_imported_js_function_receives_the_wasm_arguments() -> eyre::Result<()> {
    run("import_js.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn i64_exports_and_parameters_cross_the_boundary_as_bigint() -> eyre::Result<()> {
    run("i64.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_shared_with_wasm_and_grow_detaches_the_previous_buffer() -> eyre::Result<()> {
    run("memory.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn table_and_global_are_readable_and_writable_from_js() -> eyre::Result<()> {
    run("table_global.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn each_webassembly_error_class_is_thrown_by_its_own_operation() -> eyre::Result<()> {
    run("errors.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trapping_export_should_reject_with_a_runtime_error() -> eyre::Result<()> {
    run("trap.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn module_statics_describe_imports_exports_and_custom_sections() -> eyre::Result<()> {
    run("custom_sections.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_exports_object_is_frozen_with_a_null_prototype() -> eyre::Result<()> {
    run("exports_frozen.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_namespace_exposes_every_spec_member() -> eyre::Result<()> { run("namespace.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn module_and_instance_are_real_constructors() -> eyre::Result<()> {
    run("constructors.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn compile_rejects_garbage_with_a_compile_error() -> eyre::Result<()> {
    run("compile.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn instantiate_streaming_accepts_a_duck_typed_response() -> eyre::Result<()> {
    run("instantiate_streaming.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exported_function_adapts_its_arguments_and_results() -> eyre::Result<()> {
    run("export_adapt.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn link_failures_are_type_or_link_errors() -> eyre::Result<()> { run("link_errors.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn grow_inside_wasm_and_aliased_wrappers_detach() -> eyre::Result<()> {
    run("grow_alias.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn wasi_imports_satisfies_preview1() -> eyre::Result<()> { run("wasi.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn a_tag_export_reaches_the_exports_object() -> eyre::Result<()> { run("tag.js").await }

#[tokio::test(flavor = "multi_thread")]
async fn imports_and_exports_are_observed_in_declaration_order() -> eyre::Result<()> {
    run("declaration_order.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_buffer_handed_to_script_survives_transfer_and_detach() -> eyre::Result<()> {
    run("transfer_all.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_thrown_by_an_import_reaches_the_caller_unchanged() -> eyre::Result<()> {
    run("import_throws.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trap_in_the_start_function_is_a_runtime_error() -> eyre::Result<()> {
    run("start_trap.js").await
}
