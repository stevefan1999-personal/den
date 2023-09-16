use std::path::PathBuf;

use app::App;
use clap::Parser;
use rquickjs::async_with;

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
async fn main() -> color_eyre::Result<()> {
    console_subscriber::init();
    color_eyre::install()?;

    let cli = Cli::parse();
    let mut app = App::new().await;

    if let Some(x) = cli.file.clone() {
        app.hook_ctrlc_handler();
        match app.engine.run_file(x).await {
            Err(e) if e.is::<rquickjs::Error>() => {
                async_with!(app.engine.context => |ctx| {
                    if let Some(e) = ctx.catch().as_exception() {
                        eprintln!("{}", e)
                    }
                })
                .await;
            }
            Err(e) => {
                eprintln!("{}", e)
            }
            _ => {}
        }
    }

    if cli.repl || cli.file.is_none() {
        println!("Welcome to den, one word less than Deno");
        app.start_repl_session().await;
    }
    app.run_until_end().await;
    Ok(())
}

mod app;
mod engine;
mod loader;
mod repl;
mod resolver;
mod transpile;
