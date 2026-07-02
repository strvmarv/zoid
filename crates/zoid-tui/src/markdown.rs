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

/// Render markdown `source` into owned ratatui `Line`s. Most non-empty input
/// yields at least one line; whitespace-only input can yield an empty vec (the
/// caller — `push_message` — handles an empty body by emitting the prefix
/// alone). Empty input also yields an empty vec.
pub fn render_markdown(source: &str) -> Vec<Line<'static>> {
    let mut b = Builder::default();
    for ev in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH) {
        b.event(ev);
        if b.bail {
            return plain_lines(source);
        }
    }
    b.finish()
}

/// One TXT-styled `Line` per source row — the parse-issue / over-nesting fallback.
fn plain_lines(source: &str) -> Vec<Line<'static>> {
    source
        .split('\n')
        .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(color::TXT))))
        .collect()
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
    lines: Vec<Line<'static>>,
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
        self.lines.push(Line::from(spans));
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

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.flush(),
            Tag::Heading { .. } => {
                self.flush();
                self.heading = true;
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike = true,
            Tag::Link { .. } => self.link = true,
            Tag::BlockQuote(_) => {
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
                self.cur
                    .push(Span::styled(format!("{indent}{marker}"), Style::new().fg(color::DIM)));
            }
            Tag::CodeBlock(kind) => {
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
                    self.lines.extend(hl); // top-level fence — unchanged
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
                        self.lines.push(Line::from(spans));
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn spans(lines: &[ratatui::text::Line<'static>]) -> Vec<(String, ratatui::style::Style)> {
        lines.iter().flat_map(|l| l.spans.iter().map(|s| (s.content.to_string(), s.style))).collect()
    }

    #[test]
    fn plain_prose_is_one_txt_line() {
        let lines = render_markdown("just a sentence.");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.iter().all(|s| s.style.fg == Some(color::TXT)));
    }

    #[test]
    fn heading_is_accent_bold() {
        let lines = render_markdown("# Title");
        let (_, style) = spans(&lines).into_iter().find(|(t, _)| t.contains("Title")).unwrap();
        assert_eq!(style.fg, Some(color::CHAT_ACCENT));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn bold_and_inline_code_are_styled() {
        let lines = render_markdown("a **b** `c`");
        let s = spans(&lines);
        assert!(s.iter().any(|(t, st)| t == "b" && st.add_modifier.contains(Modifier::BOLD)));
        assert!(s.iter().any(|(t, st)| t == "c" && st.fg == Some(color::MD_CODE)));
    }

    #[test]
    fn list_items_render_with_bullets() {
        let lines = render_markdown("- one\n- two");
        assert_eq!(lines.len(), 2);
        let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert!(text.iter().any(|t: &String| t.contains(glyph::BULLET) && t.contains("one")));
    }

    #[test]
    fn fenced_code_is_highlighted_by_language() {
        let lines = render_markdown("```rust\nfn x() {}\n```");
        let has_kw = lines.iter().any(|l| l.spans.iter().any(|s| s.style.fg == Some(color::SYN_KEYWORD)));
        assert!(has_kw, "a rust fence must be syntax-highlighted");
    }

    #[test]
    fn unknown_fence_is_plain_text() {
        let lines = render_markdown("```\nplain body\n```");
        assert!(lines.iter().all(|l| l.spans.iter().all(|s| s.style.fg == Some(color::TXT))));
    }

    #[test]
    fn fenced_code_in_blockquote_keeps_quote_bar() {
        let lines = render_markdown("> ```rust\n> fn x() {}\n> ```");
        // every rendered code line must carry the quote bar prefix
        assert!(lines.iter().all(|l| l.spans.first()
            .map(|s| s.content.contains(glyph::QUOTE_BAR))
            .unwrap_or(false)),
            "blockquote fence lines must start with the quote bar");
    }

    #[test]
    fn fenced_code_in_list_is_indented() {
        let lines = render_markdown("- item\n\n  ```rust\n  fn x() {}\n  ```");
        // at least one code line is indented (leading spaces) under the list
        assert!(lines.iter().any(|l| l.spans.first()
            .map(|s| s.content.starts_with(' '))
            .unwrap_or(false)),
            "list fence lines must be indented under the item");
    }

    #[test]
    fn link_text_is_md_link_underlined() {
        let lines = render_markdown("see [docs](http://x)");
        let s = spans(&lines);
        assert!(s.iter().any(|(t, st)| t.contains("docs")
            && st.fg == Some(color::MD_LINK)
            && st.add_modifier.contains(ratatui::style::Modifier::UNDERLINED)),
            "link text must render in MD_LINK, underlined");
    }

    #[test]
    fn inline_code_inside_link_is_md_code_not_md_link() {
        let lines = render_markdown("[`c`](http://x)");
        let s = spans(&lines);
        assert!(s.iter().any(|(t, st)| t == "c" && st.fg == Some(color::MD_CODE)),
            "inline code inside a link must render in MD_CODE, not MD_LINK");
    }

    #[test]
    fn heading_inside_blockquote_is_accent_bold_not_dim() {
        let lines = render_markdown("> # Title");
        let (_, style) = spans(&lines).into_iter().find(|(t, _)| t.contains("Title")).unwrap();
        assert_eq!(style.fg, Some(color::CHAT_ACCENT), "heading fg must beat blockquote dim");
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn strikethrough_is_crossed_out() {
        let lines = render_markdown("~~struck~~");
        let s = spans(&lines);
        assert!(s.iter().any(|(t, st)| t == "struck" && st.add_modifier.contains(Modifier::CROSSED_OUT)),
            "strikethrough text must carry CROSSED_OUT");
    }
}
