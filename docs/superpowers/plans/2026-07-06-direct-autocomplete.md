# Direct-Phase Autocomplete + Grouped Vocabulary — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Direct phase (`:`-prefix in the palette) from a single preview line into a three-stage autocomplete (namespace → subcommand → arg), and regroup the `:`-command vocabulary into a consistent namespace tree (`:session new`, `:drawer repo`, `:companion on`) with no aliases for the old flat shortcuts.

**Architecture:** A new pure `direct_items(state) -> Vec<PaletteItem>` in `palette.rs` derives the three-stage list from the buffer (no stored stage). A `DirectAction` resolver encodes the fill-vs-run decision for the bin. `parse_command` is rewritten for the grouped grammar. `render_palette`'s Direct branch renders the preview line (suppressed when list is visible + `Unknown`) + the filtered list. Routing gains arrow navigation in Direct when the list is non-empty. `exec_command` touched in one arm (`RenameSession("")` reseed → `":session rename "`).

**Tech Stack:** Rust 2021, ratatui 0.x, insta snapshots. Workspace tested via `cargo test --workspace`, linted via `cargo clippy --workspace --all-targets`, formatted via `cargo fmt --all`.

**Spec:** `docs/superpowers/specs/2026-07-06-direct-autocomplete-design.md`

---

## File Structure

**Modified files (in order of the tasks):**

- `crates/zoid-tui/src/command.rs` — rewrite `parse_command` for the grouped grammar; rewrite the affected tests (delete flat-shortcut tests, add grouped tests, add namespace-is-`Unknown` tests).
- `crates/zoid-tui/src/palette.rs` — add `direct_items`, `direct_filter`, `DirectAction`, `direct_selected_action`; add unit tests for all four. Existing `all_items`/`fuzzy_score`/`nav`/`selectable_matches`/`ArgKind`/`arg_kind_for`/`Phase`/`resolve_phase` unchanged.
- `crates/zoid-tui/src/route.rs` — `route_palette_key` arrow guard gains `direct_items(state).is_empty()` check; update the `palette_direct_phase_routing` test.
- `crates/zoid-tui/src/render.rs` — `Phase::Direct` branch renders preview (suppressed per option B) + filtered list; add `direct_items`/`direct_filter`/`selectable_matches`/`nav`/`palette_row_line` reuse.
- `crates/zoid/src/main.rs` — `PaletteRun` Pick/Direct branch reworked (`direct_selected_action` → Fill/Run/Nothing); `RenameSession("")` reseed → `":session rename "`.
- `crates/zoid-tui/tests/shell_snapshot.rs` (+ `.snap` files) — 3 new Direct-stage snapshots + regenerate `palette_direct_phase_frame`.
- `docs/ux/palette.html` — update mockup to grouped vocabulary + three-stage flow.

**Untouched:** `crates/zoid-tui/src/state.rs` (no new state — `direct_items` derives from existing `query`/`mode_names`/`sessions`/`active_mode`), `Command` enum (same variants), `exec_command` arms other than `RenameSession("")`.

---

## Task 1: Rewrite `parse_command` for the grouped vocabulary

**Files:**
- Modify: `crates/zoid-tui/src/command.rs:44-74` (the `parse_command` function body)
- Test: `crates/zoid-tui/src/command.rs` (the `tests` module)

- [ ] **Step 1: Write the failing tests first**

In `crates/zoid-tui/src/command.rs`, replace the tests module's flat-shortcut tests. Delete `parses_drawer_toggle_commands`, `parses_session_commands`, `parses_companion_commands`. Add these new tests:

```rust
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
        assert_eq!(parse_command(":session"), Command::Unknown("session".into()));
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
```

Keep these existing tests (they're still valid): `parses_known_commands_with_or_without_colon`, `bare_mode_is_empty_switch_not_unknown`, `unknown_is_captured_verbatim`, `parses_delegate_with_task`, `parses_config_command`, `mode_import_parses`, `mode_update_parses`, `bare_mode_import_is_empty_arg`, `mode_reload_still_wins_over_import`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib command::tests`
Expected: FAIL — `:session new` parses as `Unknown("session new")` today (no `session new` match arm); `:drawer repo` likewise; `:companion on` parses as `CompanionEnable` via the bare `companion` arm but `:companion off` already works — the new `companion_requires_on_or_off` test fails on bare `:companion` (today it returns `CompanionEnable`, the test expects `Unknown`).

- [ ] **Step 3: Rewrite `parse_command`**

Replace the body of `parse_command` (lines 44-74):

```rust
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
        "mode" => Command::SwitchMode(String::new()),
        s if s.starts_with("mode ") => Command::SwitchMode(s["mode ".len()..].trim().to_string()),
        // --- :companion namespace ---
        "companion on" => Command::CompanionEnable,
        "companion off" => Command::CompanionDisable,
        // --- flat commands ---
        "q" | "quit" => Command::Quit,
        "config" => Command::OpenConfig,
        rest if rest == "delegate" || rest.starts_with("delegate ") => {
            Command::Delegate(rest.strip_prefix("delegate").unwrap().trim().to_string())
        }
        // --- bare namespaces are Unknown (incomplete) ---
        // "session", "drawer", "companion" fall through to Unknown.
        other => Command::Unknown(other.to_string()),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib command::tests`
Expected: PASS — all new tests green; all kept tests green.

- [ ] **Step 5: Run the full workspace to catch downstream breakage**

Run: `cargo test --workspace`
Expected: The `RenameSession("")` reseed in `main.rs` still seeds `":rename "` (now `Unknown("rename")` under the new grammar). The `palette_direct_phase_frame` snapshot may also drift (it uses `:mode Build` which is unchanged). Any integration tests using `:new`/`:rename`/`:repo` will break. **Do not fix those here** — Task 6 fixes the bin; Task 7 fixes snapshots. Confirm failures are only in the bin + snapshots, not in `zoid-tui` lib tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/command.rs
git commit -m "refactor(command): grouped :session/:drawer/:companion vocabulary (no aliases)"
```

---

## Task 2: Add `direct_filter` pure helper

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (add `direct_filter` after `resolve_phase`)
- Test: `crates/zoid-tui/src/palette.rs` (add `direct_filter` tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-tui/src/palette.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_filter`
Expected: FAIL — `direct_filter` doesn't exist.

- [ ] **Step 3: Implement `direct_filter`**

Add to `crates/zoid-tui/src/palette.rs`, after `resolve_phase`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_filter`
Expected: PASS — all 5 tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): direct_filter — extract the active-stage filter text"
```

---

## Task 3: Add `direct_items` — the three-stage list

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (add `direct_items` after `direct_filter`)
- Test: `crates/zoid-tui/src/palette.rs` (add `direct_items` stage tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-tui/src/palette.rs`:

```rust
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
            vec!["reload", "import", "update", "Build"]
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_items`
Expected: FAIL — `direct_items` doesn't exist.

- [ ] **Step 3: Implement `direct_items`**

Add to `crates/zoid-tui/src/palette.rs`, after `direct_filter`:

```rust
/// The three-stage Direct-phase list, derived from the buffer. Pure.
///
/// - Stage 1 (no complete namespace): top-level namespaces + flat commands.
/// - Stage 2 (`:ns `): subcommands for the namespace.
/// - Stage 3 (`:ns sub `): arg completions for a parameterized subcommand.
///
/// Stages are derived from `query` — no stored stage. Empty lists (free-text
/// args like `:delegate `, `:mode import `) mean the user types freely.
pub fn direct_items(state: &ShellState) -> Vec<PaletteItem> {
    use crate::command::Command;
    use crate::state::DrawerId;

    let t = state.palette.query.strip_prefix(':').unwrap_or(&state.palette.query);
    // Stage detection: split into tokens by spaces. 0 tokens → Stage 1;
    // 1 token (no trailing space) → Stage 1 (partial); 1 token + trailing
    // space → Stage 2; 2 tokens + trailing space → Stage 3.
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
        // Anything else (e.g. `:ns sub arg` — user typing an arg) → empty;
        // the list re-filters via `selectable_matches` on the arg text, but
        // `direct_items` returns the stage's full set and the filter narrows it.
        // Actually for Stage 3 with a partial arg typed (`:session rename fi`),
        // we're still Stage 3 — return the arg list and let `selectable_matches`
        // filter by `direct_filter`.
        ([ns, sub, ..], _) => stage3_items(ns, sub, state),
    }
}

fn stage1_items() -> Vec<PaletteItem> {
    use crate::command::Command;
    vec![
        PaletteItem { label: "session".into(), command: Command::Unknown("session".into()) },
        PaletteItem { label: "drawer".into(), command: Command::Unknown("drawer".into()) },
        PaletteItem { label: "mode".into(), command: Command::SwitchMode(String::new()) },
        PaletteItem { label: "companion".into(), command: Command::Unknown("companion".into()) },
        PaletteItem { label: "delegate".into(), command: Command::Delegate(String::new()) },
        PaletteItem { label: "config".into(), command: Command::OpenConfig },
        PaletteItem { label: "q".into(), command: Command::Quit },
        PaletteItem { label: "quit".into(), command: Command::Quit },
    ]
}

fn stage2_items(ns: &str, state: &ShellState) -> Vec<PaletteItem> {
    use crate::command::Command;
    use crate::state::DrawerId;
    match ns {
        "session" => vec![
            PaletteItem { label: "new".into(), command: Command::NewSession },
            PaletteItem { label: "rename".into(), command: Command::RenameSession(String::new()) },
            PaletteItem { label: "resume".into(), command: Command::ResumeSessionPicker },
        ],
        "drawer" => vec![
            PaletteItem { label: "repo".into(), command: Command::OpenDrawer(DrawerId::Repo) },
            PaletteItem { label: "session".into(), command: Command::OpenDrawer(DrawerId::Session) },
            PaletteItem { label: "context".into(), command: Command::OpenDrawer(DrawerId::Context) },
        ],
        "mode" => {
            let mut rows = vec![
                PaletteItem { label: "reload".into(), command: Command::ReloadModes },
                PaletteItem { label: "import".into(), command: Command::ModeImport(String::new()) },
                PaletteItem { label: "update".into(), command: Command::ModeUpdate(String::new()) },
            ];
            // Mode-name rows (excluding the active mode) — the `:mode <name>` direct-switch path.
            rows.extend(state.mode_names.iter().filter(|n| n.as_str() != state.active_mode).map(|n| {
                PaletteItem { label: n.clone(), command: Command::SwitchMode(n.clone()) }
            }));
            rows
        }
        "companion" => vec![
            PaletteItem { label: "on".into(), command: Command::CompanionEnable },
            PaletteItem { label: "off".into(), command: Command::CompanionDisable },
        ],
        _ => vec![],
    }
}

fn stage3_items(ns: &str, sub: &str, state: &ShellState) -> Vec<PaletteItem> {
    use crate::command::Command;
    match (ns, sub) {
        ("session", "rename") => state.sessions.iter().map(|s| {
            PaletteItem { label: s.clone(), command: Command::RenameSession(s.clone()) }
        }).collect(),
        ("mode", "update") => state.mode_names.iter().map(|n| {
            PaletteItem { label: n.clone(), command: Command::ModeUpdate(n.clone()) }
        }).collect(),
        // Free-text args — no completion list.
        ("delegate", _) | ("mode", "import") | ("session", "rename") if false => vec![],
        _ => vec![],
    }
}
```

Wait — the `("delegate", _) | ("mode", "import") | ("session", "rename") if false => vec![]` arm is wrong (the `if false` guard never fires). The free-text cases fall through to the `_ => vec![]` catch-all. Let me simplify `stage3_items`:

```rust
fn stage3_items(ns: &str, sub: &str, state: &ShellState) -> Vec<PaletteItem> {
    use crate::command::Command;
    match (ns, sub) {
        ("session", "rename") => state.sessions.iter().map(|s| {
            PaletteItem { label: s.clone(), command: Command::RenameSession(s.clone()) }
        }).collect(),
        ("mode", "update") => state.mode_names.iter().map(|n| {
            PaletteItem { label: n.clone(), command: Command::ModeUpdate(n.clone()) }
        }).collect(),
        // Free-text args (delegate, mode import) — no completion list.
        _ => vec![],
    }
}
```

Use the simplified version (drop the `if false` arm).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_items`
Expected: PASS — all 10 tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): direct_items — three-stage Direct-phase list"
```

---

## Task 4: Add `DirectAction` and `direct_selected_action` resolver

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (add `DirectAction` enum + `direct_selected_action` after `direct_items`)
- Test: `crates/zoid-tui/src/palette.rs` (add resolver tests)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/zoid-tui/src/palette.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_selected_action`
Expected: FAIL — `DirectAction` and `direct_selected_action` don't exist.

- [ ] **Step 3: Implement `DirectAction` and `direct_selected_action`**

Add to `crates/zoid-tui/src/palette.rs`, after `direct_items` (and its helpers):

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib palette::tests::direct_selected_action`
Expected: PASS — all 6 tests green.

- [ ] **Step 5: Run all palette tests to confirm no regressions**

Run: `cargo test -p zoid-tui --lib palette::tests`
Expected: PASS — all existing + new tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): DirectAction + direct_selected_action — fill-vs-run resolver"
```

---

## Task 5: Update routing — arrows active in Direct when list non-empty

**Files:**
- Modify: `crates/zoid-tui/src/route.rs:217-234` (`route_palette_key`)
- Test: `crates/zoid-tui/src/route.rs:677-695` (`palette_direct_phase_routing`)

- [ ] **Step 1: Update the `palette_direct_phase_routing` test**

In `crates/zoid-tui/src/route.rs`, replace the `palette_direct_phase_routing` test (line 677). The current test seeds `:mode Build` and asserts arrows are `Noop`. Under the new design, `:mode Build` is Stage 2 with a non-empty list (mode-name rows), so arrows must be `PaletteMove`. Add a second case for `:wat` (list empty → arrows `Noop`):

```rust
    #[test]
    fn palette_direct_phase_routing() {
        let mut s = ShellState::new();
        s.mode_names = vec!["Chat".into(), "Build".into()];
        s.overlay = Overlay::Palette;

        // Direct with a non-empty list (`:mode ` → Stage 2 subcommands + mode
        // names) → arrows navigate.
        s.palette.query = ":mode ".into();
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::PaletteMove(-1));
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::PaletteMove(1));
        assert_eq!(route_key(&s, k(KeyCode::Char('x'))), Action::PaletteChar('x'));
        assert_eq!(route_key(&s, k(KeyCode::Backspace)), Action::PaletteBackspace);

        // Direct with an empty list (`:wat` → no fuzzy match) → arrows inert.
        s.palette.query = ":wat".into();
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::Noop);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::Noop);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib route::tests::palette_direct_phase_routing`
Expected: FAIL — arrows are still `Noop` in Direct regardless of list state.

- [ ] **Step 3: Update `route_palette_key`**

Replace `route_palette_key` (lines 217-234):

```rust
fn route_palette_key(state: &ShellState, key: KeyEvent) -> Action {
    let in_arg = matches!(state.palette.stage, crate::state::PaletteStage::Arg { .. });
    let in_direct = !in_arg && state.palette.query.starts_with(':');
    let direct_list_nonempty = in_direct && !crate::palette::direct_items(state).is_empty();
    match key.code {
        // Esc: in Arg phase return to the Pick list; otherwise close.
        KeyCode::Esc if in_arg => Action::PaletteArgCancel,
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::PaletteRun,
        // Selection nav applies to Pick, and to Direct when its list is non-empty.
        KeyCode::Up if !in_arg && (!in_direct || direct_list_nonempty) => Action::PaletteMove(-1),
        KeyCode::Down if !in_arg && (!in_direct || direct_list_nonempty) => Action::PaletteMove(1),
        KeyCode::Backspace => Action::PaletteBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PaletteChar(c)
        }
        _ => Action::Noop,
    }
}
```

- [ ] **Step 4: Run the route tests to verify they pass**

Run: `cargo test -p zoid-tui --lib route::tests`
Expected: PASS — `palette_direct_phase_routing` green; all other route tests green (the `in_direct` guard only changes arrow behavior when the list is non-empty, which existing Pick/Arg tests don't seed).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/route.rs
git commit -m "feat(route): arrows active in Direct phase when list non-empty"
```

---

## Task 6: Wire the bin — `direct_selected_action` in `PaletteRun`; fix `RenameSession("")` reseed

**Files:**
- Modify: `crates/zoid/src/main.rs:2296-2328` (the `PaletteRun` arm)
- Modify: `crates/zoid/src/main.rs:3320+` (the `RenameSession` arm in `exec_command`)

- [ ] **Step 1: Replace the `PaletteRun` Pick/Direct branch**

In `crates/zoid/src/main.rs`, replace the `Action::PaletteRun` arm (lines 2296-2328). The Direct branch now uses `direct_selected_action`:

```rust
        Action::PaletteRun => match app.shell.palette.stage.clone() {
            zoid_tui::state::PaletteStage::Pick => {
                if app.shell.palette.query.starts_with(':') {
                    // Direct phase — resolve the highlighted row.
                    match zoid_tui::palette::direct_selected_action(&app.shell) {
                        zoid_tui::palette::DirectAction::Fill(text) => {
                            app.shell.palette.query = text;
                            app.shell.palette.selected = 0;
                        }
                        zoid_tui::palette::DirectAction::Run(cmd) => {
                            app.shell.close_overlay();
                            return exec_command(app, cmd).await;
                        }
                        zoid_tui::palette::DirectAction::Nothing => {
                            let cmd = zoid_tui::command::parse_command(&app.shell.palette.query);
                            app.shell.close_overlay();
                            return exec_command(app, cmd).await;
                        }
                    }
                } else {
                    // Pick phase — fuzzy list resolution.
                    if let Some(cmd) = palette_selected_command(&app.shell) {
                        match zoid_tui::palette::arg_kind_for(&cmd) {
                            Some(kind) => {
                                app.shell.palette.stage = zoid_tui::state::PaletteStage::Arg {
                                    kind,
                                    input: String::new(),
                                };
                            }
                            None => {
                                app.shell.close_overlay();
                                return exec_command(app, cmd).await;
                            }
                        }
                    }
                }
            }
            zoid_tui::state::PaletteStage::Arg { kind, input } => {
                let trimmed = input.trim();
                if !trimmed.is_empty() {
                    let cmd = kind.build(trimmed.to_string());
                    app.shell.close_overlay();
                    return exec_command(app, cmd).await;
                }
            }
        },
```

- [ ] **Step 2: Fix the `RenameSession("")` reseed in `exec_command`**

In `crates/zoid/src/main.rs`, find the `Command::RenameSession(name)` arm in `exec_command` (around line 3320). Change the reseed from `":rename "` to `":session rename "`:

```rust
        Command::RenameSession(name) => {
            if name.is_empty() {
                // Seed the palette in Direct phase so the user types the name.
                app.shell.overlay = zoid_tui::Overlay::Palette;
                app.shell.palette = Default::default();
                app.shell.palette.query = ":session rename ".into();
            } else {
                app.session
                    .rename_session(app.session_id, name.clone())
                    .await
                    .ok();
                app.shell.session_name = name;
            }
            Ok(false)
        }
```

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS — no remaining references to the old flat shortcuts; `direct_selected_action` and `DirectAction` are imported via the `zoid_tui::palette` path.

- [ ] **Step 4: Run the full workspace tests**

Run: `cargo test --workspace`
Expected: `zoid-tui` lib tests pass; `zoid` bin tests pass. Snapshot tests may fail on `palette_direct_phase_frame` (Task 7 regenerates). Confirm only snapshot tests fail.

- [ ] **Step 5: Run clippy and fmt**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

Run: `cargo fmt --all`
Expected: no changes (or apply them).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(zoid): wire direct_selected_action in PaletteRun; fix RenameSession reseed"
```

---

## Task 7: Render the Direct-phase list + preview suppression

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:693-712` (the `Phase::Direct` branch of `render_palette`)
- Modify: `crates/zoid-tui/src/render.rs:11` (imports — add `direct_items`, `direct_filter`)

- [ ] **Step 1: Update the imports**

In `crates/zoid-tui/src/render.rs`, line 11, extend the `palette` import:

```rust
use crate::palette::{
    all_items, direct_filter, direct_items, nav, resolve_phase, selectable_matches,
    PaletteItem, Phase,
};
```

- [ ] **Step 2: Replace the `Phase::Direct` branch in `render_palette`**

In `crates/zoid-tui/src/render.rs`, replace the `Phase::Direct { cmd } => { ... }` arm (lines 693-712). The new version renders the preview line (suppressed when list is non-empty + `Unknown`) and the filtered list below:

```rust
        Phase::Direct { cmd } => {
            let items = direct_items(state);
            let filter = direct_filter(&state.palette.query);
            let matches = selectable_matches(&items, filter);
            let list_nonempty = !matches.is_empty();

            let mut lines: Vec<Line> = Vec::new();

            // Preview line: show when parse_command resolved to a real Command
            // (not Unknown) OR when the list is empty. Suppress when the list is
            // visible and the buffer is an incomplete namespace (Unknown).
            let show_preview = !matches!(cmd, Command::Unknown(_)) || !list_nonempty;
            if show_preview {
                let preview: String = match cmd {
                    Command::Unknown(s) if s.is_empty() => {
                        "type a command word".to_string()
                    }
                    Command::Unknown(_) => "unknown command".to_string(),
                    Command::SwitchMode(name) => format!("→ Switch to {name}"),
                    Command::ReloadModes => "→ Reload modes".to_string(),
                    Command::ModeImport(url) => format!("→ Import mode: {url}"),
                    Command::ModeUpdate(name) => format!("→ Update mode: {name}"),
                    Command::RenameSession(name) => format!("→ Rename session: {name}"),
                    Command::Delegate(task) => format!("→ Delegate: {task}"),
                    Command::Quit => "→ Quit zoid".to_string(),
                    Command::OpenDrawer(id) => format!("→ Toggle {:?} drawer", id),
                    Command::NewSession => "→ New session".to_string(),
                    Command::ResumeSessionPicker => "→ Resume session…".to_string(),
                    Command::OpenConfig => "→ Open settings".to_string(),
                    Command::CompanionEnable => "→ Enable companion".to_string(),
                    Command::CompanionDisable => "→ Disable companion".to_string(),
                };
                lines.push(Line::styled(preview, Style::new().fg(color::DIM)));
            }

            // Filtered list below the preview line.
            let mut selected_line: usize = 0;
            let list_start = lines.len();
            let sel = nav(state.palette.selected, 0, matches.len());
            for (rank, &i) in matches.iter().enumerate() {
                if rank == sel {
                    selected_line = list_start + rank;
                }
                lines.push(palette_row_line(&items[i], rank == sel));
            }

            // Footer hint.
            if list_nonempty {
                lines.push(Line::styled(
                    "↑↓ move · ⏎ select · esc close · type to filter",
                    Style::new().fg(color::DIM),
                ));
            } else {
                lines.push(Line::styled(
                    "⏎ run · esc close",
                    Style::new().fg(color::DIM),
                ));
            }

            // Scroll-follow on the selected row.
            let vh = inner.height as usize;
            let off = selected_line.saturating_sub(vh.saturating_sub(1));
            frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
        }
```

- [ ] **Step 3: Run the `zoid-tui` lib tests**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(render): Direct-phase filtered list + preview suppression"
```

---

## Task 8: Snapshots — 3 new Direct-stage + regenerate `palette_direct_phase_frame`

**Files:**
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (add 3 tests, update 1)
- Modify: `crates/zoid-tui/tests/snapshots/` (3 new `.snap` files + 1 regenerated)

- [ ] **Step 1: Add the three new snapshot tests**

In `crates/zoid-tui/tests/shell_snapshot.rs`, near the existing `palette_direct_phase_frame` test (line 426), add:

```rust
#[test]
fn palette_direct_stage1_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.sessions = vec!["fix 500".into(), "add auth".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn palette_direct_stage2_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":session ".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}

#[test]
fn palette_direct_stage3_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.sessions = vec!["fix 500".into(), "add auth".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":session rename ".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}
```

- [ ] **Step 2: Regenerate snapshots**

Run: `cargo insta test --accept -p zoid-tui`
Expected: 3 new `.snap` files created (`palette_direct_stage1_frame`, `palette_direct_stage2_frame`, `palette_direct_stage3_frame`); `palette_direct_phase_frame` regenerated (it now renders a list for `:mode Build`).

- [ ] **Step 3: Inspect the regenerated snapshots**

Open each new `.snap` file and confirm:
- `palette_direct_stage1_frame.snap`: title `:▌`, no preview line (suppressed — list non-empty + `Unknown`), 8-row list (session / drawer / mode / companion / delegate / config / q / quit), top row highlighted.
- `palette_direct_stage2_frame.snap`: title `:session ▌`, no preview line (suppressed), 3-row list (new / rename / resume), top row highlighted.
- `palette_direct_stage3_frame.snap`: title `:session rename ▌`, preview line `→ Rename session: ` (not suppressed — `RenameSession("")` is a real Command), 2-row list (fix 500 / add auth), top row highlighted.
- `palette_direct_phase_frame.snap`: title `:mode Build`, preview `→ Switch to Build`, list shows `Build` row highlighted.

- [ ] **Step 4: Run the snapshot tests to verify they pass**

Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "test(snapshots): 3 Direct-stage snapshots + regenerate palette_direct_phase_frame"
```

---

## Task 9: Update the UX mockup

**Files:**
- Modify: `docs/ux/palette.html` (update to grouped vocabulary + three-stage flow)

- [ ] **Step 1: Update the mockup**

This is a docs-only edit. The visual companion mockups from the brainstorming session (`grouped-vocabulary.html`) become the reference. Update `docs/ux/palette.html` to:
- Show the grouped vocabulary in the command-word list (session / drawer / mode / companion / delegate / config / q instead of the flat rows).
- Update the caption to describe the three-stage autocomplete flow.
- Remove any references to the old flat shortcuts (`:new`, `:rename`, `:repo`).

Read the current `docs/ux/palette.html` first, then update the list rows and caption. The exact HTML structure stays (same CSS classes); only the row contents and caption text change.

- [ ] **Step 2: Commit**

```bash
git add docs/ux/palette.html
git commit -m "docs(ux): update palette mockup for grouped vocabulary + three-stage autocomplete"
```

---

## Self-Review (run after writing — already done)

**1. Spec coverage:**
- §1 Revised `parse_command` grammar → Task 1.
- §2 `direct_items` + stage derivation → Task 3.
- §2 `direct_filter` → Task 2.
- §3 Selection and buffer-fill → Task 4 (`DirectAction` resolver) + Task 6 (bin wiring).
- §4 Routing → Task 5.
- §5 Bin handlers (`PaletteRun` rework + `RenameSession("")` reseed) → Task 6.
- §6 Render (preview suppression + filtered list) → Task 7.
- §7 Pick `all_items` not modified → respected (no task touches `all_items`).
- §8 `parse_command` test updates → Task 1.
- §9 `palette.rs` test additions → Tasks 2, 3, 4.
- §10 Snapshot tests → Task 8.
- §11 Files touched → covered across Tasks 1-9.
- §12 Non-goals → respected (no aliases, no provenance filter, no `:config` subcommands, no MRU, no Tab, no delegate arg completions).
- UX mockup → Task 9.
- Supersession pointer on consolidation spec → already added during spec writing.

**2. Placeholder scan:** No TBD / TODO / "implement later". Every step shows exact code or commands. (Task 9's mockup edit is intentionally less prescriptive since it's HTML content — the instruction is to read the current file and update row contents + caption.)

**3. Type consistency:**
- `DirectAction` enum (Fill/Run/Nothing) — consistent across Task 4 (definition), Task 6 (bin match).
- `direct_items(state: &ShellState) -> Vec<PaletteItem>` — consistent across Task 3 (definition), Task 5 (routing guard), Task 7 (render).
- `direct_filter(query: &str) -> &str` — consistent across Task 2 (definition), Task 4 (resolver), Task 7 (render).
- `direct_selected_action(state: &ShellState) -> DirectAction` — consistent across Task 4 (definition), Task 6 (bin).
- `shell_for_direct` test helper — defined in Task 3, reused in Task 4.
- `PaletteItem { label, command }` — consistent with existing struct.

**4. Task ordering / compile-ability:**
- Task 1 (`parse_command`) leaves the bin broken (`RenameSession("")` still seeds `":rename "`, now `Unknown`). Task 6 fixes it. `zoid-tui` lib compiles after Task 1.
- Tasks 2-4 add pure functions to `palette.rs`; `zoid-tui` lib compiles throughout.
- Task 5 updates routing; `zoid-tui` lib compiles.
- Task 6 wires the bin; whole workspace compiles.
- Task 7 updates render; `zoid-tui` lib compiles (snapshots drift until Task 8).
- Task 8 regenerates snapshots.
- Task 9 is docs-only.

No mid-task compile dead-ends.