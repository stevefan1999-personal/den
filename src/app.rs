use den_core::engine::{Engine, EngineError};
use den_utils::FutureExt;
use rquickjs::{async_with, convert::Coerced};
use tokio::{
    select, signal,
    sync::mpsc,
    task::{yield_now, JoinSet},
};

use crate::repl;

pub struct App {
    pub(crate) engine: Engine,
    tasks:             JoinSet<()>,
}

impl App {
    pub async fn new() -> Self {
        Self {
            engine: Engine::new().await,
            tasks:  JoinSet::new(),
        }
    }
}

impl App {
    pub async fn start_repl_session(&mut self) {
        let stop_token = self.engine.stop_token();
        let (repl_tx, mut repl_rx) = mpsc::unbounded_channel::<String>();

        // This task accepts all strings input from a channel, so that it can be from
        // any data source Right now this is for stdin, but we can obviously
        // extend this to run from a buffer for automated E2EE test
        self.tasks.spawn({
            let engine = self.engine.clone();
            async move {
                // Each received data has to be one complete instance of script for eval, i.e.
                // full buffer.
                // Don't handle things like missing bracket balance here
                while let Some(source) = repl_rx.recv().await {
                    match engine.eval::<Coerced<String>>(&source).await {
                        // Normal case
                        Ok(Coerced(res)) => {
                            println!("{res}")
                        }
                        // Handles runtime exception during execution
                        Err(EngineError::Rquickjs(_)) => {
                            async_with!(engine.context => |ctx| {
                                let e = ctx.catch();
                                if let Some(e) = e.as_exception() {
                                    eprintln!("{e}")
                                } else if let Ok(Coerced(e)) = e.get::<Coerced<String>>() {
                                    eprintln!("{e}")
                                } else {
                                    eprintln!("unknown error")
                                }
                            })
                            .await;
                        }
                        // This can be something else such as SWC error
                        // TODO: Stop being lazy and use thiserror to filter out possible errors for
                        // optimization
                        #[allow(unreachable_patterns)]
                        Err(e) => {
                            eprintln!("{e}")
                        }
                    }
                }
            }
        });

        // The REPL runs on a different task, and send data to our REPL eval handler
        // above
        self.tasks.spawn({
            async move {
                repl::run_repl(repl_tx).await;
                stop_token.cancel();
            }
        });
    }

    pub async fn run_until_end(&mut self) {
        let rt = &self.engine.runtime;
        let mut ready_to_stop = false;

        let stop_token = self.engine.stop_token.child_token();
        let stop_token2 = stop_token.child_token();

        // This part does 3 things
        // 1. Handle if a stop signal has been received (in the form of a cancellation)
        // 2. Wait until all front tasks are completed (notably REPL)
        // 3. Ensure that after being ready to stop everything, wait for the VM to
        //    finish all the pending executions
        'select: loop {
            select! {
                _ = stop_token.cancelled(), if !ready_to_stop => {
                    ready_to_stop = true;
                },
                None = self.tasks.join_next(), if !ready_to_stop => {
                    ready_to_stop = true;
                },
                _ = rt.idle().with_cancellation(&stop_token2), if ready_to_stop => {
                    break 'select;
                },
                else => {
                    yield_now().await;
                }
            }
            yield_now().await;
        }
    }

    // Just hooks the Ctrl-C signal and then automatically stop the VM engine
    pub fn hook_ctrlc_handler(&self) {
        let stop_token = self.engine.stop_token();
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            stop_token.cancel();
        });
    }
}
