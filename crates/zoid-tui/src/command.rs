//! The `:`-command and palette-action vocabulary. Both the command line and the
//! palette resolve to a `Command`; the `zoid` bin executes it (spec §6.5).

use crate::state::{DrawerId, Mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SwitchMode(Mode),
    Quit,
    OpenDrawer(DrawerId),
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
        "files" => Command::OpenDrawer(DrawerId::Files),
        "branch" => Command::OpenDrawer(DrawerId::Branch),
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
        assert_eq!(parse_command(":files"), Command::OpenDrawer(DrawerId::Files));
        assert_eq!(parse_command(":branch"), Command::OpenDrawer(DrawerId::Branch));
    }

    #[test]
    fn unknown_is_captured_verbatim() {
        assert_eq!(parse_command(":wat"), Command::Unknown("wat".into()));
    }
}
