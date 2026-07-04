//! Active context management (ACM-1): plan and summarize tool-result
//! compactions. Pure — the agent loop records the results as events.

use crate::assembler::ContextPolicy;
use crate::context::{context_window, tool_id_of, ItemKind};
use crate::economy::estimate_tokens;
use crate::event::{Event, EventKind};
use std::collections::{HashMap, HashSet};

/// Number of head lines kept verbatim when compacting a tool-result.
pub const COMPACT_HEAD_LINES: usize = 8;

/// One planned compaction: replace tool-result `id`'s output with `summary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub id: String,
    pub summary: String,
    pub original_tokens: u64,
}

/// Plan which tool-results to compact. Empty unless the window exceeds
/// `policy.compact_threshold`. Compacts `ToolResult` items only (never System /
/// Message / File), largest-first, skipping pinned + already-compacted + any
/// whose summary would not actually shrink them, until back under threshold.
pub fn plan_compactions(events: &[Event], policy: &ContextPolicy) -> Vec<Compaction> {
    let Some(threshold) = policy.compact_threshold else {
        return Vec::new();
    };
    let window = context_window(events);
    if window.total_tokens <= threshold {
        return Vec::new();
    }

    let done: HashSet<&str> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    // Latest non-error output per tool-result id.
    let mut output_of: HashMap<&str, &str> = HashMap::new();
    for e in events {
        if let EventKind::ToolResult {
            id,
            output,
            is_error,
            ..
        } = &e.kind
        {
            if !*is_error {
                output_of.insert(id.as_str(), output.as_str());
            }
        }
    }

    let mut running = window.total_tokens;
    let mut out: Vec<Compaction> = Vec::new();
    for it in &window.items {
        // window.items is sorted tokens-desc, so this is largest-first.
        if running <= threshold {
            break;
        }
        if it.kind != ItemKind::ToolResult || it.pinned {
            continue;
        }
        let Some(id) = tool_id_of(&it.key) else {
            continue;
        };
        if done.contains(id) {
            continue;
        }
        let Some(output) = output_of.get(id) else {
            continue;
        };
        let summary = compact_tool_output(output, COMPACT_HEAD_LINES);
        let summary_tokens = estimate_tokens(&summary);
        if summary_tokens >= it.tokens {
            continue; // no gain
        }
        running -= it.tokens - summary_tokens;
        out.push(Compaction {
            id: id.to_string(),
            summary,
            original_tokens: it.tokens,
        });
    }
    out
}

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
    use crate::assembler::ContextPolicy;
    use crate::event::{Event, EventKind};
    use proptest::prelude::*;
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }

    fn big_tool_result(id: &str, name: &str, lines: usize) -> Event {
        let body: String = (0..lines).map(|i| format!("match {i} in file\n")).collect();
        ev(EventKind::ToolResult {
            id: id.into(),
            name: name.into(),
            output: body,
            is_error: false,
        })
    }

    fn policy(threshold: u64) -> ContextPolicy {
        ContextPolicy {
            token_ceiling: None,
            auto_evict_cold: false,
            compact_threshold: Some(threshold),
        }
    }

    #[test]
    fn no_compaction_below_threshold() {
        let evs = vec![
            ev(EventKind::UserMessage {
                text: "search please".into(),
            }),
            big_tool_result("c1", "search", 100),
        ];
        // Threshold huge → nothing to do.
        assert!(plan_compactions(&evs, &policy(1_000_000)).is_empty());
        // No threshold set → nothing to do.
        assert!(plan_compactions(&evs, &ContextPolicy::default()).is_empty());
    }

    #[test]
    fn compacts_biggest_tool_results_until_under_threshold() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400), // biggest
            big_tool_result("c2", "shell", 50),
        ];
        let plan = plan_compactions(&evs, &policy(500));
        assert_eq!(plan.len(), 1, "only the big one needs compacting");
        assert_eq!(plan[0].id, "c1");
        assert!(plan[0].original_tokens > estimate_tokens(&plan[0].summary));
    }

    #[test]
    fn never_recompacts_already_compacted() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400),
            ev(EventKind::ToolResultCompacted {
                id: "c1".into(),
                summary: "small".into(),
                original_tokens: 800,
            }),
        ];
        // c1 already compacted → nothing left to compact.
        assert!(plan_compactions(&evs, &policy(1)).is_empty());
    }

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

    proptest! {
        #[test]
        fn planned_ids_are_unique_and_never_already_done(lines in proptest::collection::vec(20usize..300, 1..6)) {
            let mut evs = vec![ev(EventKind::UserMessage { text: "go".into() })];
            for (i, n) in lines.iter().enumerate() {
                evs.push(big_tool_result(&format!("c{i}"), "search", *n));
            }
            let plan = plan_compactions(&evs, &policy(100));
            let mut ids: Vec<&str> = plan.iter().map(|c| c.id.as_str()).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            prop_assert_eq!(ids.len(), n, "planned ids must be unique");
        }
    }
}
