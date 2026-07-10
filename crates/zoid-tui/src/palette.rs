//! The command palette's item set + fuzzy filtering. A flat, curated,
//! runnable-only list (VSCode-style): typing filters/re-ranks it, the top match
//! is auto-selected, Enter runs it. Parameterized commands (Rename) capture
//! their argument inline via `ArgKind`. Pure; rendering lives in `render.rs`.

use crate::command::Command;
use crate::state::ShellState;

/// A parameterized palette command's argument-capture flow. The palette enters
/// an inline "Arg" phase to collect the argument, then builds the final command.
/// Extend with new variants (e.g. `Delegate`) as more commands take arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind {
    Rename,
    Delegate,
    ModeImport,
    ModeUpdate,
}

impl ArgKind {
    /// The label shown on the argument-entry prompt line.
    pub fn prompt(&self) -> &'static str {
        match self {
            ArgKind::Rename => "Rename to",
            ArgKind::Delegate => "Delegate task",
            ArgKind::ModeImport => "Import mode from URL",
            ArgKind::ModeUpdate => "Update mode",
        }
    }

    /// Build the final `Command` from the captured argument text.
    pub fn build(&self, input: String) -> Command {
        match self {
            ArgKind::Rename => Command::RenameSession(input),
            ArgKind::Delegate => Command::Delegate(input),
            ArgKind::ModeImport => Command::ModeImport(input),
            ArgKind::ModeUpdate => Command::ModeUpdate(input),
        }
    }
}

/// What a given `PaletteState` means at this instant. Pure — used by rendering
/// to branch on the `:` prefix without storing a phase. Routing derives the
/// same classification inline (`in_direct: query.starts_with(':')`, a boolean)
/// to avoid the per-keystroke `parse_command` cost. `Arg` is `PaletteStage::Arg`,
/// not a `Phase` variant — it's a real stage transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Empty or non-`:` query → fuzzy ranked list.
    Pick,
    /// Query starts with `:` → live `parse_command` preview, list hidden.
    Direct { cmd: crate::command::Command },
    /// `PaletteStage::Arg` is active → inline argument entry.
    Arg,
}

/// Resolve the current phase from the palette state. Pure. Calls
/// `parse_command`, which returns an owned `Command` — one `String` heap
/// allocation per frame for `:`-prefix queries (e.g. `:mode Build`). The render
/// path needs the `Command` regardless.
pub fn resolve_phase(state: &crate::state::PaletteState) -> Phase {
    match state.stage {
        crate::state::PaletteStage::Arg { .. } => Phase::Arg,
        crate::state::PaletteStage::Pick => {
            if state.query.starts_with(':') {
                Phase::Direct {
                    cmd: crate::command::parse_command(&state.query),
                }
            } else {
                Phase::Pick
            }
        }
    }
}

/// The filter text for the current Direct stage: everything after the last
/// space in the buffer (minus the `:` prefix). Empty after a trailing space
/// (shows all rows for the next stage). Pure.
pub fn direct_filter(query: &str) -> &str {
    let t = query.strip_prefix(':').unwrap_or(query);
    match t.rsplit_once(' ') {
        Some((_, last)) => last,
        None => t,
    }
}

/// The three-stage Direct-phase list, derived from the buffer. Pure.
///
/// - Stage 1 (no complete namespace): top-level namespaces + flat commands.
/// - Stage 2 (`:ns `): subcommands for the namespace.
/// - Stage 3 (`:ns sub `): arg completions for a parameterized subcommand.
///
/// Stages are derived from `query` — no stored stage. Empty lists (free-text
/// args like `:delegate `, `:mode import `) mean the user types freely.
pub fn direct_items(state: &ShellState) -> Vec<PaletteItem> {
    let t = state
        .palette
        .query
        .strip_prefix(':')
        .unwrap_or(&state.palette.query);
    let has_trailing_space = t.ends_with(' ');
    let trimmed = t.trim_end();
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();

    match (tokens.as_slice(), has_trailing_space) {
        // Stage 1: bare colon or partial command word (no complete namespace).
        ([], _) | ([_], false) => stage1_items(),
        // Stage 2: `:ns ` (one recognized namespace + trailing space).
        ([ns], true) => stage2_items(ns, state),
        // Stage 3: `:ns sub ` (parameterized subcommand + trailing space).
        ([ns, sub], true) => stage3_items(ns, sub, state),
        // Stage 3 with a partial arg typed (`:session rename fi`) — still Stage 3;
        // return the arg list and let `selectable_matches` filter by `direct_filter`.
        ([ns, sub, ..], _) => stage3_items(ns, sub, state),
    }
}

fn stage1_items() -> Vec<PaletteItem> {
    vec![
        PaletteItem {
            label: "session".into(),
            command: Command::Unknown("session".into()),
        },
        PaletteItem {
            label: "drawer".into(),
            command: Command::Unknown("drawer".into()),
        },
        PaletteItem {
            label: "mode".into(),
            command: Command::SwitchMode(String::new()),
        },
        PaletteItem {
            label: "companion".into(),
            command: Command::Unknown("companion".into()),
        },
        PaletteItem {
            label: "compact".into(),
            command: Command::CompactNow,
        },
        PaletteItem {
            label: "delegate".into(),
            command: Command::Delegate(String::new()),
        },
        PaletteItem {
            label: "config".into(),
            command: Command::OpenConfig,
        },
        PaletteItem {
            label: "q".into(),
            command: Command::Quit,
        },
        PaletteItem {
            label: "quit".into(),
            command: Command::Quit,
        },
    ]
}

fn stage2_items(ns: &str, state: &ShellState) -> Vec<PaletteItem> {
    use crate::state::DrawerId;
    match ns {
        "session" => vec![
            PaletteItem {
                label: "new".into(),
                command: Command::NewSession,
            },
            PaletteItem {
                label: "rename".into(),
                command: Command::RenameSession(String::new()),
            },
            PaletteItem {
                label: "resume".into(),
                command: Command::ResumeSessionPicker,
            },
        ],
        "drawer" => vec![
            PaletteItem {
                label: "repo".into(),
                command: Command::OpenDrawer(DrawerId::Repo),
            },
            PaletteItem {
                label: "session".into(),
                command: Command::OpenDrawer(DrawerId::Session),
            },
            PaletteItem {
                label: "context".into(),
                command: Command::OpenDrawer(DrawerId::Context),
            },
        ],
        "mode" => {
            let mut rows = vec![
                PaletteItem {
                    label: "reload".into(),
                    command: Command::ReloadModes,
                },
                PaletteItem {
                    label: "import".into(),
                    command: Command::ModeImport(String::new()),
                },
                PaletteItem {
                    label: "update".into(),
                    command: Command::ModeUpdate(String::new()),
                },
                PaletteItem {
                    label: "install superpowers".into(),
                    command: Command::PluginInstall("superpowers".into()),
                },
            ];
            rows.extend(
                state
                    .mode_names
                    .iter()
                    .filter(|n| n.as_str() != state.active_mode)
                    .map(|n| PaletteItem {
                        label: n.clone(),
                        command: Command::SwitchMode(n.clone()),
                    }),
            );
            rows
        }
        "companion" => vec![
            PaletteItem {
                label: "on".into(),
                command: Command::CompanionEnable,
            },
            PaletteItem {
                label: "off".into(),
                command: Command::CompanionDisable,
            },
        ],
        _ => vec![],
    }
}

fn stage3_items(ns: &str, sub: &str, state: &ShellState) -> Vec<PaletteItem> {
    match (ns, sub) {
        ("session", "rename") => state
            .sessions
            .iter()
            .map(|s| PaletteItem {
                label: s.clone(),
                command: Command::RenameSession(s.clone()),
            })
            .collect(),
        ("mode", "update") => state
            .mode_names
            .iter()
            .map(|n| PaletteItem {
                label: n.clone(),
                command: Command::ModeUpdate(n.clone()),
            })
            .collect(),
        // Free-text args (delegate, mode import) — no completion list.
        _ => vec![],
    }
}

/// What Enter should do in Direct phase with the highlighted row. Pure —
/// the bin calls this on `PaletteRun` when the buffer starts with `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectAction {
    /// Set `query` to this text and stay open (advance to the next stage).
    Fill(String),
    /// Close the overlay and run this command immediately.
    Run(Command),
    /// No row selected / empty list — fall through to `parse_command(query)`.
    Nothing,
}

/// Resolve the highlighted Direct row to a fill-or-run action. Pure.
pub fn direct_selected_action(state: &ShellState) -> DirectAction {
    let items = direct_items(state);
    let filter = direct_filter(&state.palette.query);
    let matches = selectable_matches(&items, filter);
    if matches.is_empty() {
        return DirectAction::Nothing;
    }
    let sel = nav(state.palette.selected, 0, matches.len());
    let item = &items[matches[sel]];

    // Decide Fill vs Run based on the row's command:
    // - `Unknown` (namespace) or a bare parameterized sentinel
    //   (`RenameSession("")`, `ModeImport("")`, `ModeUpdate("")`, `Delegate("")`)
    //   → Fill to the next stage.
    // - Anything else → Run.
    let is_fill = match &item.command {
        Command::Unknown(_) => true,
        Command::SwitchMode(s) if s.is_empty() => true,
        Command::RenameSession(s) if s.is_empty() => true,
        Command::ModeImport(s) if s.is_empty() => true,
        Command::ModeUpdate(s) if s.is_empty() => true,
        Command::Delegate(s) if s.is_empty() => true,
        _ => false,
    };

    if is_fill {
        // Construct the next-stage buffer: `:` + the accepted prefix + label + " ".
        // The accepted prefix is everything in the query up to and including the
        // last space (or just `:` if we're at Stage 1 with no space yet).
        let q = &state.palette.query;
        let prefix = q.strip_prefix(':').unwrap_or(q);
        let accepted = match prefix.rsplit_once(' ') {
            Some((before, _)) => format!(":{} {}", before.trim_end(), item.label),
            None => format!(":{}", item.label),
        };
        DirectAction::Fill(format!("{} ", accepted))
    } else {
        DirectAction::Run(item.command.clone())
    }
}

/// Which inline-argument flow (if any) a command needs when chosen from the
/// palette. Pure — the bin uses this to decide the Pick→Arg transition.
pub fn arg_kind_for(cmd: &Command) -> Option<ArgKind> {
    match cmd {
        Command::RenameSession(_) => Some(ArgKind::Rename),
        Command::Delegate(_) => Some(ArgKind::Delegate),
        Command::ModeImport(_) => Some(ArgKind::ModeImport),
        Command::ModeUpdate(_) => Some(ArgKind::ModeUpdate),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    pub command: Command,
}

/// The flat, curated, runnable-only item set for `mode`. Fixed order at rest;
/// `selectable_matches` re-ranks it by fuzzy score while the user types. Non-
/// implemented actions (fork/undo/pin/evict/recipe) are intentionally omitted —
/// re-add them here with their real `Command` when those features ship.
///
/// `companion_on` is the live companion-server state (source of truth: the bin's
/// running server); the companion row offers the *opposite* action, mirroring
/// how the mode rows offer every mode other than the active one.
pub fn all_items(active_mode: &str, mode_names: &[String], companion_on: bool) -> Vec<PaletteItem> {
    use crate::state::DrawerId;

    // One "Switch to <mode>" row per mode other than the active one, in order,
    // then a reload row.
    let mut mode_rows: Vec<PaletteItem> = mode_names
        .iter()
        .filter(|n| n.as_str() != active_mode)
        .map(|n| PaletteItem {
            label: format!("Switch to {n}"),
            command: Command::SwitchMode(n.clone()),
        })
        .collect();
    mode_rows.push(PaletteItem {
        label: "Reload modes".to_string(),
        command: Command::ReloadModes,
    });
    // The companion row offers the *opposite* of the current state.
    let (companion_label, companion_cmd) = if companion_on {
        ("Disable companion", Command::CompanionDisable)
    } else {
        ("Enable companion", Command::CompanionEnable)
    };
    let mut items = vec![
        PaletteItem {
            label: "New session".to_string(),
            command: Command::NewSession,
        },
        PaletteItem {
            label: "Resume session…".to_string(),
            command: Command::ResumeSessionPicker,
        },
        PaletteItem {
            label: "Rename session…".to_string(),
            command: Command::RenameSession(String::new()),
        },
        PaletteItem {
            label: "Delegate task…".to_string(),
            command: Command::Delegate(String::new()),
        },
        PaletteItem {
            label: "Import mode from URL…".to_string(),
            command: Command::ModeImport(String::new()),
        },
        PaletteItem {
            label: "Update mode…".to_string(),
            command: Command::ModeUpdate(String::new()),
        },
    ];
    items.extend(mode_rows);
    items.push(PaletteItem {
        label: "Toggle repo drawer".to_string(),
        command: Command::OpenDrawer(DrawerId::Repo),
    });
    items.push(PaletteItem {
        label: "Toggle session drawer".to_string(),
        command: Command::OpenDrawer(DrawerId::Session),
    });
    items.push(PaletteItem {
        label: "Toggle context drawer".to_string(),
        command: Command::OpenDrawer(DrawerId::Context),
    });
    items.push(PaletteItem {
        label: "Open settings".to_string(),
        command: Command::OpenConfig,
    });
    items.push(PaletteItem {
        label: "MCP servers…".to_string(),
        command: Command::OpenMcp,
    });
    items.push(PaletteItem {
        label: "Submit feedback…".to_string(),
        command: Command::Feedback,
    });
    items.push(PaletteItem {
        label: companion_label.to_string(),
        command: companion_cmd,
    });
    items.push(PaletteItem {
        label: "Quit zoid".to_string(),
        command: Command::Quit,
    });
    items
}

/// Case-insensitive fuzzy score: `Some(higher = better)` if `query` is a
/// subsequence of `label`; `None` otherwise. Empty query matches everything.
/// Contiguous substring beats scattered subsequence; earlier match beats later.
pub fn fuzzy_score(label: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = label.to_lowercase().chars().collect();
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    // Contiguous-substring bonus.
    let label_l = label.to_lowercase();
    if let Some(pos) = label_l.find(&query.to_lowercase()) {
        return Some(1000 - pos as i32);
    }
    // Scattered subsequence.
    let mut qi = 0usize;
    let mut first: Option<usize> = None;
    for (i, c) in hay.iter().enumerate() {
        if qi < needle.len() && *c == needle[qi] {
            if first.is_none() {
                first = Some(i);
            }
            qi += 1;
        }
    }
    if qi == needle.len() {
        Some(100 - first.unwrap_or(0) as i32)
    } else {
        None
    }
}

/// Indices into `items` matching `query`, ranked best-first (stable on ties).
/// Empty query returns every row in curated order.
pub fn selectable_matches(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(&it.label, query).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// Move a selection index by `delta`, wrapping at both ends (opencode-style):
/// stepping past the last row lands on the first, and up from the first lands
/// on the last. Returns 0 for an empty list. `len` is the row count.
pub fn nav(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len_i = len as i64;
    let next = selected as i64 + delta as i64;
    next.rem_euclid(len_i) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_outranks_subsequence() {
        let sub = fuzzy_score("Toggle repo drawer", "repo").unwrap();
        let seq = fuzzy_score("Switch to Build", "sib").unwrap();
        assert!(sub > seq);
    }

    #[test]
    fn no_match_is_none() {
        assert!(fuzzy_score("Quit zoid", "zzz").is_none());
    }

    fn names() -> Vec<String> {
        vec!["Chat".into(), "Build".into()]
    }

    #[test]
    fn all_items_is_flat_curated() {
        let items = all_items("Chat", &names(), false);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "New session",
                "Resume session…",
                "Rename session…",
                "Delegate task…",
                "Import mode from URL…",
                "Update mode…",
                "Switch to Build",
                "Reload modes",
                "Toggle repo drawer",
                "Toggle session drawer",
                "Toggle context drawer",
                "Open settings",
                "MCP servers…",
                "Submit feedback…",
                "Enable companion",
                "Quit zoid",
            ]
        );
        // With the companion running, the row offers the opposite action.
        let items_on = all_items("Chat", &names(), true);
        let on: Vec<&str> = items_on.iter().map(|i| i.label.as_str()).collect();
        assert!(on.contains(&"Disable companion"));
        assert!(!on.contains(&"Enable companion"));
    }

    #[test]
    fn mode_rows_offer_every_other_mode_plus_reload() {
        let items = all_items("Chat", &names(), false);
        // A "Switch to <name>" row for every mode other than the active one.
        assert_eq!(
            items
                .iter()
                .find(|i| i.command == Command::SwitchMode("Build".into()))
                .map(|i| i.label.as_str()),
            Some("Switch to Build")
        );
        // The active mode does not get a switch row.
        assert!(items
            .iter()
            .all(|i| i.command != Command::SwitchMode("Chat".into())));
        // …and a reload row is always present.
        assert!(items.iter().any(|i| i.command == Command::ReloadModes));

        // From Build, Chat gets the switch row instead.
        let items = all_items("Build", &names(), false);
        assert_eq!(
            items
                .iter()
                .find(|i| i.command == Command::SwitchMode("Chat".into()))
                .map(|i| i.label.as_str()),
            Some("Switch to Chat")
        );
    }

    #[test]
    fn empty_query_returns_all_rows_in_order() {
        let items = all_items("Chat", &names(), false);
        let idxs = selectable_matches(&items, "");
        assert_eq!(idxs, (0..items.len()).collect::<Vec<_>>());
    }

    #[test]
    fn typing_reranks_best_match_first() {
        let items = all_items("Chat", &names(), false);
        let idxs = selectable_matches(&items, "comp");
        assert_eq!(items[idxs[0]].label, "Enable companion");
        let idxs = selectable_matches(&items, "build");
        assert_eq!(items[idxs[0]].label, "Switch to Build");
    }

    #[test]
    fn nav_wraps() {
        // Down past the last row wraps to the top; up from the top wraps to the last.
        assert_eq!(nav(2, 1, 3), 0);
        assert_eq!(nav(0, -1, 3), 2);
        // Interior moves are unchanged.
        assert_eq!(nav(1, 1, 3), 2);
        assert_eq!(nav(1, -1, 3), 0);
        // Empty list is a no-op (no panic, no divide-by-zero).
        assert_eq!(nav(0, 1, 0), 0);
        // A multi-step delta still lands in range.
        assert_eq!(nav(0, 5, 3), 2);
    }

    #[test]
    fn arg_kind_for_flags_all_parameterized_commands() {
        assert_eq!(
            arg_kind_for(&Command::RenameSession(String::new())),
            Some(ArgKind::Rename)
        );
        assert_eq!(
            arg_kind_for(&Command::Delegate(String::new())),
            Some(ArgKind::Delegate)
        );
        assert_eq!(
            arg_kind_for(&Command::ModeImport(String::new())),
            Some(ArgKind::ModeImport)
        );
        assert_eq!(
            arg_kind_for(&Command::ModeUpdate(String::new())),
            Some(ArgKind::ModeUpdate)
        );
    }

    #[test]
    fn arg_kind_for_returns_none_for_zero_arg_commands() {
        assert_eq!(arg_kind_for(&Command::CompanionEnable), None);
        assert_eq!(arg_kind_for(&Command::Quit), None);
        assert_eq!(arg_kind_for(&Command::NewSession), None);
        assert_eq!(arg_kind_for(&Command::ResumeSessionPicker), None);
        assert_eq!(arg_kind_for(&Command::OpenConfig), None);
        assert_eq!(arg_kind_for(&Command::ReloadModes), None);
        assert_eq!(
            arg_kind_for(&Command::OpenDrawer(crate::state::DrawerId::Repo)),
            None
        );
        assert_eq!(arg_kind_for(&Command::SwitchMode("Build".into())), None);
    }

    #[test]
    fn direct_filter_partial_command_word() {
        assert_eq!(direct_filter(":mo"), "mo");
        assert_eq!(direct_filter(":q"), "q");
    }

    #[test]
    fn direct_filter_after_namespace_space_is_empty() {
        assert_eq!(direct_filter(":session "), "");
        assert_eq!(direct_filter(":drawer "), "");
    }

    #[test]
    fn direct_filter_partial_subcommand() {
        assert_eq!(direct_filter(":session re"), "re");
        assert_eq!(direct_filter(":drawer r"), "r");
    }

    #[test]
    fn direct_filter_after_subcommand_space_is_empty() {
        assert_eq!(direct_filter(":session rename "), "");
        assert_eq!(direct_filter(":mode import "), "");
    }

    #[test]
    fn direct_filter_typing_arg() {
        assert_eq!(direct_filter(":session rename fix"), "fix");
        assert_eq!(direct_filter(":session rename fix login"), "login");
    }

    fn shell_for_direct(query: &str) -> ShellState {
        let mut s = ShellState::new();
        s.overlay = crate::state::Overlay::Palette;
        s.mode_names = vec!["Chat".into(), "Build".into()];
        s.active_mode = "Chat".into();
        s.sessions = vec!["fix 500".into(), "add auth".into()];
        s.palette.query = query.into();
        s
    }

    #[test]
    fn direct_items_stage1_bare_colon() {
        let s = shell_for_direct(":");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "session",
                "drawer",
                "mode",
                "companion",
                "compact",
                "delegate",
                "config",
                "q",
                "quit",
            ]
        );
    }

    #[test]
    fn direct_items_stage2_session() {
        let s = shell_for_direct(":session ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["new", "rename", "resume"]);
    }

    #[test]
    fn direct_items_stage2_drawer() {
        let s = shell_for_direct(":drawer ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["repo", "session", "context"]);
    }

    #[test]
    fn direct_items_stage2_mode_includes_subcommands_and_mode_names() {
        let s = shell_for_direct(":mode ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Subcommands first, then mode-name rows (excluding the active mode Chat).
        assert_eq!(
            labels,
            vec!["reload", "import", "update", "install superpowers", "Build"]
        );
    }

    #[test]
    fn direct_items_stage2_companion() {
        let s = shell_for_direct(":companion ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["on", "off"]);
    }

    #[test]
    fn direct_items_stage3_rename_shows_sessions() {
        let s = shell_for_direct(":session rename ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["fix 500", "add auth"]);
    }

    #[test]
    fn direct_items_stage3_import_is_empty_free_text() {
        let s = shell_for_direct(":mode import ");
        assert!(direct_items(&s).is_empty());
    }

    #[test]
    fn direct_items_stage3_delegate_is_empty_free_text() {
        let s = shell_for_direct(":delegate ");
        assert!(direct_items(&s).is_empty());
    }

    #[test]
    fn direct_items_stage3_update_shows_mode_names() {
        let s = shell_for_direct(":mode update ");
        let items = direct_items(&s);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // All mode_names shown (the pure layer doesn't filter by provenance).
        assert_eq!(labels, vec!["Chat", "Build"]);
    }

    #[test]
    fn direct_items_partial_command_word_still_stage1() {
        let s = shell_for_direct(":se");
        let items = direct_items(&s);
        // Stage 1 — no trailing space yet, so we're still picking a namespace.
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"session"));
    }

    #[test]
    fn arg_kind_prompts_and_builds_for_all_variants() {
        assert_eq!(ArgKind::Rename.prompt(), "Rename to");
        assert_eq!(
            ArgKind::Rename.build("my-feature".to_string()),
            Command::RenameSession("my-feature".to_string())
        );
        assert_eq!(ArgKind::Delegate.prompt(), "Delegate task");
        assert_eq!(
            ArgKind::Delegate.build("add a test for parse()".to_string()),
            Command::Delegate("add a test for parse()".to_string())
        );
        assert_eq!(ArgKind::ModeImport.prompt(), "Import mode from URL");
        assert_eq!(
            ArgKind::ModeImport.build("github.com/o/r/tree/main/skills".to_string()),
            Command::ModeImport("github.com/o/r/tree/main/skills".to_string())
        );
        assert_eq!(ArgKind::ModeUpdate.prompt(), "Update mode");
        assert_eq!(
            ArgKind::ModeUpdate.build("Superpowers".to_string()),
            Command::ModeUpdate("Superpowers".to_string())
        );
    }

    #[test]
    fn direct_selected_action_select_namespace_fills() {
        let s = shell_for_direct(":");
        // Top row is "session" (a namespace) → Fill.
        assert_eq!(
            direct_selected_action(&s),
            DirectAction::Fill(":session ".into())
        );
    }

    #[test]
    fn direct_selected_action_select_mode_namespace_fills() {
        // `mode` at Stage 1 carries SwitchMode("") — a bare sentinel, not
        // Unknown. The is_fill match must treat it as Fill (advance to :mode ),
        // not Run (which would close the overlay on an empty mode switch).
        let s = shell_for_direct(":");
        let mut s = s;
        s.palette.selected = 2; // "mode" is the 3rd row (index 2) in stage1_items.
        assert_eq!(
            direct_selected_action(&s),
            DirectAction::Fill(":mode ".into())
        );
    }

    #[test]
    fn direct_selected_action_select_zero_arg_runs() {
        let s = shell_for_direct(":session ");
        // Top row is "new" (zero-arg) → Run.
        assert_eq!(
            direct_selected_action(&s),
            DirectAction::Run(Command::NewSession)
        );
    }

    #[test]
    fn direct_selected_action_select_parameterized_fills() {
        let s = shell_for_direct(":session ");
        // Move selection to "rename" (index 1).
        let mut s = s;
        s.palette.selected = 1;
        assert_eq!(
            direct_selected_action(&s),
            DirectAction::Fill(":session rename ".into())
        );
    }

    #[test]
    fn direct_selected_action_select_arg_runs() {
        let s = shell_for_direct(":session rename ");
        // Top row is "fix 500" (a session name) → Run.
        assert_eq!(
            direct_selected_action(&s),
            DirectAction::Run(Command::RenameSession("fix 500".into()))
        );
    }

    #[test]
    fn direct_selected_action_no_match_is_nothing() {
        let s = shell_for_direct(":wat");
        // No fuzzy match in Stage 1 → Nothing.
        assert_eq!(direct_selected_action(&s), DirectAction::Nothing);
    }

    #[test]
    fn direct_selected_action_empty_list_is_nothing() {
        let s = shell_for_direct(":delegate ");
        // Free-text Stage 3 → empty list → Nothing.
        assert_eq!(direct_selected_action(&s), DirectAction::Nothing);
    }
}
