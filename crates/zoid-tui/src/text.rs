//! Terminal-cell-aware text fitting. Widths are measured in **display columns**
//! (via `unicode-width`, the same crate ratatui's own truncator uses), not chars
//! — an emoji or CJK glyph occupies two columns, so char counting under-measures
//! and lets a row we believe fits get clipped by ratatui at render time.

use crate::tokens::glyph;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate `s` to at most `max` display columns, marking the cut with the §16
/// ellipsis glyph (`'…'`, one column wide). Never splits a glyph; the result's
/// display width is always `<= max`.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis, then take whole glyphs until the next
    // would exceed that budget (a trailing wide glyph may leave the result one
    // column short — that's fine, width stays <= max).
    let budget = max - 1;
    let mut used = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(ch);
    }
    out.push(glyph::ELLIPSIS);
    out
}

/// Truncate the *beginning* of `s` to at most `max` display columns, keeping
/// the end (where the most pertinent info lives, e.g. a file path's filename)
/// and marking the cut with the §16 ellipsis glyph. Never splits a glyph;
/// the result's display width is always `<= max`.
pub(crate) fn truncate_start(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1; // reserve one column for the leading ellipsis
    let total = UnicodeWidthStr::width(s);
    let need_to_drop = total.saturating_sub(budget);
    // Walk forward, dropping glyphs until we've shed enough display width.
    let mut dropped = 0usize;
    let mut rest = s;
    for (i, ch) in s.char_indices() {
        if dropped >= need_to_drop {
            rest = &s[i..];
            break;
        }
        dropped += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    // If the remaining text is still too wide (a wide glyph straddled the
    // boundary), trim trailing glyphs until it fits.
    let mut out = format!("{}{}", glyph::ELLIPSIS, rest);
    while UnicodeWidthStr::width(out.as_str()) > max {
        // Remove the last char and retry.
        if out.pop().is_some() {
            // popped one char; re-check width
        } else {
            break;
        }
    }
    out
}

/// Format an epoch-millis timestamp as 24-hour `HH:MM` (no date), shifted by
/// `tz_offset_secs` (local UTC offset, supplied by the bin). Pure integer math —
/// no timezone library, so snapshots stay reproducible by passing offset 0.
pub(crate) fn hhmm(epoch_ms: i64, tz_offset_secs: i32) -> String {
    let secs = epoch_ms.div_euclid(1000) + tz_offset_secs as i64;
    let sod = secs.rem_euclid(86_400); // seconds-of-day, always in [0, 86400)
    format!("{:02}:{:02}", sod / 3600, (sod % 3600) / 60)
}

/// Right-pad `s` with spaces to exactly `width` display columns. If `s` is
/// already at least `width` wide it is returned unchanged (callers truncate
/// first when a hard cap is needed).
pub(crate) fn pad_to(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

/// Break styled `content` into rows no wider than `width` (display columns),
/// preserving each span's style, breaking on spaces (dropping the break's
/// whitespace), and hard-splitting any single token longer than `width`.
/// Returns at least one (possibly empty) row. Used by `push_message` (prose
/// wrapping) and the GFM table cell-wrapping path (spec §2 step 3).
pub(crate) fn wrap_content(content: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    // Tokenize into (text, style, is_space) runs, split at whitespace boundaries.
    let mut toks: Vec<(String, Style, bool)> = Vec::new();
    for s in content {
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
                continue; // no leading spaces at the start of a wrapped row
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
            // trim any trailing spaces before wrapping the row (cur_w resets to 0
            // right after, so no need to track its decrement here)
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
            // Token longer than the column — hard-split by DISPLAY WIDTH, not char
            // count, so wide (CJK/emoji) glyphs never overflow the column and force
            // a widget re-wrap. Accumulate chars until the next one would exceed the
            // remaining width, then flush the row (always at least one char/row).
            let mut piece = String::new();
            let mut piece_w = 0usize;
            for ch in text.chars() {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_untouched() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn ascii_truncates_with_ellipsis() {
        assert_eq!(truncate("abcdef", 4), format!("abc{}", glyph::ELLIPSIS));
        assert_eq!(UnicodeWidthStr::width(truncate("abcdef", 4).as_str()), 4);
    }

    #[test]
    fn max_zero_is_empty() {
        assert_eq!(truncate("abc", 0), "");
    }

    /// A wide glyph counts as two columns: char-based truncation would have kept
    /// it, overflowing the budget. Result width must never exceed max.
    #[test]
    fn wide_glyph_respects_display_width() {
        let s = "a😀b😀c"; // each 😀 is 2 cols → total display width 7
        for max in 0..=8 {
            let out = truncate(s, max);
            assert!(
                UnicodeWidthStr::width(out.as_str()) <= max,
                "truncate({s:?}, {max}) = {out:?} exceeded {max} cols"
            );
        }
    }

    /// A *leading* wide glyph at max 0/1/2 is the tightest boundary: budget-1 can
    /// be smaller than the first glyph, so it must be dropped whole, never split.
    #[test]
    fn leading_wide_glyph_at_tiny_max() {
        assert_eq!(truncate("😀xyz", 0), "");
        // max 1 → budget 0, the 2-col glyph can't fit → ellipsis only (width 1).
        let one = truncate("😀xyz", 1);
        assert_eq!(one, glyph::ELLIPSIS.to_string());
        assert_eq!(UnicodeWidthStr::width(one.as_str()), 1);
        // max 2 → budget 1, still can't fit the 2-col glyph → ellipsis only.
        let two = truncate("😀xyz", 2);
        assert!(UnicodeWidthStr::width(two.as_str()) <= 2);
        assert_eq!(two, glyph::ELLIPSIS.to_string());
    }

    #[test]
    fn truncate_start_keeps_end() {
        // "src/main.rs" is 11 cols; max 10 → budget 9 → drop 2 cols ("sr") → "…c/main.rs"
        assert_eq!(truncate_start("src/main.rs", 10), "…c/main.rs");
        assert_eq!(
            UnicodeWidthStr::width(truncate_start("src/main.rs", 10).as_str()),
            10
        );
        // Longer path: the filename at the end is preserved.
        let long = "crates/zoid-tui/src/render.rs";
        let out = truncate_start(long, 20);
        assert!(out.ends_with("render.rs"));
        assert!(out.starts_with(glyph::ELLIPSIS));
    }

    #[test]
    fn truncate_start_short_string_untouched() {
        assert_eq!(truncate_start("abc", 10), "abc");
    }

    #[test]
    fn truncate_start_max_zero_is_empty() {
        assert_eq!(truncate_start("abc", 0), "");
    }

    #[test]
    fn truncate_start_respects_display_width() {
        let s = "a/very/deeply/nested/path/file.rs";
        for max in 1..=40 {
            let out = truncate_start(s, max);
            assert!(
                UnicodeWidthStr::width(out.as_str()) <= max,
                "truncate_start({s:?}, {max}) = {out:?} exceeded {max} cols"
            );
        }
    }

    #[test]
    fn hhmm_formats_24h_with_offset() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(hhmm(0, 0), "00:00");
        // 13:45:00 UTC (49_500_000 ms) at offset 0
        assert_eq!(hhmm(49_500_000, 0), "13:45");
        // Same instant, +2h offset -> 15:45
        assert_eq!(hhmm(49_500_000, 2 * 3600), "15:45");
        // Negative offset wraps across midnight: 00:30 UTC, -1h -> 23:30
        assert_eq!(hhmm(1_800_000, -3600), "23:30");
    }

    #[test]
    fn pad_uses_display_width() {
        // "😀" is 2 cols, so pad_to width 5 adds 3 spaces, not 4.
        assert_eq!(pad_to("😀", 5), "😀   ");
        assert_eq!(UnicodeWidthStr::width(pad_to("😀", 5).as_str()), 5);
    }

    use ratatui::text::Span;

    #[test]
    fn wrap_content_short_content_is_one_row() {
        let rows = wrap_content(&[Span::raw("hello world")], 30);
        assert_eq!(rows.len(), 1);
        let joined: String = rows[0].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn wrap_content_breaks_at_word_boundary() {
        // "alpha bravo" is 11 cols; at width 12 it fits on row 0, and
        // "alpha bravo charlie" (19 cols) overflows → "charlie" wraps to row 1.
        // (Width 10 would be wrong: 11 > 10 means "alpha bravo" itself overflows.)
        let rows = wrap_content(&[Span::raw("alpha bravo charlie")], 12);
        assert_eq!(rows.len(), 2, "should wrap into 2 rows");
        let r0: String = rows[0].iter().map(|s| s.content.to_string()).collect();
        let r1: String = rows[1].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(r0, "alpha bravo");
        assert_eq!(r1, "charlie");
    }

    #[test]
    fn wrap_content_hard_splits_overlong_token() {
        // A single 20-char word wider than width 8 must hard-split, not overflow.
        let rows = wrap_content(&[Span::raw("abcdefghijklmnopqrst")], 8);
        assert!(rows.len() >= 3, "a 20-char word at width 8 needs >=3 rows");
        // total content preserved
        let total: String = rows.iter().flat_map(|r| r.iter().map(|s| s.content.to_string())).collect();
        assert_eq!(total, "abcdefghijklmnopqrst");
    }
}
