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
  no fill when OFF. Always visible — SELECT is a persistent toggle, so it
  stays recessive when off (unlike YOLO).
- **YOLO pill** sits next to SELECT with a 1-cell gap (mirrors the old MODE–
  SELECT adjacency pattern). Text: ` ⚠ YOLO `. Colors: `WARN` (amber) on
  `WARN_DIM` (dark amber) — the matched light/dark pair for the danger
  hierarchy, mirroring `CHAT_ACCENT`/`CHAT_BG` and `BRANCH`/`SELECT_BG`.
  `WARN_DIM` already exists (tokens.rs:89) for exactly this "amber on dark
  amber" relationship. **Only rendered when `yolo == true`** — hidden entirely
  (no gap, no space reserved) when YOLO is off. YOLO is a transient danger flag,
  not a persistent toggle, so it's hidden when inactive (asymmetric with
  SELECT by design).
- **Version** moves from flush-left to center, appended to the wordmark:
  `zoid v0.1.2` centered as a single unit. The combined block is centered on
  the full width `w` (same as today — the wordmark is centered on `w`, not on
  `w - left_zone_w`): `center_start = (w - combined_w) / 2`.
- **Palette hint** stays flush-right, unchanged.

### Bottom status bar (updated)

```
[MODE pill] [· hint] ... [compact] [working] [tool] ... [zoom]
```

SELECT pill removed entirely. Mode pill and status hint remain. The 1-cell
gap after the mode pill is replaced by the `· hint` separator's leading space
(when a hint is present; no gap needed when absent). Note: removing SELECT
shrinks `left_w` by ~8 chars, which shifts `compact_pad` (render.rs:496)
leftward — the compact indicator's absolute column changes. This is
cosmetic and acceptable (the compact indicator's "⅓ anchor" is relative to
the center, not the left edge).

### No narrow-terminal fallback — but keep a guard

The TUI enforces a hard 160×40 minimum (`layout::MIN_WIDTH` / `MIN_HEIGHT`).
Below that, the "too small" overlay renders instead of the shell. At 160
columns there is always room for the left zone (SELECT + gap + YOLO ≤ ~16
chars) + the centered block + palette hint.

The old version-drop logic (`pad < ver_w + 1`) is removed, but a **saturating
guard** replaces it: if `left_zone_w >= center_start` (left chips would
overlap the centered block), drop the centered block to left-aligned at
`left_zone_w + 1` instead of centering. This guards against future chip
additions or version string growth. The invariant is:

```
left_zone_w < center_start  (normal case: centered block is clear)
left_zone_w >= center_start (guard fires: left-align the wordmark+version)
```

## Components

### `ShellState` (crates/zoid-tui/src/state.rs)

New field, placed adjacent to `select_mode` (thematic grouping with other
toggle fields):

```rust
/// Whether YOLO mode (auto-approve all tool calls) is active. Mirrors
/// `App.yolo` so the pure renderer can show a warning chip. Synced by the
/// bin each frame.
pub yolo: bool,
```

Default `false` in `ShellState::new()`. `Default` delegates to `new()`
(state.rs:906), so `ShellState::default()` inherits the field for free — no
manual `Default` body needed.

### `title_line` (crates/zoid-tui/src/render.rs)

Signature changes from `title_line(w: usize)` to
`title_line(w: usize, select_mode: bool, yolo: bool)`.

Layout construction:

1. **Left zone** — SELECT pill (always) + 1-cell gap + YOLO pill (only if
   `yolo`). SELECT styling is the same conditional as today (`select_mode` →
   filled purple, else `DIM`). YOLO is `WARN` on `WARN_DIM`.
2. **Center zone** — `zoid v0.1.2` (wordmark + space + VERSION). The combined
   block is centered on the full width: `center_start = (w - combined_w) / 2`.
   The left chips fill from column 0.
3. **Guard** — if `left_zone_w >= center_start`, left-align the centered
   block at `left_zone_w + 1` instead. (Won't fire at 160 cols, but protects
   against future growth.)
4. **Right zone** — palette hint, flush-right. Unchanged.

### `render_title` (crates/zoid-tui/src/render.rs)

`_state` parameter becomes `state` (now used). Calls
`title_line(area.width as usize, state.select_mode, state.yolo)`. No other
callers exist (only render.rs:207).

### `render_status` (crates/zoid-tui/src/render.rs)

Remove the SELECT pill spans (current lines 384–397):
- The 1-cell gap span (`Span::raw(" ")`)
- The SELECT pill span (`Span::styled(" SELECT ", select_style)`)

The mode pill remains as the first element in `left`. The `· hint` follows
directly (it has its own leading ` · ` prefix, so no gap needed).

### Bin sync (crates/zoid/src/main.rs)

Add `app.shell.yolo = app.yolo;` to the per-frame `ShellState` sync, alongside
the existing `app.shell.busy = app.streaming;` (main.rs:3153).

`App.yolo` is resolved at startup (main.rs:2667: `config.approval.yolo ||
cli_yolo`) and is a latched value — it does not change mid-session (config
reload re-resolves `config` but `app.yolo` is only set during `App`
construction). The per-frame sync is therefore a stable mirror, not a
dynamic computation. If config-reload-driven YOLO toggling is added later,
the sync line will pick it up automatically since it reads `app.yolo` each
frame.

## Testing

### Unit tests (`render.rs` test module)

1. **`title_line` SELECT ON is flush-left + filled** — `title_line(160, true,
   false)`. Assert the leftmost non-space content is `SELECT` with `BRANCH`
   fg, `SELECT_BG` bg.
2. **`title_line` SELECT OFF is recessive** — `title_line(160, false, false)`.
   Assert SELECT text present with `DIM` fg, no `SELECT_BG` fill.
3. **`title_line` YOLO shown when enabled** — `title_line(160, true, true)`.
   Assert `YOLO` text present with `WARN` fg, `WARN_DIM` bg. Assert it appears
   after SELECT with a 1-cell gap (compute positions via `.width()` on the
   rendered line, not hardcoded offsets — `⚠` is ambiguous-width).
4. **`title_line` YOLO hidden when disabled** — `title_line(160, true, false)`.
   Assert no `YOLO` text anywhere in the line.
5. **`title_line` version centered with wordmark** — `title_line(160, false,
   false)`. Assert `zoid v…` is present. Derive the expected `zoid` start
   column by scanning the rendered buffer (like `select_pill_style` does at
   render.rs:2234), not by recomputing `(w - combined_w) / 2` in the test
   (that's a tautology). Assert the scanned column equals the expected
   center.
6. **`title_line` left zone doesn't overlap centered block at 160** —
   `title_line(160, true, true)`. Assert `zoid` start column >
   left_zone_w (SELECT + gap + YOLO width, computed via `.width()`). This is
   the regression test for the guard.
7. **`render_status` has no SELECT pill** — render status bar via
   `status_buffer`, assert no `SELECT` text appears.

### Existing test updates

- `select_pill_on_is_filled_purple` / `select_pill_off_is_recessive_no_fill` —
  rewrite `status_buffer` → `title_buffer` (renders via `render_title`/
  `title_line` with a `TestBackend`), repoint `select_pill_style` to scan the
  title buffer. The old `status_buffer` and `select_pill_style` helpers are
  replaced, not duplicated.
- `title_shows_version_flush_left_and_keeps_wordmark_centered` — version is
  no longer flush-left. Rewrite to assert version is adjacent to wordmark
  and the combined block is centered (buffer-scan for the column, not
  formula recomputation).
- `title_drops_version_when_left_pad_too_narrow` — remove (no drop logic).
- `compaction_segment_absent_when_not_compacting` (render.rs:2313) — update
  the docstring: the status bar is no longer "byte-identical to the
  pre-feature layout" (SELECT was removed). The test assertion
  (`!content.contains("compacting")`) still passes.

### Snapshot tests (`crates/zoid-tui/tests/`)

Regenerate via `cargo insta test --accept -p zoid-tui`. Every snapshot that
captures the top bar changes (SELECT added, version repositioned); every
snapshot that captures the bottom bar changes (SELECT removed). That's
effectively all of them. The diff is large but mechanical. YOLO is absent in
all snapshots (snapshots run with `ShellState::new()` which defaults
`yolo: false`; confirm no test harness constructs `ShellState` with
`yolo: true` via grep at implementation time).

## Files Touched

| File | Change |
|------|--------|
| `crates/zoid-tui/src/state.rs` | Add `yolo: bool` field (adjacent to `select_mode`) + default in `new()` |
| `crates/zoid-tui/src/render.rs` | Rewrite `title_line` (new signature + guard), update `render_title`, remove SELECT from `render_status`, rewrite `status_buffer`/`select_pill_style` test helpers → `title_buffer`, update/add tests |
| `crates/zoid/src/main.rs` | Add `app.shell.yolo = app.yolo` sync |
| `crates/zoid-tui/tests/` (snapshots) | Regenerate |

## Out of Scope

- Changing SELECT's toggle behavior or keybinding (`Alt+M` / `:select`).
- Changing YOLO's activation (CLI `--yolo` / config `approval.yolo`).
- Adding a runtime toggle for YOLO (currently CLI/config only).
- Config-reload-driven YOLO toggling (`app.yolo` is latched at startup).