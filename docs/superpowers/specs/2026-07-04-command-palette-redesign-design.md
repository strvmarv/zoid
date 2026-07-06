# Command Palette Redesign (VSCode-style) — Design Spec

> **Superseded 2026-07-06** by `2026-07-06-command-surfaces-consolidation-design.md`, which merges the `:` command line and the `Ctrl+P` palette into one adaptive overlay. The "keep both" lock in §Decisions.5 below is reversed by that spec. This document remains as history.

**Date:** 2026-07-04
**Status:** Approved for planning (superseded)
**Scope:** Single implementation plan. `zoid-tui`-local logic + a handful of `zoid` bin match arms. No cross-crate refactor.

## Goal

Turn zoid's `Ctrl+P` palette from a grouped, keybind-teaching **browse menu** into a
true VSCode-style **search-first command palette**: a flat, single-column, ranked
list you filter by typing, with the top match auto-selected and `Enter` to run.
Commands that need a typed argument (Rename) capture that argument **inline**,
without leaving the overlay.

## Background: what exists today

- **Trigger:** `Ctrl+P` → `Action::OpenPalette` (`route.rs:151`). Status bar advertises `^P palette`.
- **State:** `PaletteState { query: String, selected: usize }` on `ShellState.palette`
  (`state.rs:88`), reset by `close_overlay()` (`state.rs:384`).
- **Items:** `palette.rs::all_items(mode) -> Vec<PaletteItem>` where
  `PaletteItem { group, icon, label, hint, keybind, command: Option<Command> }`
  (`palette.rs:10`). Items are grouped under headers (SESSION / MODE / BRANCH /
  CONTEXT / VIEW / SETTINGS / RECIPES), and several rows are **post-v1
  placeholders with `command: None`** (Fork from here, Undo last turn, Pin file
  to context, Evict cold items, Run recipe…), rendered dimmed and skipped by
  selection.
- **Filtering:** `fuzzy_score(label, query)` (contiguous substring beats scattered
  subsequence, earlier match ranks higher, empty query matches all);
  `selectable_matches(items, query)` returns ranked indices of command-bearing
  rows; `nav(selected, delta, len)` wraps.
- **Routing:** `route_palette_key(key)` (`route.rs:222`) → `OpenPalette`,
  `CloseOverlay`, `PaletteMove(i32)`, `PaletteChar(char)`, `PaletteBackspace`,
  `PaletteRun`. Pure resolver `palette_selected_command(state) -> Option<Command>`
  (`route.rs:434`).
- **Render:** `render_palette(frame, state, area)` (`render.rs:666`) — centered
  bordered box (`centered(conversation, 72, 18)`, shared with Objects/Verbs/
  Sessions overlays), title shows the live query, grouped headers + a row per
  item with a keybind-teaching column, scroll-follow on the selection.
- **Execution:** `Action::PaletteRun` (`main.rs:1948`) resolves the selected
  `Command`, closes the overlay, and calls `exec_command(app, cmd)` — the **same
  sink** the `:` command line uses via `Action::RunCommand`. `exec_command`
  (`main.rs:2610`) treats `RenameSession("")` as "seed the `:` command line with
  `rename `" and `RenameSession(name)` as "apply the rename".
- **`:` command line:** `parse_command(raw)` (`command.rs:28`) maps text to the
  shared `Command` enum. Opened by typing `:`. Coexists with the palette; both
  converge on `exec_command`.

## Decisions (locked)

1. **Model:** Flat search-first list. No group headers, no keybind column, no
   icons — a single column of plain labels (matches the approved mockup).
2. **At-rest order:** Fixed curated order (no MRU, no persistence). Typing
   re-ranks by `fuzzy_score`, best match first. Deterministic — friendly to the
   snapshot tests.
3. **Non-implemented entries removed:** every `command: None` row is deleted.
   The palette shows only runnable commands. (Restore them when the underlying
   features land — noted in "Future" below.)
4. **Parameterized commands:** inline argument capture. Selecting a
   parameterized command (Rename) keeps the overlay open and switches the input
   line to an argument prompt; `Enter` applies, `Esc` returns to the pick list.
5. **`:` command line:** kept, unchanged. It remains the power-user fast path and
   the no-arg `:rename` entry. `exec_command` is **not modified**.
6. **Trigger:** `Ctrl+P`, unchanged. Mode-awareness kept (Switch to Build vs
   Chat depends on current mode).

## Architecture

The palette gains a **two-phase** lifecycle modeled by a `PaletteStage` enum:

- **Pick phase** — flat search. Type to filter/re-rank; `↑/↓` moves selection;
  `Enter` either runs the selected command (zero-arg) or transitions to Arg
  phase (parameterized); `Esc` closes.
- **Arg phase** — inline argument entry for the selected parameterized command.
  Type to edit the argument; `Enter` builds the final `Command` and runs it;
  `Esc` returns to Pick (preserving the prior query/selection).

The existing pure/effectful seam is preserved: **decisions** are pure functions in
`palette.rs` (unit-tested); **acts** (entering Arg phase, running commands,
mutating `App`) live in the `zoid` bin. `exec_command` and the shared `Command`
vocabulary are untouched — only *how a `Command` is assembled* changes.

### Component: `PaletteState` + `PaletteStage` (`crates/zoid-tui/src/state.rs`)

```rust
pub struct PaletteState {
    pub query: String,        // Pick-phase filter text
    pub selected: usize,      // index into ranked matches (Pick phase)
    pub stage: PaletteStage,  // NEW
}

#[derive(Default, Clone, PartialEq, Eq)]
pub enum PaletteStage {
    #[default]
    Pick,
    Arg { kind: ArgKind, input: String },
}
```

- `PaletteState` derives keep it `Default` (→ `stage: Pick`, empty `query`,
  `selected: 0`). `ArgKind` is imported from `crate::palette`.
- `close_overlay()` resets `palette` to `Default` (already does; now also clears
  `stage` back to `Pick` and drops any `input`).
- **Invariants:** transition Pick→Arg preserves `query`/`selected`; Arg→Pick
  (via `Esc`) drops `input`, preserves `query`/`selected`. `selected` is only
  meaningful in Pick.

### Component: argument semantics (`crates/zoid-tui/src/palette.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind { Rename }   // extensible: add Delegate later

impl ArgKind {
    pub fn prompt(&self) -> &'static str {
        match self { ArgKind::Rename => "Rename to" }
    }
    pub fn build(&self, input: String) -> Command {
        match self { ArgKind::Rename => Command::RenameSession(input) }
    }
}

/// Which parameterized-argument flow (if any) a command needs when chosen
/// from the palette. Pure — used by the bin to decide Pick→Arg transition.
pub fn arg_kind_for(cmd: &Command) -> Option<ArgKind> {
    match cmd {
        Command::RenameSession(_) => Some(ArgKind::Rename),
        _ => None,
    }
}
```

### Component: flat item set (`crates/zoid-tui/src/palette.rs`)

`PaletteItem` slims to:

```rust
pub struct PaletteItem {
    pub label: &'static str,
    pub command: Command,   // no longer Option — every row is runnable
}
```

`all_items(mode: Mode) -> Vec<PaletteItem>` returns this fixed curated order,
runnable-only:

1. New session — `Command::NewSession`
2. Resume session… — `Command::ResumeSessionPicker`
3. Rename session… — `Command::RenameSession(String::new())`
4. Switch to Build / Switch to Chat — `Command::SwitchMode(other)` (mode-aware label)
5. Overview — `Command::ShowOverview`
6. Open settings — `Command::OpenConfig`
7. Quit zoid — `Command::Quit`

Retained pure helpers (unchanged behavior): `fuzzy_score`, `nav`, and
`selectable_matches(items, query) -> Vec<usize>` — the latter now never has
disabled rows to skip, but keeps its name and ranking semantics to minimize
churn. Empty query returns all in curated order.

### Component: routing (`crates/zoid-tui/src/route.rs`)

`route_palette_key` takes `&ShellState` (was `key` only) to branch on
`state.palette.stage`. One new action: `Action::PaletteArgCancel`.

| Key | Pick phase | Arg phase |
|-----|-----------|-----------|
| `Esc` | `CloseOverlay` | `PaletteArgCancel` |
| `Enter` | `PaletteRun` | `PaletteRun` |
| `↑` | `PaletteMove(-1)` | `Noop` |
| `↓` | `PaletteMove(1)` | `Noop` |
| `Backspace` | `PaletteBackspace` | `PaletteBackspace` |
| `Char(c)` (no Ctrl) | `PaletteChar(c)` | `PaletteChar(c)` |
| other | `Noop` | `Noop` |

`palette_selected_command(state) -> Option<Command>` is unchanged (Pick-phase
resolver: `all_items` → `selectable_matches` → `nav` → command clone).

### Component: bin handlers (`crates/zoid/src/main.rs`)

`OpenPalette` sets `overlay = Palette` and `palette = Default` (→ Pick).

`PaletteChar(c)` / `PaletteBackspace` branch on stage:
- Pick: edit `query`; reset `selected = 0`.
- Arg: edit `input` (no selection reset).

`PaletteArgCancel` → `stage = Pick` (drops `input`; `query`/`selected` intact).

`PaletteMove(d)` recomputes ranked-match length and `nav`s `selected` (only ever
arrives in Pick — Arg maps arrows to `Noop`).

`PaletteRun`:

```
Pick:
    cmd = palette_selected_command(shell)     // None (no matches) → do nothing
    match arg_kind_for(&cmd):
        Some(kind) => shell.palette.stage = PaletteStage::Arg { kind, input: String::new() }  // stay open
        None       => shell.close_overlay(); return exec_command(app, cmd).await
Arg { kind, input }:
    if input.is_empty() => no-op (stay in Arg)          // reject empty rename
    else => let cmd = kind.build(input);
            shell.close_overlay();
            return exec_command(app, cmd).await
```

`exec_command` is **not changed**: the palette now always hands it a non-empty
`RenameSession(name)` (applies directly); the `RenameSession("")`→seed-cmdline
branch stays live for the no-arg `:rename` command-line path.

### Component: render (`crates/zoid-tui/src/render.rs`)

`render_palette(frame, state, area)` branches on stage. Rect unchanged
(`centered(conversation, 72, 18)`).

- **Pick:** title `› {query}` (prompt glyph + query + cursor). Body: flat
  single-column list, one line per ranked match rendered `" {label}"`; selected
  row painted `color::SEL_BG`. Scroll-follow retained (harmless: the 7-item list
  never overflows an 18-row box, but the logic stays for safety/future items).
  No group headers, no keybind column, no icons.
- **Arg:** title `{prompt}: {input}` (e.g. `Rename to: my-feature`) + cursor.
  Body: a single dim hint line — `Enter apply · Esc back`.

`palette_row_line` simplifies to label-only + selection background.

## Data flow

```
Ctrl+P ──▶ OpenPalette ──▶ overlay=Palette, stage=Pick, query=""
   │
   ├─ type ──▶ PaletteChar/Backspace ──▶ query edited, selected=0, list re-ranked
   ├─ ↑/↓  ──▶ PaletteMove ──▶ selected nav (wraps)
   └─ Enter ─▶ PaletteRun (Pick)
                 ├─ arg_kind_for = None ──▶ close_overlay + exec_command(cmd)
                 └─ arg_kind_for = Some(kind) ──▶ stage=Arg{kind, input=""}
                        │
                        ├─ type ──▶ PaletteChar/Backspace ──▶ input edited
                        ├─ Esc  ──▶ PaletteArgCancel ──▶ stage=Pick (query preserved)
                        └─ Enter ─▶ PaletteRun (Arg)
                                      ├─ input empty ──▶ no-op
                                      └─ else ──▶ close_overlay + exec_command(kind.build(input))
```

## Error / edge handling

- **No matches** (query filters everything out): `palette_selected_command`
  returns `None`; `Enter` is a no-op; the overlay stays open. Body shows an empty
  list.
- **Empty argument** on `Enter` in Arg phase: no-op, stays in Arg (prevents a
  rename to empty string). User must type a name or `Esc` out.
- **`Esc` semantics:** Arg→Pick (not close) so a mis-selected parameterized
  command doesn't dump the user out of the palette; a second `Esc` (now in Pick)
  closes.
- **Ctrl-modified / unknown keys:** `Noop`; overlay capture prevents leakage to
  global handlers while `overlay == Palette`.

## Testing strategy

**`palette.rs` unit tests:**
- `all_items` is flat/curated/runnable-only: first == "New session", last ==
  "Quit zoid", contains "Overview", **no** placeholder rows, `command` is a value
  (type no longer `Option`).
- Mode-awareness: label is "Switch to Build" in Chat mode and "Switch to Chat"
  in Build mode.
- `arg_kind_for`: `RenameSession(_)` → `Some(Rename)`; `ShowOverview`/`Quit`/
  `NewSession` → `None`.
- `ArgKind::Rename.build("x") == Command::RenameSession("x")`;
  `ArgKind::Rename.prompt() == "Rename to"`.
- Retain `fuzzy_score` ranking tests (substring outranks subsequence, empty
  query returns all) and `nav` wrap test.

**`route.rs` tests:**
- Pick phase (seed `stage: Pick`): `Esc`→`CloseOverlay`, `Enter`→`PaletteRun`,
  `↑/↓`→`PaletteMove`, `Backspace`→`PaletteBackspace`, `Char`→`PaletteChar`.
- Arg phase (seed `stage: Arg { Rename, "" }`): `Esc`→`PaletteArgCancel`,
  `Enter`→`PaletteRun`, `↑/↓`→`Noop`, `Char`→`PaletteChar`,
  `Backspace`→`PaletteBackspace`.
- Keep `palette_selected_command_resolves_highlighted_row` (Pick resolver).

**`state.rs` test:** extend the `close_overlay` reset test to assert
`stage == PaletteStage::Pick` and `query.is_empty()` after close.

**Snapshot tests (`crates/zoid-tui/tests/shell_snapshot.rs`):**
- Regenerate `palette_overlay_frame` as the flat list (e.g. query `"se"` →
  ranked flat results, no headers).
- **Remove** `palette_overlay_scrolled_to_end_frame` (dead: 7 items never
  scroll) and its `.snap`.
- **Add** `palette_arg_stage_frame`: `stage = Arg { Rename, "my-feature" }` →
  snapshot the `Rename to: my-feature` prompt view.

**Full workspace:** `cargo test` + `cargo clippy --workspace` green; `cargo fmt`.

## Non-goals / Future

- **MRU / usage memory** — explicitly rejected for this pass (curated order only).
- **Removing the `:` command line** — kept.
- **Adding Delegate to the palette** — `arg_kind_for`/`ArgKind` are built to
  extend (`Delegate` → task arg), but Delegate stays `:`-only for now (YAGNI).
- **Restoring placeholder rows** (Fork, Undo, Pin file, Evict cold, Run recipe)
  — re-add to `all_items` with their real `Command` when those features ship.
- **Extracting a generic picker** shared with Objects/Verbs/Sessions overlays —
  deferred (would touch four features + snapshots).

## Files touched

- `crates/zoid-tui/src/state.rs` — `PaletteStage` enum, `stage` field, reset.
- `crates/zoid-tui/src/palette.rs` — `ArgKind`, `arg_kind_for`, slimmed
  `PaletteItem`, flat/curated `all_items`, updated unit tests.
- `crates/zoid-tui/src/route.rs` — `PaletteArgCancel` action, stage-aware
  `route_palette_key`, updated route tests.
- `crates/zoid/src/main.rs` — stage-branching `PaletteChar`/`PaletteBackspace`/
  `PaletteRun`, new `PaletteArgCancel` arm.
- `crates/zoid-tui/src/render.rs` — flat Pick render + Arg render, slimmed
  `palette_row_line`.
- `crates/zoid-tui/tests/shell_snapshot.rs` (+ snapshots) — regenerate/replace.
