use color_eyre::eyre;
use den_stdlib_core::WORLD_END;
use rquickjs::Coerced;
use tokio::{
    select, signal,
    sync::mpsc,
    task::{yield_now, JoinSet},
};

use crate::{js::Engine, repl};

pub struct App {
    pub(crate) engine: Engine,
    tasks:             JoinSet<()>,
}

impl Default for App {
    fn default() -> Self {
        let join_set = JoinSet::new();

        Self {
            engine: Engine::new(),
            tasks:  join_set,
        }
    }
}

impl App {
    pub async fn start_repl_session(&mut self) {
        let (repl_tx, mut repl_rx) = mpsc::unbounded_channel::<String>();
        let interpreter_task = {
            let engine = self.engine.clone();
            async move {
                while let Some(source) = repl_rx.recv().await {
                    let result: eyre::Result<Coerced<String>> = engine.eval(&source);
                    match result {
                        Ok(Coerced(res)) => {
                            println!("{}", res)
                        }
                        Err(err) => {
                            eprintln!("{}", err)
                        }
                    }
                    yield_now().await;
                }
            }
        };
        self.tasks.spawn(interpreter_task);
        self.tasks.spawn(repl::run_repl(repl_tx));
    }

    pub async fn run_until_end(&mut self) {
        let cancel = WORLD_END.child_token();

        'select: loop {
            select! {
                _ = cancel.cancelled() => {
                    self.engine.stop();
                },
                None = self.tasks.join_next() => {
                    break 'select;
                }
            }
        }

        let _ = self.engine.ctx.runtime().execute_pending_job();
    }

    pub fn hook_ctrlc_handler(&mut self) {
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            WORLD_END.cancel();
        });
    }
}
