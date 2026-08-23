use rustyline::{
    Behavior, Completer, Config, Editor, Helper, Highlighter, Hinter, Validator,
    error::ReadlineError, sqlite_history::SQLiteHistory, validate::MatchingBracketValidator,
};
use tokio::{sync::mpsc, task::yield_now};

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
    // An unwritable history file must not take the REPL down with it, so fall back
    // to the in-memory history.
    let history = SQLiteHistory::open(&config, "history.db")
        .or_else(|_| SQLiteHistory::with_config(&config))
        .unwrap();
    let mut rl = Editor::with_history(config, history).unwrap();
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
                    let _ = rl.add_history_entry(&text);
                }

                yield_now().await;
            }
        }
    }
}
