# Diff Line Background Highlighting — Design

**Date:** 2026-07-28
**Status:** Design (approved)

## Goal

Change the inline edit/write diff snippets in the TUI chat from foreground-only
coloring (green/red text, no background) to **background-highlighted lines**: a
subtle green/red tint behind the full row — including the line-number gutter and
padded to the terminal's right edge — with the text and `+`/`−` sign still
colored in the foreground. This matches how `git diff` and GitHub render diffs.

## Background

The diff-snippets feature (spec: `2026-07-11-tui-edit-diff-snippets-design.md`)
shows compact add/delete diff lines inline in the chat for each `edit`/`write`
tool call. Lines are computed by `zoid-tools/src/diff.rs` (`FileDiff` /
`DiffLine`), projected to render-side mirrors (`RenderDiff` / `RenderDiffLine`
in `state.rs`), and rendered in `chat.rs`.

Currently each diff line is a single `Span` with `Style::new().fg(col)` where
`col` is `color::ADDED` (bright green `OK`) for additions and `color::REMOVED`
(bright red `ERROR`) for deletions. There is no background highlight — the color
lives in the font, not the line.

## Confirmed visual decisions

These were validated through visual mockups against the actual chat background
(`CHAT_BG = Rgb(0x0d, 0x2a, 0x4d)`):

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Highlight style | Background tint + colored foreground text | Scanability from the highlight band; readability from the colored text and sign. |
| Highlight extent | Full row including the line-number gutter | Matches GitHub and most terminal diff tools. A changed line is a solid band of color. |
| Highlight width | Full terminal width — pad changed lines to `ctx.width` | Consistent solid bands regardless of text length. Matches `git diff`. |
| Context lines | No background highlight | Context lines stay on `CHAT_BG` with DIM foreground, as today. |

## Color constants

Two new constants in `crates/zoid-tui/src/tokens.rs`, alongside the existing
`ADDED`/`REMOVED`:

```
ADDED_BG:   Color::Rgb(0x1a, 0x2e, 0x1f)   // faint green tint
REMOVED_BG: Color::Rgb(0x2e, 0x1a, 0x1b)   // faint red tint
```

These are distinct from the bright foreground colors:
- `ADDED = OK = Rgb(0x3f, 0xb9, 0x50)` — bright green, stays as foreground.
- `REMOVED = ERROR = Rgb(0xf8, 0x51, 0x49)` — bright red, stays as foreground.

The existing `ADDED`/`REMOVED` constants are unchanged. They are also used by
the repo drawer's `+N -M` change counts in `render.rs` (lines 703–708), which
is foreground-only and outside the scope of this change.

## Rendering change

### Location

`crates/zoid-tui/src/chat.rs`, inside `build_conversation`, in the `ToolResult`
arm — the block at lines 339–350 that iterates `d.lines` for inline diff
snippets.

### Current code (lines 339–350)

```rust
for dl in &d.lines {
    let (sign, col) = match dl.kind {
        crate::state::RenderDiffKind::Add => ("+", color::ADDED),
        crate::state::RenderDiffKind::Del => ("−", color::REMOVED),
        crate::state::RenderDiffKind::Ctx => (" ", color::DIM),
    };
    let no = dl.new_no.or(dl.old_no).unwrap_or(0);
    lines.push(Line::from(vec![
        Span::styled(format!("      {no:>5} "), Style::new().fg(color::DIM)),
        Span::styled(format!("{sign} {}", dl.text), Style::new().fg(col)),
    ]));
}
```

### New code

```rust
for dl in &d.lines {
    let (sign, fg, bg) = match dl.kind {
        crate::state::RenderDiffKind::Add => ("+", color::ADDED, color::ADDED_BG),
        crate::state::RenderDiffKind::Del => ("−", color::REMOVED, color::REMOVED_BG),
        crate::state::RenderDiffKind::Ctx => (" ", color::DIM, color::CHAT_BG),
    };
    let no = dl.new_no.or(dl.old_no).unwrap_or(0);
    let content = format!("{sign} {}", dl.text);
    // Pad to full terminal width so the highlight band extends to the right edge.
    // GUTTER_W = 12 = 6 leading spaces + 5-char line number + 1 trailing space.
    let pad = ctx.width.saturating_sub(GUTTER_W + display_width(&content));
    let pad_str = " ".repeat(pad);
    lines.push(Line::from(vec![
        Span::styled(format!("      {no:>5} "), Style::new().fg(color::DIM).bg(bg)),
        Span::styled(format!("{content}{pad_str}"), Style::new().fg(fg).bg(bg)),
    ]));
}
```

### What changed and why

1. **Two spans instead of one foreground color.** The match now yields `(sign,
   fg, bg)` — a foreground color and a background color per diff kind. Both
   spans get `.bg(bg)`.

2. **Gutter span gets the background.** The line-number gutter span
   (`"      {no:>5} "`) now carries `.bg(bg)`, so the tint covers the full row
   from the left margin. For context lines, `bg` is `CHAT_BG` (same as the
   pane background), so no visible highlight appears.

3. **Content span is padded to `ctx.width`.** The content (`{sign} {text}`) is
   padded with spaces to fill the remaining terminal width, so the background
   tint extends to the right edge. `display_width` (already defined in
   `chat.rs` at line 1181 and already imported/used in this function) is used
   instead of `content.len()` for correct width calculation with CJK/wide
   characters.

4. **The gutter width is 12 columns.** Six leading spaces, a 5-char
   right-aligned line number, and one trailing space (`6 + 5 + 1 = 12`). The pad
   calculation subtracts this plus the content width from `ctx.width`. Using a
   named constant `GUTTER_W` (value 12) avoids a magic-number bug — the literal
   `"      {no:>5} "` is 12 chars, not the obvious 11.

### Edge cases

- **`ctx.width` smaller than gutter + content:** `saturating_sub` clamps `pad`
  to 0 — no padding, no overflow. The line is shorter than the terminal; the
  highlight covers what's there. `GUTTER_W` (12) is subtracted, matching the
  actual gutter format string width.
- **Context lines:** `bg = CHAT_BG` makes the background indistinguishable from
  the pane. The gutter stays `DIM` on `CHAT_BG` (same as today).
- **Truncated diff indicator (`…+N more`):** Unchanged — stays `DIM`
  foreground, no background. It sits outside the `for dl in &d.lines` loop.
- **Wide characters (CJK, emoji):** `display_width` handles double-width
  characters so the padding to `ctx.width` is correct.

## What does NOT change

| Component | File | Change |
|-----------|------|--------|
| Diff computation | `zoid-tools/src/diff.rs` | None |
| Render-side diff types | `zoid-tui/src/state.rs` (`RenderDiff`, `RenderDiffLine`) | None |
| Repo drawer `+N -M` counts | `zoid-tui/src/render.rs:703-708` | None (foreground-only, not diff lines) |
| Persistence / model context | — | None (purely view-layer) |
| `ADDED` / `REMOVED` constants | `zoid-tui/src/tokens.rs:109-110` | None (still used as foreground) |

## Testing

### `tokens.rs` — color constant assertions

The existing test `repo_changes_colors_reuse_status_palette` (lines 232–236)
asserts `ADDED == OK` and `REMOVED == ERROR`. Add assertions for the two new
background constants:

```rust
#[test]
fn diff_background_tints_are_distinct_from_foreground() {
    use ratatui::style::Color;
    assert_eq!(color::ADDED_BG, Color::Rgb(0x1a, 0x2e, 0x1f));
    assert_eq!(color::REMOVED_BG, Color::Rgb(0x2e, 0x1a, 0x1b));
    // Background tints are not equal to the foreground colors.
    assert_ne!(color::ADDED_BG, color::ADDED);
    assert_ne!(color::REMOVED_BG, color::REMOVED);
}
```

### `chat.rs` — diff rendering tests

The existing tests (`tool_result_renders_counts_and_inline_snippet_for_cached_edit`
at line 1582, `cached_edit_beyond_k_shows_counts_only_no_snippet` at line 1614)
check structure — kinds, line numbers, text content — via
`lines.iter().flat_map(...).map(|s| s.content...)`. They do not inspect
`Span::style`, so they pass unchanged.

Add one test verifying the background color is set on add/del spans:

```rust
#[test]
fn diff_snippet_lines_have_background_highlight() {
    use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    use zoid_core::projection::ChatMsg;

    let msgs = vec![ChatMsg::ToolResult {
        id: "tc1".into(), name: "edit".into(),
        output: "edited f.rs (1 change)".into(),
        is_error: false, compacted: false, ts: 0,
    }];
    let diff = RenderDiff {
        path: "f.rs".into(), added: 1, removed: 1, truncated_by: 0,
        lines: vec![
            RenderDiffLine { old_no: Some(1), new_no: None, kind: RenderDiffKind::Del, text: "old".into() },
            RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: "new".into() },
        ],
    };
    let cache = vec![("tc1".to_string(), diff)];
    let lines = conversation_lines_with_diffs(&msgs, false, false, 0, 80, None, &cache, 5);

    // Find the spans for the del and add lines — they should carry the
    // background tints, not just foreground color.
    let del_span = lines.iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("old"))
        .expect("del line present");
    let add_span = lines.iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("new"))
        .expect("add line present");

    assert_eq!(del_span.style.bg, Some(color::REMOVED_BG));
    assert_eq!(add_span.style.bg, Some(color::ADDED_BG));
}
```

## Scope

This is a pure view-layer color change. One file modified for rendering
(`chat.rs`), one file for new color constants (`tokens.rs`), plus tests. No
new dependencies, no persistence changes, no model-context impact.