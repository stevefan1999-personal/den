use std::ffi::OsString;
#[cfg(feature = "stdlib-ffi")]
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

/// The implemented command surface. New commands belong here only when their
/// dispatch path exists; help must never advertise a placeholder.
#[derive(Debug, Parser)]
#[command(
    name = "den",
    version,
    about,
    disable_help_subcommand = true,
    trailing_var_arg = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command:     Option<Command>,
    /// Start a REPL after an optional default-run entry finishes.
    #[arg(long)]
    pub repl:        bool,
    /// Grant FFI access to every library, or only comma-separated paths.
    #[cfg(feature = "stdlib-ffi")]
    #[arg(long, global = true, value_delimiter = ',', num_args = 0.., require_equals = true)]
    pub allow_ffi:   Option<Vec<PathBuf>>,
    /// File or URL to run when no subcommand is given.
    #[arg(value_name = "ENTRY")]
    pub entry:       Option<String>,
    /// Arguments passed to the default-run script.
    #[arg(value_name = "ARG", allow_hyphen_values = true)]
    pub script_args: Vec<OsString>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a file or URL module.
    Run(RunArgs),
    /// Evaluate source text.
    Eval(EvalArgs),
    /// Start an interactive session.
    Repl,
    /// Generate shell completions.
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
#[command(trailing_var_arg = true)]
pub struct RunArgs {
    pub entry: String,
    #[arg(value_name = "ARG", allow_hyphen_values = true)]
    pub args:  Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct EvalArgs {
    pub code:  String,
    /// Print the resulting value.
    #[arg(short, long)]
    pub print: bool,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    #[arg(value_enum)]
    pub shell: Shell,
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::{CommandFactory as _, Parser as _};

    use super::{Cli, Command};

    #[test]
    fn default_run_forwards_arguments_without_requiring_a_delimiter() {
        let cli =
            Cli::try_parse_from(["den", "main.ts", "--port", "8000"]).expect("parse default run");
        assert_eq!(cli.entry.as_deref(), Some("main.ts"));
        assert_eq!(cli.script_args, [
            OsString::from("--port"),
            OsString::from("8000")
        ]);
    }

    #[test]
    fn explicit_run_forwards_arguments() {
        let cli = Cli::try_parse_from(["den", "run", "https://example.test/main.ts", "--port"])
            .expect("parse run");
        let Some(Command::Run(run)) = cli.command else {
            panic!("run command expected")
        };
        assert_eq!(run.entry, "https://example.test/main.ts");
        assert_eq!(run.args, [OsString::from("--port")]);
    }

    #[cfg(feature = "stdlib-ffi")]
    #[test]
    fn ffi_grant_does_not_consume_the_entry() {
        let cli = Cli::try_parse_from(["den", "--allow-ffi", "main.ts"])
            .expect("parse unrestricted FFI grant");
        assert_eq!(cli.allow_ffi, Some(Vec::new()));
        assert_eq!(cli.entry.as_deref(), Some("main.ts"));

        let cli = Cli::try_parse_from(["den", "run", "--allow-ffi", "main.ts"])
            .expect("parse subcommand FFI grant");
        assert_eq!(cli.allow_ffi, Some(Vec::new()));
        assert!(matches!(cli.command, Some(Command::Run(_))));
    }

    #[test]
    fn help_lists_only_implemented_commands() {
        let command = Cli::command();
        let names = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["run", "eval", "repl", "completions"]);
    }
}
