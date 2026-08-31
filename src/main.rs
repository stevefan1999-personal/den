use std::{ffi::OsString, path::PathBuf};

use app::App;
use clap::{CommandFactory as _, Parser as _};
use cli::{Cli, Command};
use den_core::engine::EngineError;
use rquickjs::convert::Coerced;
#[cfg(not(all(feature = "tokio-console", tokio_unstable)))]
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

    let cli = Cli::parse();
    if let Some(Command::Completions(args)) = &cli.command {
        #[cfg(feature = "stdlib-ffi")]
        let has_ffi = cli.allow_ffi.is_some();
        #[cfg(not(feature = "stdlib-ffi"))]
        let has_ffi = false;
        if cli.repl || has_ffi || cli.config.is_some() {
            Cli::command()
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "runtime options cannot be used with completions",
                )
                .exit();
        }
        clap_complete::generate(
            args.shell,
            &mut Cli::command(),
            "den",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    #[cfg(feature = "stdlib-ffi")]
    let ffi = cli.allow_ffi.clone().map(|paths| {
        if paths.is_empty() {
            den_core::FfiGrant::any()
        } else {
            den_core::FfiGrant::under(paths)
        }
    });
    let config_path = cli.config.clone();
    let (entry, script_arguments, eval, repl) = match cli.command {
        Some(Command::Run(run)) => (Some(run.entry), run.args, None, cli.repl),
        Some(Command::Eval(eval)) => (None, Vec::new(), Some(eval), cli.repl),
        Some(Command::Repl) => (None, Vec::new(), None, true),
        Some(Command::Completions(_)) => unreachable!("completions returned before runtime setup"),
        None => (cli.entry, cli.script_args, None, cli.repl),
    };
    let process_arguments = script_argv(entry.as_deref(), script_arguments);
    let has_entry = entry.is_some();
    let has_eval = eval.is_some();
    let config = den_config::Config::discover(std::env::current_dir()?, config_path.as_deref())?;
    let engine = runtime_config::build_engine(config.as_ref(), process_arguments).await?;
    let mut app = App::with_engine(engine);
    #[cfg(feature = "stdlib-ffi")]
    if let Some(grant) = ffi {
        app.engine.set_ffi_grant(grant).await?;
    }
    runtime_config::run_preloads(&app.engine, config.as_ref()).await?;

    if let Some(eval) = eval {
        if eval.print {
            match app.engine.eval::<Coerced<String>>(&eval.code).await {
                Ok(Coerced(value)) => println!("{value}"),
                Err(error) => fatal(error),
            }
        } else if let Err(error) = app.engine.eval::<()>(&eval.code).await {
            fatal(error)
        }
    }
    if let Some(entry) = entry
        && let Err(error) = run_entry(&app.engine, &entry).await
    {
        fatal(error)
    }

    if repl || (!has_entry && !has_eval) {
        println!("Welcome to den, one word less than Deno");
        app.start_repl_session();
    }

    app.run_until_end().await;
    Ok(())
}

fn script_argv(entry: Option<&str>, script_arguments: Vec<OsString>) -> Vec<String> {
    let mut process_arguments = Vec::with_capacity(script_arguments.len() + 2);
    process_arguments.push(
        std::env::current_exe()
            .map_or_else(|_| "den".into(), |path| path.to_string_lossy().into_owned()),
    );
    process_arguments.extend(entry.map(str::to_owned));
    process_arguments.extend(
        script_arguments
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned()),
    );
    process_arguments
}

async fn run_entry(engine: &den_core::engine::Engine, entry: &str) -> Result<(), EngineError> {
    #[cfg(windows)]
    if matches!(
        PathBuf::from(entry).components().next(),
        Some(std::path::Component::Prefix(_))
    ) {
        return engine.run_file(PathBuf::from(entry)).await;
    }
    if is_remote_entry(entry) {
        engine.run_module(entry).await
    } else {
        engine.run_file(PathBuf::from(entry)).await
    }
}

fn is_remote_entry(entry: &str) -> bool {
    url::Url::parse(entry).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn fatal(error: impl std::fmt::Display) -> ! {
    eprintln!("{error}");
    std::process::exit(1)
}

mod app;
mod cli;
mod history;
mod repl;
mod runtime_config;

#[cfg(test)]
mod tests {
    use super::is_remote_entry;

    #[test]
    fn colon_in_a_local_filename_is_not_a_remote_module() {
        assert!(!is_remote_entry("foo:bar.js"));
        assert!(is_remote_entry("https://example.test/main.js"));
    }
}
