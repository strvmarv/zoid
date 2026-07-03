# Scrollbar + Cross-Zoom Position Anchoring — Design

**Status:** approved (brainstorming) — 2026-07-03
**Task:** #29

## Goal

Give the conversation pane an always-visible, draggable vertical scrollbar, and
preserve the reading position across zoom (altitude) changes by anchoring to the
message at the top of the viewport.

## Background

The chat conversation renders as a bare
`Paragraph::new(body).scroll((conversation_scroll, 0))` inset by `CONV_PAD = 2`
columns on each side. There is:

- **No scrollbar** — the user cannot see scroll position or drag to scroll.
- **No mouse-drag handling** — `route_mouse` handles wheel scroll and left-click
  (via `hit_test` → `Target::{DrawerHeader,Input,Conversation,None}`); `Drag`
  events fall through to `Noop`.
- **A reset-to-top on zoom** — `zoom_in`/`zoom_out` set `conversation_scroll = 0`
  on a real altitude change, because the three altitudes
  (Summary/Normal/Detail) have incomparable line counts, so a carried-over
  offset would render past the end.

Recent related work: `follow_tail` (tail-follow / scroll-to-latest, commit
`1e3f756`) and the body cache (`BodyCache`, commit `b634b2c`). `max_scroll` is
computed each frame in the bin as `body.len() (+ active-tool line) − viewport
height` and stored as `last_conv_max_scroll`.

## Global Constraints

- **Glyphs come from `tokens.rs`, never literals** (spec §16). New glyphs:
  `SCROLL_TRACK`, `SCROLL_THUMB`.
- **Pure, unit-testable geometry** — thumb math and line↔message mapping are
  pure functions, tested like `max_scroll` / `spinner_frame`.
- **No transcript re-wrap** — the scrollbar lives in the existing right-pad
  gutter, so the text width is unchanged and the body does not re-wrap.
- **Static-musl release** — no new native deps; hand-rolled rendering (no extra
  crates).
- **Determinism** — snapshot-affecting state has fixed defaults.

## Approach (chosen)

Hand-rolled pure geometry function + 1-column gutter render + drag mapping in the
bin (approach "B"). Rejected: ratatui's `Scrollbar` widget (awkward
always-visible/empty behavior, symbol-override styling vs `tokens`, internal
geometry not unit-testable) and a non-draggable position indicator (user wants
drag).

## Components

### 1. Scrollbar rendering & placement

New module `crates/zoid-tui/src/scrollbar.rs`:

```rust
/// Vertical scrollbar thumb geometry. `offset` ∈ [0, max_scroll], `track_h` =
/// conversation height in rows, `content_len` = total body lines. Returns
/// (thumb_start, thumb_len) in track rows, both within [0, track_h]. Always a
/// ≥1-row thumb; a full-height thumb when everything fits (max_scroll == 0).
pub fn scrollbar_thumb(offset: u16, max_scroll: u16, track_h: u16, content_len: u16) -> (u16, u16)
```

- **thumb_len** = `max(1, round(track_h² / content_len))`, capped at `track_h`
  (when `content_len ≤ track_h`, thumb is full height).
- **thumb_start** = `round((track_h − thumb_len) * offset / max_scroll)` when
  `max_scroll > 0`, else `0`. Guarantees the thumb sits flush at the bottom when
  `offset == max_scroll`.
- **Placement**: painted into the rightmost column of `layout.conversation`
  (`x = conversation.right() − 1`, rows `conversation.y .. conversation.bottom()`).
  With `CONV_PAD = 2`, text ends at `right − 2`; column `right − 2` is a 1-col
  gap, column `right − 1` is the bar. No text overlap, no re-wrap.
- **Glyphs**: `SCROLL_TRACK = '│'` painted dim over the full track height first,
  then `SCROLL_THUMB = '█'` painted in the Chat accent color over the thumb rows.
  Both always drawn (always-visible requirement).
- Rendering happens in `render_shell`'s Chat block, after the paragraph is
  painted. `render_shell` already returns `conv_max_scroll`; the geometry reuses
  the `max_scroll` and `text` rect it already computes.

### 2. Mouse interaction

- `hit_test` gains `Target::Scrollbar` — returned when `col == bar column`
  (`conversation.right() − 1`) and `row ∈ [conversation.y, conversation.bottom())`.
  Checked before the `Conversation` case (the bar column sits inside the
  conversation rect).
- `ShellState` gains `scrollbar_drag: bool` (default `false`) — cross-event
  memory so the pure `route_mouse` can classify bare `Drag(Left)` events as
  scrollbar drags without the bin re-deriving geometry.
- `route_mouse` (no active blocking overlay):
  - `Down(Left)` on `Target::Scrollbar` → `Action::ScrollbarGrab(row)`
  - `Drag(Left)` while `state.scrollbar_drag` → `Action::ScrollbarDrag(row)`
  - `Up(Left)` → `Action::ScrollbarRelease`
- New actions in `route::Action`: `ScrollbarGrab(u16)`, `ScrollbarDrag(u16)`,
  `ScrollbarRelease`.
- **Bin handling** (knows track geometry + `last_conv_max_scroll`):
  - `ScrollbarGrab(row)`: set `app.shell.scrollbar_drag = true`, then map row→offset
    and apply (same as drag).
  - `ScrollbarDrag(row)`: map `offset = round((row − track_top) / (track_h − 1) *
    max_scroll)`, clamped to `[0, max_scroll]`, via
    `ShellState::scroll_to_offset(offset, max_scroll)`.
  - `ScrollbarRelease`: `app.shell.scrollbar_drag = false`.
  - `track_top = conversation.y`, `track_h = conversation.height`.
- New method:

```rust
/// Set the conversation scroll to an absolute offset (clamped) and re-derive
/// tail-follow: landing at (or past) the bottom re-engages follow, any position
/// above it detaches. Mirrors `scroll_conversation` but absolute, for the
/// scrollbar drag / track click.
pub fn scroll_to_offset(&mut self, offset: u16, max_scroll: u16) {
    let next = offset.min(max_scroll);
    self.conversation_scroll = next;
    self.follow_tail = next >= max_scroll;
}
```

- **Track click** (click on the bar away from the thumb) is jump-to-position for
  free: the same absolute mapping moves the thumb center to the click row. No
  separate paging logic.

### 3. Cross-zoom position anchoring

**Data.** Extend `build_conversation` to emit per-message start-line indices via
a `msg_starts: &mut Vec<usize>` out-param (mirroring the existing `hits`
out-param): `msg_starts[i]` is the body line index where message `i` begins.
`conversation_view` (the cached path) gains a variant/wrapper that returns the
`msg_starts` too. `BodyCache` caches `msg_starts: Vec<usize>` alongside `body`,
under the same `BodyKey`.

**Pure helpers** (in `scrollbar.rs`):

```rust
/// Index of the message occupying `line` (the last message whose start ≤ line).
/// 0 when `starts` is empty or `line` precedes the first start.
pub fn msg_at_line(starts: &[usize], line: usize) -> usize

/// First body line of message `idx`; clamps to the last start (or 0 when empty).
pub fn line_of_msg(starts: &[usize], idx: usize) -> usize
```

`msg_at_line` = `starts.partition_point(|&s| s <= line).saturating_sub(1)`.
`line_of_msg` = `starts.get(idx).copied().unwrap_or(0)`.

**Flow (deferred one step, flicker-free):**

1. `App` gains `pending_zoom_anchor: Option<usize>` (default `None`).
2. On `ZoomIn`/`ZoomOut` with a real altitude change: compute
   `anchor = msg_at_line(&app.body_cache.msg_starts, conversation_scroll as usize)`
   from the **old-zoom** body *before* zooming; set
   `app.pending_zoom_anchor = Some(anchor)`. `zoom_in()`/`zoom_out()` keep their
   existing reset-to-0 (state semantics + their existing tests unchanged).
3. In the **pre-draw block**, after `body_cache.refresh` builds the new-zoom
   `body` + `msg_starts` and `max_scroll` is computed, *before* `terminal.draw`:
   if `pending_zoom_anchor` is `Some(anchor)`, set
   `conversation_scroll = min(line_of_msg(&msg_starts, anchor) as u16, max_scroll)`
   and clear it. Then `apply_follow(max_scroll)` runs — if following the tail it
   still pins to bottom; if detached, the anchor holds. All state settles before
   any cell is painted → no flicker.

**Retiring the zoom reveal animation.** The top-down reveal (`motion::zoom_reveal`
revealing lines `0..N`) and position-anchoring are incompatible: one sweeps from
the top, the other holds the anchor line. Since every zoom now either
follows-to-bottom or anchors, the reveal would always be skipped. Therefore the
bin **stops setting `zoom_changed_at`** — zoom is instant in all cases. The
`zoom_reveal` / `Anim` / `ZOOM_ANIM_MS` code stays in the tree (dormant, still
unit-tested) for a possible future transition; the motion tick's
`zoom_changed_at.is_some()` guard becomes permanently false for zoom but is left
in place (harmless, still serves the streaming caret).

### 4. Files touched

- Create: `crates/zoid-tui/src/scrollbar.rs` (geometry + line↔message helpers).
- Modify: `crates/zoid-tui/src/lib.rs` (export scrollbar module + fns).
- Modify: `crates/zoid-tui/src/tokens.rs` (add `SCROLL_TRACK`, `SCROLL_THUMB`).
- Modify: `crates/zoid-tui/src/state.rs` (add `scrollbar_drag`, `scroll_to_offset`).
- Modify: `crates/zoid-tui/src/chat.rs` (`build_conversation` emits `msg_starts`;
  wrapper to expose it).
- Modify: `crates/zoid-tui/src/render.rs` (paint the scrollbar in the gutter).
- Modify: `crates/zoid-tui/src/route.rs` (`Target::Scrollbar`, `hit_test`,
  `route_mouse` grab/drag/release, new `Action`s).
- Modify: `crates/zoid/src/main.rs` (`BodyCache.msg_starts`; `pending_zoom_anchor`;
  scrollbar drag→offset mapping in `handle_action`; anchor application in the
  pre-draw block; stop setting `zoom_changed_at`).

## Testing

- **Unit (`scrollbar.rs`)**: `scrollbar_thumb` — full thumb when content fits;
  proportional mid-scroll; clamps within track; ≥1-row thumb; flush-bottom at
  `offset == max_scroll`; `content_len == 0`. `msg_at_line` / `line_of_msg` —
  boundaries, empty `starts`, `line` before first start, `idx` out of range.
- **Unit (`state.rs`)**: `scroll_to_offset` clamps and re-derives `follow_tail`
  (detaches above bottom, re-engages at bottom).
- **Snapshot (`shell_snapshot` etc.)**: the gutter column now carries the bar, so
  status/frame snapshots regenerate; diffs must be **localized to the bar column**
  (verified via buffer-Debug). Add dedicated snapshots with the thumb at top /
  mid / bottom.
- **Cross-zoom**: construct `msg_starts` at two altitudes and assert an anchored
  message maps to the expected line after the transition.

## Out of scope

- Horizontal scrollbar (conversation is 1-line-per-row; over-wide code clips by
  design).
- Scrollbars for overlays / drawers / rail.
- Momentum/inertia or animated thumb movement.
- Anchoring finer than message granularity (e.g. sub-message line offset).
