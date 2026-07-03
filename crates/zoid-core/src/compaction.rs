//! Active context management (ACM-1): plan and summarize tool-result
//! compactions. Pure — the agent loop records the results as events.

use crate::economy::estimate_tokens;

/// Summarize an oversized tool-result body: keep the first `head_lines` lines
/// verbatim, then a one-line footer noting the elided tail. Returns the input
/// unchanged when it is already at or under `head_lines` (nothing to gain).
pub fn compact_tool_output(output: &str, head_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= head_lines {
        return output.to_string();
    }
    let head = lines[..head_lines].join("\n");
    let elided = lines.len() - head_lines;
    let elided_tokens = estimate_tokens(&lines[head_lines..].join("\n"));
    // Raw '…' is intentional (core cannot reach the tui glyph table; see zoom.rs).
    format!("{head}\n… (compacted: {elided} more lines, ~{elided_tokens} tokens elided)")
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
        // The summary must be smaller than the original.
        assert!(estimate_tokens(&s) < estimate_tokens(&body));
    }
}
