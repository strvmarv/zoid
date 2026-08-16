//! The in-memory event log. Wraps `Vec<Arc<Event>>` so that (a) handing a turn
//! its snapshot is O(n) refcount bumps, not O(total bytes) of body copies (#6a);
//! and (b) an individual tool-result body can be swapped out in place — replace
//! the `Arc` slot — without disturbing snapshots already handed to in-flight
//! turns (they hold the old, immutable `Arc`) (#6b).

use std::sync::Arc;

use zoid_core::event::{Event, EventKind};

#[derive(Debug, Clone, Default)]
pub struct EventLog(Vec<Arc<Event>>);

impl EventLog {
    pub fn new() -> Self {
        EventLog(Vec::new())
    }

    /// Build a log from owned events (e.g. a session snapshot loaded on resume).
    pub fn from_vec(events: Vec<Event>) -> Self {
        EventLog(events.into_iter().map(Arc::new).collect())
    }

    pub fn push(&mut self, e: Event) {
        self.0.push(Arc::new(e));
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Event> + Clone + '_ {
        self.0.iter().map(|a| a.as_ref())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// #6a: a per-turn snapshot. Clones the outer `Vec` only — each element is
    /// an `Arc` refcount bump, never an `Event` body copy.
    pub fn snapshot(&self) -> EventLog {
        EventLog(self.0.clone())
    }

    /// #6b: replace the `ToolResult` whose inner tool-call `id` == `tool_id`
    /// with an `Arc` whose `output` is empty. No-op if `tool_id` is absent or
    /// the matched event is not a `ToolResult`. Snapshots already handed out
    /// hold the old `Arc` and are unaffected.
    pub fn clear_tool_output(&mut self, tool_id: &str) {
        for slot in self.0.iter_mut() {
            let is_match = matches!(&slot.kind, EventKind::ToolResult { id, .. } if id == tool_id);
            if is_match {
                let mut ev = (**slot).clone();
                if let EventKind::ToolResult { output, .. } = &mut ev.kind {
                    output.clear();
                }
                *slot = Arc::new(ev);
                return;
            }
        }
    }

    /// #6b resume path: clear the body of every `ToolResult` that has a matching
    /// `ToolResultCompacted` in this log. Keeps reopening a long session from
    /// re-inflating RAM to the pre-#6b footprint.
    pub fn clear_compacted_bodies(&mut self) {
        let compacted: Vec<String> = self
            .0
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResultCompacted { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        for id in compacted {
            self.clear_tool_output(&id);
        }
    }

    #[cfg(test)]
    fn arcs(&self) -> &[Arc<Event>] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;

    fn tool_result(tool_id: &str, output: &str) -> Event {
        Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResult {
                id: tool_id.to_string(),
                name: "bash".to_string(),
                output: output.to_string(),
                is_error: false,
                error_kind: None,
            },
        )
    }

    fn compacted(tool_id: &str, summary: &str) -> Event {
        Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::ToolResultCompacted {
                id: tool_id.to_string(),
                summary: summary.to_string(),
                original_tokens: 999,
            },
        )
    }

    #[test]
    fn push_iter_len() {
        let mut log = EventLog::new();
        assert!(log.is_empty());
        log.push(tool_result("t1", "hello"));
        log.push(tool_result("t2", "world"));
        assert_eq!(log.len(), 2);
        let outputs: Vec<&str> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(outputs, vec!["hello", "world"]);
    }

    #[test]
    fn snapshot_shares_without_copying_bodies() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "a big body"));
        let snap = log.snapshot();
        assert_eq!(log.len(), snap.len());
        for (a, b) in log.arcs().iter().zip(snap.arcs().iter()) {
            assert!(
                Arc::ptr_eq(a, b),
                "snapshot must share the Arc, not clone the Event"
            );
            assert_eq!(
                Arc::strong_count(a),
                2,
                "one refcount bump per shared event"
            );
        }
    }

    #[test]
    fn clear_tool_output_empties_only_the_target() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "KEEP THIS"));
        log.push(tool_result("t2", "CLEAR THIS"));
        log.clear_tool_output("t2");
        let bodies: Vec<&str> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { output, .. } => Some(output.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(bodies, vec!["KEEP THIS", ""], "only t2's body is emptied");
    }

    #[test]
    fn clear_tool_output_is_noop_for_absent_or_non_toolresult() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "body"));
        log.push(compacted("t1", "sum")); // a ToolResultCompacted, not a ToolResult
        let before: Vec<Arc<Event>> = log.arcs().to_vec();
        log.clear_tool_output("does-not-exist");
        for (a, b) in before.iter().zip(log.arcs().iter()) {
            assert!(Arc::ptr_eq(a, b));
        }
    }

    #[test]
    fn snapshot_is_unaffected_by_later_clear() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "ORIGINAL"));
        let snap = log.snapshot();
        log.clear_tool_output("t1");
        let snap_body = snap.iter().find_map(|e| match &e.kind {
            EventKind::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        });
        assert_eq!(snap_body.as_deref(), Some("ORIGINAL"));
    }

    #[test]
    fn clear_compacted_bodies_clears_exactly_the_compacted() {
        let mut log = EventLog::new();
        log.push(tool_result("t1", "COMPACTED BODY"));
        log.push(tool_result("t2", "LIVE BODY"));
        log.push(compacted("t1", "tiny summary")); // marks t1 compacted
        log.clear_compacted_bodies();
        let bodies: Vec<(&str, &str)> = log
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult { id, output, .. } => Some((id.as_str(), output.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(bodies, vec![("t1", ""), ("t2", "LIVE BODY")]);
    }

    #[test]
    fn cleared_compacted_body_still_renders_summary() {
        use zoid_core::projection::{conversation, ChatMsg};
        let mut log = EventLog::new();
        log.push(tool_result(
            "call-9",
            "HUGE RAW OUTPUT that must never render",
        ));
        log.push(compacted("call-9", "tiny summary"));
        log.clear_tool_output("call-9"); // simulate the #6b trigger
        let msgs = conversation(log.iter());
        let rendered = msgs.iter().find_map(|m| match m {
            ChatMsg::ToolResult { id, output, .. } if id == "call-9" => Some(output.clone()),
            _ => None,
        });
        assert_eq!(
            rendered.as_deref(),
            Some("tiny summary"),
            "summary renders; cleared raw body never does"
        );
    }
}
