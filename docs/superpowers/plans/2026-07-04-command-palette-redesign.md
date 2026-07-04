# Command Palette Redesign (VSCode-style) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn zoid's `Ctrl+P` palette from a grouped keybind-teaching browse-menu into a VSCode-style flat search-first command palette, with inline argument capture for parameterized commands (Rename).

**Architecture:** The palette gains a two-phase lifecycle — `PaletteStage::{Pick, Arg{kind, input}}`. Pure decisions (`arg_kind_for`, `ArgKind::build`) live in `zoid-tui`; effects (entering Arg phase, running commands) live in the `zoid` bin. The shared `Command` enum, `exec_command`, and the `:` command line are untouched. Work is ordered so every crate compiles at each task boundary: Task 1 is additive, Task 2 wires behavior, Task 3 does the breaking slim-down + flat render + snapshots.

**Tech Stack:** Rust workspace, ratatui 0.29, insta snapshot tests. Crates: `zoid-tui` (pure TUI logic), `zoid` (effectful bin).

**Spec:** `docs/superpowers/specs/2026-07-04-command-palette-redesign-design.md`

## Global Constraints

- Commit messages: NO `Co-Authored-By` / co-author trailer (user rule).
- `zoid-tui` stays pure (no IO, no async); the `zoid` bin holds all effects.
- Do NOT modify `exec_command`, the `Command` enum, `parse_command`, or the `:` command-line routing — the palette only changes how a `Command` is *assembled*.
- At-rest palette order is a FIXED curated order — no MRU, no persistence.
- Palette shows ONLY runnable commands — every `command: None` placeholder row is removed.
- `Esc` in Arg phase returns to Pick (does not close); `Esc` in Pick closes.
- Empty argument on `Enter` in Arg phase is a no-op (cannot rename to empty).
- Final gate: `cargo test` (workspace) + `cargo clippy --workspace` clean + `cargo fmt`.

---

### Task 1: Two-phase state + argument semantics (additive)

Purely additive: introduces the `PaletteStage` phase, the `ArgKind` argument vocabulary, and the `PaletteArgCancel` action + its bin arm. Nothing is removed, so the existing grouped palette keeps working unchanged. This isolates the new type surface behind a green build before any behavior changes.

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add `PaletteStage`, add `stage` field to `PaletteState` at lines 88-92)
- Modify: `crates/zoid-tui/src/palette.rs` (add `ArgKind` + `arg_kind_for`)
- Modify: `crates/zoid-tui/src/route.rs` (add `Action::PaletteArgCancel` variant, lines 16-30 region)
- Modify: `crates/zoid/src/main.rs` (add `Action::PaletteArgCancel` handler arm near line 1953)

**Interfaces:**
- Produces (consumed by Tasks 2 & 3):
  - `PaletteStage` enum: `Pick` (default) | `Arg { kind: ArgKind, input: String }` — in `crates/zoid-tui/src/state.rs`, re-uses `crate::palette::ArgKind`.
  - `PaletteState.stage: PaletteStage` field.
  - `pub enum ArgKind { Rename }` with `pub fn prompt(&self) -> &'static str` and `pub fn build(&self, input: String) -> Command` — in `crates/zoid-tui/src/palette.rs`.
  - `pub fn arg_kind_for(cmd: &Command) -> Option<ArgKind>` — in `crates/zoid-tui/src/palette.rs`.
  - `Action::PaletteArgCancel` — in `crates/zoid-tui/src/route.rs`.

- [ ] **Step 1: Write the failing tests for `ArgKind` + `arg_kind_for`**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid-tui/src/palette.rs` (after the existing `nav_wraps` test):

```rust
    #[test]
    fn arg_kind_for_flags_only_parameterized_commands() {
        assert_eq!(arg_kind_for(&Command::RenameSession(String::new())), Some(ArgKind::Rename));
        assert_eq!(arg_kind_for(&Command::ShowOverview), None);
        assert_eq!(arg_kind_for(&Command::Quit), None);
        assert_eq!(arg_kind_for(&Command::NewSession), None);
    }

    #[test]
    fn arg_kind_builds_command_and_prompt() {
        assert_eq!(ArgKind::Rename.prompt(), "Rename to");
        assert_eq!(
            ArgKind::Rename.build("my-feature".to_string()),
            Command::RenameSession("my-feature".to_string())
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui arg_kind`
Expected: FAIL — `cannot find type/value ArgKind` / `arg_kind_for` not found.

- [ ] **Step 3: Implement `ArgKind` + `arg_kind_for`**

In `crates/zoid-tui/src/palette.rs`, immediately after the `use` lines (after line 8) add:

```rust
/// A parameterized palette command's argument-capture flow. The palette enters
/// an inline "Arg" phase to collect the argument, then builds the final command.
/// Extend with new variants (e.g. `Delegate`) as more commands take arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind {
    Rename,
}

impl ArgKind {
    /// The label shown on the argument-entry prompt line.
    pub fn prompt(&self) -> &'static str {
        match self {
            ArgKind::Rename => "Rename to",
        }
    }

    /// Build the final `Command` from the captured argument text.
    pub fn build(&self, input: String) -> Command {
        match self {
            ArgKind::Rename => Command::RenameSession(input),
        }
    }
}

/// Which inline-argument flow (if any) a command needs when chosen from the
/// palette. Pure — the bin uses this to decide the Pick→Arg transition.
pub fn arg_kind_for(cmd: &Command) -> Option<ArgKind> {
    match cmd {
        Command::RenameSession(_) => Some(ArgKind::Rename),
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui arg_kind`
Expected: PASS (2 tests).

- [ ] **Step 5: Write the failing test for the `stage` field reset**

In `crates/zoid-tui/src/state.rs`, find the existing test `close_overlay_resets_palette_query` (around line 580) and extend it (or add this new test alongside it) to assert the stage resets. Add:

```rust
    #[test]
    fn close_overlay_resets_palette_stage_to_pick() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.query = "ren".into();
        s.palette.stage = PaletteStage::Arg {
            kind: crate::palette::ArgKind::Rename,
            input: "half-typed".into(),
        };
        s.close_overlay();
        assert_eq!(s.palette.stage, PaletteStage::Pick);
        assert!(s.palette.query.is_empty());
    }
```

- [ ] **Step 6: Run the test to verify it fails**

Run: `cargo test -p zoid-tui close_overlay_resets_palette_stage`
Expected: FAIL — no field `stage` on `PaletteState` / no `PaletteStage`.

- [ ] **Step 7: Add `PaletteStage` + the `stage` field**

In `crates/zoid-tui/src/state.rs`, replace the `PaletteState` struct (lines 88-92):

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    pub stage: PaletteStage,
}

/// The palette's two-phase lifecycle. `Pick` = flat search-and-select; `Arg` =
/// inline argument entry for a parameterized command (e.g. Rename). `Default`
/// is `Pick`, so `PaletteState::default()` (used by `close_overlay`) resets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PaletteStage {
    #[default]
    Pick,
    Arg {
        kind: crate::palette::ArgKind,
        input: String,
    },
}
```

Note: `close_overlay()` at line 382 already does `self.palette = PaletteState::default();`, which now also resets `stage` to `Pick` — no change needed there.

- [ ] **Step 8: Run the test to verify it passes**

Run: `cargo test -p zoid-tui close_overlay_resets_palette_stage`
Expected: PASS.

- [ ] **Step 9: Add the `PaletteArgCancel` action + its bin arm**

In `crates/zoid-tui/src/route.rs`, add the variant to the `Action` enum right after `PaletteRun` (after line 30):

```rust
    /// Esc while in the palette's Arg (argument-entry) phase: return to the Pick
    /// list without closing the overlay.
    PaletteArgCancel,
```

In `crates/zoid/src/main.rs`, add a handler arm immediately after the `Action::PaletteRun => { ... }` block (after line 1959). Import `PaletteStage` at the top of the match's module if not already in scope (use the fully-qualified path to avoid a new `use`):

```rust
        Action::PaletteArgCancel => {
            app.shell.palette.stage = zoid_tui::state::PaletteStage::Pick;
        }
```

- [ ] **Step 10: Verify the whole workspace still compiles and all tests pass**

Run: `cargo test -p zoid-tui && cargo build -p zoid`
Expected: PASS — build clean (the new `Action` variant is handled in the bin; `PaletteArgCancel` and `Arg` are not yet constructed anywhere, which is fine for `pub` enums).

- [ ] **Step 11: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/palette.rs crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(palette): add two-phase stage + ArgKind argument vocabulary (additive)"
```

---

### Task 2: Stage-aware routing + inline argument editing

Wires the two phases end to end: routing branches on stage, and the bin's `PaletteChar`/`PaletteBackspace`/`PaletteRun` handlers edit the right buffer and drive the Pick→Arg→run transitions. Still uses the old grouped `PaletteItem` (slim-down is Task 3), so this task is behavior-only and independently reviewable.

**Files:**
- Modify: `crates/zoid-tui/src/route.rs` (`route_palette_key` at lines 222-234; caller at line 122; add route tests)
- Modify: `crates/zoid/src/main.rs` (`PaletteChar`/`PaletteBackspace`/`PaletteRun` handlers, lines 1945-1959)

**Interfaces:**
- Consumes (from Task 1): `PaletteStage`, `PaletteState.stage`, `ArgKind`, `arg_kind_for`, `Action::PaletteArgCancel`.
- Consumes (existing): `palette_selected_command(state) -> Option<Command>` (route.rs:435); `exec_command(app, cmd)` (bin).
- Produces: `route_palette_key(state: &ShellState, key: KeyEvent) -> Action` (signature change — now takes `state`).

- [ ] **Step 1: Write the failing route tests for both phases**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid-tui/src/route.rs` (the module starts at line 442). These drive through the public `route_key` with `overlay = Palette`:

```rust
    #[test]
    fn palette_pick_phase_routing() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette; // stage defaults to Pick
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::PaletteMove(-1));
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::PaletteMove(1));
        assert_eq!(route_key(&s, k(KeyCode::Backspace)), Action::PaletteBackspace);
        assert_eq!(route_key(&s, k(KeyCode::Char('r'))), Action::PaletteChar('r'));
    }

    #[test]
    fn palette_arg_phase_routing() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.stage = zoid_tui_stage_arg(); // helper below
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Esc goes BACK to Pick, not close.
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::PaletteArgCancel);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        // Arrows are inert in Arg (no list to move).
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::Noop);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::Noop);
        // Char/Backspace still edit (the bin routes them to `input` in Arg phase).
        assert_eq!(route_key(&s, k(KeyCode::Char('x'))), Action::PaletteChar('x'));
        assert_eq!(route_key(&s, k(KeyCode::Backspace)), Action::PaletteBackspace);
    }

    fn zoid_tui_stage_arg() -> crate::state::PaletteStage {
        crate::state::PaletteStage::Arg {
            kind: crate::palette::ArgKind::Rename,
            input: String::new(),
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui palette_pick_phase_routing palette_arg_phase_routing`
Expected: FAIL — `palette_arg_phase_routing` expects `PaletteArgCancel`/`Noop` but current `route_palette_key` ignores stage (returns `CloseOverlay` for Esc, `PaletteMove` for arrows).

- [ ] **Step 3: Make `route_palette_key` stage-aware**

In `crates/zoid-tui/src/route.rs`, change the caller at line 122:

```rust
        Overlay::Palette => return route_palette_key(state, key),
```

Replace the function (lines 222-234):

```rust
fn route_palette_key(state: &ShellState, key: KeyEvent) -> Action {
    let in_arg = matches!(state.palette.stage, crate::state::PaletteStage::Arg { .. });
    match key.code {
        // Esc: in Arg phase return to the Pick list; in Pick phase close.
        KeyCode::Esc if in_arg => Action::PaletteArgCancel,
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::PaletteRun,
        // Selection nav only applies to the Pick list.
        KeyCode::Up if !in_arg => Action::PaletteMove(-1),
        KeyCode::Down if !in_arg => Action::PaletteMove(1),
        KeyCode::Backspace => Action::PaletteBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PaletteChar(c)
        }
        _ => Action::Noop,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui palette_pick_phase_routing palette_arg_phase_routing`
Expected: PASS.

- [ ] **Step 5: Make the bin handlers stage-branching**

In `crates/zoid/src/main.rs`, replace the `PaletteChar`, `PaletteBackspace`, and `PaletteRun` arms (lines 1945-1959) with:

```rust
        Action::PaletteChar(c) => match &mut app.shell.palette.stage {
            zoid_tui::state::PaletteStage::Pick => {
                app.shell.palette.query.push(c);
                app.shell.palette.selected = 0;
            }
            zoid_tui::state::PaletteStage::Arg { input, .. } => input.push(c),
        },
        Action::PaletteBackspace => match &mut app.shell.palette.stage {
            zoid_tui::state::PaletteStage::Pick => {
                app.shell.palette.query.pop();
                app.shell.palette.selected = 0;
            }
            zoid_tui::state::PaletteStage::Arg { input, .. } => {
                input.pop();
            }
        },
        Action::PaletteRun => match app.shell.palette.stage.clone() {
            zoid_tui::state::PaletteStage::Pick => {
                match palette_selected_command(&app.shell) {
                    // Parameterized command → enter inline Arg phase, stay open.
                    Some(cmd) => match zoid_tui::palette::arg_kind_for(&cmd) {
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
                    },
                    // No matching row → do nothing (overlay stays open).
                    None => {}
                }
            }
            zoid_tui::state::PaletteStage::Arg { kind, input } => {
                // Empty argument is a no-op (cannot rename to empty); stay in Arg.
                if !input.is_empty() {
                    let cmd = kind.build(input);
                    app.shell.close_overlay();
                    return exec_command(app, cmd).await;
                }
            }
        },
```

- [ ] **Step 6: Verify the workspace compiles and all tests pass**

Run: `cargo test -p zoid-tui && cargo build -p zoid`
Expected: PASS. (Behaviorally: choosing "Rename session…" now enters Arg phase and typing edits the argument; render still shows the old grouped list — visuals land in Task 3.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/route.rs crates/zoid/src/main.rs
git commit -m "feat(palette): stage-aware routing + inline argument editing"
```

---

### Task 3: Flat item set + flat render + snapshots

The breaking, visible change: slim `PaletteItem` to `{label, command}`, make `all_items` a flat curated runnable-only list, render a flat single-column Pick list + an Arg prompt, and update all affected tests and snapshots. Slimming the struct breaks `render_palette`, `palette_row_line`, `palette_selected_command`, and several unit tests — all fixed here in one green step.

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (`PaletteItem` struct lines 10-19; `all_items` lines 21-132; `selectable_matches` lines 168-177; unit tests lines 191-275)
- Modify: `crates/zoid-tui/src/route.rs` (`palette_selected_command` lines 434-440)
- Modify: `crates/zoid-tui/src/render.rs` (`render_palette` lines 666-713; `palette_row_line` lines 715-727)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (palette snapshot tests, lines 369-390+)
- Delete: `crates/zoid-tui/tests/snapshots/shell_snapshot__palette_overlay_scrolled_to_end_frame.snap`

**Interfaces:**
- Consumes (from Tasks 1-2): `PaletteStage`, `ArgKind`.
- Produces:
  - `pub struct PaletteItem { pub label: &'static str, pub command: Command }` (slimmed — no `Option`, no `group`/`icon`/`hint`/`keybind`).
  - `all_items(mode: Mode) -> Vec<PaletteItem>` returning the flat curated order below.
  - `selectable_matches(items: &[PaletteItem], query: &str) -> Vec<usize>` (unchanged signature; filter simplified).
  - `render_palette(frame, state, area)` renders flat Pick list or Arg prompt based on `state.palette.stage`.

- [ ] **Step 1: Rewrite the `palette.rs` unit tests for the flat/curated list**

In `crates/zoid-tui/src/palette.rs`, replace these now-obsolete tests — `matches_exclude_disabled_rows` (lines 207-215), `empty_query_returns_all_selectable` (217-222), `settings_group_has_open_settings` (238-244), `palette_has_overview_entry` (246-252), `session_group_is_first_and_selectable` (254-274) — with:

```rust
    #[test]
    fn all_items_is_flat_curated_runnable_only() {
        let items = all_items(Mode::Chat);
        let labels: Vec<&str> = items.iter().map(|i| i.label).collect();
        assert_eq!(
            labels,
            vec![
                "New session",
                "Resume session…",
                "Rename session…",
                "Switch to Build",
                "Overview",
                "Open settings",
                "Quit zoid",
            ]
        );
        // Every row is runnable (no placeholder/disabled entries).
        assert!(items.iter().all(|i| i.command != Command::Unknown(String::new())));
    }

    #[test]
    fn mode_row_offers_the_other_mode() {
        assert_eq!(
            all_items(Mode::Chat)
                .iter()
                .find(|i| i.command == Command::SwitchMode(Mode::Build))
                .map(|i| i.label),
            Some("Switch to Build")
        );
        assert_eq!(
            all_items(Mode::Build)
                .iter()
                .find(|i| i.command == Command::SwitchMode(Mode::Chat))
                .map(|i| i.label),
            Some("Switch to Chat")
        );
    }

    #[test]
    fn empty_query_returns_all_rows_in_order() {
        let items = all_items(Mode::Chat);
        let idxs = selectable_matches(&items, "");
        assert_eq!(idxs, (0..items.len()).collect::<Vec<_>>());
    }

    #[test]
    fn typing_reranks_best_match_first() {
        let items = all_items(Mode::Chat);
        let idxs = selectable_matches(&items, "over");
        assert_eq!(items[idxs[0]].label, "Overview");
        let idxs = selectable_matches(&items, "build");
        assert_eq!(items[idxs[0]].label, "Switch to Build");
    }
```

Keep `substring_outranks_subsequence`, `no_match_is_none`, `nav_wraps`, and the two Task-1 `arg_kind_*` tests unchanged.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui -p zoid-tui --lib palette 2>&1 | head -40`
Expected: FAIL to COMPILE — the new tests reference `i.label`/`i.command` on the old struct fields that still exist, but `all_items_is_flat_curated_runnable_only` expects 7 flat rows while `all_items` still returns the grouped 12-row set (and `command` is still `Option`, so `i.command != Command::Unknown(..)` won't type-check). This confirms the struct + `all_items` must change.

- [ ] **Step 3: Slim `PaletteItem` and flatten `all_items`**

In `crates/zoid-tui/src/palette.rs`, replace the module doc + `PaletteItem` struct (lines 1-19) with:

```rust
//! The command palette's item set + fuzzy filtering. A flat, curated,
//! runnable-only list (VSCode-style): typing filters/re-ranks it, the top match
//! is auto-selected, Enter runs it. Parameterized commands (Rename) capture
//! their argument inline via `ArgKind`. Pure; rendering lives in `render.rs`.

use crate::command::Command;
use crate::state::Mode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: &'static str,
    pub command: Command,
}
```

(Note: this drops the `use crate::tokens::glyph;` import — it is no longer used here. Remove that line.)

Replace `all_items` (lines 21-132) with:

```rust
/// The flat, curated, runnable-only item set for `mode`. Fixed order at rest;
/// `selectable_matches` re-ranks it by fuzzy score while the user types. Non-
/// implemented actions (fork/undo/pin/evict/recipe) are intentionally omitted —
/// re-add them here with their real `Command` when those features ship.
pub fn all_items(mode: Mode) -> Vec<PaletteItem> {
    // The mode row offers the *other* mode.
    let (mode_label, mode_cmd) = match mode {
        Mode::Chat => ("Switch to Build", Command::SwitchMode(Mode::Build)),
        Mode::Build => ("Switch to Chat", Command::SwitchMode(Mode::Chat)),
    };
    vec![
        PaletteItem { label: "New session", command: Command::NewSession },
        PaletteItem { label: "Resume session…", command: Command::ResumeSessionPicker },
        PaletteItem { label: "Rename session…", command: Command::RenameSession(String::new()) },
        PaletteItem { label: mode_label, command: mode_cmd },
        PaletteItem { label: "Overview", command: Command::ShowOverview },
        PaletteItem { label: "Open settings", command: Command::OpenConfig },
        PaletteItem { label: "Quit zoid", command: Command::Quit },
    ]
}
```

Replace `selectable_matches` (lines 168-177) — drop the `command.is_some()` filter since every row is now runnable:

```rust
/// Indices into `items` matching `query`, ranked best-first (stable on ties).
/// Empty query returns every row in curated order.
pub fn selectable_matches(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(it.label, query).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}
```

- [ ] **Step 4: Fix `palette_selected_command` for the non-Option command**

In `crates/zoid-tui/src/route.rs`, replace `palette_selected_command` (lines 434-440):

```rust
/// Resolve the palette's selected row to its command (bin calls after PaletteRun).
/// `None` means no row matched the current query.
pub fn palette_selected_command(state: &ShellState) -> Option<Command> {
    let items = all_items(state.mode);
    let matches = selectable_matches(&items, &state.palette.query);
    let sel = nav(state.palette.selected, 0, matches.len());
    matches.get(sel).map(|&i| items[i].command.clone())
}
```

- [ ] **Step 5: Rewrite `render_palette` + `palette_row_line` for flat Pick / Arg views**

In `crates/zoid-tui/src/render.rs`, replace `render_palette` + `palette_row_line` (lines 666-727):

```rust
fn render_palette(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);

    // Title reflects the phase: Pick shows the search prompt + query; Arg shows
    // the argument prompt + typed input so far.
    let title = match &state.palette.stage {
        PaletteStage::Pick => format!(" {} {} ", glyph::USER_TURN, state.palette.query),
        PaletteStage::Arg { kind, input } => format!(" {}: {} ", kind.prompt(), input),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color::CHAT_ACCENT))
        .title(Span::styled(title, Style::new().fg(color::TXT)));
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame.render_widget(block, area);

    match &state.palette.stage {
        PaletteStage::Arg { .. } => {
            // Inline argument entry: a single dim hint line under the prompt.
            let hint = Line::styled("Enter apply · Esc back", Style::new().fg(color::DIM));
            frame.render_widget(Paragraph::new(vec![hint]), inner);
        }
        PaletteStage::Pick => {
            let items = all_items(state.mode);
            let matches = selectable_matches(&items, &state.palette.query);
            let sel = nav(state.palette.selected, 0, matches.len());

            // Flat single-column list of ranked matches; highlight the selected row.
            let mut lines: Vec<Line> = Vec::new();
            let mut selected_line: usize = 0;
            for (rank, &i) in matches.iter().enumerate() {
                if rank == sel {
                    selected_line = lines.len();
                }
                lines.push(palette_row_line(&items[i], rank == sel));
            }

            // Scroll-follow: keep the selected line within the visible viewport.
            // The curated list is short today, but this stays correct if it grows.
            let vh = inner.height as usize;
            let off = selected_line.saturating_sub(vh.saturating_sub(1));
            frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
        }
    }
}

fn palette_row_line(it: &PaletteItem, selected: bool) -> Line<'static> {
    let bg = |s: Style| if selected { s.bg(color::SEL_BG) } else { s };
    Line::from(Span::styled(
        format!(" {}", it.label),
        bg(Style::new().fg(color::TXT)),
    ))
}
```

Ensure `PaletteStage` is in scope in `render.rs`. Check the existing `use crate::state::{...}` line near the top of the file; add `PaletteStage` to it (e.g. `use crate::state::{ShellState, PaletteStage, ...};`). If `render.rs` imports state types via `use crate::state::*;`, no change is needed.

- [ ] **Step 6: Run the unit + lib tests to verify they pass**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS. If `render.rs` reports `PaletteStage` unresolved, fix the `use` as noted in Step 5, then re-run.

- [ ] **Step 7: Update the snapshot tests**

In `crates/zoid-tui/tests/shell_snapshot.rs`:

(a) Leave `palette_overlay_frame` (lines 369-375) as-is — its snapshot content will be regenerated in Step 8.

(b) Replace the `palette_overlay_scrolled_to_end_frame` test (lines 377-390+, through its closing braces) with a new Arg-phase snapshot. Delete the whole old `#[test] fn palette_overlay_scrolled_to_end_frame() { ... }` (it is dead — the 7-item list never overflows) and add:

```rust
#[test]
fn palette_arg_stage_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::Palette;
    s.palette.stage = zoid_tui::state::PaletteStage::Arg {
        kind: zoid_tui::palette::ArgKind::Rename,
        input: "my-feature".into(),
    };
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}
```

(If the test file imports names differently — e.g. `use zoid_tui::state::*;` — match its existing import style for `PaletteStage`/`ArgKind`; adjust the paths accordingly.)

- [ ] **Step 8: Delete the dead snapshot and regenerate snapshots**

```bash
rm crates/zoid-tui/tests/snapshots/shell_snapshot__palette_overlay_scrolled_to_end_frame.snap
INSTA_UPDATE=always cargo test -p zoid-tui --test shell_snapshot
```

Then inspect the regenerated snapshots to confirm they match the design:

```bash
git diff --stat crates/zoid-tui/tests/snapshots/
cat crates/zoid-tui/tests/snapshots/shell_snapshot__palette_overlay_frame.snap
cat crates/zoid-tui/tests/snapshots/shell_snapshot__palette_arg_stage_frame.snap
```

Expected: `palette_overlay_frame.snap` shows a flat list (no UPPERCASE group headers, no keybind column, results ranked for query `"build"` with "Switch to Build" first). `palette_arg_stage_frame.snap` shows the ` Rename to: my-feature ` titled box with the `Enter apply · Esc back` hint. The scrolled `.snap` is gone. If either looks wrong, fix `render_palette` and re-run before committing.

- [ ] **Step 9: Run the full workspace gate**

Run: `cargo test && cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: all green. (Pre-existing clippy warnings unrelated to the palette may remain; introduce no new ones.)

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tui/src/palette.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/render.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "feat(palette): flat search-first render + curated runnable-only item set"
```

---

## Manual smoke test (after all tasks)

Not headless-testable — verify interactively at the baseline 160×40 window:

1. `Ctrl+P` → palette opens as a flat list titled `›` with New session at top.
2. Type `ov` → list narrows, "Overview" auto-selected at top; `Enter` → jumps to Overview.
3. `Ctrl+P`, type `ren`, `Enter` → title switches to `Rename to: ` (Arg phase, list gone).
4. Type a name, `Enter` → session renamed. Re-open, `ren`, `Enter`, then `Esc` → returns to the flat pick list (not closed); `Esc` again → closes.
5. `:overview` / `:config` / `:rename foo` on the `:` command line still work (untouched).
