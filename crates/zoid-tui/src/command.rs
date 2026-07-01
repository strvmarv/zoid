//! The `:`-command and palette-action vocabulary. Both the command line and the
//! palette resolve to a `Command`; the `zoid` bin executes it (spec §6.5).

use crate::state::{DrawerId, Mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SwitchMode(Mode),
    Quit,
    OpenDrawer(DrawerId),
    NewSession,
    /// Rename the active session. Empty string = "prompt me" (the bin opens the
    /// command line seeded with `rename `); non-empty = apply directly.
    RenameSession(String),
    /// Open the resume-session picker overlay (palette-only; no `:` form).
    ResumeSessionPicker,
    Unknown(String),
}

/// Parse a command-line string. Accepts an optional leading `:` and surrounding
/// whitespace. `:build`/`:chat`, `:q`/`:quit`, `:files`/`:branch`.
pub fn parse_command(raw: &str) -> Command {
    let t = raw.trim().trim_start_matches(':').trim();
    match t {
        "build" => Command::SwitchMode(Mode::Build),
        "chat" => Command::SwitchMode(Mode::Chat),
        "q" | "quit" => Command::Quit,
        "files" => Command::OpenDrawer(DrawerId::Repo),
        "branch" => Command::OpenDrawer(DrawerId::Session),
        "new" => Command::NewSession,
        "rename" => Command::RenameSession(String::new()),
        s if s.starts_with("rename ") => Command::RenameSession(s["rename ".len()..].trim().to_string()),
        other => Command::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_commands_with_or_without_colon() {
        assert_eq!(parse_command(":build"), Command::SwitchMode(Mode::Build));
        assert_eq!(parse_command("chat"), Command::SwitchMode(Mode::Chat));
        assert_eq!(parse_command("  :q "), Command::Quit);
        assert_eq!(parse_command(":files"), Command::OpenDrawer(DrawerId::Repo));
        assert_eq!(parse_command(":branch"), Command::OpenDrawer(DrawerId::Session));
    }

    #[test]
    fn unknown_is_captured_verbatim() {
        assert_eq!(parse_command(":wat"), Command::Unknown("wat".into()));
    }

    #[test]
    fn parses_session_commands() {
        assert_eq!(parse_command(":new"), Command::NewSession);
        assert_eq!(parse_command("new"), Command::NewSession);
        assert_eq!(parse_command(":rename"), Command::RenameSession(String::new()));
        assert_eq!(parse_command(":rename fix login"), Command::RenameSession("fix login".into()));
    }
}
