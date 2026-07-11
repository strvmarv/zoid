# GFM Table Support in the TUI Markdown Renderer

## Context

`crates/zoid-tui/src/markdown.rs` renders assistant/user message bodies from
markdown into ratatui `Line`s. It handles paragraphs, headings, bold/italic/
strikethrough, inline code, links, lists (nested), blockquotes, and fenced code
blocks. **GFM tables are the gap.** As of writing, the parser option
`ENABLE_TABLES` is not set, so pipe-delimited table syntax is not even parsed as
a table — the raw `|` characters are consumed as plain text. The `Builder`'s
`start()`/`end()` switch has a `_ => {}` fallthrough that silently drops any
`Table`/`TableHead`/`TableRow`/`TableCell` tags that did reach it.

This spec adds GFM table rendering: parse, lay out columns from content, style
header and body cells, and draw box-drawing borders consistent with zoid's
existing visual language.

## Non-goals

- Horizontal scrolling or terminal-width-aware column fitting (option B).
  Tables may overflow a narrow terminal; this is accepted under the
  content-driven width model (option C).
- Nested tables (GFM forbids them; pulldown-cmark does not emit them).
- Sorting, selection, or interactivity within a rendered table.

## Decisions (recap of brainstorming)

- **Width model: C — content-measured with wrap-at-cap.** Each column is sized
  to its widest cell, capped at a maximum. Cells exceeding the cap wrap to
  multiple visual rows within their column. No terminal-width coupling; the
  `render_body` → `BodyLine` seam stays width-free, matching the existing
  architecture (code lines are width-laid in `push_message`, not `render_body`).
- **Column cap: 30 chars.** Each column's width is `min(measured_widest_cell,
  30)`. Beyond that a cell wraps.
- **Full inline formatting in cells.** Cells support `**bold**`, `*italic*`,
  `` `code` ``, `[links]()`, `~~strike~~` — the same inline styles as the rest
  of markdown, via the same flag machinery the `Builder` already maintains.

## Parser API (pulldown-cmark 0.13.4)

The crate version is fixed at 0.13.4 (`Cargo.lock`). The table event sequence
for a simple 2×2 table:

```
Start(Table([Left, Left]))          // AlignmentVector: one entry per column
  Start(TableHead)
    Start(TableCell) → Text("H1")  → End(TableCell)
    Start(TableCell) → Text("H2")  → End(TableCell)
  End(TableHead)
  Start(TableRow)
    Start(TableCell) → Text("a")   → End(TableCell)
    Start(TableCell) → Text("b")   → End(TableCell)
  End(TableRow)
End(Table)
```

- `Table(AlignmentVector)` carries per-column alignment (`Left`, `Center`,
  `Right`, `None`), in source order. We use it for cell padding.
- Cell content arrives as **nested events** between `Start(TableCell)` and
  `End(TableCell)` — not a flat string. A cell with `` `**key**` value `` emits
  `Start(Strong)`/`Text`/`End(Strong)`/`Text` inside the cell. We must collect
  styled spans, not raw text.

## §1 — Parsing: enable tables, route events

**Enable the option** in `render_body`:

```rust
let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
Parser::new_ext(source, opts)
```

**Table mode.** The `Builder` gains table-accumulation state. On `Start(Table)`,
the builder enters table mode: it records the `AlignmentVector` and begins
collecting cells into a grid (rows × columns). All `Text`/`Code`/`Start`/`End`
events between `Start(Table)` and `End(Table)` are routed to table handling, not
the prose `cur` buffer. On `End(Table)`, the accumulated grid is laid out (§2)
and flushed as `BodyKind::Table` lines appended to `self.lines`.

The table accumulator is distinct from the prose path (`cur`, `bold`, `italic`,
… flag state). It reuses the *flag-handling logic* (see §4) but not the buffer.

**Tag handling additions** in `start()`:

| Tag | Action |
|-----|--------|
| `Table(alns)` | Enter table mode; store alignments; reset grid |
| `TableHead` | Note header-section start (rows collected until `End(TableHead)` are header rows) |
| `TableRow` | Begin a new body row |
| `TableCell` | Begin accumulating a new cell's spans |

And in `end()`:

| TagEnd | Action |
|--------|--------|
| `Table` | Lay out grid (§2); flush as `BodyKind::Table` lines; exit table mode |
| `TableHead` | Close the header row |
| `TableRow` | Close the body row |
| `TableCell` | Close the cell (finalize span accumulator) |

The existing `block_sep()` is called on `Start(Table)` so a table gets
breathing room from the preceding block, consistent with paragraphs/headings/
code blocks.

## §2 — Layout: content-measured columns with wrap-at-cap

On `End(Table)`, the builder has a grid: `Vec<TableRowData>` where each row is
`Vec<Vec<Span>>` (a row of cells, each cell a list of styled spans), plus a flag
distinguishing header rows from body rows, plus the per-column alignment vector.

**Algorithm:**

1. **Measure.** For each column `c`, compute `natural_w[c] = max over all cells
   in column c of display_width(cell_spans)`. Display width uses `unicode-width`
   (already a dependency) so CJK/double-width chars measure correctly. The
   header row is included in the measurement.

2. **Cap.** `width[c] = min(natural_w[c], MAX_COL_W)` where `MAX_COL_W = 30`.

3. **Wrap overlong cells.** For each cell, if its display width exceeds
   `width[c]`, wrap it to multiple visual rows within `width[c]`. Wrapping is
   word-based (break on spaces), falling back to a hard character split for a
   single token longer than `width[c]`. This mirrors the existing `wrap_content`
   logic but operates on `Vec<Span>` rather than the prose path — the helper is
   generalized or a sibling is added (see §6). Each cell becomes
   `Vec<Vec<Span>>` — its wrapped visual rows.

4. **Row height.** A table-row's visual height is
   `max over its cells of wrapped_cell.len()`. The row renders that many visual
   lines; cells that wrapped to fewer rows are padded with blank rows (empty
   span lines of `width[c]` spaces).

5. **Pad cells to column width.** Each visual row of each cell is padded with
   spaces to `width[c]`, alignment-aware:
   - `Left` / `None`: content left, pad right.
   - `Right`: content right, pad left.
   - `Center`: split pad, left-biased for odd remainder.

6. **Emit grid lines.** For each visual row of each table-row, concatenate:
   `[left-border] [pad] cell [pad] [sep] [pad] cell [pad] ... [right-border]`.
   Borders are box-drawing glyphs (§3). Each emitted visual line is one
   `BodyLine { kind: BodyKind::Table, source: None }`.

7. **Separator lines.** A horizontal border line is emitted before the first
   row (top border), between the header row(s) and the first body row (header
   separator, GFM convention), and after the last row (bottom border). Vertical
   `│` separators run through every body line.

**Wide-table overflow.** If the sum of column widths plus borders exceeds the
terminal width, the table overflows. This is accepted (option C, non-goal). No
horizontal scrolling in v1. `push_message`'s `BodyKind::Table` arm (§3) does not
re-wrap table lines.

## §3 — Visual style & a new BodyKind

### New `BodyKind`

Add `BodyKind::Table`. In `push_message`, the new arm emits the line with the
`lead`/`indent` prefix (so tables inside lists/blockquotes still indent) but:

- **No wrapping** — table lines are pre-laid-out; re-wrapping would break
  column alignment. This mirrors how `BodyKind::Code` passes through untouched.
- **No code-panel padding** — table lines do not get the `CODE_BG` background or
  `CODE_BAR` left rule.

```rust
BodyKind::Table => {
    open = None;
    out.push(Line::from({ let mut s = lead; s.extend(line.spans); s }));
}
```

A `BodyKind::Table` line also closes any open code block (sets `open = None`),
matching `Prose`.

### Visual tokens

New entries in `tokens.rs`, keeping the single-source-of-truth pattern:

```rust
// glyph
pub const TABLE_H: char = '─';      // horizontal border
pub const TABLE_V: char = '│';      // vertical separator
pub const TABLE_TL: char = '┌';
pub const TABLE_TR: char = '┐';
pub const TABLE_BL: char = '└';
pub const TABLE_BR: char = '┘';
pub const TABLE_LT: char = '├';     // left tee
pub const TABLE_RT: char = '┤';     // right tee
pub const TABLE_TT: char = '┬';     // top tee
pub const TABLE_BT: char = '┴';     // bottom tee
pub const TABLE_CR: char = '┼';     // cross

// color
pub const TABLE_BORDER: Color = DIM;        // = color::DIM
pub const TABLE_HEADER: Color = CHAT_ACCENT; // = color::CHAT_ACCENT
```

### Styling

- **Borders** (top/bottom/separator lines, vertical `│`): `color::TABLE_BORDER`
  (`DIM`).
- **Header cells**: `color::TABLE_HEADER` (`CHAT_ACCENT`) + `BOLD` — same
  treatment as `# Headings`, reinforcing that headers are emphasis.
- **Body cells**: `color::TXT`, with inline formatting applied per §4.
- **Header separator**: a `TABLE_H` line in `TABLE_BORDER` between the header
  row(s) and first body row. (Single-weight `─`, not double-line `═` — stays
  within the existing light box-drawing set already used by `QUOTE_BAR`/`CODE_BAR`.)

A table with one header row and two body rows renders as:

```
┌──────┬──────┐
│ H1   │ H2   │     ← header: accent + bold
├──────┼──────┤
│ a    │ b    │     ← body: TXT + inline styles
│ long │ c    │     ← wrapped row (cell 1 overflowed the 30-cap)
└──────┴──────┘
```

## §4 — Inline formatting in cells

Cell content arrives as nested pulldown-cmark events. A cell is not a flat
string — it is a sequence of styled spans. The design reuses the `Builder`'s
existing inline-style flag state (`bold`, `italic`, `code`, `link`, `strike`)
and `style()` method:

- **`Start(TableCell)`** opens a fresh cell span-accumulator
  (`Vec<Span<'static>>`) on the current table row.
- **Intervening events** populate that accumulator:
  - `Text(t)` → push `Span::styled(t, self.style())` (same as the prose `text()`).
  - `Code(c)` → set `self.code = true`, push the text, unset.
  - `Start(Strong)`/`End(Strong)` → toggle `self.bold` (etc. for
    `Emphasis`/`Link`/`Strikethrough`).
- **`End(TableCell)`** finalizes the cell's span list onto the current row.

The inline-style flags (`bold`, `italic`, `code`, `link`, `strike`) are
**shared storage** with the prose path — well-formed GFM produces balanced
events, so a `Start(Strong)` inside a cell is always matched by an `End(Strong)`
before `End(TableCell)`. The protection against a malformed cell leaving flags
stuck is the existing `bail` mechanism (§5): if the table parse produces an
imbalance, the whole message falls back to `plain_lines`.

The `heading` and `quote` flags are **not consulted** by the cell-accumulating
path — a cell inside a table is never a heading, and table-cell content is not
blockquote-dimmed (the table has its own borders). The cell path computes its
style via a variant of `style()` that reads only the five inline flags and
forces `fg = TXT` (body) or `fg = TABLE_HEADER` (header) and bold for header
rows — so the shared `heading`/`quote` counters cannot bleed a tint into cells.
If a table appears *inside* a blockquote (legitimate nesting), the quote-bar
indent is applied by `push_message`'s lead/indent logic (§3); the cell text
itself stays full `TXT`/accent, not dimmed.

**Header-row styling** is applied at emit time (§2 step 6 / §3): the grid
carries a per-row `is_header` flag (set by `TableHead`/`TableRow` routing in §1).
When emitting a header row's cells, the layout substitutes `TABLE_HEADER` +
`BOLD` for the base `TXT`. This keeps measurement/padding/wrapping identical
for header and body cells and localizes the visual difference to the emit step.

**Alignment** (from `AlignmentVector`) affects only §2 step 5 — the padding
around cell content. It does not change the content's internal styling.

## §5 — Edge cases & error handling

- **Malformed tables** (missing separator row, unbalanced pipes): pulldown-cmark
  is lenient and parses what it can. We render whatever grid it yields. No
  special-casing.
- **Flag imbalance** (a `Start(Strong)` without a matching `End`): the existing
  `bail` path fires. A table in `bail` state drops the *entire message* to
  `plain_lines`, consistent with the current over-nesting fallback. Tables never
  produce a panic.
- **Empty cells**: render as blank space padded to `width[c]`. No special path.
- **Tables inside lists/blockquotes**: legitimate nesting. The table is laid out
  from content (width independent of nesting depth), and `push_message` applies
  the quote-bar/indent prefix via the `BodyKind::Table` arm's `lead`. A deeply-
  nested table may overflow more in a narrow margin — accepted under option C.
  `Start(Table)` inside a quote/list bumps the depth counter for the `MAX_DEPTH`
  check (a table counts as one nesting level), so pathological table-in-deep-
  quote nesting still bails to `plain_lines` past `MAX_DEPTH` (8).
- **Cell width measurement**: `unicode-width` (already a dependency, used for
  `indent_w` in `push_message`). Double-wide CJK and combining chars measure
  consistently with the rest of zoid's width math.
- **0-row / 0-column tables**: if pulldown-cmark yields an empty grid, `End(Table)`
  emits no lines (no border, no header) — the table vanishes cleanly rather than
  printing an empty box.
- **Single-column tables**: render as a bordered list (two `│` borders, one
  column). No special-casing; the general algorithm handles it.

## §6 — Implementation surface (summary)

All changes are additive and contained to two files plus `tokens.rs`:

1. **`crates/zoid-tui/src/markdown.rs`** — the bulk of the work:
   - Add `ENABLE_TABLES` to the parser options.
   - Add table-accumulator fields to `Builder` (grid, alignments, current row,
     current cell spans, in-header flag, in-table flag).
   - Add `Table`/`TableHead`/`TableRow`/`TableCell` arms to `start()` and `end()`.
   - Add the §2 layout algorithm (measure → cap → wrap → pad → emit) as a
     method invoked on `End(Table)`.
   - Generalize the prose `wrap_content` helper (or add a sibling) to wrap a
     `Vec<Span>` to a given width, returning `Vec<Vec<Span>>` — used for cell
     wrapping. The existing `wrap_content` in `chat.rs` stays (it serves
     `push_message`); the markdown.rs helper is a width-only span wrapper.
   - Add `BodyKind::Table`.
   - New tests (§7).

2. **`crates/zoid-tui/src/chat.rs`** — minimal:
   - Add the `BodyKind::Table` arm to `push_message` (emit with lead/indent,
     no wrap, no code padding, closes open code block).

3. **`crates/zoid-tui/src/tokens.rs`** — additive:
   - `glyph::TABLE_*` and `color::TABLE_BORDER` / `TABLE_HEADER` constants.
   - A test asserting the new tokens, matching the `markdown_tokens_present` /
     `code_container_tokens_present` pattern.

No changes to `zoid-syntax`, `zoid-core`, or the parser crate version.

## §7 — Testing

New tests in `markdown.rs`, following the existing `render_markdown` → assert-
on-`Line`/`Span` pattern:

1. **`simple_two_by_two_table_renders_bordered_grid`** — a 2-header, 2-body-row
   table renders a top border, a header row, a separator, body rows, and a
   bottom border. Asserts the presence of `TABLE_H`/`TABLE_V` glyphs and that
   the joined text contains both header and body cell text.

2. **`header_cells_are_accent_bold`** — header cell spans carry
   `color::TABLE_HEADER` and `Modifier::BOLD`; body cell spans are `color::TXT`
   without bold.

3. **`inline_bold_and_code_in_cells_are_styled`** — a cell containing
   `` `**k**` v `` produces a bold span and a `MD_CODE`-colored span within the
   same cell, proving nested-event collection works.

4. **`wide_cell_wraps_within_column_cap`** — a cell whose content exceeds
   `MAX_COL_W` (30) wraps to a second visual row; the row's visual height is 2;
   the wrapped text appears in the output.

5. **`column_width_is_widest_cell_capped_at_30`** — a column with cells
   `"short"`, `"a much longer cell than thirty chars"` measures to 30 (capped),
   not the full natural width.

6. **`alignment_left_center_right_pads_correctly`** — a column marked `:---:`
   centers its content, `---:` right-aligns (pad-left), `:---` left-aligns
   (pad-right). Asserts leading/trailing space distribution.

7. **`table_inside_blockquote_keeps_quote_bar`** — a table inside `>` renders
   with the `QUOTE_BAR` prefix (via `push_message`'s lead), and the cell content
   is full `TXT` (not dimmed), proving the quote flag does not bleed into cells.

8. **`malformed_table_does_not_panic`** — a broken table (unbalanced pipes,
   missing separator) renders *something* without panicking; the message body
   is still produced (either a best-effort table or a `plain_lines` fallback).

9. **`empty_table_emits_nothing`** — a degenerate/empty table emits no lines
   (no empty box).

10. **`plain_text_outside_table_still_renders`** — prose before and after a
    table renders normally, proving the table-mode enter/exit does not corrupt
    the prose path's `cur`/flag state.

A snapshot test (insta) in `crates/zoid-tui/tests/` locks the visual
layout of a representative table, matching the existing snapshot convention
(`shell_snapshot.rs`).
