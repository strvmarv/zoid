# Diff Line Background Highlighting — Design

**Date:** 2026-07-28
**Status:** Design (approved; reviewed by Gilfoyle — context-line bg fixed, tests expanded)

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
(`CHAT_BG = Rgb(0x0d, 0x2a, 0x4d)`). **Note:** the conversation pane is
*not* filled with `CHAT_BG` at render time — it renders directly onto the
terminal's default background (the hot path uses `buf.set_line` with no
`Clear`/`Block`; the uncached path uses a bare `Paragraph`). So `CHAT_BG` is
used only for the mockup's backdrop approximation, not as a render background.
The add/del tint RGBs should be re-validated against the real on-screen
terminal background during implementation.

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Highlight style | Background tint + colored foreground text | Scanability from the highlight band; readability from the colored text and sign. |
| Highlight extent | Full row including the line-number gutter | Matches GitHub and most terminal diff tools. A changed line is a solid band of color. |
| Highlight width | Full terminal width — pad changed lines to `ctx.width` | Consistent solid bands regardless of text length. Matches `git diff`. |
| Context lines | No background highlight | Context lines carry no `.bg()` — they render on the terminal default background with DIM foreground, exactly as today. |

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
is foreground-only and outside the scope of this change. The foreground reuse
is deliberate and unaffected: the same bright green/red works on both the
new diff background tints (in snippets) and the terminal-default background
(in the drawer). A future reader should not "fix" one context without the
other.

The new `ADDED_BG`/`REMOVED_BG` constants are distinct from `CHAT_BG`
(`Rgb(0x0d, 0x2a, 0x4d)`) — they are not aliases for the pane background.

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

A new named constant `GUTTER_W` is defined in `chat.rs` (near `display_width`
at line ~1181):

```rust
/// Width of the diff-snippet line-number gutter: 6 leading spaces + a 5-char
/// right-aligned line number + 1 trailing space. Used to pad the highlight band
/// to the full terminal width. Named (not inlined) because the literal
/// `"      {no:>5} "` is 12 chars, not the obvious 11 — a magic number here
/// invites a silent off-by-one in the pad math.
const GUTTER_W: usize = 12;
```

```rust
for dl in &d.lines {
    // Add/del lines get a background tint; context lines get NO background
    // (the conversation pane is not filled with CHAT_BG — it renders on the
    // terminal default — so setting any bg on context lines would paint a
    // visible band that contradicts the "no highlight on context" decision).
    let (sign, fg, bg) = match dl.kind {
        crate::state::RenderDiffKind::Add => ("+", color::ADDED,   Some(color::ADDED_BG)),
        crate::state::RenderDiffKind::Del => ("−", color::REMOVED, Some(color::REMOVED_BG)),
        crate::state::RenderDiffKind::Ctx => (" ", color::DIM,     None),
    };
    let no = dl.new_no.or(dl.old_no).unwrap_or(0);
    let content = format!("{sign} {}", dl.text);
    // Pad to full terminal width so the highlight band extends to the right edge.
    // Only meaningful when bg is set (add/del); for context, pad is computed but
    // the None bg means no visible effect.
    let pad = ctx.width.saturating_sub(GUTTER_W + display_width(&content));
    let pad_str = " ".repeat(pad);
    let gutter = Span::styled(format!("      {no:>5} "), Style::new().fg(color::DIM));
    let content_span = Span::styled(format!("{content}{pad_str}"), Style::new().fg(fg));
    let (gutter, content_span) = match bg {
        Some(bg) => (gutter.bg(bg), content_span.bg(bg)),
        None => (gutter, content_span),
    };
    lines.push(Line::from(vec![gutter, content_span]));
}
```

### What changed and why

1. **Two spans with optional background.** The match now yields `(sign, fg,
   bg)` where `bg` is `Option<Color>` — `Some(ADDED_BG)` for additions,
   `Some(REMOVED_BG)` for deletions, `None` for context lines. Both spans get
   `.bg(bg)` only when `bg` is `Some`.

2. **Context lines carry no background.** The conversation pane is not filled
   with `CHAT_BG` or any color at render time — it renders directly on the
   terminal's default background (hot path: `buf.set_line` with no `Clear`/
   `Block`; uncached path: bare `Paragraph`). Setting `CHAT_BG` on context lines
   would paint a visible dark-navy band on every context line, contradicting
   the "no highlight on context" decision. `None` preserves today's behavior
   exactly.

3. **Content span is padded to `ctx.width`.** The content (`{sign} {text}`) is
   padded with spaces to fill the remaining terminal width, so the background
   tint extends to the right edge. `display_width` (defined in `chat.rs` at
   line ~1181 and already used in `build_conversation`'s vicinity) is used
   instead of `content.len()` for correct width calculation with CJK/wide
   characters.

4. **`GUTTER_W` is a named constant (12 columns).** Six leading spaces, a
   5-char right-aligned line number, and one trailing space (`6 + 5 + 1 = 12`).
   Defined as a `const` in `chat.rs` near `display_width`. The literal
   `"      {no:>5} "` is 12 chars, not the obvious 11 — the named constant
   prevents a silent off-by-one in the pad math.

### Edge cases

- **`ctx.width` smaller than gutter + content:** `saturating_sub` clamps `pad`
  to 0 — no padding, no overflow. The line is shorter than the terminal; the
  highlight covers what's there. `GUTTER_W` (12) is subtracted, matching the
  actual gutter format string width.
- **Context lines:** `bg = None` — no `.bg()` is applied to either span.
  Context lines render on the terminal default background with DIM foreground,
  exactly as today. No visible highlight appears.
- **Tabs / control characters in `dl.text`:** `display_width` uses
  `UnicodeWidthStr::width`, which gives tabs and control chars a width of 0.
  If a diff line contains a tab, the computed `pad` may over-count (the tab
  renders as a single cell in the terminal), causing the highlight band to
  extend slightly past the visible text edge. This is low severity — real
  diffs in zoid's edit/write tools rarely contain raw tabs — but the spec
  acknowledges the gap. A future fix could normalize tabs before width math.
- **`ctx.width` vs `text.width` coupling:** The renderer clips each line to
  `text.width` (the inset rect width), which equals `ctx.width`. The pad math
  targets `ctx.width`, so the band lines up with the clip edge. This coupling
  is an invariant: if `ctx.width` and the inset width ever decouple, the band
  and the clip will desync silently. A comment in the code should note this.
- **Truncated diff indicator (`…+N more`):** Unchanged — stays `DIM`
  foreground, no background. It sits outside the `for dl in &d.lines` loop.
  Note: under the new design, it appears directly below tinted add/del bands
  with no tint of its own — a deliberate visual “step down” marking it as
  metadata, not a diff line.
- **Wide characters (CJK, emoji):** `display_width` handles double-width
  characters so the padding to `ctx.width` is correct (modulo the tab caveat
  above).

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

Add one test verifying the background color is set on add/del spans — and
*not* on context-line spans:

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
    // Include a context line alongside the add/del lines.
    let diff = RenderDiff {
        path: "f.rs".into(), added: 1, removed: 1, truncated_by: 0,
        lines: vec![
            RenderDiffLine { old_no: Some(2), new_no: Some(2), kind: RenderDiffKind::Ctx, text: "ctx-line".into() },
            RenderDiffLine { old_no: Some(1), new_no: None, kind: RenderDiffKind::Del, text: "del-line".into() },
            RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: "add-line".into() },
        ],
    };
    let cache = vec![("tc1".to_string(), diff)];
    let lines = conversation_lines_with_diffs(&msgs, false, false, 0, 80, None, &cache, 5);

    // Structural selection: find diff lines by their gutter + sign pattern
    // rather than substring-probing content (fragile against counts line).
    // Each diff line is a Line with exactly 2 spans: gutter + content.
    let diff_lines: Vec<_> = lines.iter()
        .filter(|l| l.spans.len() == 2)
        .filter(|l| l.spans[0].content.starts_with("      "))
        .collect();

    // Context line: no background on either span.
    let ctx_line = diff_lines.iter().find(|l| l.spans[1].content.contains("ctx-line"))
        .expect("ctx line present");
    assert_eq!(ctx_line.spans[0].style.bg, None, "gutter has no bg on context");
    assert_eq!(ctx_line.spans[1].style.bg, None, "content has no bg on context");

    // Del line: both spans have REMOVED_BG.
    let del_line = diff_lines.iter().find(|l| l.spans[1].content.contains("del-line"))
        .expect("del line present");
    assert_eq!(del_line.spans[0].style.bg, Some(color::REMOVED_BG), "gutter has del bg");
    assert_eq!(del_line.spans[1].style.bg, Some(color::REMOVED_BG), "content has del bg");

    // Add line: both spans have ADDED_BG.
    let add_line = diff_lines.iter().find(|l| l.spans[1].content.contains("add-line"))
        .expect("add line present");
    assert_eq!(add_line.spans[0].style.bg, Some(color::ADDED_BG), "gutter has add bg");
    assert_eq!(add_line.spans[1].style.bg, Some(color::ADDED_BG), "content has add bg");
}
```

### `chat.rs` — padding-width tests

Verify the highlight band actually fills to `ctx.width`, and clamps to 0 when
the terminal is too narrow:

```rust
#[test]
fn diff_highlight_band_fills_to_width() {
    // Short content at width 80: the band should pad to fill 80 columns.
    // Total band width = GUTTER_W + display_width(content) + pad == ctx.width.
}

#[test]
fn diff_highlight_clamps_when_too_wide() {
    // width smaller than GUTTER_W + content: pad saturates to 0, no panic.
}
```

### `chat.rs` — `GUTTER_W` invariant test

Prevent the exact magic-number drift the spec warns about:

```rust
#[test]
fn gutter_width_matches_format_string() {
    // The gutter literal "      {no:>5} " is 12 chars; GUTTER_W must match.
    let sample = format!("      {:>5} ", 42);
    assert_eq!(GUTTER_W, sample.len(), "GUTTER_W must match the gutter format string");
}
```

## Scope

This is a pure view-layer color change. One file modified for rendering
(`chat.rs`), one file for new color constants (`tokens.rs`), plus tests. No
new dependencies, no persistence changes, no model-context impact.