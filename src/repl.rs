use rustyline::{
    config::Configurer, error::ReadlineError, sqlite_history::SQLiteHistory,
    validate::MatchingBracketValidator, Behavior, Completer, Config, Editor, Helper, Highlighter,
    Hinter, Validator,
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
    let config = Config::default();
    let mut rl = Editor::with_history(
        config,
        SQLiteHistory::open(config, "history.db")
            .or(SQLiteHistory::with_config(config))
            .unwrap(),
    )
    .unwrap();
    rl.set_behavior(Behavior::PreferTerm);
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
                    let _ = rl.add_history_entry(&text).unwrap();
                }

                yield_now().await;
            }
        }
    }
}
