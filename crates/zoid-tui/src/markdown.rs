//! Render assistant/user message bodies from markdown to ratatui `Line`s
//! (spec §3.5). `pulldown-cmark` parses; inline styles (headings, bold, italic,
//! inline `code`, lists, blockquotes, links) map to §16 design tokens, and
//! fenced ```lang blocks reuse the Ⓡ3 highlighter (`highlight_lines`). Nesting is
//! depth-capped; anything unexpected falls back to plain text. Wrapping is the
//! caller's job (`Wrap { trim: false }`) — we only build styled spans/lines.

use crate::syntax_view::highlight_lines;
use crate::tokens::{color, glyph};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use zoid_syntax::Language;

/// Max container nesting (lists + blockquotes) before we bail to plain text.
const MAX_DEPTH: usize = 8;

/// Maximum display width of a single table column (spec §2 step 2). Cells
/// exceeding this wrap to multiple visual rows within the column.
const MAX_COL_W: usize = 30;

/// What a rendered body line is, so downstream layout can treat it correctly:
/// prose word-wraps with a hanging indent; code lines never word-wrap (their
/// leading whitespace is significant) and `CodeHead` is where the copy hint
/// attaches (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Prose,
    CodeHead,
    Code,
    Table,
}

/// A rendered body line plus its kind. `render_body` is the real builder;
/// `render_markdown` is the flatten-to-`Line` wrapper kept for callers that
/// don't need the kind (delegated-summary rendering, tests).
///
/// `source` is `Some` only on the `CodeHead` of a **top-level** code panel: it
/// carries that block's raw text so the click-to-copy map (`chat::code_hits`) can
/// pair each clickable range with its own source from this single render pass.
/// Deriving the source here — rather than re-parsing the markdown separately —
/// means a block and its source can never desync (e.g. when `render_body` bails
/// to plain text on over-nesting, no `CodeHead` is emitted, so no orphan source
/// leaks into the map).
#[derive(Debug, Clone)]
pub struct BodyLine {
    pub line: Line<'static>,
    pub kind: BodyKind,
    pub source: Option<String>,
}

/// Render markdown `source` into typed body lines. Most non-empty input yields at
/// least one line; whitespace-only input can yield an empty vec (the caller —
/// `push_message` — handles an empty body by emitting the prefix alone).
pub fn render_body(source: &str) -> Vec<BodyLine> {
    let mut b = Builder::default();
    for ev in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        b.event(ev);
        if b.bail {
            return plain_lines(source);
        }
    }
    b.finish()
}

/// Flatten [`render_body`] to plain `Line`s (drops the per-line kind).
pub fn render_markdown(source: &str) -> Vec<Line<'static>> {
    render_body(source).into_iter().map(|b| b.line).collect()
}

/// One TXT-styled prose `Line` per source row — the parse-issue / over-nesting fallback.
fn plain_lines(source: &str) -> Vec<BodyLine> {
    source
        .split('\n')
        .map(|l| BodyLine {
            line: Line::from(Span::styled(l.to_string(), Style::new().fg(color::TXT))),
            kind: BodyKind::Prose,
            source: None,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum BorderKind {
    Top,
    Mid,
    Bottom,
}

/// Build a horizontal border line: corner + (─×(w+2)) + tee/corner, joined.
/// Each column segment is `width+2` dashes (1 pad each side of the content
/// width). Returns styled spans (TABLE_BORDER).
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

/// Short display label for a fenced-code language; PlainText → "" (no tag).
fn lang_label(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::Toml => "toml",
        Language::Json => "json",
        Language::Yaml => "yaml",
        Language::Markdown => "markdown",
        Language::PlainText => "",
    }
}

/// Build the Style A code container: a dim left rule + faint background under the
/// content, led by a language-tag header row. Rows are NOT right-padded here —
/// `push_message` pads each code line to the body-column width once it knows the
/// available width, so the panel becomes a clean rectangle without ever
/// overflowing a narrow column. Offset into the body column is the caller's job.
fn code_panel(hl: Vec<Line<'static>>, lang: Language, source: String) -> Vec<BodyLine> {
    let bar = || {
        Span::styled(
            format!("{} ", glyph::CODE_BAR),
            Style::new().fg(color::DIM).bg(color::CODE_BG),
        )
    };
    let label = lang_label(lang);

    let mut out: Vec<BodyLine> = Vec::new();
    // Header row: bar + italic language tag. It also carries the block's raw
    // source for the click-to-copy map (only the head, so a block contributes
    // exactly one source in document order).
    out.push(BodyLine {
        line: Line::from(vec![
            bar(),
            Span::styled(
                label.to_string(),
                Style::new()
                    .fg(color::DIM)
                    .add_modifier(Modifier::ITALIC)
                    .bg(color::CODE_BG),
            ),
        ]),
        kind: BodyKind::CodeHead,
        source: Some(source),
    });
    // Body rows: highlighted source with the panel background under each span.
    for l in hl {
        let mut spans = vec![bar()];
        for s in l.spans {
            spans.push(Span::styled(s.content, s.style.bg(color::CODE_BG)));
        }
        out.push(BodyLine {
            line: Line::from(spans),
            kind: BodyKind::Code,
            source: None,
        });
    }
    out
}

/// Resolve a fenced-code info string ("rust", "rs", "toml", …) to a Language;
/// unknown/empty → PlainText (renders without highlighting).
fn lang_from_fence(info: &str) -> Language {
    match info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rust" | "rs" => Language::Rust,
        "toml" => Language::Toml,
        "json" => Language::Json,
        "yaml" | "yml" => Language::Yaml,
        "md" | "markdown" => Language::Markdown,
        _ => Language::PlainText,
    }
}

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

#[derive(Default)]
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
    list: Vec<Option<u64>>, // per level: next ordinal (Some) or bullet (None)
    fence: Option<Language>,
    code_buf: String,
    bail: bool,
    table: Option<TableAccum>,
}

impl Builder {
    fn style(&self) -> Style {
        let mut fg = color::TXT;
        if self.quote > 0 {
            fg = color::DIM;
        }
        if self.heading {
            fg = color::CHAT_ACCENT; // heading beats the blockquote tint
        }
        if self.link {
            fg = color::MD_LINK;
        }
        if self.code {
            fg = color::MD_CODE; // inline code beats link colouring
        }
        let mut m = Modifier::empty();
        if self.bold > 0 || self.heading {
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

    fn text(&mut self, t: &str) {
        self.cur.push(Span::styled(t.to_string(), self.style()));
    }

    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for _ in 0..self.quote {
            spans.push(Span::styled(
                format!("{} ", glyph::QUOTE_BAR),
                Style::new().fg(color::DIM),
            ));
        }
        spans.append(&mut self.cur);
        self.lines.push(BodyLine {
            line: Line::from(spans),
            kind: BodyKind::Prose,
            source: None,
        });
    }

    fn event(&mut self, ev: Event) {
        // In table mode, leaf text events feed the current cell. Start/End
        // events still dispatch to start()/end() unconditionally — the
        // TableHead/TableRow/TableCell tags MUST reach those handlers or the
        // grid never accumulates (the inline-formatting flags they toggle are
        // shared state, consumed here via cell_style()).
        if self.table.is_some() {
            match ev {
                Event::Text(t) => {
                    let st = self.cell_style();
                    if let Some(tbl) = self.table.as_mut() {
                        tbl.cur_cell.push(Span::styled(t.to_string(), st));
                    }
                    return;
                }
                Event::Code(c) => {
                    self.code = true;
                    let st = self.cell_style();
                    if let Some(tbl) = self.table.as_mut() {
                        tbl.cur_cell.push(Span::styled(c.to_string(), st));
                    }
                    self.code = false;
                    return;
                }
                Event::SoftBreak | Event::HardBreak => {
                    let st = self.cell_style();
                    if let Some(tbl) = self.table.as_mut() {
                        tbl.cur_cell.push(Span::styled(" ", st));
                    }
                    return;
                }
                _ => {} // Start/End fall through to the normal dispatch below
            }
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

    /// Emit a blank spacer between top-level block elements so paragraphs, code
    /// blocks, headings, and quotes don't stack flush (spec §3.5 breathing room).
    /// No-op at the very start, inside lists/quotes, or after an existing blank.
    fn block_sep(&mut self) {
        if self.quote > 0 || !self.list.is_empty() {
            return;
        }
        match self.lines.last() {
            None => {}
            Some(b) if b.line.spans.iter().all(|s| s.content.trim().is_empty()) => {}
            Some(_) => self.lines.push(BodyLine {
                line: Line::from(""),
                kind: BodyKind::Prose,
                source: None,
            }),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.block_sep();
                self.flush();
            }
            Tag::Heading { .. } => {
                self.block_sep();
                self.flush();
                self.heading = true;
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike = true,
            Tag::Link { .. } => self.link = true,
            Tag::BlockQuote(_) => {
                self.block_sep();
                self.flush();
                self.quote += 1;
                if self.quote as usize + self.list.len() > MAX_DEPTH {
                    self.bail = true;
                }
            }
            Tag::List(start) => {
                self.list.push(start);
                if self.quote as usize + self.list.len() > MAX_DEPTH {
                    self.bail = true;
                }
            }
            Tag::Item => {
                self.flush();
                let depth = self.list.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.list.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{} ", glyph::BULLET),
                };
                self.cur.push(Span::styled(
                    format!("{indent}{marker}"),
                    Style::new().fg(color::DIM),
                ));
            }
            Tag::CodeBlock(kind) => {
                self.block_sep();
                self.flush();
                self.fence = Some(match kind {
                    CodeBlockKind::Fenced(info) => lang_from_fence(&info),
                    CodeBlockKind::Indented => Language::PlainText,
                });
            }
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
        }
    }

    fn end(&mut self, end: TagEnd) {
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
                    // Top-level fence → the Style A container panel. The panel's
                    // head carries `raw` for the click-to-copy map (trailing
                    // fence newline trimmed to match the copied text).
                    let raw = code.trim_end_matches('\n').to_string();
                    self.lines.extend(code_panel(hl, lang, raw));
                } else {
                    // Nested in a list/quote: keep the quote-bar / indent chrome
                    // (no panel) so blockquote and list nesting still read.
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
                if let Some(t) = self.table.take() {
                    self.render_table(t);
                }
            }
            _ => {}
        }
    }

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
        // wrapped_rows[ri] = (wrapped_cells, is_header) where wrapped_cells[ci]
        // = Vec of visual rows (each a Vec<Span> already padded to widths[ci])
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
                let wrapped = crate::text::wrap_content(&base_spans, widths[c]);
                // pad each visual row to the column width with alignment
                let aligned = align_rows(wrapped, widths[c], table.alignments.get(c).copied());
                wrapped_cells.push(aligned);
            }
            wrapped_rows.push((wrapped_cells, *is_header));
        }

        // --- Steps 4+5+6: emit border + cell lines ---
        let border_top = border_line(&widths, BorderKind::Top);
        let border_mid = border_line(&widths, BorderKind::Mid);
        let border_bot = border_line(&widths, BorderKind::Bottom);

        self.lines.push(BodyLine {
            line: Line::from(border_top),
            kind: BodyKind::Table,
            source: None,
        });
        for (ri, (cells, _is_header)) in wrapped_rows.iter().enumerate() {
            let row_h = cells.iter().map(|c| c.len()).max().unwrap_or(1).max(1);
            for vh in 0..row_h {
                let mut spans: Vec<Span<'static>> = Vec::new();
                spans.push(Span::styled(
                    glyph::TABLE_V.to_string(),
                    Style::new().fg(color::TABLE_BORDER),
                ));
                for c in 0..ncols {
                    let cell_rows = &cells[c];
                    let row_spans = cell_rows.get(vh).cloned().unwrap_or_else(|| {
                        // blank padding row: spaces of width widths[c]. No fg
                        // color — this is whitespace fill, not a border glyph
                        // (TABLE_BORDER on a space is visually identical but
                        // semantically wrong; plain Style::new() is correct).
                        vec![Span::styled(" ".repeat(widths[c]), Style::new())]
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

    fn finish(mut self) -> Vec<BodyLine> {
        self.flush();
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use unicode_width::UnicodeWidthStr;

    fn spans(lines: &[ratatui::text::Line<'static>]) -> Vec<(String, ratatui::style::Style)> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style)))
            .collect()
    }

    #[test]
    fn plain_prose_is_one_txt_line() {
        let lines = render_markdown("just a sentence.");
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(color::TXT)));
    }

    #[test]
    fn heading_is_accent_bold() {
        let lines = render_markdown("# Title");
        let (_, style) = spans(&lines)
            .into_iter()
            .find(|(t, _)| t.contains("Title"))
            .unwrap();
        assert_eq!(style.fg, Some(color::CHAT_ACCENT));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn bold_and_inline_code_are_styled() {
        let lines = render_markdown("a **b** `c`");
        let s = spans(&lines);
        assert!(s
            .iter()
            .any(|(t, st)| t == "b" && st.add_modifier.contains(Modifier::BOLD)));
        assert!(s
            .iter()
            .any(|(t, st)| t == "c" && st.fg == Some(color::MD_CODE)));
    }

    #[test]
    fn list_items_render_with_bullets() {
        let lines = render_markdown("- one\n- two");
        assert_eq!(lines.len(), 2);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text
            .iter()
            .any(|t: &String| t.contains(glyph::BULLET) && t.contains("one")));
    }

    #[test]
    fn fenced_code_is_highlighted_by_language() {
        let lines = render_markdown("```rust\nfn x() {}\n```");
        let has_kw = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(color::SYN_KEYWORD))
        });
        assert!(has_kw, "a rust fence must be syntax-highlighted");
    }

    #[test]
    fn unknown_fence_is_plain_text() {
        // An unknown/empty fence is not syntax-highlighted: no span carries a
        // SYN_* hue. (The container chrome — bar/tag — is dim, so we assert the
        // absence of highlighting rather than "everything is TXT".)
        let lines = render_markdown("```\nplain body\n```");
        assert!(lines.iter().all(|l| l
            .spans
            .iter()
            .all(|s| s.style.fg != Some(color::SYN_KEYWORD)
                && s.style.fg != Some(color::SYN_TYPE)
                && s.style.fg != Some(color::SYN_FUNC))));
        // and the body text is present, TXT-colored
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(joined.contains("plain body"));
    }

    #[test]
    fn top_level_fence_has_bar_and_language_header() {
        let lines = render_markdown("```rust\nfn x() {}\n```");
        // header row carries the language tag…
        let joined: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(joined.iter().any(|t: &String| t.contains("rust")));
        // …and every panel row starts with the code bar.
        assert!(
            lines.iter().all(|l| l
                .spans
                .first()
                .map(|s| s.content.contains(glyph::CODE_BAR))
                .unwrap_or(false)),
            "each code panel row must start with the code bar"
        );
        // background panel is applied.
        assert!(lines
            .iter()
            .all(|l| l.spans.iter().any(|s| s.style.bg == Some(color::CODE_BG))));
    }

    #[test]
    fn fenced_code_in_blockquote_keeps_quote_bar() {
        let lines = render_markdown("> ```rust\n> fn x() {}\n> ```");
        // every rendered code line must carry the quote bar prefix
        assert!(
            lines.iter().all(|l| l
                .spans
                .first()
                .map(|s| s.content.contains(glyph::QUOTE_BAR))
                .unwrap_or(false)),
            "blockquote fence lines must start with the quote bar"
        );
    }

    #[test]
    fn fenced_code_in_list_is_indented() {
        let lines = render_markdown("- item\n\n  ```rust\n  fn x() {}\n  ```");
        // at least one code line is indented (leading spaces) under the list
        assert!(
            lines.iter().any(|l| l
                .spans
                .first()
                .map(|s| s.content.starts_with(' '))
                .unwrap_or(false)),
            "list fence lines must be indented under the item"
        );
    }

    #[test]
    fn link_text_is_md_link_underlined() {
        let lines = render_markdown("see [docs](http://x)");
        let s = spans(&lines);
        assert!(
            s.iter().any(|(t, st)| t.contains("docs")
                && st.fg == Some(color::MD_LINK)
                && st
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)),
            "link text must render in MD_LINK, underlined"
        );
    }

    #[test]
    fn inline_code_inside_link_is_md_code_not_md_link() {
        let lines = render_markdown("[`c`](http://x)");
        let s = spans(&lines);
        assert!(
            s.iter()
                .any(|(t, st)| t == "c" && st.fg == Some(color::MD_CODE)),
            "inline code inside a link must render in MD_CODE, not MD_LINK"
        );
    }

    #[test]
    fn heading_inside_blockquote_is_accent_bold_not_dim() {
        let lines = render_markdown("> # Title");
        let (_, style) = spans(&lines)
            .into_iter()
            .find(|(t, _)| t.contains("Title"))
            .unwrap();
        assert_eq!(
            style.fg,
            Some(color::CHAT_ACCENT),
            "heading fg must beat blockquote dim"
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn strikethrough_is_crossed_out() {
        let lines = render_markdown("~~struck~~");
        let s = spans(&lines);
        assert!(
            s.iter()
                .any(|(t, st)| t == "struck" && st.add_modifier.contains(Modifier::CROSSED_OUT)),
            "strikethrough text must carry CROSSED_OUT"
        );
    }

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
    fn table_cell_text_does_not_leak_as_prose() {
        // The accumulator must CAPTURE cell text into the grid (rendered as
        // BodyKind::Table), not drop it into the prose `cur` buffer (which
        // would emit it as BodyKind::Prose). Before Task 4's layout landed this
        // asserted the text was absent entirely; now the table renders, so the
        // invariant is "captured as Table, NOT leaked as Prose."
        let body = render_body("| H1 | H2 |\n| --- | --- |\n");
        let (in_table, in_prose) = body.iter().fold((false, false), |(tbl, prose), b| {
            let has_h = b.line.spans.iter().any(|s| s.content.contains("H1") || s.content.contains("H2"));
            match b.kind {
                BodyKind::Table => (tbl || has_h, prose),
                BodyKind::Prose => (tbl, prose || has_h),
                _ => (tbl, prose),
            }
        });
        assert!(in_table, "cell text must appear in a Table line (rendered): captured+emitted");
        assert!(!in_prose, "cell text must NOT leak into a Prose line: bad routing");
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
        // A header cell span: fg == TABLE_HEADER AND bold. Filter by the style
        // invariant directly (not span ordering) so the test doesn't rely on
        // "the first H-containing span happens to be the cell text."
        let header_ok = body
            .iter()
            .flat_map(|b| b.line.spans.iter())
            .any(|s| s.content.contains('H')
                && s.style.fg == Some(color::TABLE_HEADER)
                && s.style.add_modifier.contains(Modifier::BOLD));
        assert!(header_ok, "header cell text must be TABLE_HEADER + bold");
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
        // The 40-char cell wraps at the 30 cap: no single CONTENT span in a
        // body row exceeds 30 display columns. (Border spans are ─/│ runs and
        // are excluded — they are deliberately wider than the content column
        // because border_line adds +2 padding dashes per column, so measuring
        // them would test the border, not the cap.)
        let long = "a".repeat(40);
        let md = format!("| {} |\n| --- |\n| {} |\n", long, long);
        let body = render_body(&md);
        let max_content_w = body
            .iter()
            .flat_map(|b| b.line.spans.iter())
            .filter(|s| !s.content.chars().all(|c| c == '─') && !s.content.chars().all(|c| c == '│'))
            .map(|s| s.content.width())
            .max()
            .unwrap_or(0);
        assert!(
            max_content_w <= 30,
            "cell content exceeded the 30 cap: {max_content_w}"
        );
    }
}
