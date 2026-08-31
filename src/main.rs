use std::{env, ffi::OsString, path::PathBuf};

use app::App;
#[cfg(not(all(feature = "tokio-console", tokio_unstable)))]
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Debug, Default)]
struct Cli {
    file: Option<PathBuf>,
    repl: bool,
    /// The FFI capability this run hands the realm, if any. Off by default,
    /// and absent entirely from a build without `stdlib-ffi` — where
    /// `--allow-ffi` is an unknown option rather than a silent no-op.
    #[cfg(feature = "stdlib-ffi")]
    ffi:  Option<den_core::FfiGrant>,
}

impl Cli {
    #[cfg(feature = "stdlib-ffi")]
    const USAGE: &'static str = "Usage: den [--repl] [--allow-ffi[=PATH,...]] [FILE]";
    #[cfg(not(feature = "stdlib-ffi"))]
    const USAGE: &'static str = "Usage: den [--repl] [FILE]";

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
                println!("{}\n\n{}", env!("CARGO_PKG_DESCRIPTION"), Self::USAGE);
                return Ok(None);
            } else if !positional_only && matches!(text, Some("-V" | "--version")) {
                println!("den {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            } else if !positional_only && text == Some("--repl") {
                cli.repl = true;
            } else if !positional_only && cli.take_ffi_flag(text) {
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

    /// `--allow-ffi` grants every path, `--allow-ffi=A,B` only paths under A or
    /// B — a directory or a single library either way. Answers whether the
    /// argument was this flag.
    #[cfg(feature = "stdlib-ffi")]
    fn take_ffi_flag(&mut self, text: Option<&str>) -> bool {
        let Some(flag) =
            text.filter(|value| *value == "--allow-ffi" || value.starts_with("--allow-ffi="))
        else {
            return false;
        };
        self.ffi = Some(match flag.split_once('=') {
            None => den_core::FfiGrant::any(),
            Some((_, roots)) => den_core::FfiGrant::under(roots.split(',').map(PathBuf::from)),
        });
        true
    }

    /// Without the feature there is no grant to mint, so `--allow-ffi` stays an
    /// unknown option — a flag that silently granted nothing would be worse.
    #[cfg(not(feature = "stdlib-ffi"))]
    #[expect(
        clippy::unused_self,
        reason = "the mutating half of the pair is the one that is compiled in"
    )]
    const fn take_ffi_flag(&self, _text: Option<&str>) -> bool { false }
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
    #[cfg(feature = "stdlib-ffi")]
    if let Some(grant) = cli.ffi.clone() {
        app.engine.set_ffi_grant(grant).await?;
    }

    if let Some(x) = cli.file.clone()
        && let Err(error) = app.engine.run_file(x).await
    {
        eprintln!("{error}");
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
mod history;
mod repl;

#[cfg(test)]
#[path = "../tests/unit/cli.rs"]
mod tests;
