//! The `:`-command and palette-action vocabulary. Both the palette's Direct
//! phase (typing `:` inside `Ctrl+P`) and the palette's Pick rows resolve to a
//! `Command`; the `zoid` bin executes it (spec §6.5).

use crate::state::DrawerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Switch to the named mode (`:mode <name>` / palette).
    SwitchMode(String),
    /// Re-scan mode folders without a restart (`:mode reload`).
    ///
    /// Note: `:mode reload` is parsed as this command *before* it is treated as a
    /// mode name, so a user mode literally named `reload` is unreachable via `:mode
    /// reload` (Direct phase). Reach it via the Shift+Tab cycle or the Ctrl+P
    /// palette's "Switch to reload" row (both build `SwitchMode` directly,
    /// bypassing this parser).
    ReloadModes,
    /// Start the URL import wizard (`:mode import <url>`). Empty = usage hint.
    ModeImport(String),
    /// Start the update wizard for an existing imported mode (`:mode update <name>`).
    ModeUpdate(String),
    /// Install a plugin by bundled id or github URL (`:plugin install <arg>`).
    /// `:mode install superpowers` is a retained alias that produces this with
    /// arg = "superpowers". Empty string = usage hint.
    PluginInstall(String),
    /// List installed/available plugins (`:plugin list`).
    PluginList,
    /// Open the plugin catalog overlay (bare `:plugin`).
    PluginCatalog,
    Quit,
    OpenDrawer(DrawerId),
    NewSession,
    /// Explicitly trigger context compaction on the current event log.
    CompactNow,
    /// Rename the active session. Empty string = "prompt me" (the bin seeds the
    /// palette in Direct phase with `:session rename `); non-empty = apply directly.
    RenameSession(String),
    /// Open the resume-session picker overlay (palette-only; no `:` form).
    ResumeSessionPicker,
    /// Dispatch `task` to a single subagent (spec §6). Empty string = usage hint.
    Delegate(String),
    /// Open the full-screen config overlay (provider/model/economy/secrets).
    OpenConfig,
    /// Open the read-only MCP server status overlay (palette-only; no `:` form).
    OpenMcp,
    /// Enable the companion server (start it if needed, open the browser).
    CompanionEnable,
    /// Disable (stop) the companion server.
    CompanionDisable,
    /// Open the feedback submission overlay (`:feedback`).
    Feedback,
    /// Open the keyboard-shortcuts help overlay (`:help`).
    OpenHelp,
    /// Enter a git worktree (`:worktree <name>`). Empty string = usage hint.
    Worktree(String),
    /// Exit the current worktree (`:worktree exit`).
    WorktreeExit,
    /// Toggle "select mode": flip terminal mouse capture so the whole window
    /// supports native drag-select + terminal copy (`:select` / `:mouse`).
    ToggleSelectMode,
    Unknown(String),
}

/// Parse a command-line string. Accepts an optional leading `:` and surrounding
/// whitespace. Grouped namespaces: `:session`, `:drawer`, `:mode`, `:companion`.
/// Flat commands: `:q`/`:quit`, `:delegate`, `:config`.
pub fn parse_command(raw: &str) -> Command {
    let t = raw.trim().trim_start_matches(':').trim();
    match t {
        // --- :session namespace ---
        "session new" => Command::NewSession,
        "session resume" => Command::ResumeSessionPicker,
        "session rename" => Command::RenameSession(String::new()),
        s if s.starts_with("session rename ") => {
            Command::RenameSession(s["session rename ".len()..].trim().to_string())
        }
        // --- :drawer namespace ---
        "drawer repo" => Command::OpenDrawer(DrawerId::Repo),
        "drawer session" => Command::OpenDrawer(DrawerId::Session),
        "drawer context" => Command::OpenDrawer(DrawerId::Context),
        // --- :mode namespace (existing grouped grammar) ---
        "mode reload" => Command::ReloadModes,
        s if s.starts_with("mode import ") => {
            Command::ModeImport(s["mode import ".len()..].trim().to_string())
        }
        "mode import" => Command::ModeImport(String::new()),
        s if s.starts_with("mode update ") => {
            Command::ModeUpdate(s["mode update ".len()..].trim().to_string())
        }
        "mode update" => Command::ModeUpdate(String::new()),
        "mode install superpowers" => Command::PluginInstall("superpowers".into()),
        "mode" => Command::SwitchMode(String::new()),
        s if s.starts_with("mode ") => Command::SwitchMode(s["mode ".len()..].trim().to_string()),
        // --- :plugin namespace ---
        s if s.starts_with("plugin install ") => {
            Command::PluginInstall(s["plugin install ".len()..].trim().to_string())
        }
        "plugin install" => Command::PluginInstall(String::new()),
        "plugin list" => Command::PluginList,
        "plugin" => Command::PluginCatalog,
        // --- :companion namespace ---
        "companion on" => Command::CompanionEnable,
        "companion off" => Command::CompanionDisable,
        // --- flat commands ---
        "q" | "quit" => Command::Quit,
        "compact" => Command::CompactNow,
        "feedback" => Command::Feedback,
        "config" => Command::OpenConfig,
        "help" => Command::OpenHelp,
        "select" | "mouse" => Command::ToggleSelectMode,
        rest if rest == "delegate" || rest.starts_with("delegate ") => {
            Command::Delegate(rest.strip_prefix("delegate").unwrap().trim().to_string())
        }
        "worktree exit" => Command::WorktreeExit,
        s if s.starts_with("worktree ") => {
            Command::Worktree(s["worktree ".len()..].trim().to_string())
        }
        "worktree" => Command::Worktree(String::new()),
        // --- bare namespaces (session, drawer, companion) fall through to Unknown.
        other => Command::Unknown(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compact_command() {
        assert_eq!(parse_command(":compact"), Command::CompactNow);
        assert_eq!(parse_command("compact"), Command::CompactNow);
    }

    #[test]
    fn parses_select_mode_command() {
        assert_eq!(parse_command(":select"), Command::ToggleSelectMode);
        assert_eq!(parse_command("select"), Command::ToggleSelectMode);
        assert_eq!(parse_command(":mouse"), Command::ToggleSelectMode);
    }

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
    fn bare_mode_is_empty_switch_not_unknown() {
        // Both degenerate forms trim to "mode" and carry an empty target, which
        // the bin renders as a usage hint (not a silent Unknown or empty switch).
        assert_eq!(parse_command(":mode"), Command::SwitchMode(String::new()));
        assert_eq!(
            parse_command(":mode   "),
            Command::SwitchMode(String::new())
        );
    }

    #[test]
    fn parses_session_subcommands() {
        assert_eq!(parse_command(":session new"), Command::NewSession);
        assert_eq!(
            parse_command(":session rename"),
            Command::RenameSession(String::new())
        );
        assert_eq!(
            parse_command(":session rename fix login"),
            Command::RenameSession("fix login".into())
        );
        assert_eq!(
            parse_command(":session resume"),
            Command::ResumeSessionPicker
        );
    }

    #[test]
    fn parses_drawer_subcommands() {
        assert_eq!(
            parse_command(":drawer repo"),
            Command::OpenDrawer(DrawerId::Repo)
        );
        assert_eq!(
            parse_command(":drawer session"),
            Command::OpenDrawer(DrawerId::Session)
        );
        assert_eq!(
            parse_command(":drawer context"),
            Command::OpenDrawer(DrawerId::Context)
        );
    }

    #[test]
    fn parses_companion_subcommands() {
        assert_eq!(parse_command(":companion on"), Command::CompanionEnable);
        assert_eq!(parse_command(":companion off"), Command::CompanionDisable);
    }

    #[test]
    fn bare_namespace_is_unknown() {
        assert_eq!(
            parse_command(":session"),
            Command::Unknown("session".into())
        );
        assert_eq!(parse_command(":drawer"), Command::Unknown("drawer".into()));
        assert_eq!(
            parse_command(":companion"),
            Command::Unknown("companion".into())
        );
    }

    #[test]
    fn drawer_requires_subcommand() {
        assert_eq!(parse_command(":drawer"), Command::Unknown("drawer".into()));
        assert_eq!(
            parse_command(":drawer repo"),
            Command::OpenDrawer(DrawerId::Repo)
        );
    }

    #[test]
    fn companion_requires_on_or_off() {
        assert_eq!(
            parse_command(":companion"),
            Command::Unknown("companion".into())
        );
        assert_eq!(parse_command(":companion on"), Command::CompanionEnable);
        assert_eq!(parse_command(":companion off"), Command::CompanionDisable);
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
    fn parses_feedback_command() {
        assert_eq!(parse_command(":feedback"), Command::Feedback);
        assert_eq!(parse_command("feedback"), Command::Feedback);
    }

    #[test]
    fn parses_config_command() {
        assert_eq!(parse_command(":config"), Command::OpenConfig);
    }

    #[test]
    fn parses_help_command() {
        assert_eq!(parse_command(":help"), Command::OpenHelp);
        assert_eq!(parse_command("help"), Command::OpenHelp);
    }

    #[test]
    fn mode_import_parses() {
        assert_eq!(
            parse_command(":mode import github.com/o/r/tree/main/skills"),
            Command::ModeImport("github.com/o/r/tree/main/skills".into())
        );
    }

    #[test]
    fn mode_update_parses() {
        assert_eq!(
            parse_command(":mode update Superpowers"),
            Command::ModeUpdate("Superpowers".into())
        );
    }

    #[test]
    fn bare_mode_import_is_empty_arg() {
        assert_eq!(
            parse_command(":mode import"),
            Command::ModeImport(String::new())
        );
    }

    #[test]
    fn mode_reload_still_wins_over_import() {
        assert_eq!(parse_command(":mode reload"), Command::ReloadModes);
    }

    #[test]
    fn mode_install_superpowers_aliases_to_plugin_install() {
        assert_eq!(
            parse_command(":mode install superpowers"),
            Command::PluginInstall("superpowers".into())
        );
        assert_eq!(
            parse_command("mode install superpowers"),
            Command::PluginInstall("superpowers".into())
        );
    }

    #[test]
    fn mode_install_does_not_shadow_switch_to_a_mode_named_install() {
        // "mode install foo" is NOT the superpowers installer — it stays a switch.
        assert_eq!(
            parse_command(":mode install foo"),
            Command::SwitchMode("install foo".into())
        );
    }

    #[test]
    fn parses_plugin_install_id_and_url() {
        assert_eq!(
            parse_command(":plugin install superpowers"),
            Command::PluginInstall("superpowers".into())
        );
        assert_eq!(
            parse_command(":plugin install github.com/o/r/tree/main/skills"),
            Command::PluginInstall("github.com/o/r/tree/main/skills".into())
        );
        assert_eq!(
            parse_command(":plugin install"),
            Command::PluginInstall(String::new())
        );
    }

    #[test]
    fn worktree_enter_parses_name() {
        assert_eq!(
            parse_command(":worktree feature-x"),
            Command::Worktree("feature-x".into())
        );
    }

    #[test]
    fn worktree_exit_parses() {
        assert_eq!(parse_command(":worktree exit"), Command::WorktreeExit);
    }

    #[test]
    fn worktree_no_arg_is_empty() {
        assert_eq!(parse_command(":worktree"), Command::Worktree(String::new()));
    }

    #[test]
    fn parses_plugin_list_and_bare_plugin() {
        assert_eq!(parse_command(":plugin list"), Command::PluginList);
        assert_eq!(parse_command(":plugin"), Command::PluginCatalog);
        assert_eq!(parse_command(":plugin "), Command::PluginCatalog);
        assert_eq!(
            parse_command(":plugin install ok-skills"),
            Command::PluginInstall("ok-skills".into())
        );
    }
}
