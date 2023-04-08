use std::{io, path::PathBuf, sync::Arc};

use den_stdlib_core::WORLD_END;
use rquickjs::{Coerced, Context};
use swc_core::{base::config::IsModule, ecma::parser::Syntax};
use tokio::{
    fs, select, signal,
    sync::mpsc,
    task::{yield_now, AbortHandle, JoinSet},
};

use crate::{js, repl, transpile::EasySwcTranspiler};

pub struct App {
    transpiler:      Arc<EasySwcTranspiler>,
    ctx:             Context,
    tasks:           JoinSet<()>,
    executor_handle: Arc<AbortHandle>,
}

impl Default for App {
    fn default() -> Self {
        let mut join_set = JoinSet::new();

        let (_, ctx, executor_handle) = js::create_qjs_context(&mut join_set);
        Self {
            transpiler: Default::default(),
            tasks: join_set,
            ctx,
            executor_handle: Arc::new(executor_handle),
        }
    }
}

impl App {
    pub async fn run_file(&mut self, filename: PathBuf) -> io::Result<()> {
        let syntax = Syntax::Typescript(Default::default());
        let file = fs::read_to_string(filename.clone()).await?;
        if let Ok((src, _)) = self
            .transpiler
            .transpile(file, syntax, IsModule::Bool(true), false)
        {
            self.ctx.with(|ctx| {
                if let Err(err) = ctx.compile(filename.to_str().unwrap_or(""), src) {
                    eprintln!("{}", err)
                }
            });
        }
        Ok(())
    }

    pub async fn start_repl_session(&mut self) {
        let (repl_tx, mut repl_rx) = mpsc::unbounded_channel();
        let interpreter_task = {
            let transpiler = self.transpiler.clone();
            let ctx = self.ctx.clone();
            async move {
                let syntax = Syntax::Typescript(Default::default());
                while let Some(source) = repl_rx.recv().await {
                    if let Ok((src, _)) =
                        transpiler.transpile(source, syntax, IsModule::Bool(false), false)
                    {
                        ctx.with(|ctx| {
                            let result: rquickjs::Result<Coerced<String>> = ctx.eval(src);
                            match result {
                                Ok(Coerced(res)) => {
                                    println!("{}", res)
                                }
                                Err(err) => {
                                    eprintln!("{}", err)
                                }
                            }
                        });
                    }
                    yield_now().await;
                }
            }
        };
        self.tasks.spawn(interpreter_task);
        self.tasks.spawn(repl::run_repl(repl_tx));
    }

    pub async fn run_until_end(&mut self) {
        let cancel = WORLD_END.get().unwrap().child_token();

        'select: loop {
            select! {
                _ = cancel.cancelled() => {
                    self.executor_handle.abort();
                },
                None = self.tasks.join_next() => {
                    break 'select;
                }
            }
        }

        let _ = self.ctx.runtime().execute_pending_job();
    }

    pub fn hook_ctrlc_handler(&mut self) {
        tokio::spawn(async move {
            let _ = signal::ctrl_c().await;
            WORLD_END.get().unwrap().cancel();
        });
    }
}
