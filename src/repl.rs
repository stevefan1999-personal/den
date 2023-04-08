use den_stdlib_core::WORLD_END;
use rustyline::{
    config::Configurer, error::ReadlineError, validate::MatchingBracketValidator, Behavior,
    Completer, Editor, Helper, Highlighter, Hinter, Validator,
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
    let mut rl = Editor::new().unwrap();
    rl.set_behavior(Behavior::PreferTerm);
    rl.set_helper(Some(h));

    'repl: loop {
        'inner: loop {
            match rl.readline("> ") {
                Err(ReadlineError::Eof) => break 'repl,
                Err(ReadlineError::Interrupted) if interrupted => break 'repl,
                Err(ReadlineError::Interrupted) => {
                    println!("(To exit, press Ctrl+C again or Ctrl+D)");
                    interrupted = true;
                    break 'inner;
                }
                Err(_) => break 'inner,
                Ok(text) => {
                    interrupted = false;

                    if !text.is_empty() {
                        let _ = output_sink.send(text.clone());
                        let _ = rl.add_history_entry(&text);
                    }

                    break 'inner;
                }
            }
        }
        yield_now().await;
    }
    WORLD_END.get().unwrap().cancel();
}
