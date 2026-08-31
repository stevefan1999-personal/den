use std::path::PathBuf;

use color_eyre::eyre;
use den_core::engine::Engine;
use den_stdlib_console::Formatter;
use rquickjs::{Array, Context, Runtime, Value};

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
async fn console_logging_reaches_the_writer_without_throwing() -> eyre::Result<()> {
    run("console.js").await
}

#[tokio::test(flavor = "multi_thread")]
async fn console_debug_warn_and_error_are_callable() -> eyre::Result<()> { run("methods.js").await }

#[test]
fn formatter_handles_substitutions_cycles_and_error_stacks() -> eyre::Result<()> {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> eyre::Result<()> {
        den_util::stack::install(&ctx)?;
        let inspector = Formatter::new(3);

        let values: Array = ctx.eval(r#"["hello %s %d %%", "world", 4, { tail: true }]"#)?;
        let values = values
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(values)?;
        eyre::ensure!(text == "hello world 4 % { tail: true }", "{text}");

        let circular: Array = ctx.eval("const cycle = {}; cycle.self = cycle; [cycle]")?;
        let circular = circular
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(circular)?;
        eyre::ensure!(text == "{ self: [Circular] }", "{text}");

        let errors: Array =
            ctx.eval("[new TypeError('boom'), new DOMException('stopped', 'AbortError')]")?;
        let errors = errors
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(errors)?;
        eyre::ensure!(text.contains("TypeError: boom\n    at "), "{text}");
        eyre::ensure!(text.contains("AbortError: stopped\n    at "), "{text}");
        Ok(())
    })
}
