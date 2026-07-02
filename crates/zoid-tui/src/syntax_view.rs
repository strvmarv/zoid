//! Pure render helper for Ⓡ3 tree-sitter highlighting. Turns source text into
//! ratatui `Line`s colored from the §16 syntax palette. No `Frame`; unit-tested
//! independently. Live wiring (zoom collapse, file peek, symbol select) lands
//! in P4c/P4d which consume `zoid_syntax` directly.

use crate::tokens::color;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use zoid_syntax::{highlight, HlKind, HlSpan, Language};

/// Map a syntax bucket to its §16 color.
pub fn syn_color(kind: HlKind) -> Color {
    match kind {
        HlKind::Keyword => color::SYN_KEYWORD,
        HlKind::Func => color::SYN_FUNC,
        HlKind::Type => color::SYN_TYPE,
        HlKind::Str => color::SYN_STRING,
        HlKind::Number => color::SYN_NUMBER,
        HlKind::Comment => color::SYN_COMMENT,
    }
}

/// Highlight `source` into owned ratatui `Line`s (one per source row). Spans
/// covered by an `HlSpan` get the syntax color; gaps render in `color::TXT`.
pub fn highlight_lines(source: &str, lang: Language) -> Vec<Line<'static>> {
    let spans = highlight(source, lang); // sorted, non-overlapping byte ranges
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut span_idx = 0usize;

    // Iterate the source per line, slicing against the byte-range highlight spans.
    let mut line_start = 0usize; // byte offset of the current line's start
    for raw_line in lines_of(source) {
        let line_end = line_start + raw_line.len();
        let mut cur = line_start;
        let mut out: Vec<Span<'static>> = Vec::new();

        // Advance past spans that ended before this line.
        while span_idx < spans.len() && spans[span_idx].end <= line_start {
            span_idx += 1;
        }

        let mut i = span_idx;
        while cur < line_end {
            // Find the next span that overlaps [cur, line_end).
            while i < spans.len() && spans[i].end <= cur {
                i += 1;
            }
            match spans.get(i) {
                Some(sp) if sp.start < line_end => {
                    let s = clamp(sp, cur, line_end);
                    if s.0 > cur {
                        out.push(plain(&source[cur..s.0]));
                    }
                    out.push(Span::styled(
                        source[s.0..s.1].to_string(),
                        Style::new().fg(syn_color(sp.kind)),
                    ));
                    cur = s.1;
                    if sp.end <= line_end {
                        i += 1;
                    }
                }
                _ => {
                    out.push(plain(&source[cur..line_end]));
                    cur = line_end;
                }
            }
        }
        if out.is_empty() {
            out.push(plain("")); // preserve empty lines
        }
        lines.push(Line::from(out));
        line_start = line_end + 1; // skip the '\n'
    }
    lines
}

fn plain(s: &str) -> Span<'static> {
    Span::styled(s.to_string(), Style::new().fg(color::TXT))
}

/// Intersection of an `HlSpan` with `[lo, hi)` as byte offsets.
fn clamp(sp: &HlSpan, lo: usize, hi: usize) -> (usize, usize) {
    (sp.start.max(lo), sp.end.min(hi))
}

/// Lines WITHOUT the trailing newline, dropping a single trailing empty
/// segment so "a\n" → ["a"], not ["a", ""], and "" → [].
fn lines_of(source: &str) -> Vec<&str> {
    if source.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<&str> = source.split('\n').collect();
    if source.ends_with('\n') {
        v.pop(); // drop the empty segment after the final newline
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::color;
    use zoid_syntax::Language;

    #[test]
    fn highlight_lines_splits_rows_and_colors_keywords() {
        let lines = highlight_lines("fn x() {}\nlet y = 1;\n", Language::Rust);
        // one Line per source row (trailing newline produces no extra empty row)
        assert_eq!(lines.len(), 2);
        // the first line contains a keyword-colored span ("fn")
        let has_keyword = lines[0]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(color::SYN_KEYWORD) && s.content.contains("fn"));
        assert!(has_keyword, "`fn` should be keyword-colored");
    }

    #[test]
    fn plaintext_renders_one_txt_span_per_line() {
        let lines = highlight_lines("hello\nworld\n", Language::PlainText);
        assert_eq!(lines.len(), 2);
        assert!(lines[0]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(color::TXT)));
    }
}
