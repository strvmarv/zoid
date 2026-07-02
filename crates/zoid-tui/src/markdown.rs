//! Render assistant/user message bodies from markdown to ratatui `Line`s
//! (spec §3.5). `pulldown-cmark` parses; inline styles (headings, bold, italic,
//! inline `code`, lists, blockquotes, links) map to §16 design tokens, and
//! fenced ```lang blocks reuse the Ⓡ3 highlighter (`highlight_lines`). Nesting is
//! depth-capped; anything unexpected falls back to plain text. Wrapping is the
//! caller's job (`Wrap { trim: false }`) — we only build styled spans/lines.

use crate::syntax_view::highlight_lines;
use crate::tokens::{color, glyph};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use zoid_syntax::Language;

/// Max container nesting (lists + blockquotes) before we bail to plain text.
const MAX_DEPTH: usize = 8;

/// What a rendered body line is, so downstream layout can treat it correctly:
/// prose word-wraps with a hanging indent; code lines never word-wrap (their
/// leading whitespace is significant) and `CodeHead` is where the copy hint
/// attaches (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Prose,
    CodeHead,
    Code,
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
    for ev in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH) {
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
            _ => {}
        }
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
}
