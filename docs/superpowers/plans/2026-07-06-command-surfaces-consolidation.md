# Command Surfaces Consolidation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge the `Ctrl+P` command palette and the `:` command line into one adaptive overlay where typing `:` as the first character switches the same prompt into a direct-command mode, and fold the `:`-only commands (delegate, mode import, mode update, drawer toggles) into the palette item set.

**Architecture:** One overlay (`Overlay::Palette`), one state struct (`PaletteState`), one prompt. Three phases derived from buffer content + stage: `Pick` (fuzzy list), `Direct` (live `parse_command` preview, derived from `query.starts_with(':')`), `Arg` (inline argument entry, a real `PaletteStage::Arg` transition). The shared `Command` enum and `parse_command` are untouched; `exec_command` gains one tiny edit (the `RenameSession("")` seeding targets the palette in Direct phase instead of the deleted cmdline). The `Overlay::CommandLine` variant, `CmdlineState`, four cmdline `Action` variants, `route_cmdline_key`, `render_cmdline`, and the cmdline layout rect are deleted.

**Tech Stack:** Rust 2021, ratatui 0.x, insta snapshots. Workspace tested via `cargo test --workspace`, linted via `cargo clippy --workspace --all-targets`, formatted via `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-07-06-command-surfaces-consolidation-design.md`

---

## File Structure

**Modified files (in order of the tasks):**

- `crates/zoid-tui/src/palette.rs` — add three `ArgKind` variants; extend `arg_kind_for`; add six new rows to `all_items`; add `Phase` enum + `resolve_phase` pure helper; extend unit tests.
- `crates/zoid-tui/src/state.rs` — delete `CmdlineState` struct + `cmdline` field; delete `Overlay::CommandLine` variant; update `close_overlay` and the `close_overlay_resets_*` tests.
- `crates/zoid-tui/src/route.rs` — delete `Action::{OpenCommandLine, CmdlineChar, CmdlineBackspace, RunCommand}`; delete `route_cmdline_key`; add `Action::OpenPaletteDirect`; `route_palette_key` gains Direct-phase branch; the two `:` arms in `route_key` emit `OpenPaletteDirect`; update route tests.
- `crates/zoid-tui/src/layout.rs` — delete the `cmdline: Option<Rect>` field + the cmdline rect computation block; update `palette_rect_only_when_overlay_active` test.
- `crates/zoid-tui/src/render.rs` — delete `render_cmdline`; `render_palette` gains Direct-phase preview line; delete the `Overlay::CommandLine` dispatch arm in the top-level overlay router; update imports.
- `crates/zoid-tui/tests/shell_snapshot.rs` (+ `.snap` files) — regenerate `palette_overlay_frame`; add `palette_direct_phase_frame`; remove `command_line_frame` + its `.snap`.
- `crates/zoid-tui/examples/scenes/mod.rs` — remove the `palette` scene's stale `s.cmdline.buffer = "build"` line (or update it; the scene already sets `Overlay::Palette`).
- `crates/zoid/src/main.rs` — delete `Action::{OpenCommandLine, CmdlineChar, CmdlineBackspace, RunCommand}` arms; add `Action::OpenPaletteDirect` arm; extend `Action::PaletteRun` with the Direct branch; edit `exec_command`'s `RenameSession("")` arm to seed the palette in Direct phase; update the `RunCommand` integration test site if any (none found — the only test consumer is `route.rs`'s `cmdline_enter_parses_command`, replaced in Task 3).
- `docs/ux/palette.html` — update the caption/footnote to reflect the merged adaptive UI; remove the cmdline footer note.

**Untouched:** `crates/zoid-tui/src/command.rs` (`parse_command` + `Command` enum), `crates/zoid/src/main.rs::exec_command` arms other than `RenameSession("")`.

---

## Task 1: Extend `ArgKind` and `arg_kind_for` for the three new parameterized commands

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs:10-39` (the `ArgKind` enum, `impl ArgKind`, `arg_kind_for`)
- Test: `crates/zoid-tui/src/palette.rs` (unit tests, same file)

- [ ] **Step 1: Write the failing tests**

Add these tests to the `tests` module in `crates/zoid-tui/src/palette.rs` (after the existing `arg_kind_*` tests):

```rust
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
        assert_eq!(
            arg_kind_for(&Command::SwitchMode("Build".into())),
            None
        );
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
```

Also remove the now-superseded `arg_kind_for_flags_only_parameterized_commands` test (it only covers Rename) — its assertions are subsumed by `arg_kind_for_flags_all_parameterized_commands`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib palette::tests`
Expected: FAIL — `ArgKind::Delegate` / `ModeImport` / `ModeUpdate` don't exist; `arg_kind_for` doesn't match those arms; `prompt()`/`build()` don't have those arms.

- [ ] **Step 3: Extend the `ArgKind` enum**

In `crates/zoid-tui/src/palette.rs`, replace the `ArgKind` enum (lines 11-14):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind {
    Rename,
    Delegate,
    ModeImport,
    ModeUpdate,
}
```

- [ ] **Step 4: Extend `impl ArgKind`**

Replace the `impl ArgKind` block (lines 16-30):

```rust
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
```

- [ ] **Step 5: Extend `arg_kind_for`**

Replace `arg_kind_for` (lines 34-39):

```rust
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
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p zoid-tui --lib palette::tests`
Expected: PASS — all three new tests green; existing tests still green.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): extend ArgKind with Delegate/ModeImport/ModeUpdate"
```

---

## Task 2: Add the six new palette rows to `all_items`

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs:55-104` (`all_items`)
- Test: `crates/zoid-tui/src/palette.rs` (the `all_items_is_flat_curated` test)

- [ ] **Step 1: Update the `all_items_is_flat_curated` test first**

In `crates/zoid-tui/src/palette.rs`, replace the `all_items_is_flat_curated` test body (around line 183):

```rust
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tui --lib palette::tests::all_items_is_flat_curated`
Expected: FAIL — the current `all_items` returns the old 8-row list without the six new rows.

- [ ] **Step 3: Extend `all_items` with the six new rows**

In `crates/zoid-tui/src/palette.rs`, replace the body of `all_items` (lines 55-104):

```rust
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
```

- [ ] **Step 4: Run the palette tests to verify they pass**

Run: `cargo test -p zoid-tui --lib palette::tests`
Expected: PASS — `all_items_is_flat_curated` green; `mode_rows_offer_every_other_mode_plus_reload`, `empty_query_returns_all_rows_in_order`, `typing_reranks_best_match_first` still green (the new rows don't break fuzzy matches for "build" or "comp").

- [ ] **Step 5: Run the full workspace to catch downstream snapshot drift**

Run: `cargo test --workspace`
Expected: `shell_snapshot::palette_overlay_frame` FAILS (the snapshot was generated from the 8-row list; now it's 14 rows). Other snapshot tests may also drift if they render the palette. **Do not regenerate yet** — Task 6 handles snapshots. Confirm only snapshot tests fail, no unit-test failures.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/palette.rs
git commit -m "feat(palette): add delegate/import/update rows + drawer-toggle rows"
```

---

## Task 3: Add `Phase` enum + `resolve_phase` pure helper, and Direct-phase routing

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (add `Phase` enum + `resolve_phase`)
- Modify: `crates/zoid-tui/src/route.rs:16-100` (delete four cmdline `Action` variants; add `OpenPaletteDirect`), `route.rs:122-218` (the two `:` arms in `route_key`), `route.rs:220-248` (delete `route_cmdline_key`, extend `route_palette_key`), `route.rs` tests
- Test: `crates/zoid-tui/src/route.rs` (Direct-phase routing tests; replace `cmdline_enter_parses_command`; replace the `:` → `OpenCommandLine` test)

- [ ] **Step 1: Add the `Phase` enum and `resolve_phase` helper to `palette.rs`**

Add this near the top of `crates/zoid-tui/src/palette.rs`, after the `ArgKind` block and before `PaletteItem`:

```rust
/// What a given `PaletteState` means at this instant. Pure — used by routing
/// and rendering to branch on the `:` prefix without storing a phase.
/// `Arg` is `PaletteStage::Arg`, not a `Phase` variant — it's a real stage
/// transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Empty or non-`:` query → fuzzy ranked list.
    Pick,
    /// Query starts with `:` → live `parse_command` preview, list hidden.
    Direct { cmd: crate::command::Command },
    /// `PaletteStage::Arg` is active → inline argument entry.
    Arg,
}

/// Resolve the current phase from the palette state. Pure. Owns the parsed
/// `Command` (cheap: a trim + string compares per frame).
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
```

- [ ] **Step 2: Write the failing route tests for Direct phase**

In `crates/zoid-tui/src/route.rs` tests module, add:

```rust
    #[test]
    fn palette_direct_phase_routing() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.query = ":mode Build".into(); // Direct — derived from ':' prefix
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Esc closes (same as Pick); arrows are inert (no list to navigate).
        assert_eq!(route_key(&s, k(KeyCode::Esc)), Action::CloseOverlay);
        assert_eq!(route_key(&s, k(KeyCode::Enter)), Action::PaletteRun);
        assert_eq!(route_key(&s, k(KeyCode::Up)), Action::Noop);
        assert_eq!(route_key(&s, k(KeyCode::Down)), Action::Noop);
        // Char/Backspace still edit the buffer.
        assert_eq!(route_key(&s, k(KeyCode::Char('x'))), Action::PaletteChar('x'));
        assert_eq!(
            route_key(&s, k(KeyCode::Backspace)),
            Action::PaletteBackspace
        );
    }
```

And replace the `colon_opens_cmdline_only_when_not_input` test:

```rust
    #[test]
    fn colon_opens_palette_direct_when_not_input() {
        let mut s = ShellState::new();
        // focus Input → ':' is literal text
        assert!(matches!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::Edit(_)
        ));
        s.focus = Focus::Conversation;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
        s.focus = Focus::Rail;
        assert_eq!(
            route_key(&s, key(KeyCode::Char(':'), KeyModifiers::NONE)),
            Action::OpenPaletteDirect
        );
    }
```

And delete the `cmdline_enter_parses_command` test (around line 752) — its assertion is now covered by `palette_direct_phase_routing` + the bin's `PaletteRun` Direct branch (Task 7).

Also delete the `Overlay::CommandLine` block from the `overlay_captures_keys_first` test (route.rs:744-749). After Task 3 Step 5 deletes the `Overlay::CommandLine => return route_cmdline_key(...)` dispatch arm from `route_key`, that overlay no longer captures keys, so the test's `s.overlay = Overlay::CommandLine; assert_eq!(route_key(&s, ...), Action::Noop)` assertion would fail (it would fall through to `ctrl(&key, 'q') → Action::Quit`). Delete these six lines:

```rust
        // Same guard applies to CommandLine overlay.
        s.overlay = Overlay::CommandLine;
        assert_eq!(
            route_key(&s, key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Action::Noop
        );
```

The palette-only assertions above them stay.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p zoid-tui --lib route::tests`
Expected: FAIL — `OpenPaletteDirect` doesn't exist; `route_palette_key` doesn't branch on the `:` prefix; `colon_opens_palette_direct_when_not_input` references a missing variant.

- [ ] **Step 4: Delete the four cmdline `Action` variants; add `OpenPaletteDirect`**

In `crates/zoid-tui/src/route.rs`, edit the `Action` enum (lines 16-100). Delete `OpenCommandLine` (line 23), `CmdlineChar(char)` (line 34), `CmdlineBackspace` (line 35), `RunCommand(Command)` (line 37). Add `OpenPaletteDirect` next to `OpenPalette` (line 22):

```rust
    OpenPalette,
    OpenPaletteDirect,
    CloseOverlay,
```

(Delete the four cmdline variants entirely. Keep `PaletteMove`/`PaletteChar`/`PaletteBackspace`/`PaletteRun`/`PaletteArgCancel`.)

- [ ] **Step 5: Delete `route_cmdline_key`; extend `route_palette_key` with the Direct branch**

In `crates/zoid-tui/src/route.rs`, delete the entire `route_cmdline_key` function (lines 238-248). Then replace `route_palette_key` (lines 220-236):

```rust
fn route_palette_key(state: &ShellState, key: KeyEvent) -> Action {
    let in_arg = matches!(state.palette.stage, crate::state::PaletteStage::Arg { .. });
    let in_direct = !in_arg && state.palette.query.starts_with(':');
    match key.code {
        // Esc: in Arg phase return to the Pick list; otherwise close.
        KeyCode::Esc if in_arg => Action::PaletteArgCancel,
        KeyCode::Esc => Action::CloseOverlay,
        KeyCode::Enter => Action::PaletteRun,
        // Selection nav only applies to the Pick list (not Direct, not Arg).
        KeyCode::Up if !in_arg && !in_direct => Action::PaletteMove(-1),
        KeyCode::Down if !in_arg && !in_direct => Action::PaletteMove(1),
        KeyCode::Backspace => Action::PaletteBackspace,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::PaletteChar(c)
        }
        _ => Action::Noop,
    }
}
```

- [ ] **Step 6: Change the two `:` arms in `route_key` to emit `OpenPaletteDirect`**

In `crates/zoid-tui/src/route.rs`, the `Focus::Conversation` arm (line 201) and the `Focus::Rail` arm (line 213) both currently have:

```rust
            KeyCode::Char(':') => Action::OpenCommandLine,
```

Change both to:

```rust
            KeyCode::Char(':') => Action::OpenPaletteDirect,
```

- [ ] **Step 7: Delete the `parse_command` / `Command` imports used only by `route_cmdline_key`**

In `crates/zoid-tui/src/route.rs` line 7, the import `use crate::command::{parse_command, Command};` was used by `route_cmdline_key`. After deletion, `parse_command` is no longer referenced from `route.rs` (the Direct branch in the bin uses it, not the router). Check whether `Command` is still used elsewhere in `route.rs` — it appears in the `RunCommand(Command)` variant being deleted and in the `palette_selected_command` return type (`Option<Command>`). Keep `Command`; drop `parse_command`:

```rust
use crate::command::Command;
```

- [ ] **Step 8: Run the route tests to verify they pass**

Run: `cargo test -p zoid-tui --lib route::tests`
Expected: PASS — `palette_direct_phase_routing` and `colon_opens_palette_direct_when_not_input` green; all existing palette/arg/pick tests still green (the `in_direct` guard only adds a `Noop` for arrows when the query has a `:` prefix, which the existing Pick tests don't seed).

- [ ] **Step 9: Run the full workspace to confirm only the bin and snapshots still fail**

Run: `cargo build --workspace`
Expected: FAIL in `crates/zoid/src/main.rs` — the bin still has `Action::OpenCommandLine`/`CmdlineChar`/`CmdlineBackspace`/`RunCommand` arms. That's Task 7. The `zoid-tui` crate itself should compile.

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (all `zoid-tui` lib tests).

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tui/src/palette.rs crates/zoid-tui/src/route.rs
git commit -m "feat(tui): Phase enum + Direct-phase routing; delete cmdline Action variants"
```

---

## Task 4: Delete `CmdlineState`, `Overlay::CommandLine`, the cmdline layout rect, and `render_cmdline` together

The state, layout, and render cmdline references are interdependent — deleting the state types breaks render/layout compilation, and deleting render/layout first leaves dead state. So this task removes all the cmdline scaffolding in one atomic commit, then verifies the crate compiles and the surviving tests pass. The Direct-phase *render* (replacing what `render_cmdline` did) is Task 5.

**Files:**
- Modify: `crates/zoid-tui/src/state.rs:46-57` (`Overlay` enum), `state.rs:102-105` (`CmdlineState`), `state.rs:128-129` (`ShellState.cmdline` field), `state.rs:295-296` (`ShellState::new`), `state.rs:388-394` (`close_overlay`), `state.rs:578-588` (the `close_overlay_resets_*` test)
- Modify: `crates/zoid-tui/src/layout.rs:163-176` (`ShellLayout`), `layout.rs:266-295` (the `cmdline` computation block + `ShellLayout` initializer)
- Modify: `crates/zoid-tui/src/render.rs:190-209` (overlay dispatch), `render.rs:726-737` (`render_cmdline` — delete), `render.rs:669-716` (`render_palette` — temporarily replace the removed cmdline path with a Pick-only stub; Task 5 fills Direct)
- Test: `crates/zoid-tui/src/state.rs` (the `close_overlay_resets_palette_query` test)

- [ ] **Step 1: Update the `close_overlay_resets_palette_query` test first**

In `crates/zoid-tui/src/state.rs`, the test at line 578 asserts `s.cmdline == CmdlineState::default()`. Drop that assertion (the field is going away). Replace the test body:

```rust
    #[test]
    fn close_overlay_resets_palette_query() {
        let mut s = ShellState::new();
        s.overlay = Overlay::Palette;
        s.palette.query = "comp".into();
        s.palette.selected = 3;
        s.close_overlay();
        assert_eq!(s.overlay, Overlay::None);
        assert_eq!(s.palette, PaletteState::default());
    }
```

- [ ] **Step 2: Delete `Overlay::CommandLine` from `state.rs`**

Edit the `Overlay` enum (state.rs:46-57). Remove the `CommandLine,` line:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    Objects,
    Verbs,
    Sessions,
    Config,
    Question,
    ProviderSwitch,
}
```

- [ ] **Step 3: Delete `CmdlineState` and the `cmdline` field from `state.rs`**

Delete the `CmdlineState` struct (state.rs:102-105). In `ShellState`, delete the `pub cmdline: CmdlineState,` field (state.rs:129). In `ShellState::new` (state.rs:296), delete the `cmdline: CmdlineState::default(),` initializer line.

- [ ] **Step 4: Update `close_overlay` in `state.rs`**

In `crates/zoid-tui/src/state.rs` (line 388-394), delete the `self.cmdline = CmdlineState::default();` line:

```rust
    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
        self.palette = PaletteState::default();
        self.objects = ObjectState::default();
        self.sessions.clear();
        self.session_selected = 0;
    }
```

(Keep whatever else was after `session_selected = 0;` in the original — read the full function first to be sure.)

- [ ] **Step 5: Delete the `cmdline` field + computation from `layout.rs`**

In `crates/zoid-tui/src/layout.rs`, edit `ShellLayout` (lines 163-176): remove the `pub cmdline: Option<Rect>,` line. Delete the entire `let cmdline = if state.overlay == Overlay::CommandLine { ... } else { None };` block (lines 274-283). In the `ShellLayout { ... }` initializer (lines 285-295), delete the `cmdline,` line.

- [ ] **Step 6: Delete `render_cmdline` and its dispatch arm in `render.rs`**

Delete the `render_cmdline` function (render.rs:726-737). In the overlay dispatch (render.rs:190-209), delete the `else if state.overlay == Overlay::CommandLine { ... }` branch so the dispatch reads:

```rust
    // Overlays last, over a cleared region.
    if state.overlay == Overlay::Palette {
        if let Some(p) = layout.palette {
            render_palette(frame, state, p);
        }
    } else if state.overlay == Overlay::Objects {
```

(removing the three `CommandLine` lines between the `Palette` and `Objects` arms).

- [ ] **Step 7: Verify the crate compiles**

Run: `cargo build -p zoid-tui`
Expected: PASS — no remaining references to `Overlay::CommandLine`, `CmdlineState`, or `render_cmdline` inside the `zoid-tui` crate. (`render_palette` still renders only Pick/Arg — Direct comes in Task 5. The `palette_direct_phase_frame` snapshot test added in Task 6 will fail until Task 5, but the crate compiles.)

- [ ] **Step 8: Run the `zoid-tui` lib tests**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS — `state::tests`, `layout::tests`, `palette::tests`, `route::tests` all green. (`shell_snapshot` integration tests may fail on `palette_overlay_frame` from Task 2's row expansion — Task 6 regenerates. Lib tests don't hit snapshots.)

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/layout.rs crates/zoid-tui/src/render.rs
git commit -m "refactor(tui): delete CmdlineState + Overlay::CommandLine + cmdline layout/render"
```

---

## Task 5: Render the Direct-phase preview line in `render_palette`

**Files:**
- Modify: `crates/zoid-tui/src/render.rs:669-716` (`render_palette` — add Direct branch using `resolve_phase` from Task 3)

- [ ] **Step 1: Add the Direct-phase branch to `render_palette`**

In `crates/zoid-tui/src/render.rs`, replace `render_palette` (lines 669-716). The title must reflect three cases: Pick non-`:`, Pick `:`-prefix (Direct), Arg. The body branches via `resolve_phase` (added in Task 3). Add top-level imports with the other `use crate::palette::...` imports near the top of the file: `use crate::palette::{resolve_phase, Phase};` and `use crate::command::Command;` (if not already imported). Do not put `use` statements inside the function body.

```rust
fn render_palette(frame: &mut Frame, state: &ShellState, area: Rect) {
    frame.render_widget(Clear, area);

    let title = match &state.palette.stage {
        PaletteStage::Pick if state.palette.query.starts_with(':') => {
            format!(" {} ", state.palette.query)
        }
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

    match resolve_phase(&state.palette) {
        Phase::Arg => {
            let hint = Line::styled("Enter apply · Esc back", Style::new().fg(color::DIM));
            frame.render_widget(Paragraph::new(vec![hint]), inner);
        }
        Phase::Direct { cmd } => {
            let preview: String = match cmd {
                Command::Unknown(s) if s.is_empty() => {
                    "type :mode, :q, :delegate …".to_string()
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
            let line = Line::styled(preview, Style::new().fg(color::DIM));
            frame.render_widget(Paragraph::new(vec![line]), inner);
        }
        Phase::Pick => {
            let items = all_items(&state.active_mode, &state.mode_names, state.companion_on);
            let matches = selectable_matches(&items, &state.palette.query);
            let sel = nav(state.palette.selected, 0, matches.len());

            let mut lines: Vec<Line> = Vec::new();
            let mut selected_line: usize = 0;
            for (rank, &i) in matches.iter().enumerate() {
                if rank == sel {
                    selected_line = lines.len();
                }
                lines.push(palette_row_line(&items[i], rank == sel));
            }

            let vh = inner.height as usize;
            let off = selected_line.saturating_sub(vh.saturating_sub(1));
            frame.render_widget(Paragraph::new(lines).scroll((off as u16, 0)), inner);
        }
    }
}
```

- [ ] **Step 2: Run the `zoid-tui` lib tests**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/render.rs
git commit -m "feat(render): Direct-phase preview line in render_palette"
```

---

## Task 6: Regenerate palette snapshots; add Direct snapshot; remove cmdline snapshot

**Files:**
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs:402-428` (regenerate `palette_overlay_frame`; add `palette_direct_phase_frame`; delete `command_line_frame`)
- Modify: `crates/zoid-tui/tests/snapshots/` (regenerate `shell_snapshot__palette_overlay_frame.snap`; add `shell_snapshot__palette_direct_phase_frame.snap`; delete `shell_snapshot__command_line_frame.snap`)

- [ ] **Step 1: Delete the `command_line_frame` test and its snapshot**

In `crates/zoid-tui/tests/shell_snapshot.rs`, delete the `command_line_frame` test function (lines 422-428):

```rust
#[test]
fn command_line_frame() {
    let mut s = ShellState::new();
    s.overlay = Overlay::CommandLine;
    s.cmdline.buffer = "build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}
```

Delete the file `crates/zoid-tui/tests/snapshots/shell_snapshot__command_line_frame.snap`.

- [ ] **Step 2: Add the `palette_direct_phase_frame` test**

In `crates/zoid-tui/tests/shell_snapshot.rs`, add next to `palette_overlay_frame`:

```rust
#[test]
fn palette_direct_phase_frame() {
    let mut s = ShellState::new();
    s.mode_names = vec!["Chat".into(), "Build".into()];
    s.overlay = Overlay::Palette;
    s.palette.query = ":mode Build".into();
    insta::assert_snapshot!(draw(&s, &seeded(), 100, 24));
}
```

- [ ] **Step 3: Regenerate the snapshots**

Run: `cargo insta test --accept --workspace`
Expected: `shell_snapshot__palette_overlay_frame.snap` regenerated to the new 14-row list; `shell_snapshot__palette_direct_phase_frame.snap` created; `palette_arg_stage_frame.snap` unchanged (Arg render is unchanged).

- [ ] **Step 4: Inspect the regenerated snapshots**

Open `crates/zoid-tui/tests/snapshots/shell_snapshot__palette_overlay_frame.snap` and confirm it shows the single ranked match ` Switch to Build` (the test seeds `query = "build"`, and `selectable_matches` filters the 14-row list down to the one row whose label fuzzy-matches "build"). The full 14-row curated set is pinned by the `all_items_is_flat_curated` unit test in Task 2, not by this snapshot.

Open `shell_snapshot__palette_direct_phase_frame.snap` and confirm it shows the title ` :mode Build ` and the body `→ Switch to Build`.

- [ ] **Step 5: Run the snapshot tests to verify they pass**

Run: `cargo test -p zoid-tui --test shell_snapshot`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/
git commit -m "test(snapshots): regenerate palette; add Direct-phase; remove cmdline"
```

---

## Task 7: Wire the bin — delete cmdline arms; add `OpenPaletteDirect`; extend `PaletteRun`; fix `RenameSession("")` seeding

**Files:**
- Modify: `crates/zoid/src/main.rs:2237-2319` (the `OpenPalette` / `OpenCommandLine` / `PaletteRun` / `PaletteArgCancel` / `CmdlineChar` / `CmdlineBackspace` / `RunCommand` arms)
- Modify: `crates/zoid/src/main.rs:3263-3276` (the `RenameSession` arm in `exec_command`)
- Modify: `crates/zoid-tui/examples/scenes/mod.rs:177-183` (the `palette` scene — remove the stale `s.cmdline.buffer = "build"` line)

- [ ] **Step 1: Delete the `cmdline` scene arm in `crates/zoid-tui/examples/scenes/mod.rs`**

In `crates/zoid-tui/examples/scenes/mod.rs:181-184`, delete the entire `"cmdline" => { ... }` match arm:

```rust
        "cmdline" => {
            s.overlay = Overlay::CommandLine;
            s.cmdline.buffer = "build".into();
        }
```

It references the deleted `Overlay::CommandLine` and `s.cmdline` field (Task 4). The `"palette"` arm (lines 177-180) is already correct — leave it. Also update the doc comment in `crates/zoid-tui/examples/preview.rs:7` which lists `cmdline` as a valid scene — drop `cmdline` from the scene list there.

- [ ] **Step 2: Delete the cmdline action arms in `crates/zoid/src/main.rs`**

In `crates/zoid/src/main.rs`, delete the `Action::OpenCommandLine => { ... }` arm (lines 2241-2244), the `Action::CmdlineChar(c) => ...` arm (line 2313), the `Action::CmdlineBackspace => { ... }` arm (lines 2314-2316), and the `Action::RunCommand(c) => { ... }` arm (lines 2317-2319).

- [ ] **Step 3: Add the `OpenPaletteDirect` arm**

In `crates/zoid/src/main.rs`, right after the `Action::OpenPalette => { ... }` arm (lines 2237-2240), add:

```rust
        Action::OpenPaletteDirect => {
            app.shell.overlay = Overlay::Palette;
            app.shell.palette = Default::default();
            app.shell.palette.query.push(':');
        }
```

- [ ] **Step 4: Extend `PaletteRun` with the Direct branch**

In `crates/zoid/src/main.rs`, replace the `Action::PaletteRun` arm (lines 2279-2309). The Pick branch gains a `:`-prefix check that runs `parse_command` directly:

```rust
        Action::PaletteRun => match app.shell.palette.stage.clone() {
            zoid_tui::state::PaletteStage::Pick => {
                if app.shell.palette.query.starts_with(':') {
                    // Direct phase — parse the whole buffer and run it.
                    let cmd = zoid_tui::command::parse_command(&app.shell.palette.query);
                    app.shell.close_overlay();
                    return exec_command(app, cmd).await;
                }
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

- [ ] **Step 5: Fix the `RenameSession("")` arm in `exec_command`**

In `crates/zoid/src/main.rs` (lines 3263-3276), replace the `Command::RenameSession(name)` arm:

```rust
        Command::RenameSession(name) => {
            if name.is_empty() {
                // Seed the palette in Direct phase so the user types the name.
                app.shell.overlay = zoid_tui::Overlay::Palette;
                app.shell.palette = Default::default();
                app.shell.palette.query = ":rename ".into();
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

- [ ] **Step 6: Build the workspace**

Run: `cargo build --workspace`
Expected: PASS — no remaining references to `OpenCommandLine`, `CmdlineChar`, `CmdlineBackspace`, `RunCommand`, `CmdlineState`, or `Overlay::CommandLine`.

- [ ] **Step 7: Run the full workspace tests**

Run: `cargo test --workspace`
Expected: PASS — all unit, integration, and snapshot tests green.

- [ ] **Step 8: Run clippy and fmt**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS — no warnings (check for unused imports of `parse_command`/`Command` in `route.rs`, unused `CmdlineState` references, etc.).

Run: `cargo fmt --all`
Expected: no changes (or apply them).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid/src/main.rs crates/zoid-tui/examples/scenes/mod.rs
git commit -m "feat(zoid): wire adaptive palette — Direct branch, OpenPaletteDirect, RenameSession seeding"
```

---

## Task 8: Update the UX mockup caption

**Files:**
- Modify: `docs/ux/palette.html:32, 81-83` (the page title, the cmdline footer note, the caption paragraph)

- [ ] **Step 1: Update the page title and status bar**

In `docs/ux/palette.html` line 32, the title currently says `zoid — command palette (^P) &amp; command line (:)`. Change to:

```html
  <div class="head"><h1>zoid — command palette (^P)</h1><p>Fuzzy, mode-aware action launcher. Type <b>:</b> inside it for direct commands (:mode, :q, :delegate …).</p></div>
```

- [ ] **Step 2: Remove the cmdline footer + caption mention**

In `docs/ux/palette.html` line 81, the cmdline footer div:

```html
    <div class="cmdline">: new<span class="dim">         ← vim-style direct command (:new :rename :chat :model … :pin :q) for power users</span></div>
```

Delete this line entirely.

In `docs/ux/palette.html` line 83, the caption paragraph mentions "Two entry points: ^P opens the fuzzy palette … : is the direct command line for power users." Replace it with:

```html
  <p class="cap"><b>One adaptive palette (^P).</b> Empty query: fuzzy, mode-aware list — grouped (session · mode/app · navigate · context ⑤ · settings), each row showing its <em>keybind</em> so you learn shortcuts by using it; results rank by recency + match. <b>Type <code>:</code> as the first character</b> to switch the same prompt to direct-command mode: live-preview the parsed command, <b>⏎</b> runs it. The palette surfaces <em>global</em> verbs (new/resume session, quit, settings) plus current-mode actions — and since rail drawers carry no keybinds for now, it's how you toggle them. <span class="dim">(Switch to Build is deferred until the autonomous loop is built.)</span></p>
```

- [ ] **Step 3: Commit**

```bash
git add docs/ux/palette.html
git commit -m "docs(ux): update palette mockup for the merged adaptive UI"
```

---

## Self-Review (run after writing — already done, revised after Gilfoyle review)

**1. Spec coverage:**
- §1 State model (one overlay, one state) → Task 4.
- §2 Three phases, derived not stored → Task 3 (`resolve_phase`), Task 5 (render branch), Task 7 (bin branch).
- §3 ArgKind extension + six new rows → Task 1, Task 2.
- §4 Drawer toggles (zero-arg) → Task 2 (the three `OpenDrawer` rows).
- §5 Routing → Task 3 (also fixes `overlay_captures_keys_first` test).
- §6 Render → Task 5.
- §7 Bin handlers (delete cmdline arms, `OpenPaletteDirect`, `PaletteRun` Direct branch, `RenameSession("")` edit) → Task 7.
- §8 Deletions → Task 4 (state + layout + render cmdline removal, atomic), Task 7 (bin arms + scenes).
- §9 Testing strategy → covered across Tasks 1-7.
- §11 Non-goals → respected (no MRU, no editing chords, no tab completion, no generic picker).
- UX mockup → Task 8.
- Supersession pointer on 2026-07-04 spec → already present in the spec file (added during brainstorming); no plan task needed.

**2. Placeholder scan:** No TBD / TODO / "implement later" / "similar to Task N". Every step shows the exact code or command. (Task 7 Step 1 previously hand-waved the scenes edit — replaced with the exact `cmdline` arm deletion.)

**3. Type consistency:**
- `Phase` enum (no lifetime, owns `Command`) — consistent across Task 3 (definition) and Task 5 (render match).
- `OpenPaletteDirect` — consistent across Task 3 (Action enum), Task 3 (route test), Task 7 (bin arm).
- `arg_kind_for` arms — consistent across Task 1 (definition) and Task 1 (test).
- `RenameSession("")` seeding — `query = ":rename "` consistent across Task 7 (bin arm) and spec §7.
- `DrawerId` import in `all_items` — `use crate::state::DrawerId;` inside the function body (Task 2); matches the existing `DrawerId` path in `state.rs`.

**4. Task ordering / compile-ability:** Tasks 1-3 leave the `zoid` bin non-compiling (cmdline Action arms still present) but `zoid-tui` compiles. Task 4 atomically deletes state + layout + render cmdline scaffolding so the `zoid-tui` crate compiles after Step 7. Task 5 adds Direct render. Task 6 regenerates snapshots. Task 7 wires the bin (last, so `cargo build --workspace` passes). No mid-task compile dead-ends.

No issues remaining after Gilfoyle's blocker/major fixes applied.