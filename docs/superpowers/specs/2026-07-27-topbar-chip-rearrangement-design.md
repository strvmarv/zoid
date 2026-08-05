# Top Bar Chip Rearrangement — Design

## Problem

Two UI issues from `docs/TODO.md`:

1. **SELECT chip crowds the mode indicator** (TODO §"UI: move SELECT chip away
   from mode"). The SELECT pill renders in the bottom status bar, immediately
   right of the mode pill, making the left segment read as one wide block.

2. **No YOLO indicator** (TODO §"UI: show YOLO chip/message when enabled"). When
   YOLO mode (auto-approve all tool calls) is active, there is no visible
   indicator — the user can't tell approvals are bypassed.

## Current Layout

### Top bar (`title_line`, render.rs:296)

```
[VERSION flush-left] [pad] [zoid centered] [pad] [palette hint flush-right]
```

Three zones: version flush-left, `zoid` wordmark dead-center, palette hint
(`Esc interrupt · : command · ^P palette`) flush-right. The version overlays
the left padding; the wordmark-centering math is `(w - 4) / 2`.

### Bottom status bar (`render_status`, render.rs:371)

```
[MODE pill] [1-cell gap] [SELECT pill] [· hint] ... [compact] [working] [tool] ... [zoom]
```

SELECT sits right of the mode pill — purple-filled (`BRANCH` on `SELECT_BG`)
when ON, dim/recessive (`DIM`, no fill) when OFF.

### `yolo` on `App`, not on `ShellState`

`App.yolo` (main.rs:2099) is resolved from `config.approval.yolo || cli --yolo`.
It is never mirrored to `ShellState`, so the pure renderer can't see it. No
visible indicator exists.

## Proposed Layout

### Top bar (new)

```
[SELECT pill] [1-cell gap] [YOLO pill?] [pad] [zoid v0.1.2 centered] [pad] [palette hint flush-right]
```

- **SELECT pill** moves from the bottom bar to the top bar, flush-left. Same
  visual as today: ` SELECT ` with `BRANCH` on `SELECT_BG` when ON, `DIM` with
  no fill when OFF.
- **YOLO pill** sits next to SELECT with a 1-cell gap (mirrors the old MODE–
  SELECT adjacency pattern). Text: ` ⚠ YOLO `. Colors: `WARN` (amber) on
  `BUILD_BG` (dark brown) — the "danger" sibling of the blue/purple pills.
  **Only rendered when `yolo == true`** — hidden entirely (no gap, no space
  reserved) when YOLO is off.
- **Version** moves from flush-left to center, appended to the wordmark:
  `zoid v0.1.2` centered as a single unit. The combined width drives the
  centering math: `pad = (w - combined_w) / 2`.
- **Palette hint** stays flush-right, unchanged.

### Bottom status bar (updated)

```
[MODE pill] [· hint] ... [compact] [working] [tool] ... [zoom]
```

SELECT pill removed entirely. Mode pill and status hint remain. The 1-cell
gap after the mode pill is replaced by the `· hint` separator's leading space.

### No narrow-terminal fallback

The TUI enforces a hard 160×40 minimum (`layout::MIN_WIDTH` / `MIN_HEIGHT`).
Below that, the "too small" overlay renders instead of the shell. At 160
columns there is always room for SELECT (7 chars) + gap (1) + YOLO (8 chars) +
padding + `zoid v0.1.2` (~11 chars) + padding + palette hint (~33 chars). The
old version-drop logic (`pad < ver_w + 1`) is removed — no degradation path
needed.

## Components

### `ShellState` (crates/zoid-tui/src/state.rs)

New field:

```rust
/// Whether YOLO mode (auto-approve all tool calls) is active. Mirrors
/// `App.yolo` so the pure renderer can show a warning chip. Synced by the
/// bin each frame.
pub yolo: bool,
```

Default `false` in `ShellState::new()`.

### `title_line` (crates/zoid-tui/src/render.rs)

Signature changes from `title_line(w: usize)` to
`title_line(w: usize, select_mode: bool, yolo: bool)`.

Layout construction:

1. **Left zone** — SELECT pill (always) + 1-cell gap + YOLO pill (only if
   `yolo`). SELECT styling is the same conditional as today (`select_mode` →
   filled purple, else `DIM`). YOLO is `WARN` on `BUILD_BG`.
2. **Center zone** — `zoid v0.1.2` (wordmark + space + VERSION). The combined
   block is centered on the full width: `center_start = (w - combined_w) / 2`.
   The left chips fill from column 0; they won't overlap the centered block at
   160 cols (left zone ≤ ~16 chars, centered block starts at ~74).
3. **Right zone** — palette hint, flush-right. Unchanged.

The old version-drop logic is removed.

### `render_title` (crates/zoid-tui/src/render.rs)

`_state` parameter becomes `state` (now used). Calls
`title_line(area.width as usize, state.select_mode, state.yolo)`.

### `render_status` (crates/zoid-tui/src/render.rs)

Remove the SELECT pill spans (current lines 384–397):
- The 1-cell gap span (`Span::raw(" ")`)
- The SELECT pill span (`Span::styled(" SELECT ", select_style)`)

The mode pill remains as the first element in `left`. The `· hint` follows
directly (it has its own leading ` · ` prefix, so no gap needed).

### Bin sync (crates/zoid/src/main.rs)

Add `app.shell.yolo = app.yolo;` to the per-frame `ShellState` sync, alongside
the existing `app.shell.busy = app.streaming;` (main.rs:3153).

## Testing

### Unit tests (`render.rs` test module)

1. **`title_line` shows SELECT pill flush-left** — render at width 160,
   `select_mode: true`. Assert the leftmost non-space content is `SELECT`.
2. **`title_line` SELECT off is recessive** — `select_mode: false`. Assert
   SELECT text present with `DIM` fg, no `SELECT_BG` fill.
3. **`title_line` YOLO shown when enabled** — `yolo: true`. Assert `YOLO` text
   present with `WARN` fg, `BUILD_BG` bg. Assert it appears after SELECT with
   a 1-cell gap.
4. **`title_line` YOLO hidden when disabled** — `yolo: false`. Assert no `YOLO`
   text anywhere in the line.
5. **`title_line` version centered with wordmark** — Assert `zoid v…` is
   present and the `zoid` start column is `(w - combined_w) / 2`.
6. **`render_status` has no SELECT pill** — render status bar, assert no
   `SELECT` text appears.

### Existing test updates

- `select_pill_on_is_filled_purple` / `select_pill_off_is_recessive_no_fill` —
  these test via `status_buffer` which calls `render_status`. Since SELECT moves
  out of `render_status`, these tests move to test `title_line` / `render_title`
  instead.
- `title_shows_version_flush_left_and_keeps_wordmark_centered` — version is no
  longer flush-left. Update to assert version is adjacent to wordmark and the
  combined block is centered.
- `title_drops_version_when_left_pad_too_narrow` — remove (no drop logic).

### Snapshot tests (`crates/zoid-tui/tests/`)

Regenerate via `cargo insta test --accept -p zoid-tui`. The SELECT pill moves
from the bottom bar to the top bar in every snapshot; YOLO is absent (snapshots
run with `yolo: false`).

## Files Touched

| File | Change |
|------|--------|
| `crates/zoid-tui/src/state.rs` | Add `yolo: bool` field + default |
| `crates/zoid-tui/src/render.rs` | Rewrite `title_line`, update `render_title`, remove SELECT from `render_status`, update/add tests |
| `crates/zoid/src/main.rs` | Add `app.shell.yolo = app.yolo` sync |
| `crates/zoid-tui/tests/` (snapshots) | Regenerate |

## Out of Scope

- Changing SELECT's toggle behavior or keybinding (`Alt+M` / `:select`).
- Changing YOLO's activation (CLI `--yolo` / config `approval.yolo`).
- Adding a runtime toggle for YOLO (currently CLI/config only).