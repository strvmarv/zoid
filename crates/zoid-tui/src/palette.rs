//! The command palette's item set + fuzzy filtering. A flat, curated,
//! runnable-only list (VSCode-style): typing filters/re-ranks it, the top match
//! is auto-selected, Enter runs it. Parameterized commands (Rename) capture
//! their argument inline via `ArgKind`. Pure; rendering lives in `render.rs`.

use crate::command::Command;

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
}
