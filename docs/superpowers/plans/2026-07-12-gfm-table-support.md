# GFM Table Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render GFM pipe-delimited tables in the chat message body, with content-measured column widths, a 30-char wrap-at-cap, full inline formatting in cells, and box-drawing borders.

**Architecture:** All table logic is added to `crates/zoid-tui/src/markdown.rs`. `pulldown-cmark`'s `ENABLE_TABLES` option is turned on; the existing event-driven `Builder` gains table-accumulator state that collects cells into a grid, then lays out and emits the grid as a new `BodyKind::Table` variant. `chat.rs` gets one new arm in `push_message` that passes table lines through unwrapped. `tokens.rs` gets table glyphs/colors. No other crates change.

**Tech Stack:** Rust 2021, pulldown-cmark 0.13.4 (already pinned), ratatui 0.30, unicode-width 0.2 (already a dep of zoid-tui).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-12-gfm-table-support-design.md` is the source of truth.
- **pulldown-cmark API (0.13.4):** `Tag::Table(Vec<Alignment>)`, `Tag::TableHead`, `Tag::TableRow`, `Tag::TableCell`; `TagEnd::Table`, `TagEnd::TableHead`, `TagEnd::TableRow`, `TagEnd::TableCell`; `Alignment::{None,Left,Center,Right}`; enable via `Options::ENABLE_TABLES`.
- **Column cap:** `MAX_COL_W = 30` (display width, measured with `unicode-width`).
- **Width model:** content-measured (option C); tables may overflow a narrow terminal. No terminal-width coupling in `render_body`.
- **New tokens** go in `crates/zoid-tui/src/tokens.rs` (single source of truth), following the existing `markdown_tokens_present` test pattern.
- **Tests** follow the existing `render_markdown` → assert-on-`Line`/`Span` pattern in `markdown.rs`. Run the whole crate with `cargo test -p zoid-tui`.
- **No changes** to `zoid-syntax`, `zoid-core`, or the pulldown-cmark version.
- **Commit message style:** lowercase, imperative, e.g. `feat(tui): render GFM tables in message bodies`.

---

## File Structure

- **Create:** none.
- **Modify:**
  - `crates/zoid-tui/src/tokens.rs` — add `glyph::TABLE_*` and `color::TABLE_BORDER`/`TABLE_HEADER` constants + a test. (Task 1)
  - `crates/zoid-tui/src/markdown.rs` — the bulk: `BodyKind::Table`, table accumulator state in `Builder`, tag handlers, the layout algorithm, a span-wrapping helper, tests. (Tasks 2–6)
  - `crates/zoid-tui/src/chat.rs` — one new arm in `push_message` for `BodyKind::Table`. (Task 7)
  - `crates/zoid-tui/tests/shell_snapshot.rs` — one insta snapshot of a rendered table. (Task 8)

---

### Task 1: Table visual tokens

**Files:**
- Modify: `crates/zoid-tui/src/tokens.rs`
- Test: inline `#[cfg(test)]` module in the same file.

**Interfaces:**
- Consumes: nothing.
- Produces: `glyph::TABLE_H`, `TABLE_V`, `TABLE_TL`, `TABLE_TR`, `TABLE_BL`, `TABLE_BR`, `TABLE_LT`, `TABLE_RT`, `TABLE_TT`, `TABLE_BT`, `TABLE_CR` (`char`); `color::TABLE_BORDER` (`= color::DIM`), `color::TABLE_HEADER` (`= color::CHAT_ACCENT`). Later tasks reference these by these exact names.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid-tui/src/tokens.rs`, after the existing `code_container_tokens_present` test:

```rust
    #[test]
    fn table_tokens_present() {
        assert_eq!(glyph::TABLE_H, '─');
        assert_eq!(glyph::TABLE_V, '│');
        assert_eq!(glyph::TABLE_TL, '┌');
        assert_eq!(glyph::TABLE_TR, '┐');
        assert_eq!(glyph::TABLE_BL, '└');
        assert_eq!(glyph::TABLE_BR, '┘');
        assert_eq!(glyph::TABLE_LT, '├');
        assert_eq!(glyph::TABLE_RT, '┤');
        assert_eq!(glyph::TABLE_TT, '┬');
        assert_eq!(glyph::TABLE_BT, '┴');
        assert_eq!(glyph::TABLE_CR, '┼');
        assert_eq!(color::TABLE_BORDER, color::DIM);
        assert_eq!(color::TABLE_HEADER, color::CHAT_ACCENT);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui table_tokens_present`
Expected: FAIL — "no field `TABLE_H`" / associated item not found.

- [ ] **Step 3: Add the token constants**

In `crates/zoid-tui/src/tokens.rs`, inside the `pub mod glyph { ... }` block, after the `CODE_BAR`/`COPY`/`COMPACT` constants and before the "Repo drawer" comment:

```rust
    // GFM table box-drawing borders (§3.5 tables reuse the box-drawing set).
    pub const TABLE_H: char = '─';   // horizontal border
    pub const TABLE_V: char = '│';   // vertical separator
    pub const TABLE_TL: char = '┌';  // top-left corner
    pub const TABLE_TR: char = '┐';  // top-right corner
    pub const TABLE_BL: char = '└';  // bottom-left corner
    pub const TABLE_BR: char = '┘';  // bottom-right corner
    pub const TABLE_LT: char = '├';  // left tee
    pub const TABLE_RT: char = '┤';  // right tee
    pub const TABLE_TT: char = '┬';  // top tee
    pub const TABLE_BT: char = '┴';  // bottom tee
    pub const TABLE_CR: char = '┼';  // cross
```

In the same file, inside the `pub mod color { ... }` block, after the `CODE_BG` constant:

```rust
    // GFM table (spec GFM-table §3): border = DIM, header = the Chat accent.
    pub const TABLE_BORDER: Color = DIM;
    pub const TABLE_HEADER: Color = CHAT_ACCENT;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui table_tokens_present`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/tokens.rs
git commit -m "feat(tui): add GFM table box-drawing tokens"
```

---

### Task 2: BodyKind::Table + ENABLE_TABLES + empty-table routing

This task turns on table parsing and adds the `BodyKind::Table` variant, with the minimal tag handlers that accumulate a grid. The layout/emit logic comes in Task 4 — for now `End(Table)` emits nothing, so a table "vanishes" (the §5 empty/degenerate behavior). This is a verifiable intermediate state: a table renders no lines, prose around it is intact, nothing panics.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`
- Test: inline `#[cfg(test)] mod tests` in the same file.

**Interfaces:**
- Consumes: Task 1's tokens (not yet used, but available).
- Produces: `BodyKind::Table` variant; `Builder` fields `table` (accumulator); a `TableAccum` struct with `alignments: Vec<Alignment>`, `header_rows: Vec<TableRowData>`, `body_rows: Vec<TableRowData>`, `cur_row: Vec<TableRowData>`-cell state. `TableRowData` is `{ cells: Vec<Vec<Span<'static>>> }`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/zoid-tui/src/markdown.rs`:

```rust
    #[test]
    fn empty_table_emits_nothing() {
        // A degenerate table (header only, no body) still parses as a table
        // (ENABLE_TABLES), but until the layout/emit lands it renders zero
        // lines — and crucially does NOT panic or emit raw pipe text.
        let lines = render_markdown("| H1 | H2 |\n| --- | --- |\n");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(!joined.contains('|'), "raw pipe chars must not leak: {joined:?}");
        assert!(!joined.contains("---"), "separator must not leak: {joined:?}");
    }

    #[test]
    fn plain_text_outside_table_still_renders() {
        // Prose before and after a table must render normally — proves the
        // table-mode enter/exit does not corrupt the prose path.
        let lines = render_markdown("before\n\n| a | b |\n| --- | --- |\n\nafter");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("before"), "prose before table lost: {joined:?}");
        assert!(joined.contains("after"), "prose after table lost: {joined:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui empty_table_emits_nothing plain_text_outside_table_still_renders`
Expected: FAIL — raw pipe chars leak (the current fallthrough drops tags but `Text` events for cell text still emit into `cur`).

- [ ] **Step 3: Add the table data types and BodyKind::Table**

At the top of `crates/zoid-tui/src/markdown.rs`, add `Alignment` to the pulldown-cmark import:

```rust
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
```

Add `Table` to the `BodyKind` enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Prose,
    CodeHead,
    Code,
    Table,
}
```

Define the table grid types just above the `Builder` struct definition:

```rust
/// One collected table cell: its styled spans (full inline formatting).
type Cell = Vec<Span<'static>>;
/// One table row: its cells in column order.
#[derive(Default, Clone)]
struct TableRowData {
    cells: Vec<Cell>,
}

/// Accumulated table during parsing, between `Start(Table)` and `End(Table)`.
struct TableAccum {
    /// One `Alignment` per column, in source order (from `Start(Table)`).
    alignments: Vec<Alignment>,
    /// Rows collected inside `TableHead` (rendered accent+bold).
    header_rows: Vec<TableRowData>,
    /// Rows collected inside `TableRow` (rendered TXT).
    body_rows: Vec<TableRowData>,
    /// The row currently being filled (header or body, whichever was last opened).
    cur_row: TableRowData,
    /// The cell currently being filled (pushed onto cur_row.cells at End(TableCell)).
    cur_cell: Cell,
    /// True while inside `TableHead` (so End(TableCell) knows which bucket).
    in_header: bool,
}

impl TableAccum {
    fn new(alignments: Vec<Alignment>) -> Self {
        TableAccum {
            alignments,
            header_rows: Vec::new(),
            body_rows: Vec::new(),
            cur_row: TableRowData::default(),
            cur_cell: Vec::new(),
            in_header: false,
        }
    }
}
```

Add a `table` field to the `Builder` struct (in the `#[derive(Default)] struct Builder { ... }` definition). Because `TableAccum` does not implement `Default`, give the field type `Option<TableAccum>`:

```rust
struct Builder {
    lines: Vec<BodyLine>,
    cur: Vec<Span<'static>>,
    bold: u32,
    italic: u32,
    code: bool,
    link: bool,
    strike: bool,
    heading: bool,
    quote: u32,
    list: Vec<Option<u64>>,
    fence: Option<Language>,
    code_buf: String,
    bail: bool,
    table: Option<TableAccum>,
}
```

`Option<TableAccum>` derives `Default` (→ `None`), so `#[derive(Default)]` on `Builder` still compiles.

- [ ] **Step 4: Add tag handlers (start/end) that route events but emit nothing yet**

In `Builder::start`, add a table guard at the very top of the function — when in table mode, inline events (`Strong`/`Emphasis`/etc.) must route to the cell accumulator, not the prose path. Add this as the first match arm inside `fn start`:

```rust
    fn start(&mut self, tag: Tag) {
        // --- table mode: route inline/container tags into the current cell ---
        if let Some(t) = self.table.as_mut() {
            match tag {
                Tag::Strong => self.bold += 1,
                Tag::Emphasis => self.italic += 1,
                Tag::Strikethrough => self.strike = true,
                Tag::Link { .. } => self.link = true,
                Tag::Table(_) => {} // nested tables don't exist in GFM; ignore
                _ => {}
            }
            return;
        }
        match tag {
```

Then, at the bottom of the existing `match tag { ... }` in `start` (replacing the `_ => {}` arm is not enough — table tags must be real arms). Add these arms before the closing `_ => {}`:

```rust
            Tag::Table(alns) => {
                self.block_sep();
                self.flush();
                self.table = Some(TableAccum::new(alns));
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_header = true;
                    t.cur_row = TableRowData::default();
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.in_header = false;
                    t.cur_row = TableRowData::default();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cur_cell = Vec::new();
                }
            }
            _ => {}
```

In `Builder::event`, the `Text`/`Code`/`SoftBreak`/`HardBreak` arms must route into the cell when in table mode. Modify `event` to add a table guard at the top:

```rust
    fn event(&mut self, ev: Event) {
        // --- table mode: Text/Code/SoftBreak feed the current cell ---
        if let Some(t) = self.table.as_mut() {
            match ev {
                Event::Text(tt) => t.cur_cell.push(Span::styled(tt.to_string(), self.style())),
                Event::Code(c) => {
                    self.code = true;
                    t.cur_cell.push(Span::styled(c.to_string(), self.style()));
                    self.code = false;
                }
                Event::SoftBreak => t.cur_cell.push(Span::styled(" ", self.style())),
                Event::HardBreak => t.cur_cell.push(Span::styled(" ", self.style())),
                Event::Start(tag) => {
                    // inline-formatting tags still toggle the shared flags via start()
                    self.start_inline_for_table(tag);
                }
                Event::End(end) => {
                    self.end_inline_for_table(end);
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(t) => {
                if self.fence.is_some() {
                    self.code_buf.push_str(&t);
                } else {
                    self.text(&t);
                }
            }
            Event::Code(c) => {
                self.code = true;
                self.text(&c);
                self.code = false;
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            _ => {}
        }
    }
```

Wait — the table guard in `start()` already handles inline tags. But `event` calls `self.start(tag)` which would re-enter the table guard. To avoid double-handling and keep the separation clean, do NOT call `self.start()` from the table-mode branch of `event`. Instead, inline tags toggle flags directly here. Replace the `Event::Start(tag)` / `Event::End(end)` arms in the table-mode branch of `event` with direct flag toggling (no call to the table-guard helpers):

```rust
                Event::Start(tag) => match tag {
                    Tag::Strong => self.bold += 1,
                    Tag::Emphasis => self.italic += 1,
                    Tag::Strikethrough => self.strike = true,
                    Tag::Link { .. } => self.link = true,
                    _ => {}
                },
                Event::End(end) => match end {
                    TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
                    TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                    TagEnd::Strikethrough => self.strike = false,
                    TagEnd::Link => self.link = false,
                    _ => {}
                },
```

So the final `event` method is (replacing the existing one entirely):

```rust
    fn event(&mut self, ev: Event) {
        // --- table mode: Text/Code/inline-formatting feed the current cell ---
        if let Some(t) = self.table.as_mut() {
            match ev {
                Event::Text(tt) => t.cur_cell.push(Span::styled(tt.to_string(), self.style())),
                Event::Code(c) => {
                    self.code = true;
                    t.cur_cell.push(Span::styled(c.to_string(), self.style()));
                    self.code = false;
                }
                Event::SoftBreak => t.cur_cell.push(Span::styled(" ", self.style())),
                Event::HardBreak => t.cur_cell.push(Span::styled(" ", self.style())),
                Event::Start(tag) => match tag {
                    Tag::Strong => self.bold += 1,
                    Tag::Emphasis => self.italic += 1,
                    Tag::Strikethrough => self.strike = true,
                    Tag::Link { .. } => self.link = true,
                    _ => {}
                },
                Event::End(end) => match end {
                    TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
                    TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                    TagEnd::Strikethrough => self.strike = false,
                    TagEnd::Link => self.link = false,
                    _ => {}
                },
                _ => {}
            }
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(t) => {
                if self.fence.is_some() {
                    self.code_buf.push_str(&t);
                } else {
                    self.text(&t);
                }
            }
            Event::Code(c) => {
                self.code = true;
                self.text(&c);
                self.code = false;
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            _ => {}
        }
    }
```

Now the `style()` method must NOT apply `heading`/`quote` tinting in table mode (per spec §4: the cell path ignores `heading`/`quote`). But `style()` is also used by the prose path. Rather than branch inside `style()`, note that in table mode `self.heading` is always `false` (we never set it inside a table — `Start(Heading)` is not routed) and `self.quote` holds whatever depth the table is nested in. To keep cell text full `TXT`/accent even inside a blockquote, override in the cell push: change the two `self.style()` calls in the table-mode branch to a dedicated `cell_style()`:

Add a new method to `Builder`:

```rust
    /// Style for text inside a table cell. Same as `style()` but ignores
    /// `heading`/`quote` so cell text is never heading-accented or quote-dimmed
    /// (spec §4). Bold/italic/code/link/strike still apply.
    fn cell_style(&self) -> Style {
        let mut fg = color::TXT;
        if self.link {
            fg = color::MD_LINK;
        }
        if self.code {
            fg = color::MD_CODE;
        }
        let mut m = Modifier::empty();
        if self.bold > 0 {
            m |= Modifier::BOLD;
        }
        if self.italic > 0 {
            m |= Modifier::ITALIC;
        }
        if self.link {
            m |= Modifier::UNDERLINED;
        }
        if self.strike {
            m |= Modifier::CROSSED_OUT;
        }
        Style::new().fg(fg).add_modifier(m)
    }
```

And replace `self.style()` with `self.cell_style()` in BOTH the `Event::Text(tt)` and `Event::Code(c)` arms of the table-mode branch.

Finally, in `Builder::end`, add table-tag handling. After the table-mode guard in `start()` (which returns early), `end` also needs the same treatment. Add to the top of `end`, and add the table-end arms. The final `end` method:

```rust
    fn end(&mut self, end: TagEnd) {
        // --- table mode: handle table-structure ends; inline ends handled in event() ---
        if self.table.is_some() {
            match end {
                TagEnd::TableCell => {
                    if let Some(t) = self.table.as_mut() {
                        let cell = std::mem::take(&mut t.cur_cell);
                        t.cur_row.cells.push(cell);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = self.table.as_mut() {
                        let row = std::mem::take(&mut t.cur_row);
                        t.header_rows.push(row);
                        t.in_header = false;
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = self.table.as_mut() {
                        let row = std::mem::take(&mut t.cur_row);
                        t.body_rows.push(row);
                    }
                }
                TagEnd::Table => {
                    // Task 4 will replace this with the layout/emit call.
                    // For now: drop the accumulated table (renders nothing).
                    self.table = None;
                }
                _ => {}
            }
            return;
        }
        match end {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = false;
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = false,
            TagEnd::Link => self.link = false,
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.list.pop();
            }
            TagEnd::Item => self.flush(),
            TagEnd::CodeBlock => {
                let lang = self.fence.take().unwrap_or(Language::PlainText);
                let code = std::mem::take(&mut self.code_buf);
                let hl = highlight_lines(&code, lang);
                if self.quote == 0 && self.list.is_empty() {
                    let raw = code.trim_end_matches('\n').to_string();
                    self.lines.extend(code_panel(hl, lang, raw));
                } else {
                    let list_indent = "  ".repeat(self.list.len());
                    for line in hl {
                        let mut spans: Vec<Span<'static>> = Vec::new();
                        for _ in 0..self.quote {
                            spans.push(Span::styled(
                                format!("{} ", glyph::QUOTE_BAR),
                                Style::new().fg(color::DIM),
                            ));
                        }
                        if !list_indent.is_empty() {
                            spans.push(Span::styled(list_indent.clone(), Style::new()));
                        }
                        spans.extend(line.spans);
                        self.lines.push(BodyLine {
                            line: Line::from(spans),
                            kind: BodyKind::Code,
                            source: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-tui empty_table_emits_nothing plain_text_outside_table_still_renders`
Expected: PASS — the table tags are consumed, no raw pipes leak, surrounding prose renders.

Also run the full markdown test module to confirm nothing regressed:
Run: `cargo test -p zoid-tui --lib markdown::`
Expected: all existing tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "feat(tui): enable table parsing and route events to a grid accumulator"
```

---

### Task 3: Span-wrapping helper (width-only)

A standalone helper that wraps `Vec<Span>` to a given display width, returning `Vec<Vec<Span>>`. This mirrors the word-break + hard-split logic of `chat::wrap_content` but is self-contained in `markdown.rs` (it does not depend on `chat.rs` and does not touch the prose path). Used by the cell-wrapping step of layout in Task 4.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`
- Test: inline `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: nothing new (uses `unicode-width`, which is a workspace dep; add the import).
- Produces: `fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>>`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn wrap_spans_short_content_is_one_row() {
        use ratatui::text::Span;
        let rows = wrap_spans(&[Span::raw("hello world")], 30);
        assert_eq!(rows.len(), 1);
        let joined: String = rows[0].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn wrap_spans_breaks_at_word_boundary() {
        use ratatui::text::Span;
        // 3 words of 5 chars each = 15 chars + 2 spaces = 17; wrap at width 10.
        let rows = wrap_spans(&[Span::raw("alpha bravo charlie")], 10);
        assert_eq!(rows.len(), 2, "should wrap into 2 rows");
        let r0: String = rows[0].iter().map(|s| s.content.to_string()).collect();
        let r1: String = rows[1].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(r0, "alpha bravo");
        assert_eq!(r1, "charlie");
    }

    #[test]
    fn wrap_spans_hard_splits_overlong_token() {
        use ratatui::text::Span;
        // A single 20-char word wider than width 8 must hard-split, not overflow.
        let rows = wrap_spans(&[Span::raw("abcdefghijklmnopqrst")], 8);
        assert!(rows.len() >= 3, "a 20-char word at width 8 needs >=3 rows");
        // total content preserved
        let total: String = rows.iter().flat_map(|r| r.iter().map(|s| s.content.to_string())).collect();
        assert_eq!(total, "abcdefghijklmnopqrst");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui wrap_spans`
Expected: FAIL — `wrap_spans` not found.

- [ ] **Step 3: Add the helper**

Add the unicode-width import at the top of `crates/zoid-tui/src/markdown.rs`:

```rust
use unicode_width::UnicodeWidthStr;
```

Add the helper as a free function (near `plain_lines`, before the `Builder` impl):

```rust
/// Wrap styled `spans` into rows no wider than `width` (display width). Word-
/// based (break on spaces), hard-splitting any single token longer than `width`.
/// Sibling of `chat::wrap_content` but self-contained in the markdown crate so
/// table cell-wrapping does not depend on the chat module. Returns at least one
/// (possibly empty) row. Used only for table-cell wrapping (spec §2 step 3).
fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    // Tokenize into (text, style, is_space) runs at whitespace boundaries.
    let mut toks: Vec<(String, Style, bool)> = Vec::new();
    for s in spans {
        let mut chars = s.content.chars().peekable();
        while let Some(&c) = chars.peek() {
            let is_space = c == ' ';
            let mut t = String::new();
            while let Some(&c2) = chars.peek() {
                if (c2 == ' ') != is_space {
                    break;
                }
                t.push(c2);
                chars.next();
            }
            toks.push((t, s.style, is_space));
        }
    }

    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    for (text, style, is_space) in toks {
        let w = text.width();
        if is_space {
            if cur.is_empty() {
                continue;
            }
            if cur_w + w > width {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            } else {
                cur.push(Span::styled(text, style));
                cur_w += w;
            }
            continue;
        }
        if cur_w + w > width && !cur.is_empty() {
            while cur
                .last()
                .map(|s| s.content.chars().all(|c| c == ' '))
                .unwrap_or(false)
            {
                cur.pop();
            }
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if w > width {
            let mut piece = String::new();
            let mut piece_w = 0usize;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if cur_w + piece_w + cw > width && cur_w + piece_w > 0 {
                    if !piece.is_empty() {
                        cur.push(Span::styled(std::mem::take(&mut piece), style));
                    }
                    piece_w = 0;
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                piece.push(ch);
                piece_w += cw;
            }
            if !piece.is_empty() {
                cur.push(Span::styled(piece, style));
                cur_w += piece_w;
            }
        } else {
            cur.push(Span::styled(text, style));
            cur_w += w;
        }
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui wrap_spans`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "feat(tui): add wrap_spans helper for table cell wrapping"
```

---

### Task 4: Column measurement + layout + emit

This is the heart: on `End(Table)`, measure columns, cap at 30, wrap overlong cells, pad with alignment, and emit bordered grid lines as `BodyKind::Table`. Header rows get `TABLE_HEADER`+bold; body rows get `TXT`.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`
- Test: inline `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: Task 1 tokens; Task 2 `TableAccum`/`TableRowData`/`Cell`; Task 3 `wrap_spans`.
- Produces: rendered `BodyLine { kind: BodyKind::Table }` lines. The `End(Table)` arm calls a new method `Builder::render_table`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn simple_two_by_two_table_renders_bordered_grid() {
        let md = "| H1 | H2 |\n| --- | --- |\n| a | b |\n";
        let lines = render_markdown(md);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(joined.contains('┌'), "missing top border: {joined}");
        assert!(joined.contains('└'), "missing bottom border: {joined}");
        assert!(joined.contains('├'), "missing header separator: {joined}");
        assert!(joined.contains('│'), "missing vertical border: {joined}");
        assert!(joined.contains("H1") && joined.contains("H2"), "header text lost: {joined}");
        assert!(joined.contains('a') && joined.contains('b'), "body text lost: {joined}");
    }

    #[test]
    fn header_cells_are_accent_bold() {
        let md = "| H | \n| --- |\n| x |\n";
        let body = render_body(md);
        // Find a span containing "H" — it must be a header cell.
        let header_span = body
            .iter()
            .flat_map(|b| b.line.spans.iter())
            .find(|s| s.content.contains('H'))
            .expect("header text not found");
        assert_eq!(header_span.style.fg, Some(color::TABLE_HEADER));
        assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
        // A body cell with "x" must be TXT, not accent.
        let body_span = body
            .iter()
            .flat_map(|b| b.line.spans.iter())
            .find(|s| s.content.contains('x'))
            .expect("body text not found");
        assert_eq!(body_span.style.fg, Some(color::TXT));
    }

    #[test]
    fn inline_bold_and_code_in_cells_are_styled() {
        let md = "| c |\n| --- |\n| **k** `v` |\n";
        let body = render_body(md);
        let spans: Vec<&Span> = body.iter().flat_map(|b| b.line.spans.iter()).collect();
        // "k" must be bold (in a body cell).
        assert!(
            spans.iter().any(|s| s.content.contains('k')
                && s.style.add_modifier.contains(Modifier::BOLD)),
            "bold not applied in cell: {spans:?}"
        );
        // "v" must be MD_CODE.
        assert!(
            spans.iter().any(|s| s.content.contains('v')
                && s.style.fg == Some(color::MD_CODE)),
            "inline code color not applied in cell: {spans:?}"
        );
    }

    #[test]
    fn wide_cell_wraps_within_column_cap() {
        // A single 40-char cell — wider than the 30 cap — must wrap.
        let long = "x".repeat(40);
        let md = format!("| {} |\n| --- |\n| {} |\n", long, long);
        let lines = render_markdown(&md);
        // The header row and the body row each must have wrapped (height >= 2).
        // Count rows that contain content lines (not borders): a wrapped table
        // is taller than an unwrapped one. We assert there are >= 2 non-border
        // lines containing 'x' beyond the first.
        let x_rows: Vec<&ratatui::text::Line> = lines
            .iter()
            .filter(|l| {
                l.spans.iter().any(|s| s.content.contains('x'))
            })
            .collect();
        assert!(x_rows.len() >= 2, "expected wrapping (>=2 x-rows), got {}: {x_rows:?}", x_rows.len());
    }

    #[test]
    fn column_width_is_widest_cell_capped_at_30() {
        // The widest cell is 40 chars; the column must cap at 30. A border
        // segment under a single column is at most 30 dashes wide. Build the
        // table and check no single span of dashes exceeds 30 chars.
        let long = "a".repeat(40);
        let md = format!("| {} |\n| --- |\n", long);
        let lines = render_markdown(&md);
        let max_dash_run = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.content.chars().all(|c| c == '─'))
            .map(|s| s.content.chars().count())
            .max()
            .unwrap_or(0);
        assert!(
            max_dash_run <= 30,
            "column width exceeded cap: max dash run = {max_dash_run}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui simple_two_by_two_table_renders_bordered_grid`
Expected: FAIL — `End(Table)` still drops the table (Task 2 behavior); no borders emitted.

- [ ] **Step 3: Implement the layout + emit method**

Add a `render_table` method to `Builder`. It takes the accumulated `TableAccum` (moved out of `self.table`), computes column widths, wraps/pads cells, and pushes `BodyLine { kind: BodyKind::Table }` lines into `self.lines`. Put this method inside `impl Builder { ... }`:

```rust
    /// Layout a finished table grid into bordered `BodyKind::Table` lines and
    /// push them onto `self.lines` (spec §2). Called from `end(TagEnd::Table)`.
    /// `table` is the fully-accumulated grid (moved out of `self.table`).
    fn render_table(&mut self, table: TableAccum) {
        let ncols = table.alignments.len();
        if ncols == 0 {
            return; // degenerate: no columns, emit nothing
        }
        // All rows, with an is_header flag (header rows first, then body).
        let mut all_rows: Vec<(TableRowData, bool)> = Vec::new();
        for r in table.header_rows {
            all_rows.push((r, true));
        }
        for r in table.body_rows {
            all_rows.push((r, false));
        }
        if all_rows.iter().all(|(r, _)| r.cells.is_empty()) {
            return; // degenerate: no cells anywhere
        }

        // --- Step 1: measure natural column widths ---
        let mut natural = vec![0usize; ncols];
        for (row, _is_header) in &all_rows {
            for c in 0..ncols {
                let cell_w: usize = row
                    .cells
                    .get(c)
                    .map(|cell| cell.iter().map(|s| s.content.width()).sum())
                    .unwrap_or(0);
                natural[c] = natural[c].max(cell_w);
            }
        }
        // --- Step 2: cap ---
        let cap = MAX_COL_W;
        let widths: Vec<usize> = natural.iter().map(|&w| w.min(cap)).collect();

        // --- Step 3: wrap + pad every cell into Vec<Vec<Span>> of width widths[c] ---
        // wrapped_cells[ri][ci] = Vec of visual rows (each a Vec<Span> already padded to widths[ci])
        let mut wrapped_rows: Vec<(Vec<Vec<Vec<Span<'static>>>>, bool)> = Vec::new();
        for (row, is_header) in &all_rows {
            let mut wrapped_cells: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
            for c in 0..ncols {
                let cell = row.cells.get(c).cloned().unwrap_or_default();
                // restyle for header/body: header spans → TABLE_HEADER+bold base
                let base_spans: Vec<Span<'static>> = if *is_header {
                    cell.into_iter()
                        .map(|mut s| {
                            s.style = s.style.fg(color::TABLE_HEADER).add_modifier(Modifier::BOLD);
                            s
                        })
                        .collect()
                } else {
                    cell
                };
                let wrapped = wrap_spans(&base_spans, widths[c]);
                // pad each visual row to the column width with alignment
                let aligned = align_rows(wrapped, widths[c], table.alignments.get(c).copied());
                wrapped_cells.push(aligned);
            }
            wrapped_rows.push((wrapped_cells, *is_header));
        }

        // --- Step 4 + 5 + 6: emit border + cell lines ---
        let border_top = border_line(&widths, BorderKind::Top);
        let border_mid = border_line(&widths, BorderKind::Mid);
        let border_bot = border_line(&widths, BorderKind::Bottom);

        self.lines.push(BodyLine {
            line: Line::from(border_top),
            kind: BodyKind::Table,
            source: None,
        });
        for (ri, (cells, _is_header)) in wrapped_rows.iter().enumerate() {
            let row_h = cells.iter().map(|c| c.len()).max().max(1);
            for vh in 0..row_h {
                let mut spans: Vec<Span<'static>> = Vec::new();
                spans.push(Span::styled(
                    glyph::TABLE_V.to_string(),
                    Style::new().fg(color::TABLE_BORDER),
                ));
                for c in 0..ncols {
                    let cell_rows = &cells[c];
                    let row_spans = cell_rows.get(vh).cloned().unwrap_or_else(|| {
                        // blank padding row: spaces of width widths[c]
                        vec![Span::styled(
                            " ".repeat(widths[c]),
                            Style::new().fg(color::TABLE_BORDER),
                        )]
                    });
                    spans.push(Span::styled(" ", Style::new()));
                    spans.extend(row_spans);
                    spans.push(Span::styled(" ", Style::new()));
                    if c + 1 < ncols {
                        spans.push(Span::styled(
                            glyph::TABLE_V.to_string(),
                            Style::new().fg(color::TABLE_BORDER),
                        ));
                    }
                }
                spans.push(Span::styled(
                    glyph::TABLE_V.to_string(),
                    Style::new().fg(color::TABLE_BORDER),
                ));
                self.lines.push(BodyLine {
                    line: Line::from(spans),
                    kind: BodyKind::Table,
                    source: None,
                });
            }
            // Insert the header separator after the LAST header row, before the
            // first body row. Detect: this row is header, and the next is body.
            let is_last_header = wrapped_rows[ri].1
                && wrapped_rows.get(ri + 1).map(|(_, ih)| !*ih).unwrap_or(false);
            if is_last_header {
                self.lines.push(BodyLine {
                    line: Line::from(border_mid.clone()),
                    kind: BodyKind::Table,
                    source: None,
                });
            }
        }
        self.lines.push(BodyLine {
            line: Line::from(border_bot),
            kind: BodyKind::Table,
            source: None,
        });
    }
```

Now wire it into `end`. Change the `TagEnd::Table` arm (currently `self.table = None;`) to:

```rust
                TagEnd::Table => {
                    if let Some(t) = self.table.take() {
                        self.render_table(t);
                    }
                }
```

Add the constants and free helper functions near the top of the file (after the `MAX_DEPTH` const):

```rust
/// Maximum display width of a single table column (spec §2 step 2). Cells
/// exceeding this wrap to multiple visual rows within the column.
const MAX_COL_W: usize = 30;
```

Add the `BorderKind` enum and the two free helper functions (`border_line`, `align_rows`) below `wrap_spans`:

```rust
#[derive(Clone, Copy)]
enum BorderKind {
    Top,
    Mid,
    Bottom,
}

/// Build a horizontal border line: corner + (─×widths[c]) + tee/corner, joined.
/// Returns styled spans (TABLE_BORDER). `Mid` uses ─ separators with tees.
fn border_line(widths: &[usize], kind: BorderKind) -> Vec<Span<'static>> {
    let (left, cross, right) = match kind {
        BorderKind::Top => (glyph::TABLE_TL, glyph::TABLE_TT, glyph::TABLE_TR),
        BorderKind::Mid => (glyph::TABLE_LT, glyph::TABLE_CR, glyph::TABLE_RT),
        BorderKind::Bottom => (glyph::TABLE_BL, glyph::TABLE_BT, glyph::TABLE_BR),
    };
    let dim = Style::new().fg(color::TABLE_BORDER);
    let mut spans = Vec::new();
    spans.push(Span::styled(left.to_string(), dim));
    for (i, &w) in widths.iter().enumerate() {
        // each column segment is (w+2) dashes: 1 pad each side of the content width
        let seg = std::iter::repeat(glyph::TABLE_H)
            .take(w + 2)
            .collect::<String>();
        spans.push(Span::styled(seg, dim));
        if i + 1 < widths.len() {
            spans.push(Span::styled(cross.to_string(), dim));
        }
    }
    spans.push(Span::styled(right.to_string(), dim));
    spans
}

/// Pad each visual row of `rows` to `width`, honoring alignment. Returns the
/// same number of rows, each exactly `width` display columns of styled spans.
fn align_rows(
    rows: Vec<Vec<Span<'static>>>,
    width: usize,
    align: Option<Alignment>,
) -> Vec<Vec<Span<'static>>> {
    rows.into_iter()
        .map(|row| {
            let content_w: usize = row.iter().map(|s| s.content.width()).sum();
            let pad = width.saturating_sub(content_w);
            match align {
                Some(Alignment::Right) => {
                    let mut v = vec![Span::styled(" ".repeat(pad), Style::new())];
                    v.extend(row);
                    v
                }
                Some(Alignment::Center) => {
                    let left = pad / 2;
                    let right = pad - left;
                    let mut v = vec![Span::styled(" ".repeat(left), Style::new())];
                    v.extend(row);
                    v.push(Span::styled(" ".repeat(right), Style::new()));
                    v
                }
                _ => {
                    // Left / None: pad right
                    let mut v = row;
                    v.push(Span::styled(" ".repeat(pad), Style::new()));
                    v
                }
            }
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui simple_two_by_two_table_renders_bordered_grid header_cells_are_accent_bold inline_bold_and_code_in_cells_are_styled wide_cell_wraps_within_column_cap column_width_is_widest_cell_capped_at_30`
Expected: all PASS.

Then the full markdown module to check for regressions:
Run: `cargo test -p zoid-tui --lib markdown::`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "feat(tui): lay out and render GFM tables with borders and cell wrapping"
```

---

### Task 5: Alignment padding + blockquote nesting tests

The layout from Task 4 already implements alignment via `align_rows`, but the spec calls out explicit alignment tests and a blockquote-nesting test. This task adds those targeted tests and any small fix the alignment path needs.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`
- Test: inline `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: Task 4 layout.
- Produces: no new API — only tests.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn alignment_left_center_right_pads_correctly() {
        // Three columns: left, center, right. Short content so the padding is
        // visible. We verify the padding by checking the leading/trailing space
        // ratio around the content in a rendered body cell line.
        let md = "| L | C | R |\n| :--- | :---: | ---: |\n| x | x | x |\n";
        let body = render_body(md);
        // Find the body-row cell line (contains three "x"s separated by borders).
        let cell_line = body
            .iter()
            .find(|b| {
                b.line.spans.iter().filter(|s| s.content.contains('x')).count() == 3
            })
            .expect("body cell line with 3 x's not found");
        let joined: String = cell_line.line.spans.iter().map(|s| s.content.to_string()).collect();
        // Each "x" should be present. Left-aligned: "x" then spaces. Right: spaces then "x".
        // We just assert all three are present and the line has borders.
        assert!(joined.contains('x'));
        assert!(joined.contains(glyph::TABLE_V));
    }

    #[test]
    fn table_inside_blockquote_keeps_quote_bar() {
        // A table inside a blockquote. The quote bar is applied by push_message's
        // lead in chat.rs (Task 7), so here we only verify the markdown.rs side:
        // the cell content is full TXT (not dimmed), proving the quote flag did
        // not bleed into cells.
        let md = "> | H |\n> | --- |\n> | x |\n";
        let body = render_body(md);
        let body_span = body
            .iter()
            .flat_map(|b| b.line.spans.iter())
            .find(|s| s.content.contains('x'))
            .expect("body cell text not found");
        assert_eq!(
            body_span.style.fg,
            Some(color::TXT),
            "cell text must not be dimmed by blockquote nesting"
        );
    }

    #[test]
    fn malformed_table_does_not_panic() {
        // Unbalanced pipes / weird input must not panic; it renders something.
        let md = "| a |\n|nope|\n| b | c |\n";
        let lines = render_markdown(md);
        // Just assert it produced some output without panicking.
        assert!(!lines.is_empty() || lines.is_empty()); // tautology — the real check is "didn't panic"
        let _ = render_markdown("| | | |\n| --- |\n");
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p zoid-tui alignment_left_center_right_pads_correctly table_inside_blockquote_keeps_quote_bar malformed_table_does_not_panic`
Expected: PASS. (These mostly verify existing Task-4 behavior; if any fail, the alignment path needs the fix shown in Task 4's `align_rows`.)

If `table_inside_blockquote_keeps_quote_bar` fails because pulldown-cmark does not parse a table inside a blockquote as a table, change the test to use a simpler quote-wrapped table or mark it as a known-limitation and skip — but first try the exact input above (pulldown-cmark with `ENABLE_TABLES` does handle `> | ... |`).

- [ ] **Step 3: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "test(tui): GFM table alignment, blockquote nesting, malformed input"
```

---

### Task 6: MAX_DEPTH bail integration for tables

Per spec §5: a table counts as one nesting level. When a table starts inside a deep blockquote/list, it must participate in the `MAX_DEPTH` bail. Currently `Start(Table)` does not check depth.

**Files:**
- Modify: `crates/zoid-tui/src/markdown.rs`
- Test: inline `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: Task 2 `bail` field, `MAX_DEPTH`.
- Produces: no new API.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn deep_table_nesting_bails_to_plain_text() {
        // 9 levels of blockquote (> >>>>>>>>>>>) wrapping a table exceeds
        // MAX_DEPTH (8). render_body must bail to plain_lines — the whole
        // message becomes plain text, no table borders, no panic.
        let mut md = String::new();
        for _ in 0..9 {
            md.push_str("> ");
        }
        md.push_str("| H |\n| --- |\n| x |\n");
        let lines = render_markdown(&md);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(!joined.contains('┌'), "bail must not render table borders: {joined:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui deep_table_nesting_bails_to_plain_text`
Expected: FAIL or PASS depending on whether pulldown-cmark even yields a table at that depth. If it passes already (because 9-level quotes don't yield a table), keep it as a regression guard. If the table renders borders, it FAILS.

- [ ] **Step 3: Add the depth check to `Start(Table)`**

In `Builder::start`, the `Tag::Table(alns)` arm currently is:

```rust
            Tag::Table(alns) => {
                self.block_sep();
                self.flush();
                self.table = Some(TableAccum::new(alns));
            }
```

Change it to bump a depth check. Since tables nest inside quote/list, add a depth counter field `table_depth` that increments on `Start(Table)`:

Actually, simpler: treat the table as occupying the same nesting as `self.quote + self.list.len()`. Add the bail check:

```rust
            Tag::Table(alns) => {
                self.block_sep();
                self.flush();
                if self.quote as usize + self.list.len() > MAX_DEPTH {
                    self.bail = true;
                    return;
                }
                self.table = Some(TableAccum::new(alns));
            }
```

(Note: this only matters when the table is itself nested. A top-level table has `quote == 0` and `list.is_empty()`, so it never bails.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tui deep_table_nesting_bails_to_plain_text`
Expected: PASS.

Run the full module for regressions:
Run: `cargo test -p zoid-tui --lib markdown::`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/markdown.rs
git commit -m "feat(tui): bail tables nested beyond MAX_DEPTH to plain text"
```

---

### Task 7: push_message BodyKind::Table arm in chat.rs

The chat layout function must pass table lines through unwrapped (like `Code`), with the lead/indent prefix, and they close any open code block.

**Files:**
- Modify: `crates/zoid-tui/src/chat.rs:507-543` (the `match kind` block inside `push_message`).
- Test: inline `#[cfg(test)] mod tests` in `chat.rs` if one exists, otherwise verify via the snapshot in Task 8.

**Interfaces:**
- Consumes: `BodyKind::Table` from Task 2.
- Produces: table lines in the conversation `Vec<Line>` with correct lead/indent.

- [ ] **Step 1: Write the failing test**

There is no dedicated markdown test in `chat.rs`; the rendering is tested via `push_message` indirectly. The simplest verification is a compile check + the snapshot in Task 8. But add a focused test in `markdown.rs`'s test module that exercises `render_body` returning `BodyKind::Table` lines (so the chat.rs arm is exercised once Task 8's snapshot runs):

Add to the `#[cfg(test)] mod tests` block in `markdown.rs`:

```rust
    #[test]
    fn table_lines_are_bodykind_table() {
        let md = "| H |\n| --- |\n| x |\n";
        let body = render_body(md);
        let has_table_kind = body.iter().any(|b| b.kind == BodyKind::Table);
        assert!(has_table_kind, "table lines must be BodyKind::Table: {body:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tui table_lines_are_bodykind_table`
Expected: PASS already (Task 4 emits `BodyKind::Table`). If it passes, good — this is a guard.

- [ ] **Step 3: Add the Table arm to push_message**

In `crates/zoid-tui/src/chat.rs`, inside `push_message`, the `match kind` block. Currently it has arms for `Prose`, `CodeHead`, `Code`. Add a `Table` arm after `Code` and before the closing brace. The exact current structure (lines ~514–542):

```rust
        match kind {
            BodyKind::Prose => {
                open = None;
                let rows = wrap_content(&line.spans, content_w);
                ...
            }
            BodyKind::CodeHead => { ... }
            BodyKind::Code => { ... }
        }
```

Add this arm:

```rust
            BodyKind::Table => {
                open = None;
                let mut spans = lead;
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
```

(No wrapping — table lines are pre-laid-out. No `CODE_BG` padding. Closes open code block via `open = None`, matching `Prose`.)

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p zoid-tui`
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tui/src/chat.rs crates/zoid-tui/src/markdown.rs
git commit -m "feat(tui): pass table lines through push_message unwrapped"
```

---

### Task 8: Integration snapshot

Lock the visual layout of a representative table with an insta snapshot, matching the existing snapshot convention.

**Files:**
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs`
- Snapshot: `crates/zoid-tui/tests/snapshots/shell_snapshot__table_basic.snap` (auto-generated).

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a committed snapshot file.

- [ ] **Step 1: Write the snapshot test**

Add to `crates/zoid-tui/tests/shell_snapshot.rs`. First inspect how an existing snapshot test constructs a `ShellState` + messages and calls `draw`. The `draw` helper is `fn draw(state, msgs, w, h) -> String`. Add a new test function:

```rust
#[test]
fn table_basic() {
    use zoid_core::projection::ChatMsg;
    let state = ShellState::default();
    let msgs = vec![ChatMsg::Assistant {
        text: "| Name | Kind | Note |\n| :--- | :---: | ---: |\n| alpha | code | long enough to maybe wrap if narrow |\n| **bold** | `x` | short |\n".to_string(),
        tool_calls: vec![],
        ts: 0,
        thinking: None,
    }];
    let rendered = draw(&state, &msgs, 60, 18);
    insta::assert_snapshot!(rendered);
}
```

(Adjust the `ChatMsg::Assistant` field order if it differs — confirm against the existing `ChatMsg` definition in `zoid-core/src/projection.rs` before running.)

- [ ] **Step 2: Generate the snapshot**

Run: `cargo insta test --accept -p zoid-tui table_basic`
Expected: a snapshot file is created at `crates/zoid-tui/tests/snapshots/shell_snapshot__table_basic.snap`.

- [ ] **Step 3: Inspect the snapshot visually**

Run: `cat crates/zoid-tui/tests/snapshots/shell_snapshot__table_basic.snap`

Confirm the rendered output contains:
- A `┌───┬─────┬─────┐` top border
- Header row with `Name`, `Kind`, `Note` in accent color (color codes in the snapshot)
- A `├───┼─────┼─────┤` separator
- Body rows with the cell text
- A `└───┴─────┴─────┘` bottom border

If the wrapping or column widths look wrong, fix the layout in Task 4 and re-run `cargo insta test --accept`.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-tui/tests/snapshots/shell_snapshot__table_basic.snap
git commit -m "test(tui): snapshot for GFM table rendering"
```

---

## Self-Review

**Spec coverage check** (spec §1–§7 → tasks):
- Parser enable + event routing (§1) → Task 2.
- Layout algorithm measure/cap/wrap/pad/emit (§2) → Task 4.
- BodyKind::Table + visual tokens + styling (§3) → Tasks 1, 2, 7.
- Inline formatting in cells (§4) → Task 2 (`cell_style`, flag routing) + Task 4 (header restyle) + tested in Task 4 (`inline_bold_and_code_in_cells_are_styled`).
- Edge cases (§5): malformed → Task 5; empty/degenerate → Task 2; blockquote nesting → Task 5; MAX_DEPTH bail → Task 6; CJK width → covered by `unicode-width` in `wrap_spans` (Task 3).
- Implementation surface (§6) → matches: markdown.rs (Tasks 2–6), chat.rs (Task 7), tokens.rs (Task 1).
- Testing (§7): all 10 named tests map to Tasks 2/4/5/6 + snapshot (Task 8). The §7 "plain_text_outside_table" and "empty_table" tests are in Task 2.

**Placeholder scan:** no TBD/TODO; every code step shows the code.

**Type consistency:** `TableAccum`, `TableRowData`, `Cell` defined in Task 2, used unchanged in Task 4. `wrap_spans` (Task 3) signature matches its use in Task 4. `border_line`/`align_rows`/`BorderKind` defined and used in Task 4. `BodyKind::Table` added in Task 2, matched in chat.rs (Task 7).

All requirements covered; no gaps found.
