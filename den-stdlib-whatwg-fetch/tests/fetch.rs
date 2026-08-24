use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;

fn case(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/js")
        .join(name)
}

async fn run(name: &str) -> eyre::Result<()> {
    Engine::new().await.run_file::<()>(case(name)).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_and_request_are_globals_and_constructible() -> eyre::Result<()> {
    run("headers.js").await?;
    run("globals.js").await?;
    run("request.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_get_and_post_against_a_local_listener() -> eyre::Result<()> {
    let server = den_stdlib_whatwg::local_http::serve(|incoming| {
        if incoming.path.ends_with("/post") {
            den_stdlib_whatwg::local_http::Outgoing::ok(incoming.body, "text/plain")
        } else {
            den_stdlib_whatwg::local_http::Outgoing::ok(
                b"{\"ok\":true}".to_vec(),
                "application/json",
            )
        }
    })
    .await;
    // SAFETY: test-only keys, set before the engine starts.
    unsafe {
        std::env::set_var("DEN_TEST_GET_URL", server.url("/get"));
        std::env::set_var("DEN_TEST_POST_URL", server.url("/post"));
    }
    run("fetch_http.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_aborts_when_the_signal_is_already_aborted() -> eyre::Result<()> {
    run("fetch_abort_already.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_aborts_an_in_flight_request() -> eyre::Result<()> {
    let server = den_stdlib_whatwg::local_http::serve(|_| {
        den_stdlib_whatwg::local_http::Outgoing {
            status:  200,
            headers: vec![],
            body:    Vec::new(),
            hang:    false,
            silent:  true,
        }
    })
    .await;
    // SAFETY: test-only key, set before the engine starts.
    unsafe {
        std::env::set_var("DEN_TEST_URL", server.url("/hang"));
    }
    run("fetch_abort_inflight.js").await
}
