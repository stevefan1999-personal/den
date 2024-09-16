use den_core::engine::{Engine, EngineError};
use futures::prelude::*;
use rquickjs::{async_with, convert::Coerced};
use tokio::{signal, sync::mpsc, task::yield_now};

use crate::repl;

pub struct App {
    pub(crate) engine: Engine,
    repl:              bool,
}

impl App {
    pub async fn new() -> Self {
        Self {
            engine: Engine::new().await,
            repl:   false,
        }
    }
}

impl App {
    pub fn start_repl_session(&mut self) {
        let (repl_tx, mut repl_rx) = mpsc::unbounded_channel::<String>();

        // This task accepts all strings input from a channel, so that it can be from
        // any data source Right now this is for stdin, but we can obviously
        // extend this to run from a buffer for automated E2EE test
        tokio::spawn({
            let engine = self.engine.clone();
            let token = engine.stop_token.child_token();
            async move {
                let subtoken = token.child_token();
                token
                    .run_until_cancelled(async move {
                        // Each received data has to be one complete instance of script for eval,
                        // i.e. full buffer.
                        // Don't handle things like missing bracket balance here
                        while let Some(source) = repl_rx.recv().await {
                            let fut = {
                                let engine = engine.clone();
                                let source = source.clone();
                                let subtoken = subtoken.child_token();
                                async move {
                                    match subtoken.run_until_cancelled(engine.eval::<Coerced<String>>(&source)).await {
                                        Some(Ok(Coerced(res))) => {
                                            println!("{res}")
                                        }
                                        // Handles runtime exception during execution
                                        Some(Err(EngineError::Rquickjs(_))) => {
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
                                        #[allow(unreachable_patterns)]
                                        Some(Err(e)) => {
                                            eprintln!("{e}")
                                        },
                                        None => {}
                                    }
                                }
                            };
                            tokio::spawn(fut);
                            yield_now().await;
                        }
                    })
                    .await;
            }
        });

        // The REPL runs on a different task, and send data to our REPL eval handler
        // above
        tokio::spawn({
            let stop_token = self.engine.stop_token.clone();
            repl::run_repl(repl_tx).then(move |_| async move { stop_token.cancel() })
        });

        self.repl = true;
    }

    pub async fn run_until_end(&mut self) {
        // This part does 3 things
        // 1. Handle if a stop signal has been received (in the form of a cancellation)
        // 2. Wait until all front tasks are completed (notably if REPL is involved)
        // 3. Ensure that after being ready to stop everything, wait for the VM to
        //    finish all the pending executions

        tokio::spawn(self.engine.runtime.drive());

        // tokio::spawn(self.tasks.join_all()).await;

        if self.repl {
            self.engine.stop_token.child_token().cancelled().await;
        }
        self.engine
            .stop_token
            .run_until_cancelled(self.engine.runtime.idle())
            .await;
    }

    // Just hooks the Ctrl-C signal and then automatically stop the VM engine
    pub fn hook_ctrlc_handler(&mut self) {
        let stop_token = self.engine.stop_token.clone();
        tokio::spawn(signal::ctrl_c().then(move |_| async move { stop_token.cancel() }));
    }
}
