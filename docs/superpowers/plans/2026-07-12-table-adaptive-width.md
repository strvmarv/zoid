# Adaptive Table Column Widths Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat 30-char per-column table cap with width-aware column sizing so few-column tables use the room they have and only wrap when the table would actually overflow.

**Architecture:** `crates/zoid-tui/src/markdown.rs` renders markdown tables at parse time, today capping every column at `MAX_COL_W = 30` with no knowledge of terminal width. We thread the available content width (`content_w`) from `chat.rs` — which already knows it — into `render_body` / `render_markdown` / `render_table`, and replace the flat cap with a budget-based algorithm: columns keep natural width when the table fits, otherwise the widest columns shrink first down to a `MIN_COL_W` floor. Split into a behavior-preserving plumbing task (Task 1) and the algorithm swap + tests (Task 2) so the refactor and the behavior change are independently reviewable.

**Tech Stack:** Rust, ratatui (`Line`/`Span`), `unicode-width` (`UnicodeWidthStr::width`), pulldown-cmark.

## Global Constraints

- Never add a `Co-Authored-By` or any co-author trailer to commit messages (user CLAUDE.md).
- Column display widths are measured with `unicode_width::UnicodeWidthStr::width`, never `.len()` (matches existing `render_table`).
- Table chrome width is `3 * ncols + 1` (one pad space each side of every column = `2*ncols`, plus `ncols + 1` vertical bars). Define it once; the border math and the width math must never disagree.
- `MIN_COL_W = 8` is the shrink floor; a column already narrower than it keeps its natural width (the floor bounds shrinking, it never inflates).
- Tables do **not** stretch to fill the pane — a table stays as narrow as its content when it fits.
- Tie-break when picking the widest column to shrink: lowest column index wins (deterministic).

---

### Task 1: Thread `content_w` through the render entry points (behavior-preserving)

Add the `content_w` parameter to `render_body`, `render_markdown`, and `render_table`, and update every call site. **Do not change layout yet** — `render_table` still uses `MAX_COL_W`, so all existing output is byte-identical. This isolates the mechanical signature churn from the behavior change in Task 2.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs:56` (`render_body`), `:68` (`render_markdown`), `:607` (`render_table` signature only), and all test call sites in the same file.
- Modify: `crates/zoid-tui/src/chat.rs:226`, `:263` (assistant/user), `:871` (summary), `:924` (thinking).

**Interfaces:**
- Produces:
  - `pub fn render_body(source: &str, content_w: usize) -> Vec<BodyLine>`
  - `pub fn render_markdown(source: &str, content_w: usize) -> Vec<Line<'static>>`
  - `fn render_table(&mut self, table: TableAccum, content_w: usize)` (private; `content_w` is accepted but unused this task)

- [ ] **Step 1: Change the three signatures in `markdown.rs`**

`render_body` (line 56) and `render_markdown` (line 68):

```rust
pub fn render_body(source: &str, content_w: usize) -> Vec<BodyLine> {
    let mut b = Builder::default();
    b.content_w = content_w;
    for ev in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        b.event(ev);
        if b.bail {
            return plain_lines(source);
        }
    }
    b.finish()
}

/// Flatten [`render_body`] to plain `Line`s (drops the per-line kind).
pub fn render_markdown(source: &str, content_w: usize) -> Vec<Line<'static>> {
    render_body(source, content_w).into_iter().map(|b| b.line).collect()
}
```

Add a `content_w` field to the `Builder` struct. `Builder` uses `#[derive(Default)]` (line 267), so the field defaults to `0` and `render_body` overwrites it before any event is processed (shown above as `b.content_w = content_w`) — no manual `Default` impl needed. Add the field after `table: Option<TableAccum>,` (line 282):

```rust
    /// Available content width for width-aware table layout. Set by
    /// `render_body` before the first event; `0` for a bare `Builder::default()`.
    content_w: usize,
```

At the `render_table` call in the `TagEnd::Table` handler (line 596-598), pass the stored width:

```rust
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.render_table(t, self.content_w);
                }
            }
```

And change the `render_table` signature (line 607):

```rust
fn render_table(&mut self, table: TableAccum, content_w: usize) {
```

Leave the body unchanged (it still computes `let cap = MAX_COL_W;` etc.). `content_w` is unused this task — add `let _ = content_w;` at the top of the function to silence the warning, with a comment `// used in Task 2`.

- [ ] **Step 2: Update every `markdown.rs` test call site**

Add a test width constant at the top of the `mod tests` block:

```rust
/// Wide enough that width-agnostic tests never trigger table wrapping.
const TEST_W: usize = 80;
```

Then update all `render_markdown(<x>)` → `render_markdown(<x>, TEST_W)` and `render_body(<x>)` → `render_body(<x>, TEST_W)` in the test module. There are ~30 sites (lines 754–1170). Do this mechanically, e.g.:

```bash
# Preview first, then apply. Only inside the tests module.
grep -n 'render_markdown(\|render_body(' crates/zoid-tui/src/markdown.rs
```

Apply with `sed` scoped to the test region, then eyeball the diff — the `render_body`/`render_markdown` *definitions* (lines 56, 68) already take the new arg and must NOT be rewritten by the sed:

```bash
sed -i '90,$ s/render_markdown(\(.*\))/render_markdown(\1, TEST_W)/; 90,$ s/render_body(\(.*\))/render_body(\1, TEST_W)/' crates/zoid-tui/src/markdown.rs
```

(The `90,$` range starts below the definitions. Review every changed line; fix any double-wrap or the two cap tests, which Task 2 rewrites anyway.)

- [ ] **Step 3: Update the four `chat.rs` call sites**

Assistant/user messages (`chat.rs:222-228` and `:259-265`) — compute `content_w` from the prefix the code already builds, and pass it to `render_body`. `push_message` keeps taking `ctx.width` (it computes its own indent internally):

```rust
// user branch (was: render_body(text),)
let indent_w: usize = prefix.iter().map(|s| s.content.width()).sum();
let content_w = ctx.width.saturating_sub(indent_w).max(1);
push_message(
    &mut lines,
    &mut code_ranges,
    prefix,
    render_body(text, content_w),
    ctx.width,
);
```

Do the same in the assistant branch (line 263), but note it renders `&shown` (which is `text.clone()` possibly with a streaming caret appended), **not** `text` — preserve `&shown`:

```rust
// assistant branch (was: render_body(&shown),)
let indent_w: usize = prefix.iter().map(|s| s.content.width()).sum();
let content_w = ctx.width.saturating_sub(indent_w).max(1);
push_message(
    &mut lines,
    &mut code_ranges,
    prefix,
    render_body(&shown, content_w),
    ctx.width,
);
```

Ensure `use unicode_width::UnicodeWidthStr;` is in scope in `chat.rs` (it is already used by `push_message`).

Summary (`chat.rs:871`) and thinking (`chat.rs:924`) both indent by a literal `"    "` (4 cols) and have `width` in scope:

```rust
for line in crate::markdown::render_markdown(summary, width.saturating_sub(4)) {
```
```rust
for line in crate::markdown::render_markdown(thinking_text, width.saturating_sub(4)) {
```

- [ ] **Step 4: Build and run the full TUI test suite — expect no behavior change**

Run: `cargo test -p zoid-tui`
Expected: PASS. Because `render_table` still uses `MAX_COL_W`, every existing table test (including `wide_cell_wraps_within_column_cap` and `column_width_is_widest_cell_capped_at_30`) passes unchanged. If a table test fails, the sed touched layout — revert and redo.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs crates/zoid-tui/src/chat.rs
git commit -m "refactor(tui): thread content_w into markdown table rendering (no behavior change)"
```

---

### Task 2: Budget-based adaptive column widths + indent regression test

Replace the flat `MAX_COL_W` cap with the budget/floor/widest-first algorithm. Add a pure `table_col_widths` helper (directly unit-testable), regold the two cap-coupled tests, add adaptive-width tests, and lock the message-prefix indent fix with a `chat.rs` test.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs:19-21` (constants), `:636-638` (width computation in `render_table`), add helper fns near `align_rows` (~line 115), update tests at `:1037-1077`, add new tests.
- Test: `crates/zoid-tui/src/markdown.rs` (`mod tests`), `crates/zoid-tui/src/chat.rs` (`mod tests`).

**Interfaces:**
- Consumes (from Task 1): `content_w: usize` param on `render_table`; `TEST_W` const in `markdown.rs` tests.
- Produces:
  - `const MIN_COL_W: usize = 8;`
  - `fn table_chrome_w(ncols: usize) -> usize`
  - `fn table_col_widths(natural: &[usize], content_w: usize) -> Vec<usize>`

- [ ] **Step 1: Write the failing pure-function unit tests**

Add to `mod tests` in `markdown.rs`:

```rust
#[test]
fn col_widths_fit_naturally_when_room() {
    // sum(natural)=52, chrome=7, budget=60-7=53 → no shrink.
    assert_eq!(table_col_widths(&[8, 44], 60), vec![8, 44]);
}

#[test]
fn col_widths_shrink_widest_first() {
    // natural [8,60], content_w=44, chrome=7, budget=37, overflow=31.
    // Widest (col 1) absorbs all of it: 60-31=29; narrow col untouched.
    assert_eq!(table_col_widths(&[8, 60], 44), vec![8, 29]);
}

#[test]
fn col_widths_never_shrink_below_floor_and_tolerate_overflow() {
    // content_w=18, chrome=7, budget=11. Both floors=8; can't fit → [8,8].
    assert_eq!(table_col_widths(&[8, 60], 18), vec![8, 8]);
}

#[test]
fn col_widths_do_not_inflate_already_narrow_columns() {
    // Columns narrower than MIN_COL_W keep their natural width when it fits.
    assert_eq!(table_col_widths(&[3, 3], 100), vec![3, 3]);
}

#[test]
fn col_widths_chrome_math_is_exact() {
    // budget == sum(natural): [10,10], chrome=7 → content_w=27 fits with no wrap.
    assert_eq!(table_col_widths(&[10, 10], 27), vec![10, 10]);
    // one tighter must force a 1-col shrink of the (tied) widest = col 0.
    assert_eq!(table_col_widths(&[10, 10], 26), vec![9, 10]);
}
```

- [ ] **Step 2: Run them to verify they fail (no such function)**

Run: `cargo test -p zoid-tui col_widths`
Expected: FAIL to compile — `cannot find function 'table_col_widths'`.

- [ ] **Step 3: Add the constants and helper functions**

In `markdown.rs`, replace the `MAX_COL_W` const (lines 19-21) with:

```rust
/// Minimum column content width we shrink to before tolerating horizontal
/// overflow. A column already narrower than this keeps its natural width.
const MIN_COL_W: usize = 8;
```

Add near `align_rows` (~line 115):

```rust
/// Non-content width a table spends on chrome for `ncols` columns: one pad
/// space on each side of every column (`2*ncols`) plus `ncols + 1` vertical
/// bars. Must stay consistent with `border_line`'s geometry (which derives the
/// same `Σw + 3*ncols + 1` independently — they are not coupled in code).
fn table_chrome_w(ncols: usize) -> usize {
    3 * ncols + 1
}

/// Per-column display widths given each column's natural (unwrapped) content
/// width and the available content width `content_w`. Columns keep their
/// natural width when the whole table fits; otherwise the widest columns
/// shrink first, never below `MIN_COL_W` (or their natural width if already
/// narrower). If even all floors overflow the budget, the floors are returned
/// and the caller tolerates horizontal overflow (the viewport clips).
fn table_col_widths(natural: &[usize], content_w: usize) -> Vec<usize> {
    let ncols = natural.len();
    let budget = content_w.saturating_sub(table_chrome_w(ncols));
    let mut widths = natural.to_vec();
    let total: usize = widths.iter().sum();
    if total <= budget {
        return widths;
    }
    let mut overflow = total - budget;
    let floor: Vec<usize> = natural.iter().map(|&w| w.min(MIN_COL_W)).collect();
    while overflow > 0 {
        // Widest column still above its floor; lowest index wins ties.
        let mut target: Option<usize> = None;
        for c in 0..ncols {
            if widths[c] > floor[c] && target.map_or(true, |t| widths[c] > widths[t]) {
                target = Some(c);
            }
        }
        match target {
            Some(c) => {
                widths[c] -= 1;
                overflow -= 1;
            }
            None => break, // every column at floor: tolerate overflow
        }
    }
    widths
}
```

- [ ] **Step 4: Use the helper in `render_table`**

In `render_table` (the Step 2 block, lines 636-638), replace:

```rust
        // --- Step 2: cap ---
        let cap = MAX_COL_W;
        let widths: Vec<usize> = natural.iter().map(|&w| w.min(cap)).collect();
```

with:

```rust
        // --- Step 2: adaptive widths (fit natural, else shrink widest first) ---
        let widths: Vec<usize> = table_col_widths(&natural, content_w);
```

Remove the `let _ = content_w;` line added in Task 1 (it is now used).

- [ ] **Step 5: Run the pure-function tests — expect PASS**

Run: `cargo test -p zoid-tui col_widths`
Expected: PASS (all five tests).

- [ ] **Step 6: Regold the two cap-coupled tests**

Replace `wide_cell_wraps_within_column_cap` (lines 1037-1054) — a 40-char cell only wraps when the width forces it, so drive it with a narrow `content_w`:

```rust
#[test]
fn wide_cell_wraps_when_over_budget() {
    // A 40-char cell wraps only when the available width can't hold it.
    let long = "x".repeat(40);
    let md = format!("| {} |\n| --- |\n| {} |\n", long, long);
    let lines = render_markdown(&md, 30); // budget 30-4=26 < 40 → wraps
    let x_rows: Vec<&ratatui::text::Line> = lines
        .iter()
        .filter(|l| l.spans.iter().any(|s| s.content.contains('x')))
        .collect();
    assert!(x_rows.len() >= 2, "expected wrapping (>=2 x-rows), got {}", x_rows.len());
}
```

Replace `column_width_is_widest_cell_capped_at_30` (lines 1056-1077) with a floor/no-wrap pair:

```rust
#[test]
fn wide_cell_does_not_wrap_when_width_allows() {
    // Same 40-char cell, but a wide pane keeps it on one row (no cap anymore).
    let long = "a".repeat(40);
    let md = format!("| {} |\n| --- |\n| {} |\n", long, long);
    let body = render_body(&md, 80); // budget 80-4=76 >= 40 → no wrap
    let max_content_w = body
        .iter()
        .flat_map(|b| b.line.spans.iter())
        .filter(|s| !s.content.chars().all(|c| c == '─') && !s.content.chars().all(|c| c == '│'))
        .map(|s| s.content.width())
        .max()
        .unwrap_or(0);
    assert_eq!(max_content_w, 40, "wide pane must not wrap the 40-char cell");
}
```

- [ ] **Step 7: Add a two-column no-wrap integration test**

```rust
#[test]
fn few_columns_use_available_width_no_wrap() {
    // The screenshot case: 2 columns, plenty of width → the wide column keeps
    // its natural width and does not wrap at 30.
    let md = "| Commit | What |\n| --- | --- |\n| 9856d34 | Registry entry plus fifty two model ids and thirty nine caps |\n";
    let body = render_body(md, 100);
    // The 'What' cell is ~58 chars; with content_w=100 it must stay one row.
    let what_row_count = body
        .iter()
        .filter(|b| b.line.spans.iter().any(|s| s.content.contains("Registry entry")))
        .count();
    assert_eq!(what_row_count, 1, "wide 'What' column must not wrap");
    // And no rendered line exceeds content_w.
    for b in &body {
        let w: usize = b.line.spans.iter().map(|s| s.content.width()).sum();
        assert!(w <= 100, "table line exceeded content_w: {w}");
    }
}
```

- [ ] **Step 8: Add the `chat.rs` indent regression test**

In `chat.rs` `mod tests` (reuse the existing `view` helper at line 1153):

```rust
#[test]
fn indented_table_never_exceeds_width() {
    // Regression: an assistant-message table must fit within `width` including
    // the "HH:MM zoid " prefix indent (the indent-overflow bug).
    let width = 50;
    let msgs = vec![ChatMsg::Assistant {
        thinking: None,
        text: "| Commit | What |\n| --- | --- |\n| 9856d34 | Registry entry plus fifty two model ids and thirty nine caps |\n".into(),
        tool_calls: vec![],
        ts: 0,
    }];
    let lines = conversation_view(&msgs, &view(Zoom::Normal), false, width, None, &[], 0);
    for l in &lines {
        let w: usize = l.spans.iter().map(|s| s.content.width()).sum();
        assert!(w <= width, "line exceeds width {width}: got {w}");
    }
}
```

- [ ] **Step 9: Run the full suite**

Run: `cargo test -p zoid-tui`
Expected: PASS — including the pre-existing degenerate-table guards `empty_table_emits_nothing` and `table_cell_text_does_not_leak_as_prose` (they cover the `render_table` early returns at `markdown.rs:609`/`620`; confirm they still pass rather than adding a new test). Then confirm no stray `MAX_COL_W` references remain:

Run: `grep -rn MAX_COL_W crates/`
Expected: no output.

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs crates/zoid-tui/src/chat.rs
git commit -m "feat(tui): adaptive table column widths (fit natural, shrink widest first)"
```

---

## Notes / open question for review

- **Perf of `table_col_widths`:** the shrink loop is `O(overflow * ncols)`. `overflow = Σnatural − budget`, so it is bounded by the total natural content width, **not** by terminal width — a single huge unbroken cell could drive many decrements per render. This does not change the complexity class, because `wrap_content` and the natural-width measurement already do `O(content)` work on that same cell, so the loop is dominated by work the renderer already does. Acceptable for a TUI; left simple deliberately (a one-pass widest-first distribution would be `O(ncols log ncols)` but is YAGNI here).
- **Wrapper vs. signature change:** this plan changes the `render_body` / `render_markdown` signatures outright (spec-faithful, no lingering unbounded-width default). The cost is ~30 mechanical test-site edits in Task 1. The alternative — keep zero-arg wrappers delegating to `*_w` variants with a default width — avoids that churn but leaves an ambiguous default. Flagging for the reviewer.
- **Known limitation (from spec):** Task 2 fixes the message-prefix indent overflow, but a table nested inside a markdown list item still isn't width-tracked at the `render_body` boundary. Out of scope; would require deferring table layout into `push_message` (Approach B).
