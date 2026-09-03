//! The REPL's process-level contract, none of which anything else observes:
//! history is a plain rustyline `FileHistory` file in the working directory
//! appended after every accepted line, and EOF prints the line still in flight
//! and then ends the process. A missing `load_history`/`append_history` call or
//! a changed `HISTORY_PATH` silently loses a user's history; a dropped print or
//! a Ctrl-D that never returns is just as silent.
//!
//! Gated behind the same `required-features` as `stack_traces.rs`: both spawn
//! the built `den` binary, which only works where the test host can execute it
//! (the cross/QEMU matrices in targets.yml never enable these features).

use std::{
    io::{self, Write as _},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use color_eyre::eyre;

type TestResult<T = ()> = eyre::Result<T>;

/// `App::EOF_GRACE` plus room for a loaded host.
const EOF_DEADLINE: Duration = Duration::from_secs(30);

/// One REPL session in `directory`: feed a single line, close stdin, wait for
/// the EOF exit, and hand back everything it printed.
fn session(directory: &Path, line: &str) -> TestResult<String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_den"))
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| eyre::eyre!("missing REPL stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| eyre::eyre!("missing REPL stdout"))?;
    writeln!(stdin, "{line}")?;
    stdin.flush()?;
    drop(stdin);

    // The pipe closes when the process dies, so draining it off-thread is also
    // the deadline: `wait_with_output` would hang the whole run instead of
    // failing this test on a REPL that never leaves EOF.
    let (printed_tx, printed_rx) = mpsc::channel();
    thread::spawn(move || printed_tx.send(io::read_to_string(stdout)));
    let printed = match printed_rx.recv_timeout(EOF_DEADLINE) {
        Ok(printed) => printed?,
        Err(_timeout) => {
            child.kill()?;
            eyre::bail!("REPL did not exit within {EOF_DEADLINE:?} of EOF");
        }
    };
    eyre::ensure!(child.wait()?.success(), "REPL did not exit 0 on EOF");
    Ok(printed)
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

#[test]
fn the_last_line_is_evaluated_and_printed_before_eof_exits() -> TestResult {
    let directory = tempfile::tempdir()?;
    let printed = session(directory.path(), "let x = 1")?;
    eyre::ensure!(
        printed.contains("undefined"),
        "the last line's result was dropped:\n{printed}"
    );
    Ok(())
}

#[test]
fn eof_exits_even_while_the_last_line_is_still_awaiting() -> TestResult {
    let directory = tempfile::tempdir()?;
    session(directory.path(), "await new Promise(() => {})")?;
    Ok(())
}
