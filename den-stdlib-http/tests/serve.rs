use std::{net::TcpStream, path::PathBuf, time::Duration};

use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::{Error, Function, function::Async};

fn field<'a>(
    mut value: &'a serde_json::Value, path: &[&str],
) -> eyre::Result<&'a serde_json::Value> {
    for segment in path {
        value = value
            .get(*segment)
            .ok_or_else(|| eyre::eyre!("missing HTTP result field {}", path.join(".")))?;
    }
    Ok(value)
}

#[tokio::test(flavor = "multi_thread")]
async fn same_realm_fetch_serves_port_zero_and_close_ends_the_event_loop() -> eyre::Result<()> {
    let engine = Engine::new().await;
    engine
        .context
        .with(|ctx| {
            let request = Function::new(
                ctx.clone(),
                Async(|url: String| {
                    async move {
                        let client = reqwest::Client::builder()
                            .http2_prior_knowledge()
                            .build()
                            .map_err(|error| {
                                Error::new_from_js_message(
                                    "HTTP/2 response",
                                    "string",
                                    error.to_string(),
                                )
                            })?;
                        let response = client.get(url).send().await.map_err(|error| {
                            Error::new_from_js_message(
                                "HTTP/2 response",
                                "string",
                                error.to_string(),
                            )
                        })?;
                        if response.version() != http::Version::HTTP_2 {
                            return Err(Error::new_from_js_message(
                                "HTTP response",
                                "HTTP/2 response",
                                "server did not select HTTP/2",
                            ));
                        }
                        response.text().await.map_err(|error| {
                            Error::new_from_js_message(
                                "HTTP/2 response",
                                "string",
                                error.to_string(),
                            )
                        })
                    }
                }),
            )?;
            ctx.globals().set("__denH2Fetch", request)
        })
        .await?;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/js/serve.js");
    let run = tokio::time::timeout(Duration::from_secs(10), engine.run_file(path)).await?;
    if run.is_err() {
        engine
            .context
            .with(|ctx| {
                let caught = ctx.catch();
                let message = caught
                    .as_object()
                    .and_then(|object| object.get::<_, String>("message").ok());
                let stack = caught
                    .as_object()
                    .and_then(|object| object.get::<_, String>("stack").ok());
                eprintln!("den:http fixture: {message:?}\n{stack:?}");
            })
            .await;
    }
    run?;
    let result = engine.eval::<String>("globalThis.__httpResult").await?;
    let result: serde_json::Value = serde_json::from_str(&result)?;

    eyre::ensure!(
        field(&result, &["body"])? == "POST /echo hello",
        "body mismatch"
    );
    for name in [
        "remote",
        "local",
        "bindError",
        "invalidDrain",
        "forcedDrain",
        "asyncDispose",
        "badTarget",
        "badGetBody",
        "methodMiss",
        "head",
        "handlerFailure",
        "authority",
        "http2",
        "framing",
        "signal",
        "oversizedRequest",
        "oversizedResponse",
        "streamingResponse",
        "importOnly",
    ] {
        eyre::ensure!(field(&result, &[name])? == true, "{name} was false");
    }
    eyre::ensure!(
        field(&result, &["pending", "requests"])? == 0,
        "requests pending"
    );
    eyre::ensure!(
        field(&result, &["pending", "connections"])? == 0,
        "connections pending"
    );
    eyre::ensure!(
        field(&result, &["slowBody"])? == "drained",
        "slow body mismatch"
    );

    let port = field(&result, &["port"])?
        .as_u64()
        .ok_or_else(|| eyre::eyre!("missing port"))? as u16;
    eyre::ensure!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "server still accepts connections after close"
    );
    engine.shutdown().await;
    Ok(())
}
