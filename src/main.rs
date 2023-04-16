use std::{default::Default, path::PathBuf};

use app::App;
use clap::Parser;

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
    color_eyre::install()?;

    let cli = Cli::parse();
    let mut app = App::default();

    if let Some(x) = cli.file.clone() {
        app.hook_ctrlc_handler();
        app.engine.run_file(x).await?;
    }

    if cli.repl || cli.file.is_none() {
        println!("Welcome to den, one word less than Deno");
        app.start_repl_session().await;
    }

    app.run_until_end().await;
    Ok(())
}

mod app;
mod js;
mod loader;
mod repl;
mod resolver;
mod transpile;
