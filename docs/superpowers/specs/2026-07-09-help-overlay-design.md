# Keyboard-Shortcuts Help Overlay — Design

**Date:** 2026-07-09
**Status:** Approved (brainstorming), pending implementation
**Author:** strvmarv (+ zoid assistant)

## Goal

Give users a discoverable, in-app reference of every keyboard shortcut and
command. Reachable three ways — a dedicated `?` key (from conversation focus),
a command-palette row, and a `:help` command — and advertised by a hint line on
the empty-session screen (first-time users and every new session).

## Motivation

zoid has a rich keymap (Ctrl+P palette, Shift+Tab modes, Alt+P provider switch,
semantic zoom, focus cycling, scroll keys, per-overlay keys) but no in-app place
to see it. New beta users have no way to learn the shortcuts short of reading
source or docs. This closes that gap with a low-risk overlay that mirrors the
existing read-only `/mcp` overlay pattern.

## Non-Goals (YAGNI)

- No user-customizable keybindings.
- No per-mode or context-sensitive help (one static reference for all).
- No search/filter within the help overlay.
- No external/browser help; this is terminal-only.

## User-Facing Behavior

### Opening the overlay

The help overlay opens (`app.shell.overlay = Overlay::Help`) via:

1. **`?`** — only when the **conversation pane is focused**. It must NOT open
   when the input box is focused, so a literal `?` typed into a message is
   preserved. (Routed as a new `Action::OpenHelp` in the Conversation-focus arm
   of `route_key`.)
2. **Command palette** — a `Keyboard shortcuts…` row in the pick-phase list
   (open with `Ctrl+P`), plus a `help` completion in the `:` direct phase.
3. **`:help`** — a new `Command::OpenHelp` parsed from the `help` command word.

### Closing the overlay

`Esc` or `q` closes it (mirrors the `/mcp` overlay's `route_mcp_key`), returning
to `Overlay::None` via `close_overlay()`.

### Scrolling

The content is comprehensive and will overflow a short terminal, so the overlay
supports vertical scroll: `↑`/`k` up, `↓`/`j` down, `PageUp`/`PageDown` by a
page, clamped to `[0, max_scroll]`. Scroll state is a `help_scroll: usize` field
on `ShellState`, reset to `0` when the overlay closes.

### Sizing

Help uses its own centered rect (`help::HELP_RECT_W` × `HELP_RECT_H` = 84 × 26),
larger than the 72×18 palette box, clamped to the conversation area by
`layout::centered` (which clamps both dimensions, so it is panic-safe on any
terminal). On terminals large enough to show all content, no scrolling is
needed; on smaller terminals, scroll fills the gap. Scroll has a single source
of truth: the `ScrollHelp` handler only increments `help_scroll`, and the bin
clamps it per-frame against the real rect height (mirroring how `conv_max_scroll`
clamps conversation scroll) so it can never run away past the last page. Rows are
kept compact (≤ ~56 cols, enforced by a unit test) so they don't wrap; on a very
narrow conversation width they clip gracefully like the `/mcp` rows do.

## Overlay Content

A bordered box titled ` keyboard shortcuts `. Rows are grouped into sections
with a dimmed section label in the left column and the shortcuts in the body.
Exact groupings (derived from the current keymap in `route.rs`):

**Global (any focus)**
- `Ctrl+P` — command palette
- `Ctrl+O` — object / action picker
- `Ctrl+Q` — quit zoid
- `Esc` / `Ctrl+C` — cancel current turn (press `Esc` again to force-stop)
- `Shift+Tab` — switch mode · `Tab` — change focus
- `Alt+P` — switch provider / model
- `Alt+←` / `Alt+→` — semantic zoom out / in
- `?` — this help (from the conversation pane)

**Input**
- `Enter` — send · `Shift+Enter` / `Alt+Enter` — newline
- `:` (empty box) — command palette
- `Shift+Del` — delete line · `Shift+Home` / `Shift+End` — cursor to start / end

**Conversation (when focused)**
- `j` / `↓` — scroll down · `k` / `↑` — scroll up
- `=` / `+` — zoom in · `-` / `_` — zoom out
- `Shift+Home` / `Shift+End` — scroll to top / bottom
- `Esc` — return to input · `?` — this help

**Overlays & pickers**
- `↑` / `↓` — move selection · `Enter` — choose
- `Esc` — close (`q` in read-only overlays)

**Commands (in palette or after `:`)**
- `:help` · `:compact` · `:config` · `:feedback` · `:q`
- `:mode install superpowers`

**Mouse**
- scroll to scroll · `Ctrl`+scroll to zoom

The rendered content is produced by a single pure function so it can be unit
tested and kept in sync in one place.

## The Hint

A new line on the empty-session screen:

> `Press ? (or run :help) for keyboard shortcuts`

Styled in `CHAT_ACCENT`, added to **both** the new-user and returning-user
branches of `onboarding::empty_state_lines`, so it shows for first-time users
and at the top of every new session. It sits below the existing content (and
below the Superpowers offer on the new-user path). It mentions `:help` as well
as `?` because the input box is focused by default (where `?` is literal), so
`:help` is the always-works path.

## Architecture / Integration Seams

Pure renderer/router lives in crate `zoid-tui`; the binary (`crates/zoid`) drives
it. A new `Overlay::Help` variant must be registered at every seam — the two
compiler-blind spots (`layout.rs` `matches!` and `render.rs` `if/else` chain) are
the known silent-failure traps and must be covered by the layout guard test.

1. **`state.rs`** — add `Overlay::Help`; add `help_scroll: usize` to `ShellState`
   (init `0` in `new()`, reset in `close_overlay()`).
2. **`command.rs`** — add `Command::OpenHelp`; parse `"help" => Command::OpenHelp`.
3. **`layout.rs`** — Help gets its own centered rect (own branch, not the shared
   72×18 palette `matches!`, because it is sized differently); extend the overlay
   guard test to assert Help gets a rect.
4. **`render.rs`** — add the `Overlay::Help` dispatch branch; add
   `render_help_overlay`; add the pure content builder; add a `Command::OpenHelp`
   arm to the exhaustive palette-preview `match`.
5. **`route.rs`** — add `Action::OpenHelp`; route `?` in the Conversation-focus
   arm; add `Overlay::Help => route_help_key(...)` to the overlay match plus a
   `route_help_key` (Esc/`q` close, scroll keys); add `Overlay::Help` to the
   `route_paste` selection-only arm.
6. **`palette.rs`** — add a `Keyboard shortcuts…` row to `all_items`, and a
   `help` completion to `stage1_items`.
7. **`onboarding.rs`** — add the hint constant + `lines.push` in both branches.
8. **`main.rs`** — add `Command::OpenHelp => { overlay = Help }` in `exec_command`;
   handle `Action::OpenHelp` in the action dispatch.

## Testing Strategy

- **Pure content builder** — unit test asserts representative shortcuts are
  present (`Ctrl+P`, `Shift+Tab`, `Alt+P`, `:help`) and section labels appear.
- **Render test** — `help_overlay_lists_shortcuts`: draw `render_help_overlay`
  into a `TestBackend`, assert the buffer contains key tokens (mirrors
  `mcp_overlay_lists_servers`).
- **Scroll test** — assert `help_scroll` advances and clamps; a render at a
  non-zero scroll shows later content.
- **Routing tests** — `?` from Conversation focus → `Action::OpenHelp`; `?` from
  Input focus → NOT `OpenHelp` (stays an edit); `Esc`/`q` in `Overlay::Help` →
  `CloseOverlay`.
- **Command test** — `parse_command("help") == Command::OpenHelp` (and `:help`).
- **Layout guard** — extend the existing overlay-rect array test to include
  `Overlay::Help`.
- **Onboarding tests** — the hint line appears in both new-user and
  returning-user output.

## Risks

- **Silent invisible render** if `layout.rs`/`render.rs` seams are missed — the
  guard test mitigates this.
- **`?` shadowing input** — mitigated by routing `?` only from Conversation
  focus; covered by a routing test.
- **Content drift** — shortcuts listed in help could fall out of sync with
  `route.rs`. Accepted risk (no single source of truth for the keymap today);
  the pure builder keeps it to one editable location. A future refactor could
  derive help from the router, but that is out of scope.
