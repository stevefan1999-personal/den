use std::path::PathBuf;

use app::App;
use clap::Parser;
use den_core::engine::EngineError;
use rquickjs::{async_with, Coerced};


#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg()]
    file:       Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    repl:       bool,
    #[arg(long, default_value_t = true)]
    typescript: bool,
}

#[tokio::main]
async fn main() -> color_eyre::eyre::Result<()> {
    #[cfg(all(feature = "tokio-console", tokio_unstable))]
    {
        console_subscriber::init();
    }
    color_eyre::install()?;

    let cli = Cli::parse();
    let mut app = App::new().await;

    if let Some(x) = cli.file.clone() {
        app.hook_ctrlc_handler();
        match app
            .engine
            .stop_token
            .child_token()
            .run_until_cancelled(app.engine.run_file::<()>(x))
            .await
        {
            Some(Err(EngineError::Rquickjs(_))) => {
                async_with!(app.engine.context => |ctx| {
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
            #[allow(unreachable_patterns)]
            Some(Err(e)) => {
                eprintln!("{e}")
            }
            _ => {}
        }
    }

    if cli.repl || cli.file.is_none() {
        println!("Welcome to den, one word less than Deno");
        app.start_repl_session();
    }

    app.run_until_end().await;
    Ok(())
}

mod app;
mod repl;
