//! Faithful buffer → HTML converter for the marketing site (feature `web-capture`).
//! Walks a rendered `TestBackend` buffer and emits a colored `<pre>` that mirrors
//! the terminal grid. Not compiled into the product binary.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

/// Resolve a ratatui `Color` to a CSS hex, or `None` to inherit the `<pre>` default.
/// `Reset`/`Indexed`/named colors inherit — the design tokens are all `Rgb`, and the
/// `<pre>` carries the default text/background, so inheriting is exactly right.
fn css(color: Color) -> Option<String> {
    match color {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

fn push_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(ch),
        }
    }
}

/// Convert a rendered buffer into a colored `<pre>` mirroring the terminal grid.
pub fn buffer_to_html(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::from("<pre class=\"tui\">");
    for y in area.y..area.y + area.height {
        // Rows are *separated* by `\n`, not terminated: emit the newline before
        // every row except the first, so `</pre>` follows the last row's content
        // with no trailing blank line (which `<pre>` would render).
        if y > area.y {
            out.push('\n');
        }
        let mut x = area.x;
        // Open-span state for the current run.
        let mut run = String::new();
        let mut cur: Option<(Option<String>, Option<String>)> = None;

        let flush = |out: &mut String,
                     run: &mut String,
                     cur: &mut Option<(Option<String>, Option<String>)>| {
            if let Some((fg, bg)) = cur.take() {
                let mut style = String::new();
                if let Some(fg) = fg {
                    let _ = write!(style, "color:{fg};");
                }
                if let Some(bg) = bg {
                    let _ = write!(style, "background:{bg};");
                }
                if style.is_empty() {
                    out.push_str(run);
                } else {
                    let _ = write!(
                        out,
                        "<span style=\"{}\">{}</span>",
                        style.trim_end_matches(';'),
                        run
                    );
                }
            } else {
                out.push_str(run);
            }
            run.clear();
        };

        while x < area.x + area.width {
            let cell = &buf[(x, y)];
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let key = (css(fg), css(bg));
            if cur.as_ref() != Some(&key) {
                flush(&mut out, &mut run, &mut cur);
                cur = Some(key);
            }
            let sym = cell.symbol();
            push_escaped(&mut run, sym);
            // Advance by the glyph's display width so a 2-col glyph skips its
            // reserved continuation cell (ratatui leaves it blank).
            let w = sym.width().max(1) as u16;
            x += w;
        }
        flush(&mut out, &mut run, &mut cur);
    }
    out.push_str("</pre>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn emits_rgb_span_and_escapes_html() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.set_string(
            0,
            0,
            "a<b",
            Style::default().fg(Color::Rgb(0x58, 0xa6, 0xff)),
        );
        let html = buffer_to_html(&buf);
        assert!(html.starts_with("<pre class=\"tui\">"));
        assert!(html.contains("color:#58a6ff"));
        assert!(html.contains("a&lt;b"));
        assert!(html.trim_end().ends_with("</pre>"));
    }

    #[test]
    fn reversed_swaps_fg_and_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        buf.set_string(
            0,
            0,
            "x",
            Style::default()
                .fg(Color::Rgb(0x0d, 0x11, 0x17))
                .bg(Color::Rgb(0x58, 0xa6, 0xff))
                .add_modifier(Modifier::REVERSED),
        );
        let html = buffer_to_html(&buf);
        // After REVERSED swap, the glyph paints in the (former) bg color.
        assert!(html.contains("color:#58a6ff"));
        assert!(html.contains("background:#0d1117"));
    }

    #[test]
    fn rows_are_separated_not_terminated() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 2));
        buf.set_string(0, 0, "a", Style::default());
        buf.set_string(0, 1, "b", Style::default());
        let html = buffer_to_html(&buf);
        // Strip the known wrapper and compare the exact body: two rows must be
        // *joined* by a single `\n` with NO trailing newline before `</pre>`.
        let body = html
            .strip_prefix("<pre class=\"tui\">")
            .and_then(|s| s.strip_suffix("</pre>"))
            .expect("wrapper present");
        assert_eq!(body, "a\nb");
    }
}
