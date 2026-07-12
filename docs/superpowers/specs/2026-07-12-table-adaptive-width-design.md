# Adaptive table column widths

**Date:** 2026-07-12
**Status:** Design
**Refines:** `2026-07-12-gfm-table-support-design.md` (original GFM table rendering)
**Area:** `crates/zoid-tui/src/markdown.rs`, `crates/zoid-tui/src/chat.rs`

## Problem

Rendered markdown tables use a flat per-column cap, `MAX_COL_W = 30`
(`markdown.rs:21`). Every column is `natural.min(30)`, blind to how many columns
the table has and how much horizontal room the message pane actually offers.

The result: a 2-column table (e.g. `Commit | What`) wraps the `What` column at 30
characters even when half the pane sits empty to its right. The cap protects
against wide tables in narrow terminals but penalizes the common
few-columns / wide-terminal case.

## Goal

Replace the flat cap with adaptive widths driven by the **available content
width**:

- **Fit natural, cap at available.** Each column takes its natural (unwrapped)
  content width; wrapping only begins once the whole table would exceed the
  available width. The table stays as narrow as its content — it does **not**
  stretch to fill the pane (GitHub-style full-width tables are explicitly out of
  scope).
- **Graceful overflow.** When natural widths genuinely exceed the available
  width, shrink columns using **min-floor + widest-first** so narrow columns
  never collapse and wide columns absorb the shrink first.

## Non-goals

- Stretching tables to fill the full pane width.
- Column-width hints from markdown source (alignment is already honored; width
  is derived, not authored).
- Fixing markdown-internal nesting width tracking beyond the message prefix
  indent (see "Known limitation" below).

## Width algorithm

Implemented in `render_table` (`markdown.rs`). Inputs: `natural[c]` (measured as
today), `ncols`, and the new `content_w` (available text-column width).

Constants:

- Remove `MAX_COL_W`.
- Add `MIN_COL_W: usize = 8` (tunable) — the shrink floor.

### Chrome overhead

Each column renders as ` content ` (one pad space each side) separated and
bounded by `ncols + 1` vertical bars. So:

```
table_width = sum(widths) + 2*ncols + (ncols + 1)
            = sum(widths) + 3*ncols + 1
```

This `3*ncols + 1` constant is derived directly from the emit loop
(`markdown.rs` cell-row emission: leading `│`, then per column
`space + content + space`, then a separating/closing `│`). It MUST be defined
once (a named helper or `const`) so the border math and width math cannot drift.

The per-content budget is:

```
budget = content_w.saturating_sub(3*ncols + 1)
```

### Steps

1. **Measure** `natural[c]` (unchanged from today).
2. **Fits naturally?** If `sum(natural) <= budget` → `widths = natural`. No
   wrapping. (This is the primary fix.)
3. **Overflow → min-floor + widest-first.** Otherwise:
   - `overflow = sum(natural) - budget`.
   - Per-column effective floor: `floor[c] = min(natural[c], MIN_COL_W)`. A
     column already narrower than `MIN_COL_W` keeps its natural width — the floor
     bounds how far we shrink, it never inflates a column.
   - Start `widths = natural`. While `overflow > 0` and at least one column sits
     above its floor: decrement the current **widest** above-floor column by 1,
     `overflow -= 1`. (Ties broken by lowest column index for determinism.)
4. **Can't-fit fallback.** If every column reaches its floor and
   `sum(floor) > budget` (very narrow terminal and/or many columns), lay out at
   the floors and let the table overflow horizontally. The viewport clips it.
   Deterministic; no panic; no divide-by-zero.

### Worked examples

`content_w = 60`, table `Commit | What`, `natural = [8, 44]`:

- chrome `= 3*2 + 1 = 7`, `budget = 53`, `sum(natural) = 52 <= 53` → widths
  `[8, 44]`, no wrapping. Table width `= 52 + 7 = 59 <= 60`. ✓

`content_w = 44`, same `natural = [8, 60]`:

- `budget = 44 - 7 = 37`, `sum(natural) = 68`, `overflow = 31`.
- floors `[min(8,8), min(60,8)] = [8, 8]`. Widest is col 1 (`60`); shrink it
  31 → `60 - 31 = 29` (still above floor). widths `[8, 29]`. `Commit` untouched. ✓

`content_w = 18`, `natural = [8, 60]`:

- `budget = 11`, `overflow = 57`. Shrink col 1 to floor 8 (absorbs 52), 5 left;
  shrink col 0 from 8 to floor 8 → cannot (already at floor). Remaining overflow
  5 unabsorbed → widths `[8, 8]`, table width `= 16 + 7 = 23 > 18`. Overflow
  tolerated (fallback). ✓

## Wiring (Approach A: thread `content_w`)

Chosen over deferring table layout into `push_message` (Approach B) to keep the
change contained and the width algorithm in one testable function. Trade-off:
"available width" knowledge spreads to `render_body` callers, and `indent_w` is
computed both at the call site and in `push_message` (minor duplication).

Signature changes:

- `render_body(source: &str)` → `render_body(source: &str, content_w: usize)`.
- `render_markdown(source: &str)` → `render_markdown(source: &str, content_w: usize)`.
- Thread `content_w` into `render_table`.

Call sites (`chat.rs`):

- `:226` (user), `:263` (assistant): compute `content_w = ctx.width - indent_w`
  once from the `prefix` they already build, and pass it to **both**
  `render_body` and `push_message`. (`push_message` continues to compute its own
  `content_w` for prose/code; passing the same value keeps them consistent.)
- `:871` (summary), `:924` (thinking): pass the appropriate available width for
  those surfaces into `render_markdown`.

All other `render_body` / `render_markdown` callers (including tests) pass an
explicit width.

## The indent bug (documented; partially fixed by A)

Today `push_message`'s `BodyKind::Table` branch (`chat.rs:641-646`) prepends the
message indent to a table that was laid out to the **full** width. So any
indented table — continuation lines under a message prefix, or a table nested in
a blockquote/list — already overruns the right edge by `indent_w`. This is a
pre-existing latent bug, independent of the width cap.

Approach A fixes the **message-prefix** case: laying the table out to
`content_w = ctx.width - indent_w` means the indent is already accounted for, so
the prefixed/continuation lines no longer overflow.

**Known limitation:** markdown-*internal* nesting depth (a table inside a list
item, where the indent comes from the markdown structure rather than the message
prefix) is not tracked by `content_w` at the `render_body` boundary. Such tables
may still overflow by their internal indent. Fully solving this would require
Approach B (deferring table layout into the width-aware pass) and is out of scope
for this refinement.

## Testing

`markdown.rs` unit tests (call `render_table` / `render_body` with explicit
`content_w`):

- **Wide width** → natural widths, no wrapping; assert the `What`-style column is
  not wrapped and table width `<= content_w`.
- **Mid width** → widest column shrinks, narrow column unchanged (the
  `[8, 60] @ 44 → [8, 29]` case).
- **Tiny width** → all columns at floor, overflow tolerated, no panic.
- **Chrome math** → a table whose `sum(natural)` exactly equals `budget` fits
  with no wrapping and no off-by-one (guards the `3*ncols + 1` constant).
- **Single-column** and **degenerate/empty** tables still behave (regression).

`chat.rs` test:

- An indented table's emitted lines never exceed `width` (locks the
  message-prefix indent fix).

## Rollout

Single change set; no config, no migration. `MIN_COL_W` is a source constant.
Existing table snapshot tests may need regolding where the previous 30-cap
changed wrapping.
