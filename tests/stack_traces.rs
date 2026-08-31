use std::{
    fs,
    io::Write as _,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use color_eyre::eyre;
use oxc_sourcemap::{SourceMap, Token};

type TestResult<T = ()> = eyre::Result<T>;

fn run(source: &str, name: &str) -> TestResult<std::process::Output> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(name);
    fs::write(&path, source)?;
    Ok(Command::new(env!("CARGO_BIN_EXE_den")).arg(path).output()?)
}

#[test]
fn uncaught_typescript_uses_authored_locations_and_error_name() -> TestResult {
    let output = run(
        "type Noise = {\n  one: number;\n  two: string;\n};\n\nfunction inner(_value: Noise): \
         never {\n  throw new TypeError(\"mapped boom\");\n}\n\nfunction outer(): never {\n  \
         return inner({ one: 1, two: \"x\" });\n}\n\nouter();\n",
        "mapped.ts",
    )?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        !output.status.success(),
        "throwing program exited successfully"
    );
    eyre::ensure!(stdout.is_empty(), "fatal error leaked to stdout: {stdout}");
    eyre::ensure!(
        stderr.matches("TypeError: mapped boom").count() == 1,
        "{stderr}"
    );
    for location in [":7:13", ":11:10", ":14:1"] {
        eyre::ensure!(
            stderr.contains(location),
            "missing {location} in:\n{stderr}"
        );
    }
    eyre::ensure!(
        !stderr.contains(":2:12"),
        "generated location leaked:\n{stderr}"
    );
    Ok(())
}

#[test]
fn console_uses_real_streams_and_formats_error_stacks() -> TestResult {
    let output = run(
        "type Marker = { value: number };\nconsole.log(\"out\", { value: 1 \
         });\nconsole.debug(\"debug\", 2);\nfunction fail(): void {\n  console.error(new \
         TypeError(\"console boom\"));\n}\nfail();\n",
        "console.ts",
    )?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        output.status.success(),
        "console-only program failed:\n{stderr}"
    );
    eyre::ensure!(
        stdout.contains("out { value: 1 }"),
        "missing stdout object: {stdout}"
    );
    eyre::ensure!(
        stdout.contains("debug 2"),
        "debug was filtered out: {stdout}"
    );
    eyre::ensure!(
        !stdout.contains("console boom"),
        "console.error leaked to stdout: {stdout}"
    );
    eyre::ensure!(stderr.starts_with("TypeError: console boom\n"), "{stderr}");
    eyre::ensure!(
        stderr.contains("console.ts:5:"),
        "stack was not source mapped:\n{stderr}"
    );
    Ok(())
}

#[test]
fn unhandled_rejection_prints_one_mapped_stack() -> TestResult {
    let output = run(
        "type Marker = {\n  value: number;\n};\nconst marker: Marker = { value: 1 \
         };\nPromise.reject(new TypeError(`rejected ${marker.value}`));\n",
        "rejection.ts",
    )?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        stderr
            .matches("Uncaught (in promise) TypeError: rejected 1")
            .count()
            == 1,
        "rejection was missing or duplicated:\n{stderr}"
    );
    eyre::ensure!(
        stderr.contains("rejection.ts:5:"),
        "stack was not mapped:\n{stderr}"
    );
    Ok(())
}

#[test]
fn detached_timer_error_uses_the_common_mapped_reporter() -> TestResult {
    let output = run(
        "type Marker = {\n  value: number;\n};\nconst marker: Marker = { value: 1 \
         };\nsetTimeout(() => {\n  throw new RangeError(`timer ${marker.value}`);\n}, 0);\n",
        "timer.ts",
    )?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        stderr.matches("RangeError: timer 1").count() == 1,
        "{stderr}"
    );
    eyre::ensure!(
        stderr.contains("timer.ts:6:"),
        "stack was not mapped:\n{stderr}"
    );
    Ok(())
}

#[test]
fn inline_source_mapping_url_is_composed_with_the_runtime_transform() -> TestResult {
    let map = SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![std::borrow::Cow::Borrowed("original.ts")],
        vec![Some(std::borrow::Cow::Borrowed(
            "\n\n\n\n\n\n  throw new Error('from bundle');\n",
        ))],
        vec![Token::new(0, 0, 6, 2, Some(0), None)].into_boxed_slice(),
        None,
    );
    let source = format!(
        "function fail() {{ throw new Error('from bundle'); }} fail();\n//# sourceMappingURL={}",
        map.to_data_url()
    );
    let output = run(&source, "bundle.js")?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        !output.status.success(),
        "throwing bundle exited successfully"
    );
    eyre::ensure!(
        stderr.contains("original.ts:7:3"),
        "inline map was ignored:\n{stderr}"
    );
    Ok(())
}

#[test]
fn external_file_source_mapping_url_is_loaded_before_execution() -> TestResult {
    let map = SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![std::borrow::Cow::Borrowed("src/original.ts")],
        vec![None],
        vec![Token::new(0, 0, 8, 4, Some(0), None)].into_boxed_slice(),
        None,
    );
    let directory = tempfile::tempdir()?;
    let script = directory.path().join("bundle.js");
    fs::write(
        &script,
        "function fail() { throw new Error('external map'); } fail();\n//# \
         sourceMappingURL=bundle.js.map",
    )?;
    fs::write(directory.path().join("bundle.js.map"), map.to_json_string())?;
    let output = Command::new(env!("CARGO_BIN_EXE_den"))
        .arg(script)
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        !output.status.success(),
        "throwing bundle exited successfully"
    );
    eyre::ensure!(
        stderr.contains("/src/original.ts:9:5"),
        "external map was ignored:\n{stderr}"
    );
    Ok(())
}

#[test]
fn console_trace_starts_at_the_javascript_caller() -> TestResult {
    let output = run(
        "type Marker = { value: number };\n\nfunction traceMe(): void {\n  \
         console.trace('here');\n}\ntraceMe();\n",
        "trace.ts",
    )?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(output.status.success(), "console.trace failed:\n{stderr}");
    eyre::ensure!(stderr.starts_with("Trace: here\n"), "{stderr}");
    eyre::ensure!(
        stderr.contains("traceMe") && stderr.contains("trace.ts:4:"),
        "{stderr}"
    );
    eyre::ensure!(
        !stderr.contains("(native)"),
        "console.trace leaked its native frame:\n{stderr}"
    );
    Ok(())
}

#[test]
fn repl_exception_is_not_reprinted_as_an_unhandled_rejection() -> TestResult {
    let mut child = Command::new(env!("CARGO_BIN_EXE_den"))
        .arg("--repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| eyre::eyre!("missing REPL stdin"))?;
    writeln!(stdin, "throw new TypeError('repl boom')")?;
    thread::sleep(Duration::from_millis(200));
    drop(stdin);
    let output = child.wait_with_output()?;
    let stderr = String::from_utf8(output.stderr)?;
    eyre::ensure!(
        stderr.matches("TypeError: repl boom").count() == 1,
        "{stderr}"
    );
    eyre::ensure!(!stderr.contains("Uncaught (in promise)"), "{stderr}");
    Ok(())
}
