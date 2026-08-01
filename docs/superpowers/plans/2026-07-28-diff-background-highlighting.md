# Diff Line Background Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Change TUI inline diff snippets from foreground-only coloring to full-row background-highlighted lines with colored foreground text, matching `git diff` / GitHub.

**Architecture:** Two new color constants (`ADDED_BG`, `REMOVED_BG`) in `tokens.rs`; one rendering change in `chat.rs` that adds `.bg()` to add/del diff-line spans (but not context lines), pads changed lines to `ctx.width` with spaces so the tint reaches the terminal's right edge, and uses a named `GUTTER_W` constant for the pad math. No changes to diff computation, types, persistence, or model context.

**Tech Stack:** Rust, ratatui 0.30 (`Span`, `Style`, `Stylize` trait — `Span::bg()` is available via `Styled` impl), `unicode-width` (already a dependency via `display_width`).

## Global Constraints

- Two new color constants only: `ADDED_BG = Color::Rgb(0x1a, 0x2e, 0x1f)`, `REMOVED_BG = Color::Rgb(0x2e, 0x1a, 0x1b)`. Verbatim RGB values.
- Existing `ADDED = OK` and `REMOVED = ERROR` constants are unchanged.
- Context lines get `bg = None` — no `.bg()` applied. The conversation pane is NOT filled with `CHAT_BG`; it renders on the terminal default background.
- `GUTTER_W = 12` as a named `const` in `chat.rs` near `display_width`.
- Use `display_width` (not `content.len()`) for CJK/wide-char correctness.
- No new dependencies. No persistence, DB, or model-context changes.

---

## File Structure

| File | Responsibility | Action |
|------|---------------|--------|
| `crates/zoid-tui/src/tokens.rs` | Color constants | Add `ADDED_BG` / `REMOVED_BG` + test |
| `crates/zoid-tui/src/chat.rs` | Diff line rendering + `GUTTER_W` | Add `GUTTER_W` const, modify diff-line loop, add 4 tests |

---

### Task 1: Add `ADDED_BG` / `REMOVED_BG` color constants

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs:109-110` (after the `REMOVED` constant)
- Test: `crates/zoid-tui/src/tokens.rs` (in the `#[cfg(test)] mod tests` block, after `repo_changes_colors_reuse_status_palette` at line 236)

**Interfaces:**
- Produces: `color::ADDED_BG: Color` and `color::REMOVED_BG: Color` — two new `pub const` values in the `color` module. Task 2's rendering code references these as `color::ADDED_BG` / `color::REMOVED_BG`.

- [ ] **Step 1: Write the failing test**

Add this test to the `tests` module in `tokens.rs`, immediately after the `repo_changes_colors_reuse_status_palette` test (which ends at line 236):

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

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib tokens::tests::diff_background_tints`
Expected: FAIL — `color::ADDED_BG` not found (no such constant)

- [ ] **Step 3: Write minimal implementation**

Add the two new constants in `tokens.rs`, immediately after the `REMOVED` line (line 110), before the tree-sitter syntax palette comment at line 112:

```rust
    // repo drawer changes line — reuses the status palette (§16: uniform language).
    pub const ADDED: Color = OK; // +added lines
    pub const REMOVED: Color = ERROR; // -removed lines

    // Diff line background tints — subtle green/red bands behind add/del rows.
    // Distinct from CHAT_BG (0x0d,0x2a,0x4d): the conversation pane is NOT filled
    // with CHAT_BG, so these are standalone tints, not pane-background aliases.
    pub const ADDED_BG: Color = Color::Rgb(0x1a, 0x2e, 0x1f); // faint green
    pub const REMOVED_BG: Color = Color::Rgb(0x2e, 0x1a, 0x1b); // faint red
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui --lib tokens::tests::diff_background_tints`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs
git commit -m "feat: add ADDED_BG/REMOVED_BG diff background tint constants"
```

---

### Task 2: Add `GUTTER_W` constant and modify the diff-line rendering loop

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:339-350` (the `for dl in &d.lines` loop)
- Modify: `crates/zoid-tui/src/chat.rs` near line 1181 (add `GUTTER_W` const near `display_width`)
- Test: `crates/zoid-tui/src/chat.rs` (in the `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `color::ADDED_BG` / `color::REMOVED_BG` from Task 1. `display_width` (already defined at `chat.rs:1181`).
- Produces: `GUTTER_W: usize` — a `const` used by the rendering loop and by Task 3's tests.

- [ ] **Step 1: Write the failing test for background highlight**

Add this test to the `tests` module in `chat.rs`. Place it immediately after the `tool_result_renders_counts_and_inline_snippet_for_cached_edit` test (which ends at line 1612):

```rust
#[test]
fn diff_snippet_lines_have_background_highlight() {
    use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    use zoid_core::projection::ChatMsg;

    let msgs = vec![ChatMsg::ToolResult {
        id: "tc1".into(),
        name: "edit".into(),
        output: "edited f.rs (1 change)".into(),
        is_error: false,
        compacted: false,
        ts: 0,
    }];
    // Include a context line alongside the add/del lines.
    let diff = RenderDiff {
        path: "f.rs".into(),
        added: 1,
        removed: 1,
        truncated_by: 0,
        lines: vec![
            RenderDiffLine { old_no: Some(2), new_no: Some(2), kind: RenderDiffKind::Ctx, text: "ctx-line".into() },
            RenderDiffLine { old_no: Some(1), new_no: None, kind: RenderDiffKind::Del, text: "del-line".into() },
            RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: "add-line".into() },
        ],
    };
    let cache = vec![("tc1".to_string(), diff)];
    let lines = conversation_lines_with_diffs(&msgs, false, false, 0, 80, None, &cache, 5);

    // Structural selection: find diff lines by their gutter pattern (2 spans,
    // first starts with 6 leading spaces) rather than substring-probing content.
    let diff_lines: Vec<_> = lines.iter()
        .filter(|l| l.spans.len() == 2)
        .filter(|l| l.spans[0].content.starts_with("      "))
        .collect();

    // Context line: no background on either span, DIM foreground.
    let ctx_line = diff_lines.iter().find(|l| l.spans[1].content.contains("ctx-line"))
        .expect("ctx line present");
    assert_eq!(ctx_line.spans[0].style.bg, None, "gutter has no bg on context");
    assert_eq!(ctx_line.spans[1].style.bg, None, "content has no bg on context");
    assert_eq!(ctx_line.spans[1].style.fg, Some(color::DIM), "content has DIM fg on context");

    // Del line: both spans have REMOVED_BG, content has REMOVED fg.
    let del_line = diff_lines.iter().find(|l| l.spans[1].content.contains("del-line"))
        .expect("del line present");
    assert_eq!(del_line.spans[0].style.bg, Some(color::REMOVED_BG), "gutter has del bg");
    assert_eq!(del_line.spans[1].style.bg, Some(color::REMOVED_BG), "content has del bg");
    assert_eq!(del_line.spans[1].style.fg, Some(color::REMOVED), "content has REMOVED fg on del");

    // Add line: both spans have ADDED_BG, content has ADDED fg.
    let add_line = diff_lines.iter().find(|l| l.spans[1].content.contains("add-line"))
        .expect("add line present");
    assert_eq!(add_line.spans[0].style.bg, Some(color::ADDED_BG), "gutter has add bg");
    assert_eq!(add_line.spans[1].style.bg, Some(color::ADDED_BG), "content has add bg");
    assert_eq!(add_line.spans[1].style.fg, Some(color::ADDED), "content has ADDED fg on add");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui --lib diff_snippet_lines_have_background_highlight`
Expected: FAIL — the del/add assertions fail (today all diff lines have `style.bg == None`, not `Some(REMOVED_BG)`/`Some(ADDED_BG)`). The context-line assertions *pass* both before and after the change (today `bg == None`, which is the desired state) — that's the point: context lines are unchanged.

- [ ] **Step 3: Add the `GUTTER_W` constant**

Add this `const` in `chat.rs`, immediately before the `display_width` function definition (line 1178). Place it at module level (outside any function):

```rust
/// Width of the diff-snippet line-number gutter: 6 leading spaces + a 5-char
/// right-aligned line number + 1 trailing space. Used to pad the highlight
/// band to the full terminal width. Named (not inlined) because the literal
/// `"      {no:>5} "` is 12 chars, not the obvious 11 — a magic number here
/// invites a silent off-by-one in the pad math.
const GUTTER_W: usize = 12;
```

- [ ] **Step 4: Replace the diff-line rendering loop**

Replace the block at `chat.rs` lines 339–350 (the `for dl in &d.lines` loop inside the `ToolResult` arm) with the following. **The `for` keyword sits at exactly 24 columns** (the loop is nested inside `if let Some(d) = diff { if inline_ids.contains(...) { ... } }` within the `ToolResult` arm). Inner statements are at 28; the `match bg` body at 32. Match this exactly — `cargo fmt` will rewrite every line if you get it wrong:

```rust
                        for dl in &d.lines {
                            // Add/del lines get a background tint; context lines get
                            // NO background (the conversation pane is not filled with
                            // CHAT_BG — it renders on the terminal default — so
                            // setting any bg on context lines would paint a visible
                            // band that contradicts "no highlight on context").
                            let (sign, fg, bg) = match dl.kind {
                                crate::state::RenderDiffKind::Add => ("+", color::ADDED,   Some(color::ADDED_BG)),
                                crate::state::RenderDiffKind::Del => ("−", color::REMOVED, Some(color::REMOVED_BG)),
                                crate::state::RenderDiffKind::Ctx => (" ", color::DIM,     None),
                            };
                            let no = dl.new_no.or(dl.old_no).unwrap_or(0);
                            let content = format!("{sign} {}", dl.text);
                            // Pad to full terminal width so the highlight band
                            // extends to the right edge. Currently ctx.width ==
                            // the renderer's inset clip width (text.width) by
                            // construction (render.rs passes text.width); this
                            // comment future-proofs against a refactor that
                            // decouples them.
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

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid-tui --lib diff_snippet_lines_have_background_highlight`
Expected: PASS

- [ ] **Step 6: Run existing diff tests to verify no regression**

Run: `cargo test -p zoid-tui --lib tool_result_renders_counts_and_inline_snippet_for_cached_edit cached_edit_beyond_k_shows_counts_only_no_snippet`
Expected: PASS (these tests only check `s.content`, not `s.style`, so adding `.bg()` doesn't affect them)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "feat: background-highlighted diff lines with full-row tint

Add/del lines get a subtle green/red background band (ADDED_BG/REMOVED_BG)
across the full row including the gutter, padded to terminal width.
Context lines keep no background. Uses named GUTTER_W=12 constant."
```

---

### Task 3: Add padding-width and `GUTTER_W` invariant tests

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs` (test module, after the test added in Task 2)

**Interfaces:**
- Consumes: `GUTTER_W` from Task 2. `conversation_lines_with_diffs` and `display_width` (existing). `color::ADDED_BG` / `color::REMOVED_BG` from Task 1.

- [ ] **Step 1: Write the `GUTTER_W` invariant test**

Add this test to the `tests` module in `chat.rs`, after the `diff_snippet_lines_have_background_highlight` test:

```rust
#[test]
fn gutter_width_matches_format_string() {
    // The gutter literal "      {no:>5} " is 12 chars; GUTTER_W must match.
    let sample = format!("      {:>5} ", 42);
    assert_eq!(GUTTER_W, sample.len(), "GUTTER_W must match the gutter format string");
}
```

- [ ] **Step 2: Write the padding-width test**

Add this test after the `gutter_width_matches_format_string` test:

```rust
#[test]
fn diff_highlight_band_fills_to_width() {
    use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    use zoid_core::projection::ChatMsg;

    let width = 80usize;
    let msgs = vec![ChatMsg::ToolResult {
        id: "tc1".into(),
        name: "edit".into(),
        output: "edited f.rs (1 change)".into(),
        is_error: false,
        compacted: false,
        ts: 0,
    }];
    let diff = RenderDiff {
        path: "f.rs".into(),
        added: 1,
        removed: 0,
        truncated_by: 0,
        lines: vec![
            RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: "short".into() },
        ],
    };
    let cache = vec![("tc1".to_string(), diff)];
    let lines = conversation_lines_with_diffs(&msgs, false, false, 0, width, None, &cache, 5);

    // Find the add line (2 spans, first starts with 6 leading spaces, content
    // starts with "+").
    let add_line = lines.iter()
        .find(|l| l.spans.len() == 2 && l.spans[0].content.starts_with("      ") && l.spans[1].content.starts_with('+'))
        .expect("add line present");

    // Total visual width = gutter span width + content span width.
    // The gutter is always GUTTER_W (12) chars. The content span includes
    // the padded spaces, so its width should be width - GUTTER_W.
    let gutter_w = display_width(add_line.spans[0].content.as_ref());
    let content_w = display_width(add_line.spans[1].content.as_ref());
    assert_eq!(gutter_w, GUTTER_W, "gutter width matches GUTTER_W");
    assert_eq!(gutter_w + content_w, width, "total band width fills to ctx.width");
}
```

- [ ] **Step 3: Write the clamp test**

Add this test after the `diff_highlight_band_fills_to_width` test:

```rust
#[test]
fn diff_highlight_clamps_when_too_wide() {
    use crate::state::{RenderDiff, RenderDiffKind, RenderDiffLine};
    use zoid_core::projection::ChatMsg;

    // Width smaller than GUTTER_W + content — pad must saturate to 0, no panic.
    let width = 10usize;
    let msgs = vec![ChatMsg::ToolResult {
        id: "tc1".into(),
        name: "edit".into(),
        output: "edited f.rs".into(),
        is_error: false,
        compacted: false,
        ts: 0,
    }];
    let diff = RenderDiff {
        path: "f.rs".into(),
        added: 1,
        removed: 0,
        truncated_by: 0,
        lines: vec![
            RenderDiffLine { old_no: None, new_no: Some(1), kind: RenderDiffKind::Add, text: "a very long line that exceeds the narrow width".into() },
        ],
    };
    let cache = vec![("tc1".to_string(), diff)];
    // Must not panic — saturating_sub clamps pad to 0.
    let lines = conversation_lines_with_diffs(&msgs, false, false, 0, width, None, &cache, 5);

    // The add line should still render with the correct background.
    let add_line = lines.iter()
        .find(|l| l.spans.len() == 2 && l.spans[0].content.starts_with("      ") && l.spans[1].content.starts_with('+'))
        .expect("add line present");
    assert_eq!(add_line.spans[0].style.bg, Some(color::ADDED_BG), "gutter has add bg even when clamped");
    assert_eq!(add_line.spans[1].style.bg, Some(color::ADDED_BG), "content has add bg even when clamped");
}
```

- [ ] **Step 4: Run all new tests to verify they pass**

Run: `cargo test -p zoid-tui --lib gutter_width_matches_format_string diff_highlight_band_fills_to_width diff_highlight_clamps_when_too_wide`
Expected: PASS (all three)

- [ ] **Step 5: Run the full zoid-tui test suite to verify no regressions**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (all tests, including the existing diff tests from Task 2 Step 6)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/chat.rs
git commit -m "test: add GUTTER_W invariant, padding-width, and clamp tests

- gutter_width_matches_format_string: locks GUTTER_W to the literal
- diff_highlight_band_fills_to_width: verifies band fills to ctx.width
- diff_highlight_clamps_when_too_wide: verifies saturating_sub clamps to 0"
```

---

### Task 4: Visual verification

**Files:** None modified — manual visual check only.

- [ ] **Step 1: Build the TUI binary**

Run: `cargo build -p zoid`
Expected: Compiles without errors or warnings

- [ ] **Step 2: Run the full test suite one final time**

Run: `cargo test -p zoid-tui`
Expected: PASS (all lib + integration tests)

- [ ] **Step 3: Manual visual check (if a terminal is available)**

Run zoid, perform an edit, and verify:
1. Add/del lines have a subtle green/red background band across the full row
2. Context lines have no background tint
3. The band extends to the right edge of the terminal
4. The `+`/`−` sign and text are still colored green/red in the foreground
5. The `…+N more` truncation indicator has no background

If no terminal is available for manual checking, this step is satisfied by the test suite in Step 2.