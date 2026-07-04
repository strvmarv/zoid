//! Pure geometry + line↔message mapping for the conversation scrollbar. Kept
//! out of the renderer so the math is unit-testable (spec §13 determinism),
//! like `motion::spinner_frame`.

/// Vertical scrollbar thumb geometry. `offset` ∈ [0, max_scroll], `track_h` =
/// track height in rows, `content_len` = total body lines. Returns
/// (thumb_start, thumb_len) in track rows, both within [0, track_h]. Always a
/// ≥1-row thumb when the track is non-empty; a full-height thumb when everything
/// fits (max_scroll == 0). The thumb sits flush at the bottom when
/// offset == max_scroll.
pub fn scrollbar_thumb(offset: u16, max_scroll: u16, track_h: u16, content_len: u16) -> (u16, u16) {
    if track_h == 0 {
        return (0, 0);
    }
    if max_scroll == 0 || content_len <= track_h {
        return (0, track_h); // everything fits → full-height thumb
    }
    // Thumb length ∝ viewport/content. viewport == track_h.
    let len = (((track_h as u32 * track_h as u32) + content_len as u32 / 2) / content_len as u32)
        .max(1)
        // `.min(track_h)` is defensive; unreachable here since the line-above guard
        // ensures content_len > track_h, so track_h²/content_len < track_h.
        .min(track_h as u32) as u16;
    let travel = track_h - len; // rows the thumb can move
    let start =
        ((travel as u32 * offset as u32 + max_scroll as u32 / 2) / max_scroll as u32) as u16;
    (start.min(travel), len)
}

/// Index of the message occupying `line`: the last message whose start ≤ `line`.
/// Returns 0 when `starts` is empty or `line` precedes the first start. When
/// several messages share a start line (Summary collapses a turn onto one line),
/// returns the last of them.
pub fn msg_at_line(starts: &[usize], line: usize) -> usize {
    starts.partition_point(|&s| s <= line).saturating_sub(1)
}

/// First body line of message `idx`. Clamps to 0 when `idx` is out of range or
/// `starts` is empty.
pub fn line_of_msg(starts: &[usize], idx: usize) -> usize {
    starts.get(idx).copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_thumb_when_content_fits() {
        // content_len <= track_h: nothing to scroll → full-height thumb at top.
        assert_eq!(scrollbar_thumb(0, 0, 10, 8), (0, 10));
        assert_eq!(scrollbar_thumb(0, 0, 10, 10), (0, 10));
    }

    #[test]
    fn thumb_is_proportional_and_clamped() {
        // track_h 10, content 40 → thumb_len = round(10*10/40) = 3 (>=1).
        let (_, len) = scrollbar_thumb(0, 30, 10, 40);
        assert_eq!(len, 3);
        // never zero even for huge content
        let (_, len) = scrollbar_thumb(0, 9990, 10, 10000);
        assert_eq!(len, 1);
        // thumb stays within the track
        let (start, len) = scrollbar_thumb(30, 30, 10, 40);
        assert!(start + len <= 10, "thumb overflows track: {start}+{len}");
        // offset > max_scroll (contract violation) is defensively clamped, not
        // allowed to push the thumb off the bottom.
        assert_eq!(scrollbar_thumb(50, 30, 10, 40), (7, 3));
    }

    #[test]
    fn single_row_track_and_near_full_content() {
        // track_h == 1: a lone thumb row, no travel.
        assert_eq!(scrollbar_thumb(0, 5, 1, 100), (0, 1));
        // content_len == track_h + 1: thumb one row short of full, one row travel;
        // at offset == max it sits flush at the bottom (start == travel == 1).
        assert_eq!(scrollbar_thumb(3, 3, 10, 11), (1, 9));
    }

    #[test]
    fn thumb_at_top_and_bottom() {
        // offset 0 → thumb at row 0
        assert_eq!(scrollbar_thumb(0, 30, 10, 40).0, 0);
        // offset == max_scroll → thumb flush at the bottom (start = track_h - len)
        let (start, len) = scrollbar_thumb(30, 30, 10, 40);
        assert_eq!(start, 10 - len);
    }

    #[test]
    fn zero_track_or_content_is_safe() {
        assert_eq!(scrollbar_thumb(0, 0, 0, 0), (0, 0));
        assert_eq!(scrollbar_thumb(5, 5, 0, 100), (0, 0));
        // content_len == 0 with a live track: falls in the "fits" branch (no
        // divide-by-zero) → full-height thumb.
        assert_eq!(scrollbar_thumb(0, 5, 10, 0), (0, 10));
    }

    #[test]
    fn msg_at_line_finds_the_containing_message() {
        // messages start at lines [0, 4, 9]
        let starts = [0usize, 4, 9];
        assert_eq!(msg_at_line(&starts, 0), 0);
        assert_eq!(msg_at_line(&starts, 3), 0); // still inside msg 0
        assert_eq!(msg_at_line(&starts, 4), 1); // boundary → msg 1
        assert_eq!(msg_at_line(&starts, 100), 2); // past the end → last msg
    }

    #[test]
    fn msg_at_line_handles_collapsed_and_empty() {
        // Summary: msgs 0..2 collapse onto turn line 0, msg 3 onto line 1.
        let starts = [0usize, 0, 0, 1];
        assert_eq!(msg_at_line(&starts, 0), 2, "last msg sharing line 0");
        assert_eq!(msg_at_line(&starts, 1), 3);
        // empty → 0
        assert_eq!(msg_at_line(&[], 5), 0);
    }

    #[test]
    fn line_of_msg_maps_back_and_clamps() {
        let starts = [0usize, 4, 9];
        assert_eq!(line_of_msg(&starts, 0), 0);
        assert_eq!(line_of_msg(&starts, 2), 9);
        assert_eq!(line_of_msg(&starts, 99), 0, "out of range → 0");
        assert_eq!(line_of_msg(&[], 0), 0);
    }
}
