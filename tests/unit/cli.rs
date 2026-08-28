use std::{ffi::OsString, path::PathBuf};

use super::Cli;

fn parse(args: &[&str]) -> color_eyre::eyre::Result<Option<Cli>> {
    Cli::parse_args(args.iter().map(|arg| OsString::from(*arg)))
}

#[test]
fn cli_parser_handles_the_argument_matrix() {
    let empty = parse(&[])
        .expect("empty arguments parse")
        .expect("run the CLI");
    assert!(!empty.repl);
    assert_eq!(empty.file, None);

    let parsed = parse(&["--repl", "script.js"])
        .expect("valid arguments parse")
        .expect("run the CLI");
    assert!(parsed.repl);
    assert_eq!(parsed.file, Some(PathBuf::from("script.js")));

    let positional = parse(&["--", "--repl"])
        .expect("the delimiter makes the next argument positional")
        .expect("run the CLI");
    assert!(!positional.repl);
    assert_eq!(positional.file, Some(PathBuf::from("--repl")));

    assert!(parse(&["first.js", "second.js"]).is_err());
    assert!(parse(&["--unknown"]).is_err());
}

#[test]
fn help_and_version_stop_before_starting_the_runtime() {
    assert!(parse(&["--help"]).expect("help parses").is_none());
    assert!(parse(&["--version"]).expect("version parses").is_none());
}
