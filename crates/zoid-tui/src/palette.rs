//! The command palette's item set + fuzzy filtering (spec §6.5; mockup
//! `palette.html`). Grouped, mode-aware, each row teaching its keybind.
//! Post-v1 rows (branch/recipes) have `command: None` → rendered dimmed, not
//! selectable. Pure; rendering lives in `render.rs`.

use crate::command::Command;
use crate::state::Mode;
use crate::tokens::glyph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub group: String,
    pub icon: char,
    pub label: &'static str,
    pub hint: &'static str,
    pub keybind: &'static str,
    /// `None` = disabled (post-v1), shown dimmed and skipped by selection.
    pub command: Option<Command>,
}

/// The full, ordered item set for `mode` (grouped exactly as `palette.html`).
pub fn all_items(mode: Mode) -> Vec<PaletteItem> {
    // The mode row offers the *other* mode.
    let (mode_label, mode_cmd) = match mode {
        Mode::Chat => ("Switch to Build", Command::SwitchMode(Mode::Build)),
        Mode::Build => ("Switch to Chat", Command::SwitchMode(Mode::Chat)),
    };
    vec![
        // session — leads the palette (matches palette.html).
        PaletteItem {
            group: "session".to_string(),
            icon: glyph::NEW,
            label: "New session",
            hint: "fresh thread + clean context budget",
            keybind: ":new",
            command: Some(Command::NewSession),
        },
        PaletteItem {
            group: "session".to_string(),
            icon: glyph::RESUME,
            label: "Resume session…",
            hint: "this repo, most-recent first",
            keybind: "⏎",
            command: Some(Command::ResumeSessionPicker),
        },
        PaletteItem {
            group: "session".to_string(),
            icon: glyph::RENAME,
            label: "Rename session…",
            hint: "rename the current thread",
            keybind: ":rename",
            command: Some(Command::RenameSession(String::new())),
        },
        PaletteItem {
            group: "mode".to_string(),
            icon: glyph::MODE_SWITCH,
            label: mode_label,
            hint: "continue this conversation into the loop",
            keybind: "⇧Tab",
            command: Some(mode_cmd),
        },
        // branch group — post-v1, disabled/dimmed
        PaletteItem {
            group: format!("branch {}", glyph::BRANCH),
            icon: glyph::BRANCH,
            label: "Fork from here",
            hint: "new branch at this turn",
            keybind: ":fork",
            command: None,
        },
        PaletteItem {
            group: format!("branch {}", glyph::BRANCH),
            icon: glyph::UNDO,
            label: "Undo last turn",
            hint: "move head back",
            keybind: "u",
            command: None,
        },
        // context ⑤ — placeholder (real actions land P3), disabled for now
        PaletteItem {
            group: "context".to_string(),
            icon: glyph::EDIT,
            label: "Pin file to context",
            hint: "coming soon",
            keybind: "",
            command: None,
        },
        PaletteItem {
            group: "context".to_string(),
            icon: glyph::EVICT,
            label: "Evict cold items",
            hint: "coming soon",
            keybind: "",
            command: None,
        },
        // settings
        PaletteItem {
            group: "settings".to_string(),
            icon: glyph::SETTINGS,
            label: "Open settings",
            hint: "provider · model · economy · secrets",
            keybind: ":config",
            command: Some(Command::OpenConfig),
        },
        PaletteItem {
            group: "settings".to_string(),
            icon: glyph::SETTINGS,
            label: "Quit zoid",
            hint: "exit",
            keybind: "^C",
            command: Some(Command::Quit),
        },
        // recipes — post-v1
        PaletteItem {
            group: "recipes".to_string(),
            icon: glyph::RECIPE,
            label: "Run recipe…",
            hint: "coming soon",
            keybind: "",
            command: None,
        },
    ]
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

/// Indices into `items` of *selectable* rows (have a command) matching `query`,
/// ranked best-first (stable on ties).
pub fn selectable_matches(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.command.is_some())
        .filter_map(|(i, it)| fuzzy_score(it.label, query).map(|s| (i, s)))
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

    #[test]
    fn matches_exclude_disabled_rows() {
        let items = all_items(Mode::Chat);
        // "Fork from here" is post-v1 (command None) — never selectable.
        let idxs = selectable_matches(&items, "fork");
        assert!(idxs.is_empty());
        let idxs = selectable_matches(&items, "build");
        assert_eq!(items[idxs[0]].label, "Switch to Build");
    }

    #[test]
    fn empty_query_returns_all_selectable() {
        let items = all_items(Mode::Chat);
        let selectable = items.iter().filter(|i| i.command.is_some()).count();
        assert_eq!(selectable_matches(&items, "").len(), selectable);
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
    fn settings_group_has_open_settings() {
        let items = all_items(Mode::Chat);
        assert!(selectable_matches(&items, "settings")
            .iter()
            .any(|&i| items[i].command == Some(Command::OpenConfig)));
    }

    #[test]
    fn session_group_is_first_and_selectable() {
        let items = all_items(Mode::Chat);
        // The session group leads the palette (matches palette.html).
        assert_eq!(items[0].group, "session");
        let labels: Vec<&str> = items
            .iter()
            .filter(|i| i.group == "session")
            .map(|i| i.label)
            .collect();
        assert_eq!(
            labels,
            vec!["New session", "Resume session…", "Rename session…"]
        );
        // All three are selectable (have commands).
        for l in ["New session", "Resume session…", "Rename session…"] {
            assert!(selectable_matches(&items, l)
                .iter()
                .any(|&i| items[i].label == l));
        }
    }
}
