use std::path::PathBuf;

use rustyline::{
    Behavior, Completer, Config, Editor, Helper, Highlighter, Hinter, Validator,
    error::ReadlineError, validate::MatchingBracketValidator,
};
use tokio::{sync::mpsc, task::yield_now};

use crate::history::Surreal;

const HISTORY_PATH: &str = "history.surrealkv";

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
    // rustyline 18 dropped `Configurer::set_behavior`: the behaviour has to be
    // baked into the `Config` before the editor creates its terminal.
    let config = Config::builder().behavior(Behavior::PreferTerm).build();
    let path = PathBuf::from(HISTORY_PATH);
    let history = match Surreal::open(&config, path.clone()) {
        Ok(history) => history,
        Err(error) => {
            eprintln!("cannot open SurrealKV REPL history, using memory: {error}");
            Surreal::in_memory(&config, path)
        }
    };
    let Ok(mut rl) = Editor::with_history(config, history) else {
        eprintln!("cannot initialize REPL editor");
        return;
    };
    rl.set_helper(Some(h));

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
                }

                yield_now().await;
            }
        }
    }
    if let Err(error) = rl.history_mut().close().await {
        eprintln!("cannot close SurrealKV REPL history: {error}");
    }
}
