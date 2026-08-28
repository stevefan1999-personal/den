use std::{env, ffi::OsString, path::PathBuf};

use app::{App, print_js_error};
use den_core::engine::EngineError;
#[cfg(not(all(feature = "tokio-console", tokio_unstable)))]
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Debug, Default)]
struct Cli {
    file: Option<PathBuf>,
    repl: bool,
}

impl Cli {
    fn parse() -> color_eyre::eyre::Result<Option<Self>> {
        Self::parse_args(env::args_os().skip(1))
    }

    fn parse_args<I>(args: I) -> color_eyre::eyre::Result<Option<Self>>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut cli = Self::default();
        let mut positional_only = false;
        for arg in args {
            let text = arg.to_str();
            if !positional_only && text == Some("--") {
                positional_only = true;
            } else if !positional_only && matches!(text, Some("-h" | "--help")) {
                println!(
                    "{}\n\nUsage: den [--repl] [FILE]",
                    env!("CARGO_PKG_DESCRIPTION")
                );
                return Ok(None);
            } else if !positional_only && matches!(text, Some("-V" | "--version")) {
                println!("den {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            } else if !positional_only && text == Some("--repl") {
                cli.repl = true;
            } else if !positional_only && text.is_some_and(|value| value.starts_with('-')) {
                return Err(color_eyre::eyre::eyre!(
                    "unknown option: {}",
                    arg.to_string_lossy()
                ));
            } else if cli.file.replace(PathBuf::from(arg)).is_some() {
                return Err(color_eyre::eyre::eyre!("only one input file is supported"));
            }
        }
        Ok(Some(cli))
    }
}

#[tokio::main]
async fn main() -> color_eyre::eyre::Result<()> {
    #[cfg(all(feature = "tokio-console", tokio_unstable))]
    {
        console_subscriber::init();
    }
    color_eyre::install()?;
    #[cfg(not(all(feature = "tokio-console", tokio_unstable)))]
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .pretty()
        .init();

    let Some(cli) = Cli::parse()? else {
        return Ok(());
    };
    let mut app = App::new().await;

    if let Some(x) = cli.file.clone()
        && let Err(error) = app.engine.run_file::<()>(x).await
    {
        match error {
            EngineError::Rquickjs(_) => {
                app.engine.context.with(|ctx| print_js_error(&ctx)).await;
            }
            error => eprintln!("{error}"),
        }
        // A failed entry file is fatal: Node and Deno exit here and never fall
        // through into the REPL.
        std::process::exit(1)
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
