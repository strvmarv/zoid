//! Minimal hand-rolled CLI parsing for the `zoid` binary (spec §2 component A).
//! Three flags and one subcommand do not justify a `clap` dependency.

/// The parsed intent of a process invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    /// Launch the TUI (default; no recognised args).
    Run,
    /// Print version and exit.
    Version,
    /// Print help and exit.
    Help,
    /// Run the self-updater and exit.
    Update,
    /// Unrecognised argument; carries the offending token.
    Unknown(String),
}

/// Parse process arguments (excluding argv[0]) into a [`Cli`] intent. Only the
/// first token is significant; the subcommands/flags take no operands.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Cli {
    match args.into_iter().next().as_deref() {
        None => Cli::Run,
        Some("--version" | "-V") => Cli::Version,
        Some("--help" | "-h") => Cli::Help,
        Some("update") => Cli::Update,
        Some(other) => Cli::Unknown(other.to_string()),
    }
}

/// The line printed by `--version`.
pub fn version_string() -> String {
    format!("zoid {}", env!("CARGO_PKG_VERSION"))
}

/// The text printed by `--help`.
pub fn help_text() -> String {
    "\
zoid - event-sourced terminal agent

USAGE:
    zoid            Launch the TUI
    zoid update     Download and install the latest release
    zoid --version  Print version
    zoid --help     Print this help"
        .to_string()
}
