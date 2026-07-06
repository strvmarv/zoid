# Direct-Phase Autocomplete + Grouped Vocabulary — Design Spec

**Date:** 2026-07-06
**Status:** Approved for planning
**Scope:** Single implementation plan. `zoid-tui`-local logic + `zoid` bin match arms. Supersedes the "Direct is a single preview line, list hidden" lock in `2026-07-06-command-surfaces-consolidation-design.md` §2 (Direct phase).
**Depends on:** `2026-07-06-command-surfaces-consolidation-design.md` (the merged adaptive palette — this spec extends its Direct phase).

## Goal

Turn Direct phase (`:`-prefix in the palette) from a single live-preview line into a **three-stage autocomplete**: the filtered list of `:`-commands appears below the preview line, and selecting a row advances through namespace → subcommand → arg. The `:`-command vocabulary is **regrouped** into a consistent namespace tree (`:session new`, `:drawer repo`, `:companion on`) — the autocomplete is the discovery mechanism for the new grammar. The old flat shortcuts (`:new`, `:rename`, `:repo`, `:session`, `:context`, `:companion`) are removed (no aliases).

## Background: what exists today

After the command-surfaces-consolidation merge, the palette has three phases:
- **Pick** — fuzzy ranked list of curated actions (`all_items`).
- **Direct** (`:`-prefix query) — a single live-preview line showing `parse_command(query)` resolved; list hidden; arrows `Noop`; Enter runs `parse_command(query)` → `exec_command`.
- **Arg** — inline argument entry for parameterized palette rows.

The `:`-command vocabulary (`parse_command` in `command.rs`) is flat: `:mode`, `:q`, `:repo`, `:session`, `:context`, `:new`, `:rename`, `:delegate`, `:config`, `:companion`, `:companion off`, `:mode import`, `:mode update`, `:mode reload`. Only `:mode` is grouped (it already has subcommands).

## Decisions (locked)

1. **Three-stage autocomplete in Direct phase.** Stage 1: namespaces + flat commands. Stage 2: subcommands. Stage 3: arg completions. Stages are derived from the buffer (no stored stage).
2. **Grouped vocabulary, no aliases.** `:new`→`:session new`, `:rename`→`:session rename`, `:repo`→`:drawer repo`, `:session`→`:drawer session`, `:context`→`:drawer context`, `:companion`→`:companion on`. New: `:session resume`. `:mode`, `:delegate`, `:config`, `:q`/`:quit` stay as-is (already single-purpose or already grouped).
3. **Selecting a row fills the buffer and advances** (namespaces + parameterized subcommands) **or runs immediately** (zero-arg leaves + Stage-3 arg rows). The list stays visible after fill (option C from brainstorming: "fill buffer, keep list").
4. **Preview line suppression (option B from brainstorming).** The preview line shows only when `parse_command(query)` resolves to a real `Command` (not `Unknown`) OR when the list is empty. When the list is visible and Enter would fill-advance (incomplete namespace → `Unknown`), the preview is suppressed — the list is the teacher.
5. **Pick-phase `all_items` rows are not modified.** The grouped grammar lives in `parse_command` + `direct_items`; the Pick rows keep their human-readable labels.
6. **`exec_command` touched in exactly one arm:** `RenameSession("")` reseeds `":session rename "` instead of `":rename "`. All other arms unchanged.
7. **`Command` enum unchanged.** Same variants, same sink. Only the text→Command mapping (`parse_command`) and the Direct-phase autocomplete (`direct_items`) change.
8. **No new Action variants.** `PaletteMove`/`PaletteRun`/`PaletteChar`/`PaletteBackspace`/`PaletteArgCancel` handle all cases. Direct gains arrow navigation when the list is non-empty (today arrows are `Noop` in Direct).

## Architecture

### Revised `parse_command` grammar (`crates/zoid-tui/src/command.rs`)

| New syntax | `Command` variant |
|---|---|
| `:session new` | `NewSession` |
| `:session rename` / `:session rename <name>` | `RenameSession("")` / `RenameSession(name)` |
| `:session resume` | `ResumeSessionPicker` |
| `:drawer repo` | `OpenDrawer(DrawerId::Repo)` |
| `:drawer session` | `OpenDrawer(DrawerId::Session)` |
| `:drawer context` | `OpenDrawer(DrawerId::Context)` |
| `:mode` / `:mode <name>` | `SwitchMode("")` / `SwitchMode(name)` |
| `:mode reload` | `ReloadModes` |
| `:mode import` / `:mode import <url>` | `ModeImport("")` / `ModeImport(url)` |
| `:mode update` / `:mode update <name>` | `ModeUpdate("")` / `ModeUpdate(name)` |
| `:companion on` | `CompanionEnable` |
| `:companion off` | `CompanionDisable` |
| `:delegate` / `:delegate <task>` | `Delegate("")` / `Delegate(task)` |
| `:config` | `OpenConfig` |
| `:q` / `:quit` | `Quit` |
| bare `:session` / `:drawer` / `:companion` | `Unknown(input)` (incomplete namespace) |
| anything else | `Unknown(input)` |

**Removed (no aliases):** `:new`, `:rename`, `:repo`, `:session`, `:context`, `:companion` (bare). Nesting depth: at most three tokens (`:session rename <name>`). `parse_command` prefix-matches the longest subcommand first, then the arg — same pattern as today's `:mode import <url>`.

### `direct_items` and stage derivation (`crates/zoid-tui/src/palette.rs`)

A new pure function sits alongside `all_items`:

```rust
pub fn direct_items(state: &ShellState) -> Vec<PaletteItem>
```

It inspects `state.palette.query` (including the `:` prefix) and returns the appropriate stage's list:

- **Stage 1 — namespaces + flat commands.** Buffer is `:` with no complete namespace (no trailing space after a recognized namespace word). Returns: `session`, `drawer`, `mode`, `companion`, `delegate`, `config`, `q`, `quit`. Each row's `label` is the word; `command` is the bare form's `Command` if it has one (`Quit`, `Delegate("")`, `SwitchMode("")`) or `Unknown` for pure namespaces (`session`, `drawer`, `companion` — selecting these fills the buffer, doesn't run).
- **Stage 2 — subcommands.** Buffer is `:ns ` (recognized namespace + trailing space). Returns:
  - `:session ` → `new`, `rename`, `resume`
  - `:drawer ` → `repo`, `session`, `context`
  - `:mode ` → `reload`, `import`, `update`, plus one row per mode name (the `:mode <name>` direct-switch path)
  - `:companion ` → `on`, `off`
  
  Zero-arg subcommands run immediately on select. Parameterized ones (`rename`, `import`, `update`) are bare sentinels — selecting fills the buffer to `:ns sub ` and advances to Stage 3.
- **Stage 3 — arg completions.** Buffer is `:ns sub ` (parameterized subcommand + trailing space). Returns:
  - `:session rename ` → one row per `state.sessions` (label = name, `command` = `RenameSession(name)`). User can also type a new name freely.
  - `:mode import ` → empty list (free-text URL).
  - `:mode update ` → one row per `state.mode_names` (the pure layer shows all; `exec_command`'s `ModeUpdate` arm rejects non-imported with its existing "no provenance" message).
  - `:delegate ` → empty list (free-text task).

Stage is **derived from the buffer**, not stored. `direct_items` is pure: `&ShellState` in, `Vec<PaletteItem>` out. `fuzzy_score` filters within each stage; `nav` wraps selection.

### `direct_filter` — the filter text for the current stage

A pure helper extracts the filter text (the last incomplete token after `:`):

```rust
pub fn direct_filter(query: &str) -> &str
```

- `:mo` → `mo`
- `:session ` → `` (empty — shows all subcommands)
- `:session re` → `re`
- `:session rename ` → `` (empty — shows all session names)
- `:session rename fix` → `fix`

The filter is everything after the last space in the buffer (minus the `:` prefix).

### Selection and buffer-fill

**Selection state:** `palette.selected` (shared with Pick), reset to 0 on every `PaletteChar`/`PaletteBackspace`. `nav` wraps.

**Selecting a Stage-1 namespace** (`session`, `drawer`, `mode`, `companion`): bin sets `query = format!(":{label} ")`. List re-derives to Stage 2. Overlay stays open.

**Selecting a Stage-1 flat command** (`q`, `quit`, `delegate`, `config`): `q`/`quit`/`config` run immediately (`parse_command` → `exec_command` → `close_overlay`). `delegate` fills to `:delegate ` → Stage 3 free-text.

**Selecting a Stage-2 zero-arg subcommand** (`new`, `resume`, `reload`, `repo`, `session`, `context`, `on`, `off`, or a mode-name row): runs immediately.

**Selecting a Stage-2 parameterized subcommand** (`rename`, `import`, `update`): bin sets `query = format!(":{ns} {label} ")`. List re-derives to Stage 3. Overlay stays open.

**Selecting a Stage-3 arg row** (session name for `:session rename `, mode name for `:mode update `): bin sets `query = format!(":{ns} {sub} {label}")` → `parse_command(query)` → `exec_command` → `close_overlay`.

**Enter with no list (Stage 3 free-text, or no fuzzy match):** falls through to `parse_command(query)` → `exec_command` as today.

**Enter with a list but the user typed past it** (e.g. `:session rename fix login` — a new name not in `state.sessions`): the list filters by `fuzzy_score`; if no match, list is empty, Enter falls through to `parse_command` on the whole buffer.

**Backspace:** pops a char. Backspacing the trailing space in `:session rename ` returns to `:session rename` (Stage 2). Backspacing `rename` returns to `:session ` (Stage 2). Backspacing the space after `session` returns to `:session` (Stage 1). Natural reverse of fill-on-select.

### `DirectAction` resolver (`crates/zoid-tui/src/palette.rs`)

A pure resolver encodes the fill-vs-run decision so the bin doesn't reason about stages:

```rust
pub enum DirectAction {
    Fill(String),   // set query to this, stay open (advance to next stage)
    Run(Command),   // close + exec_command this (zero-arg leaf or complete arg)
    Nothing,        // no row selected / empty list — fall through to parse_command
}

pub fn direct_selected_action(state: &ShellState) -> DirectAction
```

The resolver calls `direct_items(state)`, applies `selectable_matches` + `nav` to get the highlighted row, then decides Fill/Run/Nothing based on the row's `command` (namespaces + parameterized subcommands → Fill; zero-arg leaves + Stage-3 args → Run; empty list → Nothing).

### Routing (`crates/zoid-tui/src/route.rs`)

`route_palette_key` arrow guard gains a list-nonempty check for Direct. The routing decision depends on `stage`, `in_direct`, and `direct_items(state).is_empty()`:

| Key | Pick (non-`:`) | Direct, list empty | Direct, list non-empty | Arg |
|---|---|---|---|---|
| `Esc` | `CloseOverlay` | `CloseOverlay` | `CloseOverlay` | `PaletteArgCancel` |
| `Enter` | `PaletteRun` | `PaletteRun` | `PaletteRun` | `PaletteRun` |
| `↑`/`↓` | `PaletteMove(∓1)` | `Noop` | `PaletteMove(∓1)` | `Noop` |
| `Backspace` | `PaletteBackspace` | `PaletteBackspace` | `PaletteBackspace` | `PaletteBackspace` |
| `Char(c)` | `PaletteChar(c)` | `PaletteChar(c)` | `PaletteChar(c)` | `PaletteChar(c)` |
| other | `Noop` | `Noop` | `Noop` | `Noop` |

No new Action variants. The `in_direct` arrow guard changes from `!in_arg && !in_direct` (today) to `!in_arg && (!in_direct || !direct_items(state).is_empty())`.

### Render (`crates/zoid-tui/src/render.rs`)

`Phase::Direct` branch renders up to two body elements:
1. **Preview line** — the live `parse_command(query)` result. **Suppressed when the list is non-empty AND `parse_command` returns `Unknown`** (option B). Shows when `parse_command` resolves to a real `Command` OR when the list is empty.
2. **Filtered list** — below the preview line, one row per `selectable_matches(direct_items(state), direct_filter(query))`, highlighted row painted `SEL_BG`. Same `palette_row_line` rendering as Pick. Only shown when non-empty. Scroll-follow reuses the Pick logic.

Footer hint: `↑↓ move · ⏎ select · esc close · type to filter` when list non-empty; `⏎ run · esc close` when empty.

### Bin handlers (`crates/zoid/src/main.rs`)

`Action::PaletteRun` Pick/Direct branch reworked:

```
match stage:
  Pick:
    if query.starts_with(':'):
        match direct_selected_action(&shell):
            Fill(text) => shell.palette.query = text; shell.palette.selected = 0;  // stay open
            Run(cmd)  => close_overlay(); return exec_command(app, cmd).await;
            Nothing   => let cmd = parse_command(&query); close_overlay(); return exec_command(app, cmd).await;
    else:
        // existing Pick behavior — fuzzy list resolution
        ...
  Arg { kind, input }:
    // unchanged
    ...
```

`exec_command`'s `RenameSession("")` arm reseeds `":session rename "` instead of `":rename "` (one-line edit). All other `exec_command` arms unchanged.

### Pick-phase `all_items` — not modified

The Pick rows keep their human-readable labels ("New session", "Toggle repo drawer", etc.) and existing `Command` variants. The grouped grammar lives in `parse_command` + `direct_items`. The only cross-impact: `all_items`'s "Rename session…" row carries `RenameSession(String::new())` → `exec_command` reseeds with `":session rename "` (the §exec_command edit covers both paths).

## Data flow

```
Ctrl+P ──▶ OpenPalette ──▶ overlay=Palette, stage=Pick, query=""  (Pick phase)
':' from Conversation ──▶ OpenPaletteDirect ──▶ query=":"  (Direct, Stage 1)

Direct, Stage 1 (query = ":"):
  list = direct_items(state)  →  [session, drawer, mode, companion, delegate, config, q, quit]
  filter = direct_filter(":")  →  ""
  preview suppressed (Unknown + list non-empty)
  ↑↓ navigates; ⏎ on "session" → Fill(":session ") → Stage 2

Direct, Stage 2 (query = ":session "):
  list = direct_items(state)  →  [new, rename, resume]
  filter = direct_filter(":session ")  →  ""
  preview suppressed (Unknown + list non-empty)
  ⏎ on "new" → Run(NewSession) → exec_command → close_overlay
  ⏎ on "rename" → Fill(":session rename ") → Stage 3

Direct, Stage 3 (query = ":session rename "):
  list = direct_items(state)  →  [fix 500, add auth, …]  (from state.sessions)
  filter = direct_filter(":session rename ")  →  ""
  preview shows (parse_command(":session rename ") → RenameSession("") → real Command)
  ⏎ on "fix 500" → Run(RenameSession("fix 500")) → exec_command → close_overlay
  type "new name" → list filters; ⏎ falls through to parse_command(":session rename new name") → Run

Direct, free-text Stage 3 (query = ":delegate "):
  list = direct_items(state)  →  []  (empty)
  preview shows (parse_command(":delegate ") → Delegate("") → real Command, but list empty so preview shows)
  type "add a test" → ⏎ → Nothing → parse_command(":delegate add a test") → Run(Delegate("add a test"))
```

## Error / edge handling

- **Bare `:`** → Stage 1, full list, top row auto-selected. Preview suppressed. `Enter` on `Nothing` (no row? can't happen — list is non-empty, `selected` is 0). `Enter` runs `direct_selected_action` → top row is `session` (a namespace) → `Fill(":session ")`. So bare `:` + Enter advances to `:session `. Matches the "list is the teacher" model.
- **Unknown command word** (`:wat`) → Stage 1, `direct_items` filters to empty (no fuzzy match). Preview shows `→ unknown command` (list empty → preview not suppressed). Enter → `Nothing` → `parse_command(":wat")` → `Unknown("wat")` → `exec_command` no-ops (silent `Ok(false)`).
- **Incomplete namespace** (`:session`, `:drawer`, `:companion`) → `parse_command` returns `Unknown`. Direct list: Stage 1 filters to the namespace row (e.g. `:session` → `session` row matches). Preview suppressed. Enter → `Fill(":session ")` → Stage 2. (Or if the user typed `:ses`, the list filters to `session`; Enter fills to `:session `.)
- **Empty Stage 3 arg on Enter** (`:session rename ` with no session selected and no typed name) → list is non-empty (session names), `selected` is 0 → `direct_selected_action` returns `Run(RenameSession(first_session_name))`. To rename to a *new* name, the user types it (list filters to empty if no match) → `Nothing` → `parse_command` → `RenameSession("typed name")`. If `state.sessions` is empty, the list is empty → `Nothing` → `parse_command(":session rename ")` → `RenameSession("")` → `exec_command` reseeds `":session rename "` (re-prompt). Matches today's `:rename` bare behavior.
- **Backspace across stage boundary** — natural reverse: `:session rename ` → backspace → `:session rename` (Stage 2); → backspace → `:session ` (Stage 2 subcommands); → backspace → `:session` (Stage 1); → backspace → `:` (Stage 1 full list).
- **`parse_command` trimming** — `:session rename ` (trailing space) trims to `session rename` → matches the bare subcommand → `RenameSession("")`. `:session rename fix login` → `RenameSession("fix login")`. Same trimming behavior as today.

## Testing strategy

**`command.rs` unit tests:**
- Delete: `parses_drawer_toggle_commands`, `parses_session_commands`, `parses_companion_commands` (flat shortcuts gone).
- Rewrite: `parses_drawer_subcommands` (`:drawer repo` → `OpenDrawer(Repo)` etc.); `parses_session_subcommands` (`:session new` → `NewSession`, `:session rename` → `RenameSession("")`, `:session rename fix login` → `RenameSession("fix login")`, `:session resume` → `ResumeSessionPicker`); `parses_companion_subcommands` (`:companion on`/`off`).
- Keep: `parses_known_commands_with_or_without_colon`, `bare_mode_is_empty_switch_not_unknown`, `unknown_is_captured_verbatim`, `parses_delegate_with_task`, `parses_config_command`, `mode_import_parses`, `mode_update_parses`, `bare_mode_import_is_empty_arg`, `mode_reload_still_wins_over_import`.
- New: `bare_namespace_is_unknown` (`:session`/`:drawer`/`:companion` → `Unknown`); `session_subcommand_with_arg`; `drawer_requires_subcommand`; `companion_requires_on_or_off`.

**`palette.rs` unit tests:**
- `direct_items` stage tests: Stage 1 (bare `:`, partial `:se`), Stage 2 (`:session `, `:drawer `, `:mode `, `:companion `), Stage 3 (`:session rename `, `:mode import `, `:delegate `, `:mode update `).
- `direct_filter` tests: partial, after-space, partial-sub, after-sub-space, typing-arg.
- `direct_selected_action` tests: select-namespace-fills, select-zero-arg-runs, select-parameterized-fills, select-arg-runs, no-match-nothing.
- Existing `all_items`/`fuzzy_score`/`nav`/`selectable_matches`/`ArgKind`/`arg_kind_for`/`Phase`/`resolve_phase` tests unchanged.

**`route.rs` tests:**
- Update `palette_direct_phase_routing`: arrows now `PaletteMove` when list non-empty (seed `:session ` so `direct_items` is non-empty); arrows `Noop` when list empty (seed `:wat`).
- Keep `palette_pick_phase_routing`, `palette_arg_phase_routing`.

**Snapshot tests (`crates/zoid-tui/tests/shell_snapshot.rs`):**
- Add `palette_direct_stage1_frame` (`query = ":"`).
- Add `palette_direct_stage2_frame` (`query = ":session "`).
- Add `palette_direct_stage3_frame` (`query = ":session rename "`, `sessions = ["fix 500", "add auth"]`).
- Regenerate `palette_direct_phase_frame` (`query = ":mode Build"` — now shows list + preview).

**Full workspace:** `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all`.

## Files touched

- `crates/zoid-tui/src/command.rs` — rewrite `parse_command` for grouped grammar; update tests.
- `crates/zoid-tui/src/palette.rs` — add `direct_items`, `direct_filter`, `DirectAction`, `direct_selected_action`; add unit tests.
- `crates/zoid-tui/src/route.rs` — arrow guard gains `direct_items` list-nonempty check; update Direct routing test.
- `crates/zoid-tui/src/render.rs` — `Phase::Direct` branch renders preview (suppressed per option B) + filtered list; add 3 snapshots + regenerate 1.
- `crates/zoid/src/main.rs` — `PaletteRun` Direct branch reworked (`direct_selected_action` → Fill/Run/Nothing); `RenameSession("")` reseed → `":session rename "`.
- `crates/zoid-tui/tests/shell_snapshot.rs` (+ `.snap` files) — 3 new + 1 regenerated.
- `docs/ux/palette.html` — update mockup to grouped vocabulary + three-stage flow.
- `docs/superpowers/specs/2026-07-06-command-surfaces-consolidation-design.md` — add supersession pointer.

## Non-goals / Future

- **Aliases for old flat shortcuts** — explicitly rejected. `:new`, `:rename`, `:repo`, `:session`, `:context`, `:companion` are gone.
- **Mode-name completions for `:mode update` filtered by provenance** — pure layer shows all `mode_names`; `exec_command`'s `ModeUpdate` arm rejects non-imported. Future: bin marks imported modes on `ShellState` so the pure layer can filter.
- **`:config` subcommands** (`:config provider`, `:config model`) — stays flat (opens settings overlay). Separate spec.
- **Recency / MRU ranking within Direct stages** — curated order only.
- **Tab completion** (fill without advancing stages) — out of scope; Enter fills + advances.
- **Arg completions for `:delegate`** (recent paths, task templates) — empty list, free-text. Future feature.