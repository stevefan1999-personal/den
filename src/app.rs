use den_core::engine::Engine;
use rquickjs::convert::Coerced;
use tokio::sync::mpsc;

use crate::repl;

pub struct App {
    pub(crate) engine: Engine,
    repl_rx:           Option<mpsc::UnboundedReceiver<String>>,
}

impl App {
    pub async fn new() -> Self {
        Self {
            engine:  Engine::new().await,
            repl_rx: None,
        }
    }
}

impl App {
    pub fn start_repl_session(&mut self) {
        let (repl_tx, repl_rx) = mpsc::unbounded_channel::<String>();

        // The REPL runs on a different task and sends complete scripts to the
        // `ctx.spawn`ed pump started in `run_until_end`. Closing the REPL ends
        // the process outright: `run_repl` has already closed the history, and
        // anything still spawned on the engine is abandoned exactly as it would
        // be on signal death.
        tokio::spawn(async move {
            repl::run_repl(repl_tx).await;
            std::process::exit(0)
        });

        self.repl_rx = Some(repl_rx);
    }

    pub async fn run_until_end(&mut self) {
        // Event loop is `runtime.idle()` only. `drive()` polls the same
        // scheduler while releasing the lock; spawning it *and* calling
        // `idle()` makes two loopers fight. The only way to do JS work during
        // `idle()` is `ctx.spawn`, so the REPL eval pump is one of those.
        if let Some(repl_rx) = self.repl_rx.take() {
            let engine = self.engine.clone();
            self.engine
                .context
                .with(move |ctx| {
                    ctx.spawn(Self::repl_pump(ctx.clone(), engine, repl_rx));
                })
                .await;
        }

        self.engine.runtime.idle().await;
        self.engine.shutdown().await;
    }

    /// Eval each REPL line on this context. Lives as a `ctx.spawn`ed future so
    /// `idle()` can poll it while holding the runtime lock; `engine.eval`
    /// would deadlock on that lock.
    async fn repl_pump(
        ctx: rquickjs::Ctx<'_>, engine: Engine, mut repl_rx: mpsc::UnboundedReceiver<String>,
    ) {
        while let Some(source) = repl_rx.recv().await {
            let src = match engine.prepare_eval_source(&source) {
                Ok(src) => src,
                Err(error) => {
                    eprintln!("{error}");
                    continue;
                }
            };
            match Engine::eval_prepared::<Coerced<String>>(ctx.clone(), &src).await {
                Ok(Coerced(res)) => println!("{res}"),
                Err(rquickjs::Error::Exception) => print_js_error(&ctx),
                Err(error) => eprintln!("{error}"),
            }
        }
    }
}

pub fn print_js_error(ctx: &rquickjs::Ctx<'_>) {
    let e = ctx.catch();
    if let Some(e) = e.as_exception() {
        eprintln!("{e}")
    } else if let Ok(Coerced(e)) = e.get::<Coerced<String>>() {
        eprintln!("{e}")
    } else {
        eprintln!("unknown error")
    }
}
