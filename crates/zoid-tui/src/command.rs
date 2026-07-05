//! The `:`-command and palette-action vocabulary. Both the command line and the
//! palette resolve to a `Command`; the `zoid` bin executes it (spec §6.5).

use crate::state::DrawerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Switch to the named mode (`:mode <name>` / palette).
    SwitchMode(String),
    /// Re-scan mode folders without a restart (`:mode reload`).
    ReloadModes,
    Quit,
    OpenDrawer(DrawerId),
    NewSession,
    /// Rename the active session. Empty string = "prompt me" (the bin opens the
    /// command line seeded with `rename `); non-empty = apply directly.
    RenameSession(String),
    /// Open the resume-session picker overlay (palette-only; no `:` form).
    ResumeSessionPicker,
    /// Dispatch `task` to a single subagent (spec §6). Empty string = usage hint.
    Delegate(String),
    /// Open the full-screen config overlay (provider/model/economy/secrets).
    OpenConfig,
    /// Enable the companion server (start it if needed, open the browser).
    CompanionEnable,
    /// Disable (stop) the companion server.
    CompanionDisable,
    Unknown(String),
}

/// Parse a command-line string. Accepts an optional leading `:` and surrounding
/// whitespace. `:mode <name>`/`:mode reload`, `:q`/`:quit`, `:repo`/`:session`/`:context`.
pub fn parse_command(raw: &str) -> Command {
    let t = raw.trim().trim_start_matches(':').trim();
    match t {
        "mode reload" => Command::ReloadModes,
        s if s.starts_with("mode ") => Command::SwitchMode(s["mode ".len()..].trim().to_string()),
        "q" | "quit" => Command::Quit,
        "repo" => Command::OpenDrawer(DrawerId::Repo),
        "session" => Command::OpenDrawer(DrawerId::Session),
        "context" => Command::OpenDrawer(DrawerId::Context),
        "new" => Command::NewSession,
        "rename" => Command::RenameSession(String::new()),
        s if s.starts_with("rename ") => {
            Command::RenameSession(s["rename ".len()..].trim().to_string())
        }
        rest if rest == "delegate" || rest.starts_with("delegate ") => {
            Command::Delegate(rest.strip_prefix("delegate").unwrap().trim().to_string())
        }
        "config" => Command::OpenConfig,
        "companion" => Command::CompanionEnable,
        "companion off" => Command::CompanionDisable,
        other => Command::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_commands_with_or_without_colon() {
        assert_eq!(
            parse_command(":mode Superpowers"),
            Command::SwitchMode("Superpowers".into())
        );
        assert_eq!(parse_command("mode reload"), Command::ReloadModes);
        assert_eq!(parse_command("  :q "), Command::Quit);
    }

    #[test]
    fn parses_drawer_toggle_commands() {
        assert_eq!(parse_command(":repo"), Command::OpenDrawer(DrawerId::Repo));
        assert_eq!(
            parse_command(":session"),
            Command::OpenDrawer(DrawerId::Session)
        );
        assert_eq!(
            parse_command(":context"),
            Command::OpenDrawer(DrawerId::Context)
        );
    }

    #[test]
    fn unknown_is_captured_verbatim() {
        assert_eq!(parse_command(":wat"), Command::Unknown("wat".into()));
    }

    #[test]
    fn parses_delegate_with_task() {
        assert_eq!(
            parse_command(":delegate add a test for parse()"),
            Command::Delegate("add a test for parse()".into())
        );
        assert_eq!(parse_command(":delegate"), Command::Delegate(String::new()));
    }

    #[test]
    fn parses_config_command() {
        assert_eq!(parse_command(":config"), Command::OpenConfig);
    }

    #[test]
    fn parses_companion_commands() {
        assert_eq!(parse_command("companion"), Command::CompanionEnable);
        assert_eq!(parse_command(":companion off"), Command::CompanionDisable);
    }

    #[test]
    fn parses_session_commands() {
        assert_eq!(parse_command(":new"), Command::NewSession);
        assert_eq!(parse_command("new"), Command::NewSession);
        assert_eq!(
            parse_command(":rename"),
            Command::RenameSession(String::new())
        );
        assert_eq!(
            parse_command(":rename fix login"),
            Command::RenameSession("fix login".into())
        );
    }
}
