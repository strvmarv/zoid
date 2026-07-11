//! Live-edge re-assertion policy (spec: re-floor). Pure functions over the
//! event log; monotonic under BOTH eviction (marks-but-keeps) and compaction
//! (#6b empties tool bodies, but ToolResultCompacted preserves original_tokens).

use crate::economy::estimate_tokens;
use crate::event::{Event, EventKind};
use std::collections::HashMap;

pub fn cumulative_appended<'a>(events: impl IntoIterator<Item = &'a Event> + Clone) -> u64 {
    let orig: HashMap<&str, u64> = events
        .clone()
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, original_tokens, .. } => {
                Some((id.as_str(), *original_tokens))
            }
            _ => None,
        })
        .collect();
    events
        .into_iter()
        .map(|e| match &e.kind {
            EventKind::UserMessage { text }
            | EventKind::AssistantMessage { text }
            | EventKind::ModelDelta { text } => estimate_tokens(text),
            EventKind::ToolResult { id, output, .. } => orig
                .get(id.as_str())
                .copied()
                .unwrap_or_else(|| estimate_tokens(output)),
            _ => 0,
        })
        .sum()
}

pub fn reassertion_due<'a>(events: impl IntoIterator<Item = &'a Event> + Clone, interval: u64) -> bool {
    if interval == 0 {
        return false;
    }
    let last = events
        .clone()
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::DirectiveReasserted { at_cumulative } => Some(*at_cumulative),
            _ => None,
        })
        .last()
        .unwrap_or(0);
    cumulative_appended(events).saturating_sub(last) >= interval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, EvictionMarker};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event { Event::new(Ulid::new(), None, 0, kind) }
    fn user(t: &str) -> Event { ev(EventKind::UserMessage { text: t.into() }) }
    fn tool(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult { id: id.into(), name: "shell".into(), output: out.into(), is_error: false })
    }
    fn compacted(id: &str, original_tokens: u64) -> Event {
        ev(EventKind::ToolResultCompacted { id: id.into(), summary: "sum".into(), original_tokens })
    }

    #[test]
    fn disabled_interval_never_due() {
        assert!(!reassertion_due(&vec![user(&"x".repeat(9000))], 0));
    }

    #[test]
    fn fires_at_threshold_and_marker_resets_baseline() {
        let big = user(&"x".repeat(3000)); // 3000 chars / 3 = 1000 est tokens
        let log = vec![big.clone()];
        assert!(reassertion_due(&log, 1000));
        assert!(!reassertion_due(&log, 1001));
        let mut log2 = log.clone();
        log2.push(ev(EventKind::DirectiveReasserted { at_cumulative: 1000 }));
        assert!(!reassertion_due(&log2, 1000));
        log2.push(user(&"y".repeat(3000)));
        assert!(reassertion_due(&log2, 1000));
    }

    #[test]
    fn monotonic_under_compaction_body_clear() {
        let before = vec![tool("t1", &"z".repeat(3000))];
        assert_eq!(cumulative_appended(&before), 1000);
        // Simulate #6b: body emptied; ToolResultCompacted preserves original_tokens.
        let after = vec![tool("t1", ""), compacted("t1", 1000)];
        assert_eq!(cumulative_appended(&after), 1000,
            "compacted+cleared result must still count at original_tokens (monotonic)");
    }

    #[test]
    fn monotonic_under_eviction_marker() {
        let mut log = vec![user(&"a".repeat(3000)), tool("t1", &"b".repeat(3000))];
        let before = cumulative_appended(&log);
        log.push(ev(EventKind::TurnsEvicted {
            ids: vec![log[0].id],
            reclaimed_tokens: 1000,
            marker: EvictionMarker { spans: vec![] },
        }));
        assert_eq!(cumulative_appended(&log), before, "evicted events still counted");
    }
}
