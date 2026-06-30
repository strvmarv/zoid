//! The context-economy projections (spec §8): token ledger, churn timeline,
//! and the per-item token estimator. All pure functions of the event log.

use crate::event::{Event, EventKind};
use std::collections::HashSet;

/// Aggregate token spend over a scope of the log (spec §8). `total` is
/// `input + output`; `cached` is the cache-read subset of input, surfaced
/// separately (it is *not* added into `total` again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub total: u64,
}

/// Fold the log into a `TokenLedger` by summing every event's `tokens`.
pub fn token_ledger(events: &[Event]) -> TokenLedger {
    let mut l = TokenLedger::default();
    for e in events {
        if let Some(t) = e.tokens {
            l.input += t.input;
            l.output += t.output;
            l.cached += t.cached;
        }
    }
    l.total = l.input + l.output;
    l
}

/// Estimate the token cost of a string as `ceil(chars / 4)` — the standard
/// rough heuristic (≈4 chars/token). Aggregate ledger numbers use real
/// provider `Usage`; this is for per-item context sizing where the provider
/// gives no breakdown.
pub fn estimate_tokens(s: &str) -> u64 {
    let chars = s.chars().count() as u64;
    chars.div_ceil(4)
}

/// One turn's churn (spec §8 ⑤c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChurnPoint {
    pub turn: usize,
    pub tokens: u64,
    pub resent_tokens: u64,
}

/// Per-turn token deltas with re-sent-file flagging.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChurnTimeline {
    pub points: Vec<ChurnPoint>,
}

/// Extract a file path from a tool call's JSON args, trying common keys.
pub(crate) fn tool_path(args_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args_json).ok()?;
    for key in ["path", "file_path", "file"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

pub fn churn_timeline(events: &[Event]) -> ChurnTimeline {
    let mut points: Vec<ChurnPoint> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut cur: Option<ChurnPoint> = None;

    for e in events {
        match &e.kind {
            EventKind::UserMessage { .. } => {
                if let Some(p) = cur.take() {
                    points.push(p);
                }
                cur = Some(ChurnPoint { turn: points.len(), tokens: 0, resent_tokens: 0 });
            }
            EventKind::ToolCall { args, .. } => {
                if let (Some(p), Some(path)) = (cur.as_mut(), tool_path(args)) {
                    if seen_paths.contains(&path) {
                        p.resent_tokens += estimate_tokens(&path).max(1);
                    }
                    seen_paths.insert(path);
                }
            }
            _ => {}
        }
        if let (Some(p), Some(t)) = (cur.as_mut(), e.tokens) {
            p.tokens += t.input + t.output;
        }
    }
    if let Some(p) = cur.take() {
        points.push(p);
    }
    ChurnTimeline { points }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, TokenStat};
    use proptest::prelude::*;
    use ulid::Ulid;

    #[test]
    fn estimate_tokens_is_chars_over_four_rounded_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);     // ceil(1/4)
        assert_eq!(estimate_tokens("abcd"), 1);  // 4/4
        assert_eq!(estimate_tokens("abcde"), 2); // ceil(5/4)
        // counts chars, not bytes
        assert_eq!(estimate_tokens("é"), 1);
    }

    fn usage(input: u64, output: u64, cached: u64) -> Event {
        Event {
            id: Ulid::new(),
            parent: None,
            branch: Default::default(),
            ts: 0,
            kind: EventKind::Usage,
            tokens: Some(TokenStat { input, output, cached }),
        }
    }

    #[test]
    fn ledger_sums_usage_and_ignores_untokened_events() {
        let evs = vec![
            Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hi".into() }),
            usage(100, 40, 10),
            usage(50, 20, 5),
        ];
        let l = token_ledger(&evs);
        assert_eq!(l.input, 150);
        assert_eq!(l.output, 60);
        assert_eq!(l.cached, 15);
        assert_eq!(l.total, 210); // input + output, cached not double-counted
    }

    #[test]
    fn ledger_of_empty_log_is_zero() {
        assert_eq!(token_ledger(&[]), TokenLedger::default());
    }

    proptest! {
        #[test]
        fn ledger_total_equals_input_plus_output(stats in proptest::collection::vec((0u64..10_000, 0u64..10_000, 0u64..10_000), 0..50)) {
            let evs: Vec<Event> = stats.iter().map(|&(i,o,c)| usage(i,o,c)).collect();
            let l = token_ledger(&evs);
            prop_assert_eq!(l.total, l.input + l.output);
            prop_assert_eq!(l.input, stats.iter().map(|s| s.0).sum::<u64>());
        }
    }

    fn umsg(text: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: text.into() })
    }
    fn toolcall_read(id: &str, path: &str) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::ToolCall {
            id: id.into(), name: "read_file".into(),
            args: format!(r#"{{"path":"{path}"}}"#),
        })
    }

    #[test]
    fn churn_segments_by_user_message_and_flags_resent_files() {
        let evs = vec![
            umsg("turn 1"),
            toolcall_read("c1", "src/a.rs"),
            usage(100, 20, 0),
            umsg("turn 2"),
            toolcall_read("c2", "src/a.rs"), // re-sent file → resent
            toolcall_read("c3", "src/b.rs"), // new file → not resent
            usage(140, 30, 0),
        ];
        let t = churn_timeline(&evs);
        assert_eq!(t.points.len(), 2);
        assert_eq!(t.points[0].turn, 0);
        assert_eq!(t.points[0].tokens, 120);       // 100+20
        assert_eq!(t.points[0].resent_tokens, 0);  // first sight of a.rs
        assert_eq!(t.points[1].turn, 1);
        assert_eq!(t.points[1].tokens, 170);       // 140+30
        // a.rs re-sent: estimate_tokens of its path-based cost is > 0
        assert!(t.points[1].resent_tokens > 0);
    }

    #[test]
    fn churn_empty_when_no_turns() {
        assert_eq!(churn_timeline(&[]), ChurnTimeline::default());
    }
}
