//! Minimal hand-rolled CLI parsing for the `zoid` binary (spec §2 component A).
//! Three flags and one subcommand do not justify a `clap` dependency.

/// The parsed intent of a process invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    /// Launch the TUI (default; no recognised args). `companion` starts the
    /// companion server at boot when set.
    Run { companion: bool },
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
        None => Cli::Run { companion: false },
        Some("--version" | "-V") => Cli::Version,
        Some("--help" | "-h") => Cli::Help,
        Some("update") => Cli::Update,
        Some("--companion") => Cli::Run { companion: true },
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
    zoid --help     Print this help
    zoid --companion  Launch with the companion browser view enabled"
        .to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_companion_flag() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string()]),
            super::Cli::Run { companion: true }
        );
        assert_eq!(
            super::parse_args(Vec::<String>::new()),
            super::Cli::Run { companion: false }
        );
        assert_eq!(
            super::parse_args(vec!["--version".to_string()]),
            super::Cli::Version
        );
    }
}
