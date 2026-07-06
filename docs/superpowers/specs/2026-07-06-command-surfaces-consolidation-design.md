# Command Surfaces Consolidation (Adaptive Palette) — Design Spec

**Date:** 2026-07-06
**Status:** Approved for planning
**Scope:** Single implementation plan. `zoid-tui`-local logic + `zoid` bin match arms. No cross-crate refactor. Supersedes the "keep both" lock in `2026-07-04-command-palette-redesign-design.md` §Decisions.5.

## Goal

Collapse zoid's two command surfaces — the `Ctrl+P` command palette and the `:` command line — into **one adaptive overlay**: a palette-primary widget where typing `:` as the first character switches the same prompt into a direct-command mode. Both entry points converge on the shared `Command` enum and the existing `exec_command` sink. The `:` muscle memory is preserved exactly; the discoverable fuzzy list is preserved exactly; the two code paths (state, routing, render, actions) become one.

## Background: what exists today

- **Command Palette (`Ctrl+P`)** — `crates/zoid-tui/src/palette.rs`. Flat, fuzzy-filtered, ranked list of curated runnable actions. Two-phase lifecycle: `Pick` (search-and-select) → `Arg` (inline argument entry for `Rename`). Items: New/Resume/Rename session, Switch to \<mode\>, Reload modes, Open settings, Enable/Disable companion, Quit.
- **Command Line (`:`)** — `crates/zoid-tui/src/command.rs::parse_command`. Vim-style direct text. Maps `:mode <name>`, `:mode reload`, `:mode import <url>`, `:mode update <name>`, `:q`, `:repo`/`:session`/`:context`, `:new`, `:rename`, `:delegate <task>`, `:config`, `:companion [off]` to `Command`.
- **Shared sink:** both resolve to the `Command` enum and feed `exec_command` in `crates/zoid/src/main.rs:3097`.
- **Divergence today:**
  - `:`-only: `:delegate`, `:mode import`, `:mode update`, drawer toggles (`:repo`/`:session`/`:context`).
  - Palette-only: `ResumeSessionPicker`.
  - Both: New/Rename/SwitchMode/ReloadModes/OpenConfig/Companion/Quit.
- **Prior lock (`2026-07-04-command-palette-redesign-design.md` §Decisions.5):** "keep both, `exec_command` not modified." This spec reverses the "keep both" half; `exec_command` is essentially untouched (one tiny edit — see §Decisions.7).

## Decisions (locked)

1. **Model:** One overlay (`Overlay::Palette`), one state struct (`PaletteState`), one prompt. The buffer content selects the phase — no stored phase for Direct.
2. **Three phases, two stages:**
   - `Pick` stage, query empty or non-`:` → **Pick phase** (fuzzy list, today's behavior).
   - `Pick` stage, query starts with `:` → **Direct phase** (list hidden, live `parse_command` preview, `Enter` runs).
   - `Arg` stage → **Arg phase** (inline argument entry, reached only from Pick by selecting a parameterized row).
3. **Direct is derived, not stored:** `query.starts_with(':')` is the test. No new `PaletteStage` variant. The `:` transition is a render/route/exec branch, not an action.
4. **Fold `:`-only parameterized commands into the palette:** `ArgKind` grows from `Rename` to four variants. `Delegate`, `ModeImport`, `ModeUpdate` become palette rows with inline Arg capture, multi-word free-text. `parse_command` stays the Direct-mode parser for the same commands.
5. **Fold `:`-only zero-arg commands into the palette:** `:repo`/`:session`/`:context` become three plain `OpenDrawer(DrawerId)` palette rows. No `ArgKind`.
6. **`:`-from-conversation trigger preserved:** typing `:` in Conversation/Rail focus opens the palette with the buffer pre-seeded to `":"` (new `Action::OpenPaletteDirect`). `Ctrl+P` opens it empty (`Action::OpenPalette`, unchanged).
7. **`exec_command` essentially untouched** — one tiny edit: the `RenameSession("")` arm seeds the palette in Direct phase (`query = ":rename "`) instead of the deleted cmdline. It already handles `Unknown(String)`, `Delegate("")`, `ModeImport("")`, `ModeUpdate("")` as silent no-ops / status hints. The palette's Arg phase always hands it a non-empty arg (existing Rename contract).
8. **`Command` enum and `parse_command` untouched.** They remain the shared vocabulary.

## Architecture

### State model (`crates/zoid-tui/src/state.rs`)

`Overlay::CommandLine` and `CmdlineState` are **removed**. The palette absorbs everything.

```rust
pub struct PaletteState {
    pub query: String,        // the single buffer; ':' prefix ⇒ Direct phase
    pub selected: usize,      // Pick-phase list index
    pub stage: PaletteStage,  // Pick | Arg
}
```

`Overlay` loses the `CommandLine` variant. `close_overlay()` resets `palette = Default` (already does — covers Direct-seeded case for free). The `cmdline` field on `ShellState` is deleted along with `CmdlineState`.

### Phase selection (`crates/zoid-tui/src/palette.rs`)

A single pure resolver decides what a given `PaletteState` means:

```rust
pub enum Phase<'a> {
    Pick,                          // empty or non-':' query → fuzzy list
    Direct { cmd: &'a Command },   // query starts with ':' → live-parsed
    // Arg is PaletteStage::Arg, not a Phase — it's a real stage transition.
}
```

- **Pick** — `selectable_matches` ranks `all_items`, top match auto-selected, `Enter` runs it (or enters Arg for parameterized).
- **Direct** — list hidden; buffer (minus the `:`) run through `parse_command` live; rendered preview shows the resolved `Command`; `Enter` runs `parse_command(query)` via `exec_command`. Arrows inert.
- **Arg** — reached only by selecting a parameterized row in Pick. Esc → back to Pick, query preserved.

**Transitions:**

| From | Event | To |
|---|---|---|
| Pick (non-`:`) | user types `:` as first char | Direct (same overlay, no stage change) |
| Direct | user backspaces the `:` | Pick |
| Pick | `Enter` on parameterized row | Arg |
| Arg | `Esc` | Pick (query preserved) |
| Arg / Direct | `Enter` (valid) | close + `exec_command` |

The `:` transition is **not** a state mutation — it's a render/routing branch on the buffer prefix.

### ArgKind extension (`crates/zoid-tui/src/palette.rs`)

```rust
pub enum ArgKind {
    Rename,           // "Rename to"            → RenameSession(String)
    Delegate,         // "Delegate task"        → Delegate(String)   (free-text, multi-word)
    ModeImport,       // "Import mode from URL" → ModeImport(String)
    ModeUpdate,       // "Update mode"          → ModeUpdate(String)
}
```

`arg_kind_for` maps the placeholder forms to their `ArgKind`:

```rust
pub fn arg_kind_for(cmd: &Command) -> Option<ArgKind> {
    match cmd {
        Command::RenameSession(_) => Some(ArgKind::Rename),
        Command::Delegate(_)      => Some(ArgKind::Delegate),
        Command::ModeImport(_)    => Some(ArgKind::ModeImport),
        Command::ModeUpdate(_)    => Some(ArgKind::ModeUpdate),
        _ => None,
    }
}
```

Each `ArgKind` gains `prompt()` and `build(input)` arms. Free-text Arg rendering uses the same single-line input Rename uses today — `PaletteChar`/`PaletteBackspace` already handle arbitrary strings. No new editing machinery.

### Flat item set (`crates/zoid-tui/src/palette.rs`)

`all_items(active_mode, mode_names, companion_on)` returns this fixed curated order, runnable-only:

1. New session — `Command::NewSession`
2. Resume session… — `Command::ResumeSessionPicker`
3. Rename session… — `Command::RenameSession(String::new())`
4. **Delegate task…** — `Command::Delegate(String::new())`  *(new)*
5. **Import mode from URL…** — `Command::ModeImport(String::new())`  *(new)*
6. **Update mode…** — `Command::ModeUpdate(String::new())`  *(new)*
7. Switch to \<mode\> rows — `Command::SwitchMode(other)` per non-active mode
8. Reload modes — `Command::ReloadModes`
9. **Toggle repo drawer** — `Command::OpenDrawer(DrawerId::Repo)`  *(new)*
10. **Toggle session drawer** — `Command::OpenDrawer(DrawerId::Session)`  *(new)*
11. **Toggle context drawer** — `Command::OpenDrawer(DrawerId::Context)`  *(new)*
12. Open settings — `Command::OpenConfig`
13. Enable/Disable companion — `Command::CompanionEnable`/`CompanionDisable` (state-aware)
14. Quit zoid — `Command::Quit`

Retained pure helpers unchanged: `fuzzy_score`, `nav`, `selectable_matches(items, query) -> Vec<usize>`. Empty query returns all in curated order.

### Routing (`crates/zoid-tui/src/route.rs`)

`route_palette_key` already takes `&ShellState` and branches on `stage`. Direct adds a second branch on the `:` prefix. **No new Action variant for the Direct *phase*** — Direct is detected inside `route_palette_key` by `state.palette.query.starts_with(':')`. (A new `Action::OpenPaletteDirect` exists, but it's an *entry point* for opening the palette pre-seeded — see below — not a phase selector.)

| Key | Pick (non-`:`) | Direct (`:` prefix) | Arg |
|---|---|---|---|
| `Esc` | `CloseOverlay` | `CloseOverlay` | `PaletteArgCancel` |
| `Enter` | `PaletteRun` | `PaletteRun` | `PaletteRun` |
| `↑`/`↓` | `PaletteMove(∓1)` | `Noop` | `Noop` |
| `Backspace` | `PaletteBackspace` | `PaletteBackspace` | `PaletteBackspace` |
| `Char(c)` (no Ctrl) | `PaletteChar(c)` | `PaletteChar(c)` | `PaletteChar(c)` |
| other | `Noop` | `Noop` | `Noop` |

**Deleted:** `route_cmdline_key`, `Action::OpenCommandLine`, `Action::CmdlineChar`, `Action::CmdlineBackspace`, `Action::RunCommand(Command)`.

**New:** `Action::OpenPaletteDirect` — opens the palette with `query` pre-seeded to `":"`. The global `:` key (Conversation/Rail focus) emits this instead of the deleted `OpenCommandLine`. One line change per focus arm in `route_key`.

`palette_selected_command(state)` is unchanged (Pick resolver); Direct doesn't use it.

### Render (`crates/zoid-tui/src/render.rs`)

`render_palette` keeps its centered rect and branches on **stage first, then phase**:

- **Arg stage** — unchanged: title `{prompt}: {input}▌`, single dim hint line `Enter apply · Esc back`.
- **Pick stage, non-`:` query** — unchanged: flat ranked list, `› {query}▌` title, selected row highlighted, scroll-follow.
- **Pick stage, `:` prefix** (Direct) — list hidden. Title `:{rest}▌`. Body is a **single preview line**:
  - Known command → dim label of the resolved `Command`, e.g. `→ Switch to Build` or `→ Delegate: add a test for parse()`.
  - Unknown → dim `unknown command`.
  - Only `:` present → dim hint `type :mode, :q, :delegate …`.
  - Footer hint row stays (`↑↓ move · ⏎ run · esc close`).

`render_cmdline` is deleted. `palette_row_line` is unchanged (label-only + selection bg); the new parameterized rows and drawer rows render identically to existing rows. The cmdline rect in `ShellLayout` is deleted (the `palette` rect already covers the merged overlay).

### Bin handlers (`crates/zoid/src/main.rs`)

Cmdline action arms deleted: `OpenCommandLine`, `CmdlineChar`, `CmdlineBackspace`, `RunCommand(Command)`.

**`Action::OpenPalette`** — unchanged for `Ctrl+P` (resets `palette = Default`, query empty → Pick).

**`Action::OpenPaletteDirect`** (new) — `overlay = Palette`, `palette = Default`, then `palette.query.push(':')`.

**`Action::PaletteChar(c)`** — already branches on `stage`. No change: pushing a char onto the buffer is correct whether empty, non-`:` query, or `:` prefix. Phase is derived at render/route/exec time.

**`Action::PaletteBackspace`** — already branches on `stage`. Popping the last char naturally transitions Direct → Pick when `:` is backspaced. No special handling.

**`Action::PaletteRun`** — gains a Direct branch:

```
match stage:
  Pick:
    if query.starts_with(':'):
        // Direct
        let cmd = parse_command(&query);
        close_overlay();
        exec_command(app, cmd).await      // Unknown(String) → exec_command's hint
    else:
        // existing Pick behavior — fuzzy list resolution
        if let Some(cmd) = palette_selected_command(shell):
            match arg_kind_for(&cmd):
                Some(kind) => stage = Arg { kind, input: "" }
                None       => close_overlay(); exec_command(app, cmd).await
  Arg { kind, input }:
    // unchanged
    if !input.trim().is_empty():
        close_overlay(); exec_command(app, kind.build(input.trim())).await
```

`exec_command` is **almost not modified**: one tiny edit. The `Command::RenameSession("")` arm currently seeds the deleted cmdline (`overlay = CommandLine`, `cmdline.buffer = "rename "`). With the cmdline gone, it instead seeds the **palette in Direct phase**:

```rust
Command::RenameSession(name) => {
    if name.is_empty() {
        app.shell.overlay = zoid_tui::Overlay::Palette;
        app.shell.palette = Default::default();
        app.shell.palette.query = ":rename ".into();   // Direct phase, pre-seeded
    } else {
        app.session.rename_session(app.session_id, name.clone()).await.ok();
        app.shell.session_name = name;
    }
    Ok(false)
}
```

This preserves the no-arg `:rename` power-user path: `:rename` (from cmdline or `:rename ` typed in Direct) → `parse_command` → `RenameSession("")` → re-opens the palette pre-seeded with `:rename ` so the user types the name and Enter applies it via `parse_command` again. The palette's own Rename row goes through Arg phase (different path, same `exec_command` non-empty arm). No other `exec_command` arm changes.

## Data flow

```
Ctrl+P ──▶ OpenPalette ──▶ overlay=Palette, stage=Pick, query=""
   │
   ├─ type non-':' ──▶ PaletteChar/Backspace ──▶ query edited, selected=0, list re-ranked (Pick)
   ├─ type ':' first ─▶ PaletteChar(':')      ──▶ query=":" (Direct — derived)
   │     │
   │     ├─ type ──▶ PaletteChar/Backspace ──▶ query edited, parse_command preview live
   │     └─ Enter ─▶ PaletteRun (Direct)
   │                   └─ parse_command(query) → close_overlay + exec_command
   │
   ├─ ↑/↓ (Pick) ──▶ PaletteMove ──▶ selected nav (wraps)
   └─ Enter (Pick, non-':') ─▶ PaletteRun
                  ├─ arg_kind_for = None ──▶ close_overlay + exec_command(cmd)
                  └─ arg_kind_for = Some(kind) ──▶ stage=Arg{kind, input=""}
                         │
                         ├─ type ──▶ PaletteChar/Backspace ──▶ input edited
                         ├─ Esc  ──▶ PaletteArgCancel ──▶ stage=Pick (query preserved)
                         └─ Enter ─▶ PaletteRun (Arg)
                                       ├─ input empty ──▶ no-op
                                       └─ else ──▶ close_overlay + exec_command(kind.build(input))

':' from Conversation/Rail focus ──▶ OpenPaletteDirect ──▶ overlay=Palette, query=":" (Direct)
```

## Error / edge handling

- **No matches** (Pick, non-`:`): `palette_selected_command` returns `None`; `Enter` is a no-op; overlay stays open. Body shows empty list.
- **Unknown `:` command** (Direct): `parse_command` returns `Unknown(String)`; `exec_command`'s `Unknown(_)` arm is a silent `Ok(false)` today. The Direct preview line in `render_palette` shows `unknown command` *before* Enter is pressed (so the user sees it live); after Enter, the overlay closes and `exec_command` no-ops. No change to `exec_command`.
- **Empty Direct buffer** (`query == ":"`): `parse_command(":")` returns `Unknown("")` (after trim). Preview shows the `type :mode, :q, :delegate …` hint; `Enter` runs `exec_command(Unknown(""))` — silent no-op, matching today's `:` line behavior on bare `:`.
- **Empty argument** on `Enter` in Arg: no-op, stays in Arg (prevents rename/delegate/import/update to empty). Existing rule, extended to all four `ArgKind`s.
- **Esc semantics:** Arg→Pick (not close) so a mis-selected parameterized command doesn't dump the user out; a second `Esc` (now in Pick) closes. Direct `Esc` closes (same as Pick).
- **Ctrl-modified / unknown keys:** `Noop`; overlay capture prevents leakage to global handlers while `overlay == Palette`.

## Testing strategy

**`palette.rs` unit tests:**
- `all_items` extended: assert the new curated order (Delegate / Import / Update / drawer toggles present in the right slots), mode-aware rows still correct, companion opposite-action still correct.
- `arg_kind_for`: `Delegate("")` / `ModeImport("")` / `ModeUpdate("")` → `Some(Delegate)` / `Some(ModeImport)` / `Some(ModeUpdate)`; `RenameSession("")` → `Some(Rename)`; zero-arg commands (`OpenDrawer`, `OpenConfig`, `Quit`, `NewSession`, `ResumeSessionPicker`, `SwitchMode`, `ReloadModes`, `CompanionEnable`/`Disable`) → `None`.
- `ArgKind` variants: `prompt()` and `build(input)` for all four.
- Existing `fuzzy_score` / `nav` / `selectable_matches` tests unchanged.

**`route.rs` tests:**
- Pick (non-`:`): unchanged.
- Direct (`:` prefix): seed `palette.query = ":mode Build"`; `Esc`→`CloseOverlay`, `Enter`→`PaletteRun`, `↑/↓`→`Noop`, `Char`/`Backspace`→edit.
- Arg: unchanged.
- `:` from Conversation focus → `OpenPaletteDirect` (replaces the old `OpenCommandLine` test).
- Delete `cmdline_enter_parses_command`; replace with Direct-phase `PaletteRun` test (seed `query = ":mode Build"`, assert bin resolves to `SwitchMode`).

**`state.rs` tests:** `close_overlay` resets `palette` to `Default` (query empty, stage Pick) — covers the Direct-seeded case for free. Delete cmdline-state reset test if any.

**Snapshot tests (`crates/zoid-tui/tests/shell_snapshot.rs`):**
- Regenerate `palette_overlay_frame` (new curated order).
- `palette_arg_stage_frame` still valid (one of four ArgKinds now; keep Rename as the canonical example).
- **Add** `palette_direct_phase_frame`: `query = ":mode Build"` → preview line `→ Switch to Build`.
- **Remove** cmdline snapshot + its `.snap`.

**`main.rs` integration tests:** the existing `:mode import` / `:mode update` / `:delegate` integration tests move from the cmdline path to the Direct-palette path — same `Command` output, different action (`PaletteRun` not `RunCommand`). The wizard spawn arms in `exec_command` are untouched.

**Full workspace:** `cargo test --workspace` + `cargo clippy --workspace --all-targets` + `cargo fmt`.

## What gets deleted

- `Overlay::CommandLine` variant (`state.rs`).
- `CmdlineState` struct + `cmdline` field on `ShellState` (`state.rs`).
- `Action::OpenCommandLine`, `Action::CmdlineChar`, `Action::CmdlineBackspace`, `Action::RunCommand(Command)` (`route.rs`).
- `route_cmdline_key` (`route.rs`).
- `render_cmdline` + cmdline rect in `ShellLayout` (`layout.rs`, `render.rs`).
- Cmdline snapshot test + `.snap` (`tests/shell_snapshot.rs`).
- Cmdline action arms in `main.rs`.

**Stays:** `parse_command` (Direct mode calls it live), `Command` enum (shared vocabulary), `exec_command` (shared sink).

## Files touched

- `crates/zoid-tui/src/state.rs` — delete `CmdlineState`, `Overlay::CommandLine`, `cmdline` field.
- `crates/zoid-tui/src/palette.rs` — three new `ArgKind` variants + `arg_kind_for` arms; six new rows in `all_items` (three parameterized, three drawer toggles); `Phase` enum for the pure resolver; updated unit tests.
- `crates/zoid-tui/src/route.rs` — delete `route_cmdline_key` + four cmdline `Action` variants; `route_palette_key` gains Direct-phase branch; `:` from Conversation/Rail → `OpenPaletteDirect`; new `Action::OpenPaletteDirect`; updated route tests.
- `crates/zoid-tui/src/layout.rs` — delete cmdline rect.
- `crates/zoid-tui/src/render.rs` — delete `render_cmdline`; `render_palette` gains Direct-phase preview line; updated snapshots.
- `crates/zoid-tui/tests/shell_snapshot.rs` (+ `.snap` files) — regenerate palette, add Direct snapshot, remove cmdline snapshot.
- `crates/zoid/src/main.rs` — delete cmdline action arms; add `OpenPaletteDirect` arm; extend `PaletteRun` with the Direct branch; update integration tests.
- `crates/zoid-tui/src/command.rs` — **untouched**.
- `docs/superpowers/specs/2026-07-04-command-palette-redesign-design.md` — add a supersession pointer at the top.
- `docs/ux/palette.html` — update caption/footnote to reflect the merged adaptive UI; remove the cmdline footer note.

## Non-goals / Future

- **MRU / usage memory** — still rejected (curated order).
- **Free-text arg editing chords** — Arg-phase input stays the simple `push`/`pop` loop; no cursor-left/right, no word-jump. Power users who want real editing type `:delegate …` via Direct mode, which goes through `parse_command` whole-buffer (no in-arg editing needed).
- **Restoring placeholder rows** (Fork, Undo, Pin file, Evict, Run recipe) — re-add to `all_items` when those features ship, same as today's spec.
- **Extracting a generic picker** shared with Objects/Verbs/Sessions — still deferred.
- **`:`-prefix tab completion in Direct mode** — out of scope; Direct is a live-preview command line, not a completer. A future iteration could offer `Tab` to complete the recognized command word, but YAGNI for this pass.