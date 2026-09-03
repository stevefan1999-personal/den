use std::io::{self, IsTerminal as _};

use rustyline::{
    Behavior, Completer, Config, Editor, Helper, Highlighter, Hinter, Validator,
    error::ReadlineError, history::FileHistory, validate::MatchingBracketValidator,
};
use tokio::{sync::mpsc, task::yield_now};

const HISTORY_PATH: &str = "history.txt";

#[derive(Completer, Helper, Highlighter, Hinter, Validator)]
struct InputValidator {
    #[rustyline(Validator)]
    brackets: MatchingBracketValidator,
}

pub async fn run_repl(output_sink: mpsc::UnboundedSender<String>) {
    let h = InputValidator {
        brackets: MatchingBracketValidator::new(),
    };
    let mut interrupted = false;
    // Prefer the real console for interactive editing, but honor redirected
    // stdin in scripts and tests. On Windows, `PreferTerm` opens CONIN$ and
    // CONOUT$ directly, which would bypass the caller's pipes.
    let behavior = if io::stdin().is_terminal() {
        Behavior::PreferTerm
    } else {
        Behavior::Stdio
    };
    let config = Config::builder().behavior(behavior).build();
    let history = FileHistory::with_config(&config);
    let Ok(mut rl) = Editor::with_history(config, history) else {
        eprintln!("cannot initialize REPL editor");
        return;
    };
    rl.set_helper(Some(h));
    let _ = rl.load_history(HISTORY_PATH);

    'repl: loop {
        match rl.readline("> ") {
            Err(ReadlineError::Eof) => break 'repl,
            Err(ReadlineError::Interrupted) if interrupted => break 'repl,
            Err(ReadlineError::Interrupted) => {
                println!("(To exit, press Ctrl+C again or Ctrl+D)");
                interrupted = true;
                yield_now().await;
            }
            Err(_) => yield_now().await,
            Ok(text) => {
                interrupted = false;

                if !text.is_empty() {
                    let _ = output_sink.send(text.clone());
                    if let Err(error) = rl.add_history_entry(&text) {
                        eprintln!("cannot add REPL history: {error}");
                    }
                    // The REPL exits the process the moment `run_repl` returns,
                    // so history has to reach disk per accepted line.
                    let _ = rl.append_history(HISTORY_PATH);
                }

                yield_now().await;
            }
        }
    }
}
