//! Signal-death parity with Node/Deno/Bun: den subscribes to no signal, so the
//! kernel's default disposition kills it wherever it happens to be. Children
//! are spawned through `Command` and never through a shell — a non-interactive
//! shell's `&` hands the child `SIGINT = SIG_IGN` and every case here would
//! wedge instead of dying.
#![cfg(unix)]

use std::{
    io::{BufRead as _, BufReader},
    os::unix::process::ExitStatusExt as _,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, channel},
    thread,
    time::{Duration, Instant},
};

/// Every wait in this file is bounded by it, so a den that refuses to die fails
/// the assert instead of hanging the suite. Generous because these are
/// debug-build engine boots on a possibly loaded CI box.
const DEADLINE: Duration = Duration::from_secs(30);

/// A den child plus a drained view of its stdout. Draining matters: den's
/// `.pretty()` tracing formatter writes three physical lines per `console.log`,
/// and a child blocked on a full pipe never reaches the state under test.
struct Den {
    child:    Child,
    lines:    Receiver<String>,
    _scratch: tempfile::TempDir,
}

impl Den {
    fn start(flags: &[&str], script: &str) -> Self {
        let scratch = tempfile::tempdir().expect("scratch dir");
        let path = scratch.path().join("case.js");
        std::fs::write(&path, script).expect("write case");
        let mut child = Command::new(env!("CARGO_BIN_EXE_den"))
            .args(flags)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn den");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = channel();
        thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .try_for_each(|line| tx.send(line))
        });
        Self {
            child,
            lines,
            _scratch: scratch,
        }
    }

    fn wait_for_line(&self, needle: &str) {
        let start = Instant::now();
        while let Some(remaining) = DEADLINE.checked_sub(start.elapsed()) {
            match self.lines.recv_timeout(remaining) {
                Ok(line) if line.contains(needle) => return,
                Ok(_) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("den exited before printing {needle:?}")
                }
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        panic!("never saw {needle:?} within {DEADLINE:?}")
    }

    /// Everything the child ever printed, collected after it closed stdout.
    fn remaining_output(&self) -> String {
        std::iter::from_fn(|| self.lines.recv_timeout(DEADLINE).ok()).collect()
    }

    fn signal(&self, signal: i32) {
        // SAFETY: `kill` on a pid this process owns and has not yet reaped.
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }

    fn wait(&mut self) -> ExitStatus {
        let start = Instant::now();
        while start.elapsed() < DEADLINE {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("den was still alive after {DEADLINE:?}")
    }
}

impl Drop for Den {
    fn drop(&mut self) {
        // A failed assert must not leak a spinning den into the rest of the run.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn timer_pending_sigint_is_signal_death() {
    let mut den = Den::start(&[], r#"setTimeout(() => {}, 1e9); console.log("armed")"#);
    den.wait_for_line("armed");
    den.signal(libc::SIGINT);
    assert_eq!(den.wait().signal(), Some(libc::SIGINT));
}

#[test]
fn tight_loop_sigint_is_signal_death() {
    let mut den = Den::start(&[], r#"console.log("start"); while (true) {}"#);
    den.wait_for_line("start");
    den.signal(libc::SIGINT);
    assert_eq!(den.wait().signal(), Some(libc::SIGINT));
}

#[test]
fn uncaught_top_level_throw_exits_1() {
    let mut den = Den::start(&[], r#"throw new Error("boom")"#);
    assert_eq!(den.wait().code(), Some(1));
}

#[test]
fn repl_with_broken_file_exits_1() {
    let mut den = Den::start(&["--repl"], r#"throw new Error("boom")"#);
    assert_eq!(den.wait().code(), Some(1));
    assert!(!den.remaining_output().contains("Welcome to den"));
}

#[test]
#[ignore = "needs the event-loop signal delivery of commit 6"]
fn listener_survives_two_sigints_then_dies_of_sigterm() {
    let mut den = Den::start(
        &[],
        r#"process.addSignalListener("SIGINT", () => console.log("caught")); setTimeout(() => {}, 1e9); console.log("armed")"#,
    );
    den.wait_for_line("armed");
    den.signal(libc::SIGINT);
    den.wait_for_line("caught");
    den.signal(libc::SIGINT);
    den.wait_for_line("caught");
    den.signal(libc::SIGTERM);
    assert_eq!(den.wait().signal(), Some(libc::SIGTERM));
}

#[test]
#[ignore = "needs the event-loop signal delivery of commit 6"]
fn listener_alone_does_not_keep_den_alive() {
    let mut den = Den::start(&[], r#"process.addSignalListener("SIGUSR1", () => {})"#);
    assert_eq!(den.wait().code(), Some(0));
}

#[test]
#[ignore = "needs the event-loop signal delivery of commit 6"]
fn self_signal_reaches_the_listener() {
    let mut den = Den::start(
        &[],
        r#"process.addSignalListener("SIGUSR1", () => { console.log("got it"); process.exit(0) });
           process.kill(process.pid, "SIGUSR1");
           await new Promise(() => {})"#,
    );
    den.wait_for_line("got it");
    assert_eq!(den.wait().code(), Some(0));
}

#[test]
#[ignore = "needs the event-loop signal delivery of commit 6"]
fn signal_is_delivered_during_top_level_await_and_after_the_module_returns() {
    // The first SIGINT lands while the entry module is parked on a top-level
    // await, the second after it returned: both phases must share one inbox.
    let mut den = Den::start(
        &[],
        r#"let n = 0;
           process.addSignalListener("SIGINT", () => console.log(`caught ${++n}`));
           setTimeout(() => {}, 1e9);
           console.log("armed");
           await new Promise(resolve => setTimeout(resolve, 1500));
           console.log("module done");"#,
    );
    den.wait_for_line("armed");
    den.signal(libc::SIGINT);
    den.wait_for_line("caught 1");
    den.wait_for_line("module done");
    den.signal(libc::SIGINT);
    den.wait_for_line("caught 2");
    den.signal(libc::SIGTERM);
    assert_eq!(den.wait().signal(), Some(libc::SIGTERM));
}
