//! Active context management (ACM-1): plan and summarize tool-result
//! compactions. Pure — the agent loop records the results as events.

use crate::economy::estimate_tokens;

/// Summarize an oversized tool-result body: keep the first `head_lines` lines
/// verbatim, then a one-line footer noting the elided tail. Returns the input
/// unchanged whenever compaction would not actually shrink it — either
/// because it is already at or under `head_lines`, or because the elided
/// tail is so small that the footer text itself costs more tokens than it
/// saves. Invariant: `estimate_tokens(result) <= estimate_tokens(output)`.
pub fn compact_tool_output(output: &str, head_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= head_lines {
        return output.to_string();
    }
    let head = lines[..head_lines].join("\n");
    let elided = lines.len() - head_lines;
    // Estimate the elided tail's tokens from char counts directly, rather
    // than materializing a full copy of the (potentially huge) tail via
    // `join`.
    let total_chars = output.chars().count() as u64;
    let head_chars = head.chars().count() as u64;
    let elided_tokens = total_chars.saturating_sub(head_chars).div_ceil(4);
    // Raw '…' is intentional (core cannot reach the tui glyph table; see zoom.rs).
    let candidate =
        format!("{head}\n… (compacted: {elided} more lines, ~{elided_tokens} tokens elided)");
    if estimate_tokens(&candidate) < estimate_tokens(output) {
        candidate
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_returned_unchanged() {
        let out = "line1\nline2\nline3";
        assert_eq!(compact_tool_output(out, 8), out);
    }

    #[test]
    fn long_output_keeps_head_and_reports_elision() {
        let body: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let s = compact_tool_output(&body, 5);
        // Head preserved verbatim.
        assert!(s.starts_with("line1\nline2\nline3\nline4\nline5\n"));
        // Footer reports the elided line count (20 - 5 = 15).
        assert!(s.contains("15 more lines"), "footer missing: {s}");
        // The summary must be strictly smaller than the original.
        assert!(estimate_tokens(&s) < estimate_tokens(&body));
    }

    #[test]
    fn small_elided_tail_is_returned_unchanged() {
        // Only one line would be elided; the footer text would cost more
        // tokens than it saves, so the input must come back verbatim.
        let out = "a\nb";
        assert_eq!(compact_tool_output(out, 1), out);
        assert!(estimate_tokens(&compact_tool_output(out, 1)) <= estimate_tokens(out));
    }

    #[test]
    fn empty_input_returns_empty_without_panic() {
        assert_eq!(compact_tool_output("", 0), "");
        assert_eq!(compact_tool_output("", 5), "");
    }

    #[test]
    fn head_lines_zero_never_grows_output() {
        let body: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let s = compact_tool_output(&body, 0);
        assert!(estimate_tokens(&s) <= estimate_tokens(&body));

        let small = "a\nb";
        let s2 = compact_tool_output(small, 0);
        assert!(estimate_tokens(&s2) <= estimate_tokens(small));
    }

    #[test]
    fn result_never_exceeds_input_tokens() {
        // General invariant check across a spread of inputs.
        for (body, head_lines) in [
            ("", 0),
            ("a", 1),
            ("a\nb", 0),
            ("a\nb", 1),
            ("a\nb\nc\nd", 2),
        ] {
            let s = compact_tool_output(body, head_lines);
            assert!(
                estimate_tokens(&s) <= estimate_tokens(body),
                "compact_tool_output({body:?}, {head_lines}) grew: {s:?}"
            );
        }
    }
}
