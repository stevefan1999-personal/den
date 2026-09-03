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

        let numbers: Array =
            ctx.eval(r#"["%d %i %f %c leftover %z %%", 3.7, "4", 1.25, "css", "x"]"#)?;
        let numbers = numbers
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(numbers)?;
        eyre::ensure!(text.contains('3'), "{text}");
        eyre::ensure!(text.contains("%z"), "{text}");
        eyre::ensure!(text.contains('%'), "{text}");

        let empty = inspector.format_values(Vec::<Value>::new())?;
        eyre::ensure!(empty.is_empty(), "{empty}");

        let values: Array = ctx.eval(
            r#"[
              [1, { nested: true }],
              function named() {},
              Symbol("tag"),
              null,
              undefined,
              true
            ]"#,
        )?;
        let values = values
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(values)?;
        eyre::ensure!(
            text.contains("[ 1, [Object] ]") || text.contains("nested"),
            "{text}"
        );
        eyre::ensure!(text.contains("[Function: named]"), "{text}");
        eyre::ensure!(text.contains("Symbol(tag)"), "{text}");
        eyre::ensure!(text.contains("null"), "{text}");
        eyre::ensure!(text.contains("undefined"), "{text}");
        eyre::ensure!(text.contains("true"), "{text}");

        let inf: Array = ctx.eval(r#"["%d %d %d", Infinity, -Infinity, Number.NaN]"#)?;
        let inf = inf.iter::<Value>().collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(inf)?;
        eyre::ensure!(text.contains("Infinity"), "{text}");
        eyre::ensure!(text.contains("-Infinity"), "{text}");
        eyre::ensure!(text.contains("NaN"), "{text}");
        Ok(())
    })
}

// Byte-for-byte output of every primitive arm, so folding String/Int/Bool into
// one coercion cannot move a character.
#[test]
fn formatter_prints_primitives_verbatim_inside_containers() -> eyre::Result<()> {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> eyre::Result<()> {
        den_util::stack::install(&ctx)?;
        let inspector = Formatter::new(3);

        let values: Array = ctx.eval(r#"[[1, "two", true, -3, 1.5, 10n, null, undefined]]"#)?;
        let values = values
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(values)?;
        eyre::ensure!(
            text == "[ 1, two, true, -3, 1.5, 10n, null, undefined ]",
            "{text}"
        );

        let keyed: Array = ctx.eval(r#"[{ a: "x", 2: false, nested: { deep: [1] } }]"#)?;
        let keyed = keyed
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(keyed)?;
        eyre::ensure!(
            text == "{ 2: false, a: x, nested: { deep: [ 1 ] } }",
            "{text}"
        );

        // NaN keeps its name once the redundant is_nan() branch is gone;
        // Infinity must keep its branch (Rust prints f64::INFINITY as "inf").
        let numbers: Array = ctx.eval(r#"["%d|%i|%f|%d", NaN, "abc", "1.5x", 7]"#)?;
        let numbers = numbers
            .iter::<Value>()
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let text = inspector.format_values(numbers)?;
        eyre::ensure!(text == "NaN|NaN|1.5|7", "{text}");
        Ok(())
    })
}
