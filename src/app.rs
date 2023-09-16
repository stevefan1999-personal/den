use color_eyre::eyre;
use den_stdlib_core::WORLD_END;
use rquickjs::{async_with, convert::Coerced};
use tokio::{
    select, signal,
    sync::mpsc,
    task::{yield_now, JoinSet},
};

use crate::{engine::Engine, repl};

pub struct App {
    pub(crate) engine: Engine,
    tasks:             JoinSet<()>,
}

impl App {
    pub async fn new() -> Self {
        let join_set = JoinSet::new();

        Self {
            engine: Engine::new().await,
            tasks:  join_set,
        }
    }
}

impl App {
    pub async fn start_repl_session(&mut self) {
        let (repl_tx, mut repl_rx) = mpsc::unbounded_channel::<String>();
        self.tasks.spawn({
            let engine = self.engine.clone();
            async move {
                while let Some(source) = repl_rx.recv().await {
                    let result: eyre::Result<Coerced<String>> = engine.eval(&source).await;
                    match result {
                        Ok(Coerced(res)) => {
                            println!("{}", res)
                        }
                        Err(e) if e.is::<rquickjs::Error>() => {
                            async_with!(engine.context => |ctx| {
                                if let Some(e) = ctx.catch().as_exception() {
                                    eprintln!("{}", e)
                                }
                            })
                            .await;
                        }
                        Err(e) => {
                            eprintln!("{}", e)
                        }
                    }
                }
            }
        });
        self.tasks.spawn(repl::run_repl(repl_tx));
    }

    pub async fn run_until_end(&mut self) {
        let rt = &self.engine.runtime;
        let mut stoppable = false;

        'select: loop {
            select! {
                _ = self.engine.stop_token.cancelled(), if !stoppable => {
                    stoppable = true;
                },
                None = self.tasks.join_next(), if !stoppable => {
                    stoppable = true;
                },
                _ = rt.idle() => {
                    if stoppable {
                        break 'select;
                    }
                }
                else => {
                    if stoppable {
                        break 'select;
                    }
                }
            }
            yield_now().await;
        }

        rt.idle().await;
    }

    pub fn hook_ctrlc_handler(&mut self) {
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            WORLD_END.cancel();
        });
    }
}
