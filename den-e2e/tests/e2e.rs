//! Cross-crate Engine cases, one JS file per scenario (same shape as
//! `den-stdlib-wasm/tests/webassembly.rs`).

use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file(case(name)).await?;
    engine.shutdown().await;
    Ok(())
}

#[cfg(feature = "typescript")]
fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../examples")
        .join(name)
}

#[cfg(feature = "typescript")]
async fn run_example(name: &str) -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine.run_file(example(name)).await?;
    engine.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn https_esm_imports_resolve_through_fetch() -> eyre::Result<()> {
    run("https_esm.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_reads_a_scripted_tcp_http_server() -> eyre::Result<()> {
    run("fetch_http_server.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_fetches_https_and_posts_the_body() -> eyre::Result<()> {
    run("worker_fetch/main.js").await
}

#[cfg(feature = "wasm")]
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_instantiates_wasm_and_posts_the_export() -> eyre::Result<()> {
    run("worker_wasm/main.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn path_fs_and_text_write_a_note_on_disk() -> eyre::Result<()> {
    run("notes_on_disk.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_persists_rows_through_a_real_file() -> eyre::Result<()> {
    run("sqlite_file.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn gzip_round_trips_a_note_through_compression_streams() -> eyre::Result<()> {
    run("gzip_note.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn url_pattern_selects_a_real_https_json_document() -> eyre::Result<()> {
    run("url_pattern_fetch.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_stdout_is_written_to_a_file() -> eyre::Result<()> {
    run("spawn_writes_file.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_timer_posts_a_temporal_instant() -> eyre::Result<()> {
    run("worker_timer/main.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn json_import_attributes_feed_assert() -> eyre::Result<()> {
    run("import_json.js").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn a_typescript_entry_file_reaches_assert() -> eyre::Result<()> {
    run("typed_note.ts").await
}

#[tokio::test(flavor = "multi_thread")]
async fn random_bytes_round_trip_through_blob_and_file_reader() -> eyre::Result<()> {
    run("random_blob.js").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn tcp_listen_and_connect_echo_over_an_async_iterator() -> eyre::Result<()> {
    run_example("tcp-echo.ts").await
}

#[cfg(all(feature = "typescript", feature = "ring"))]
#[tokio::test(flavor = "multi_thread")]
async fn tls_listen_and_connect_echo_over_an_async_iterator() -> eyre::Result<()> {
    let certified = rcgen::generate_simple_self_signed(["localhost".to_string()])?;
    let cert = certified.cert.pem();
    let key = certified.signing_key.serialize_pem();
    let engine = Engine::new().await;
    engine
        .context
        .with(|ctx| {
            ctx.globals().set("__denTlsCert", cert)?;
            ctx.globals().set("__denTlsKey", key)
        })
        .await?;
    engine.run_file(example("tls-echo.ts")).await?;
    engine.shutdown().await;
    Ok(())
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn the_event_loop_drives_timers_events_and_stream_chunks() -> eyre::Result<()> {
    run_example("event-loop.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn kv_commits_and_rejects_a_stale_snapshot() -> eyre::Result<()> {
    run_example("kv.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn fs_and_path_write_an_atomic_note() -> eyre::Result<()> {
    run_example("fs-path.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn web_crypto_digests_and_mints_a_uuid() -> eyre::Result<()> {
    run_example("crypto.ts").await
}

#[cfg(all(feature = "typescript", feature = "wasm"))]
#[tokio::test(flavor = "multi_thread")]
async fn wasm_instantiates_from_a_bytes_import() -> eyre::Result<()> {
    run_example("wasm.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn import_attributes_load_json_text_and_bytes() -> eyre::Result<()> {
    run_example("import-attrs.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn webgpu_compute_runs_when_an_adapter_exists() -> eyre::Result<()> {
    run_example("webgpu-compute.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn canvas_image_data_becomes_a_bitmap() -> eyre::Result<()> {
    run_example("canvas.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn intl_and_temporal_print_a_calendar_date() -> eyre::Result<()> {
    run_example("intl-temporal.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn message_and_broadcast_channels_deliver() -> eyre::Result<()> {
    run_example("channels.ts").await
}

#[cfg(all(feature = "typescript", feature = "wasm"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_module_worker_instantiates_wasm() -> eyre::Result<()> {
    run_example("worker-compute.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn udp_listen_and_send_echo() -> eyre::Result<()> {
    run_example("udp-echo.ts").await
}

#[cfg(all(feature = "typescript", unix))]
#[tokio::test(flavor = "multi_thread")]
async fn unix_listen_and_connect_echo() -> eyre::Result<()> {
    run_example("unix-echo.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn spawn_echoes_and_lookup_resolves_localhost() -> eyre::Result<()> {
    run_example("spawn.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn fetch_talks_to_a_scripted_http_server() -> eyre::Result<()> {
    run_example("fetch-server.ts").await
}

#[cfg(feature = "typescript")]
#[tokio::test(flavor = "multi_thread")]
async fn gzip_round_trips_through_compression_streams() -> eyre::Result<()> {
    run_example("compress.ts").await
}

#[cfg(all(feature = "typescript", feature = "react"))]
#[tokio::test(flavor = "multi_thread")]
async fn tsx_and_sqlite_render_a_notes_page() -> eyre::Result<()> {
    run("tsx_sqlite_site.tsx").await
}

#[cfg(all(feature = "typescript", feature = "react"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_module_worker_renders_notes_from_sqlite() -> eyre::Result<()> {
    run("tsx_worker_notes.tsx").await
}

#[cfg(all(feature = "wasm", feature = "wasi"))]
mod quickjs_wasi {
    use color_eyre::eyre;

    use super::run;

    #[tokio::test(flavor = "multi_thread")]
    async fn the_namespace_exposes_eval_flags_and_intrinsics() -> eyre::Result<()> {
        run("quickjs_wasi/namespace.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn eval_code_keeps_bindings_across_calls() -> eyre::Result<()> {
        run("quickjs_wasi/eval.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn host_values_round_trip_through_handles_and_dump() -> eyre::Result<()> {
        run("quickjs_wasi/host_values.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn array_buffers_copy_across_the_boundary() -> eyre::Result<()> {
        run("quickjs_wasi/buffers.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn global_symbols_are_the_same_on_both_sides() -> eyre::Result<()> {
        run("quickjs_wasi/symbols.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn define_prop_honors_enumerable_flags() -> eyre::Result<()> {
        run("quickjs_wasi/define_prop.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_guest_function_can_be_called_from_the_host() -> eyre::Result<()> {
        run("quickjs_wasi/call_guest.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_function_can_be_called_from_the_guest() -> eyre::Result<()> {
        run("quickjs_wasi/host_functions.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn guest_exceptions_surface_as_js_exception() -> eyre::Result<()> {
        run("quickjs_wasi/errors.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn interrupt_handler_stops_an_infinite_loop() -> eyre::Result<()> {
        run("quickjs_wasi/interrupt.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn memory_limit_stops_an_allocating_loop() -> eyre::Result<()> {
        run("quickjs_wasi/memory.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn promises_drain_through_pending_jobs() -> eyre::Result<()> {
        run("quickjs_wasi/promises.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_module_loader_is_invoked_for_type_module() -> eyre::Result<()> {
        run("quickjs_wasi/modules.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bytecode_skips_parse_on_the_next_run() -> eyre::Result<()> {
        run("quickjs_wasi/bytecode.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_snapshot_restores_heap_and_host_callbacks() -> eyre::Result<()> {
        run("quickjs_wasi/snapshot.js").await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stripped_intrinsics_block_eval_and_proxy() -> eyre::Result<()> {
        run("quickjs_wasi/sandbox.js").await
    }
}
