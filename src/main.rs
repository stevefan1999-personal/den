use rquickjs::Coerced;
use std::default::Default;
use std::panic;
use swc_core::base::config::IsModule;
use swc_core::ecma::parser::Syntax;
use tokio::signal;
use tokio::sync::{broadcast, mpsc};
use tokio::task::{yield_now, JoinSet};
use transpile::EasySwcTranspiler;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let (repl_tx, mut repl_rx) = mpsc::unbounded_channel();
    let (ctrlc_tx, ctrlc_rx) = broadcast::channel(16);

    let (rt, ctx) = js::create_qjs_context(ctrlc_rx);
    let mut easy_swc = EasySwcTranspiler::default();
    let repl = repl::run_repl(repl_tx);

    let mut set = JoinSet::new();

    let ctrlc = async move {
        loop {
            let _ = signal::ctrl_c().await;
            let _ = ctrlc_tx.send(());
        }
    };

    let interpreter = async move {
        let syntax = Syntax::Typescript(Default::default());

        while let Some(source) = repl_rx.recv().await {
            if let Ok(src) = easy_swc.transpile(source, syntax, IsModule::Bool(false)) {
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

        Ok(())
    };

    set.spawn(ctrlc);
    set.spawn(repl);
    set.spawn(interpreter);

    loop {
        match set.join_next().await {
            Some(Err(e)) => {
                if let Ok(reason) = e.try_into_panic() {
                    panic::resume_unwind(reason);
                }
            }
            _ => {}
        }
        yield_now().await;
    }

    Ok(())
}

mod js;
mod loader;
mod repl;
mod resolver;
mod transpile;
