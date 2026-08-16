use zoid::cli::{parse_args, version_string, Cli};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_args_launches_tui() {
    assert_eq!(
        parse_args(args(&[])),
        Cli::Run {
            companion: false,
            new: false,
            resume: None,
            yolo: false
        }
    );
}

#[test]
fn version_flags() {
    assert_eq!(parse_args(args(&["--version"])), Cli::Version);
    assert_eq!(parse_args(args(&["-V"])), Cli::Version);
}

#[test]
fn help_flags() {
    assert_eq!(parse_args(args(&["--help"])), Cli::Help);
    assert_eq!(parse_args(args(&["-h"])), Cli::Help);
}

#[test]
fn update_subcommand() {
    assert_eq!(parse_args(args(&["update"])), Cli::Update);
}

#[test]
fn unknown_arg_is_reported() {
    assert_eq!(
        parse_args(args(&["--bogus"])),
        Cli::Unknown("--bogus".to_string())
    );
}

#[test]
fn version_string_tracks_pkg_version() {
    assert_eq!(
        version_string(),
        format!("zoid {}", env!("CARGO_PKG_VERSION"))
    );
}
