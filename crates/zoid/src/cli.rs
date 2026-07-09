//! Minimal hand-rolled CLI parsing for the `zoid` binary (spec §2 component A).
//! Three flags and one subcommand do not justify a `clap` dependency.

/// The parsed intent of a process invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    /// Launch the TUI (default; no recognised args). `companion` starts the
    /// companion server at boot when set. `new` forces a fresh session. `resume`
    /// carries a session id (full ULID or last-4) to resume directly.
    Run {
        companion: bool,
        new: bool,
        resume: Option<String>,
        yolo: bool,
    },
    /// Print version and exit.
    Version,
    /// Print help and exit.
    Help,
    /// Run the self-updater and exit.
    Update,
    /// Remove zoid's data (sessions, config, cache, secrets); with `purge`,
    /// also delete the binary. Exits after running.
    Uninstall {
        purge: bool,
    },
    /// Unrecognised argument; carries the offending token.
    Unknown(String),
}

/// Parse process arguments (excluding argv[0]) into a [`Cli`] intent.
///
/// Recognised flags (any order): `--companion`, `--new`, `--resume <id>`.
/// `--new` and `--resume` are mutually exclusive. `--resume` requires exactly
/// one following argument (the id). Subcommands (`update`) and standalone flags
/// (`--version`, `--help`) take precedence and exit immediately.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Cli {
    let args: Vec<String> = args.into_iter().collect();
    // Subcommands and standalone flags take precedence (first token only).
    match args.first().map(|s| s.as_str()) {
        Some("--version") | Some("-V") => return Cli::Version,
        Some("--help") | Some("-h") => return Cli::Help,
        Some("update") => return Cli::Update,
        Some("uninstall") => {
            // Only `--purge` may follow; anything else is an error.
            let mut purge = false;
            for a in &args[1..] {
                match a.as_str() {
                    "--purge" => purge = true,
                    other => return Cli::Unknown(other.to_string()),
                }
            }
            return Cli::Uninstall { purge };
        }
        _ => {}
    }

    let mut companion = false;
    let mut new = false;
    let mut resume: Option<String> = None;
    let mut yolo = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--companion" => companion = true,
            "--yolo" => yolo = true,
            "--new" => new = true,
            "--resume" => {
                // Require exactly one id argument following --resume.
                if i + 1 >= args.len() || args[i + 1].starts_with("--") {
                    return Cli::Unknown("--resume".to_string());
                }
                resume = Some(args[i + 1].clone());
                i += 1; // consume the id
            }
            other => return Cli::Unknown(other.to_string()),
        }
        i += 1;
    }

    // --new and --resume are mutually exclusive.
    if new && resume.is_some() {
        return Cli::Unknown("--new --resume".to_string());
    }

    Cli::Run {
        companion,
        new,
        resume,
        yolo,
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
    zoid                      Launch the TUI
    zoid --new                Start a fresh session (no picker)
    zoid --resume <id>        Resume a session by ULID (full or last-4)
    zoid --companion          Launch with the companion browser view enabled
    zoid --yolo               Disable all approval prompts (dangerous)
    zoid update               Download and install the latest release
    zoid uninstall            Remove zoid's data (sessions, config, cache)
    zoid uninstall --purge    Also delete the zoid binary
    zoid --version            Print version
    zoid --help               Print this help"
        .to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_companion_flag() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string()]),
            super::Cli::Run { companion: true, new: false, resume: None, yolo: false }
        );
        assert_eq!(
            super::parse_args(Vec::<String>::new()),
            super::Cli::Run { companion: false, new: false, resume: None, yolo: false }
        );
        assert_eq!(
            super::parse_args(vec!["--version".to_string()]),
            super::Cli::Version
        );
    }

    #[test]
    fn parses_new_flag() {
        assert_eq!(
            super::parse_args(vec!["--new".to_string()]),
            super::Cli::Run { companion: false, new: true, resume: None, yolo: false }
        );
    }

    #[test]
    fn parses_resume_with_id() {
        assert_eq!(
            super::parse_args(vec!["--resume".to_string(), "01AB".to_string()]),
            super::Cli::Run { companion: false, new: false, resume: Some("01AB".to_string()), yolo: false }
        );
    }

    #[test]
    fn parses_companion_and_new_together() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string(), "--new".to_string()]),
            super::Cli::Run { companion: true, new: true, resume: None, yolo: false }
        );
    }

    #[test]
    fn parses_companion_and_resume_together() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string(), "--resume".to_string(), "XYZW".to_string()]),
            super::Cli::Run { companion: true, new: false, resume: Some("XYZW".to_string()), yolo: false }
        );
    }

    #[test]
    fn new_and_resume_together_is_unknown() {
        // Mutually exclusive — both flags together must be an error.
        let result = super::parse_args(vec!["--new".to_string(), "--resume".to_string(), "01AB".to_string()]);
        assert!(matches!(result, super::Cli::Unknown(_)), "--new + --resume together must be an error");
    }

    #[test]
    fn resume_without_id_is_unknown() {
        let result = super::parse_args(vec!["--resume".to_string()]);
        assert!(matches!(result, super::Cli::Unknown(_)), "--resume without an id must be an error");
    }

    #[test]
    fn parses_yolo_flag() {
        assert_eq!(
            super::parse_args(vec!["--yolo".to_string()]),
            super::Cli::Run { companion: false, new: false, resume: None, yolo: true }
        );
    }

    #[test]
    fn yolo_combines_with_companion() {
        assert_eq!(
            super::parse_args(vec!["--companion".to_string(), "--yolo".to_string()]),
            super::Cli::Run { companion: true, new: false, resume: None, yolo: true }
        );
    }

    #[test]
    fn parses_uninstall_and_purge() {
        assert_eq!(
            super::parse_args(vec!["uninstall".to_string()]),
            super::Cli::Uninstall { purge: false }
        );
        assert_eq!(
            super::parse_args(vec!["uninstall".to_string(), "--purge".to_string()]),
            super::Cli::Uninstall { purge: true }
        );
    }

    #[test]
    fn uninstall_with_unknown_flag_is_unknown() {
        let r = super::parse_args(vec!["uninstall".to_string(), "--everything".to_string()]);
        assert!(matches!(r, super::Cli::Unknown(_)), "unknown uninstall flag must error");
    }
}
