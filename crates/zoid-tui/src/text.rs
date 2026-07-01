//! Terminal-cell-aware text fitting. Widths are measured in **display columns**
//! (via `unicode-width`, the same crate ratatui's own truncator uses), not chars
//! — an emoji or CJK glyph occupies two columns, so char counting under-measures
//! and lets a row we believe fits get clipped by ratatui at render time.

use crate::tokens::glyph;
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
}
