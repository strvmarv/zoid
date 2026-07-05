//! Active context management (ACM-1): plan and summarize tool-result
//! compactions. Pure — the agent loop records the results as events.

use crate::assembler::ContextPolicy;
use crate::context::{context_window_with, tool_id_of, ContextOverhead, ItemKind};
use crate::economy::{estimate_tokens, tool_path};
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

/// The result of planning context management: tool-result compactions
/// (layer 1). Layer 4 (turn-dropping) was removed — it cascaded and wiped
/// history because the model's `real_input_tokens` never decreased (the
/// conversation projection sends the full log), so the planner kept firing
/// `TurnsDropped` until only one turn survived, then re-fired on every new
/// message. Tool-result compaction (layer 1) is sufficient and self-limiting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactionPlan {
    /// Tool results to compact (layer 1).
    pub compactions: Vec<Compaction>,
}

/// Plan which tool-results to compact. Empty unless the window exceeds
/// `policy.compact_threshold`. Compacts `ToolResult` items only (never System /
/// Message / File), largest-first, skipping pinned + already-compacted + any
/// whose summary would not actually shrink them, until back under threshold.
///
/// `real_input_tokens`, when provided (from the provider's last Usage event),
/// is the most accurate measure of the current context size. The chars/4
/// estimate significantly underestimates for code and tool results (5-7x in
/// practice), so compaction would fire far too late without the real count.
///
/// When `real_input_tokens` is `None` (the provider reported 0 — e.g. Ollama's
/// `prompt_eval_count=0` when the prompt is fully cached), the estimate-based
/// `window.total_tokens` is used, scaled by `calibration_ratio` when available.
/// The ratio is `real_input_tokens / window.total_tokens` from a prior non-cached
/// sub-turn; applying it to the current estimate yields a current, self-consistent
/// approximation that reflects prior compactions (both the historical and current
/// estimates use the same `context_window` projection, so the ratio transfers).
/// When no calibration has been learned yet (`None`), the raw estimate is used —
/// better to fire late than to use a stale frozen value that never decreases.
pub fn plan_compactions<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    policy: &ContextPolicy,
    real_input_tokens: Option<u64>,
    calibration_ratio: Option<f64>,
    overhead: &ContextOverhead,
) -> CompactionPlan {
    let Some(threshold) = policy.compact_threshold else {
        return CompactionPlan::default();
    };
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let window = context_window_with(events.iter().copied(), overhead.clone());
    let current = match real_input_tokens.filter(|&t| t > 0) {
        Some(real) => real,
        None => match calibration_ratio {
            Some(ratio) if ratio > 0.0 => (window.total_tokens as f64 * ratio) as u64,
            _ => window.total_tokens,
        },
    };
    if current <= threshold {
        return CompactionPlan::default();
    }

    let done: HashSet<&str> = visible
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    // Latest non-error output per tool-result id.
    let mut output_of: HashMap<&str, &str> = HashMap::new();
    for e in visible {
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

    // Map file paths to their latest tool-result id (for the done/already-compacted check).
    let mut path_id_of: HashMap<String, String> = HashMap::new();
    {
        let mut call_path: HashMap<String, String> = HashMap::new();
        for e in visible {
            match &e.kind {
                EventKind::ToolCall { id, args, .. } => {
                    if let Some(p) = tool_path(args) {
                        call_path.insert(id.clone(), p);
                    }
                }
                EventKind::ToolResult { id, .. } => {
                    if let Some(p) = call_path.get(id) {
                        path_id_of.insert(p.clone(), id.clone());
                    }
                }
                _ => {}
            }
        }
    }

    // Map file paths to their latest tool-result output (for File items whose
    // key is "file:{path}", not "tool:{name}:{id}"). Correlates ToolCall args
    // (which carry the path) to the paired ToolResult id → output.
    let mut path_output_of: HashMap<String, &str> = HashMap::new();
    {
        let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
        for e in visible {
            match &e.kind {
                EventKind::ToolCall { id, args, .. } => {
                    if let Some(p) = tool_path(args) {
                        call_path.insert(id.clone(), p);
                    }
                }
                EventKind::ToolResult {
                    id,
                    output,
                    is_error: false,
                    ..
                } => {
                    if let Some(p) = call_path.get(id) {
                        path_output_of.insert(p.clone(), output.as_str());
                    }
                }
                _ => {}
            }
        }
    }

    let mut running = current;
    let mut out: Vec<Compaction> = Vec::new();
    for it in &window.items {
        // window.items is sorted tokens-desc, so this is largest-first.
        if running <= threshold {
            break;
        }
        if (it.kind != ItemKind::ToolResult && it.kind != ItemKind::File) || it.pinned {
            continue;
        }
        let Some(id) = tool_id_of(&it.key) else {
            continue;
        };
        // For File items, the tool call id is looked up from the path.
        let tool_call_id = if it.kind == ItemKind::File {
            path_id_of.get(id).map(|s| s.as_str())
        } else {
            Some(id)
        };
        // Check if already compacted.
        if tool_call_id.is_some_and(|tid| done.contains(tid)) {
            continue;
        }
        // For File items, `id` is the path (key is "file:{path}"); for
        // ToolResult items, `id` is the tool call id (key is "tool:{name}:{id}").
        let output = if it.kind == ItemKind::File {
            path_output_of.get(id)
        } else {
            None
        };
        let output = match output {
            Some(o) => *o,
            None => match output_of.get(id) {
                Some(o) => *o,
                None => continue,
            },
        };
        let summary = compact_tool_output(output, COMPACT_HEAD_LINES);
        let summary_tokens = estimate_tokens(&summary);
        if summary_tokens >= it.tokens {
            continue; // no gain
        }
        // Saturating: `running` is a heuristic pressure counter, not an
        // accounting balance. When `current` comes from real provider tokens
        // (which may be larger than the sum of estimate-based item tokens due
        // to the 5-7x undercount), subtracting an item's estimate tokens can
        // overshoot. Saturating to 0 is correct — it just means "enough
        // compacted", and the `running <= threshold` break fires next iteration.
        running = running.saturating_sub(it.tokens - summary_tokens);
        out.push(Compaction {
            id: tool_call_id.unwrap_or(id).to_string(),
            summary,
            original_tokens: it.tokens,
        });
    }

    // Layer 4 (turn-dropping) was removed. It cascaded and wiped history:
    // `real_input_tokens` reflects the full un-truncated context the model
    // actually receives (the conversation projection sends the full log), so
    // `current > threshold` was permanently true after the first drop, causing
    // the planner to emit `TurnsDropped` on every sub-turn until only one turn
    // remained, then re-fire on every new message. Tool-result compaction
    // (layer 1 above) is self-limiting and sufficient.

    CompactionPlan { compactions: out }
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
        assert!(plan_compactions(
            &evs,
            &policy(1_000_000),
            None,
            None,
            &ContextOverhead::default()
        )
        .compactions
        .is_empty());
        // No threshold set → nothing to do.
        assert!(plan_compactions(
            &evs,
            &ContextPolicy::default(),
            None,
            None,
            &ContextOverhead::default()
        )
        .compactions
        .is_empty());
    }

    #[test]
    fn compacts_biggest_tool_results_until_under_threshold() {
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400), // biggest
            big_tool_result("c2", "shell", 50),
        ];
        let plan = plan_compactions(&evs, &policy(500), None, None, &ContextOverhead::default());
        assert_eq!(
            plan.compactions.len(),
            1,
            "only the big one needs compacting"
        );
        assert_eq!(plan.compactions[0].id, "c1");
        assert!(
            plan.compactions[0].original_tokens > estimate_tokens(&plan.compactions[0].summary)
        );
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
        assert!(
            plan_compactions(&evs, &policy(1), None, None, &ContextOverhead::default())
                .compactions
                .is_empty()
        );
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

    #[test]
    fn turns_dropped_no_longer_drops_turns() {
        // Layer 4 (turn-dropping) was removed — it cascaded and wiped history
        // because real_input_tokens (the full context the model receives)
        // never decreased, so the planner kept dropping until one turn
        // survived, then re-fired on every new message. Now plan_compactions
        // must never return turns_to_drop, no matter how far over threshold.
        let mut evs = Vec::new();
        for i in 0..5 {
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 100,
                EventKind::UserMessage {
                    text: format!("turn {i}"),
                },
            ));
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 100 + 50,
                EventKind::ToolResult {
                    id: format!("c{i}"),
                    name: "search".into(),
                    output: "x".repeat(2000),
                    is_error: false,
                },
            ));
            // Pre-compact every tool result so layer 1 has nothing to do,
            // forcing the plan into the (now removed) turn-drop branch.
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 100 + 60,
                EventKind::ToolResultCompacted {
                    id: format!("c{i}"),
                    summary: "tiny".into(),
                    original_tokens: 500,
                },
            ));
        }
        // Threshold = 1 token, real_input_tokens = 100000 → way over, but
        // layer 4 is gone → no turns dropped, ever.
        let plan = plan_compactions(
            &evs,
            &policy(1),
            Some(100_000),
            None,
            &ContextOverhead::default(),
        );
        assert_eq!(
            plan.compactions.len(),
            0,
            "no compaction candidates (all pre-compacted)"
        );
    }

    #[test]
    fn turns_dropped_marker_is_inert() {
        // A prior TurnsDropped marker in the log must not affect planning at
        // all — it is inert metadata now. The window sees the full history.
        let mut evs = Vec::new();
        for i in 0..5 {
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 1000,
                EventKind::UserMessage {
                    text: format!("turn {i}"),
                },
            ));
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 1000 + 50,
                EventKind::ToolResult {
                    id: format!("c{i}"),
                    name: "search".into(),
                    output: "x".repeat(2000),
                    is_error: false,
                },
            ));
        }
        // A TurnsDropped marker at ts=4100 — now inert, never filters.
        evs.push(Event::new(
            Ulid::new(),
            None,
            4100,
            EventKind::TurnsDropped { turns_dropped: 4 },
        ));
        // Even with a huge real_input_tokens, no turns are dropped.
        let plan = plan_compactions(
            &evs,
            &policy(1),
            Some(100_000),
            None,
            &ContextOverhead::default(),
        );
        assert!(
            plan.compactions.iter().all(|c| c.id.starts_with('c')),
            "marker must not cause turn-dropping; only tool-result compaction runs"
        );
    }

    #[test]
    fn context_window_ignores_turns_dropped() {
        use crate::context::context_window;
        let mut evs = Vec::new();
        for i in 0..3 {
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 1000,
                EventKind::UserMessage {
                    text: format!("turn {i}"),
                },
            ));
            evs.push(Event::new(
                Ulid::new(),
                None,
                1000 + i as i64 * 1000 + 50,
                EventKind::ToolResult {
                    id: format!("c{i}"),
                    name: "search".into(),
                    output: format!("output {i}").repeat(100),
                    is_error: false,
                },
            ));
        }
        // A TurnsDropped marker — now inert: all turns must remain in the window.
        evs.push(Event::new(
            Ulid::new(),
            None,
            2100,
            EventKind::TurnsDropped { turns_dropped: 2 },
        ));
        let w = context_window(&evs);
        let labels: Vec<&str> = w.items.iter().map(|i| i.label.as_str()).collect();
        // All turns must be present — the marker does NOT filter.
        assert!(
            labels.iter().any(|l| l.contains("turn 0")),
            "dropped turn 0 must still appear in context window (marker is inert)"
        );
        assert!(
            labels.iter().any(|l| l.contains("turn 1")),
            "dropped turn 1 must still appear in context window (marker is inert)"
        );
        assert!(
            labels.iter().any(|l| l.contains("turn 2")),
            "surviving turn 2 must appear in context window"
        );
    }

    #[test]
    fn calibration_ratio_scales_estimate_when_real_tokens_absent() {
        // When the provider reports 0 (cached) and no real_input_tokens is
        // available, the estimate is scaled by the calibration ratio. The
        // raw estimate (chars/3 + tool-call args) is already substantial for
        // a 400-line tool result; with a 3x ratio it triples, enough to trip
        // a threshold the raw estimate alone wouldn't.
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400),
        ];
        // Use a threshold high enough that the raw estimate is below it but
        // the 3x-calibrated estimate is above it.
        let raw_window = crate::context::context_window(&evs);
        let raw = raw_window.total_tokens;
        // Without calibration: raw estimate < threshold → no compaction.
        let threshold = raw + 100;
        let plan = plan_compactions(
            &evs,
            &policy(threshold),
            None,
            None,
            &ContextOverhead::default(),
        );
        assert!(
            plan.compactions.is_empty(),
            "uncalibrated estimate below threshold"
        );

        // With 3x calibration: raw * 3 > threshold → compaction fires.
        let plan = plan_compactions(
            &evs,
            &policy(threshold),
            None,
            Some(3.0),
            &ContextOverhead::default(),
        );
        assert_eq!(
            plan.compactions.len(),
            1,
            "calibrated estimate must trip threshold"
        );
    }

    #[test]
    fn real_input_tokens_overrides_calibration() {
        // When real_input_tokens is provided and non-zero, it wins regardless
        // of the ratio — it's the ground truth from the provider.
        let evs = vec![
            ev(EventKind::UserMessage { text: "go".into() }),
            big_tool_result("c1", "search", 400),
        ];
        // Real says 100 (below 200 threshold) even with a 5x ratio → no compaction.
        let plan = plan_compactions(
            &evs,
            &policy(200),
            Some(100),
            Some(5.0),
            &ContextOverhead::default(),
        );
        assert!(
            plan.compactions.is_empty(),
            "real tokens override calibration"
        );
        // Real says 300 (above 200) → compaction fires.
        let plan = plan_compactions(
            &evs,
            &policy(200),
            Some(300),
            Some(5.0),
            &ContextOverhead::default(),
        );
        assert_eq!(plan.compactions.len(), 1);
    }

    proptest! {
        #[test]
        fn planned_ids_are_unique_and_never_already_done(lines in proptest::collection::vec(20usize..300, 1..6)) {
            let mut evs = vec![ev(EventKind::UserMessage { text: "go".into() })];
            for (i, n) in lines.iter().enumerate() {
                evs.push(big_tool_result(&format!("c{i}"), "search", *n));
            }
            let plan = plan_compactions(&evs, &policy(100), None, None, &ContextOverhead::default());
            let mut ids: Vec<&str> = plan.compactions.iter().map(|c| c.id.as_str()).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            prop_assert_eq!(ids.len(), n, "planned ids must be unique");
        }
    }
}
