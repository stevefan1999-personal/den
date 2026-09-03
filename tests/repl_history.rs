//! REPL history is a plain rustyline `FileHistory` file in the working
//! directory, appended after every accepted line. Nothing else observes the
//! wiring: a missing `load_history`/`append_history` call, or a changed
//! `HISTORY_PATH`, silently loses a user's history instead of failing a build.
//!
//! Gated behind the same `required-features` as `stack_traces.rs`: both spawn
//! the built `den` binary, which only works where the test host can execute it
//! (the cross/QEMU matrices in targets.yml never enable these features).

use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use color_eyre::eyre;

type TestResult<T = ()> = eyre::Result<T>;

/// One REPL session in `directory`: feed a single line, close stdin, wait for
/// the EOF exit.
fn session(directory: &Path, line: &str) -> TestResult {
    let mut child = Command::new(env!("CARGO_BIN_EXE_den"))
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| eyre::eyre!("missing REPL stdin"))?;
    writeln!(stdin, "{line}")?;
    stdin.flush()?;
    drop(stdin);
    eyre::ensure!(child.wait()?.success(), "REPL did not exit 0 on EOF");
    Ok(())
}

#[test]
fn repl_history_accumulates_across_sessions_in_one_file() -> TestResult {
    const FIRST: &str = "globalThis.first = 1";
    const SECOND: &str = "globalThis.second = 2";

    let directory = tempfile::tempdir()?;
    session(directory.path(), FIRST)?;

    let path = directory.path().join("history.txt");
    let after_first = std::fs::read_to_string(&path)?;
    eyre::ensure!(
        after_first.contains(FIRST),
        "the first line was not appended before exit:\n{after_first}"
    );

    session(directory.path(), SECOND)?;
    let after_second = std::fs::read_to_string(&path)?;
    eyre::ensure!(
        after_second.contains(FIRST) && after_second.contains(SECOND),
        "the second session did not extend the first session's history:\n{after_second}"
    );
    eyre::ensure!(
        !directory.path().join("history.surrealkv").exists(),
        "the REPL still opens a SurrealKV store"
    );
    Ok(())
}
