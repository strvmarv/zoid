//! The model's live task list: a full-snapshot event (`EventKind::Tasks`) and
//! the `tasks()` projection that returns the latest snapshot. The event layer
//! is faithful — cardinality (e.g. "one Active") is NOT enforced here.

use crate::event::{Event, EventKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Active,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub text: String,
    pub status: TaskStatus,
}

/// Parse the `update_tasks` argument object into task items. Faithful: accepts
/// any well-formed list; the only errors are structural (missing `tasks`, wrong
/// types, unknown status string).
pub fn parse_task_items(args: &serde_json::Value) -> Result<Vec<TaskItem>, String> {
    let arr = args
        .get("tasks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing or non-array `tasks`".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, it) in arr.iter().enumerate() {
        let text = it
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("task[{i}]: missing or non-string `text`"))?
            .to_string();
        let status = match it.get("status").and_then(|v| v.as_str()) {
            Some("pending") => TaskStatus::Pending,
            Some("active") => TaskStatus::Active,
            Some("done") => TaskStatus::Done,
            other => return Err(format!("task[{i}]: bad status {other:?}")),
        };
        out.push(TaskItem { text, status });
    }
    Ok(out)
}

/// The latest task snapshot (last-write-wins), or empty if none was published.
/// Ignores subagent branches, matching the conversation projection.
pub fn tasks(events: &[Event]) -> Vec<TaskItem> {
    events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Tasks { items } => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind};
    use serde_json::json;

    fn tasks_event(items: Vec<TaskItem>) -> Event {
        Event::new(ulid::Ulid::new(), None, 0, EventKind::Tasks { items })
    }

    #[test]
    fn parse_reads_text_and_status() {
        let got = parse_task_items(&json!({"tasks": [
            {"text": "read spec", "status": "done"},
            {"text": "write code", "status": "active"},
            {"text": "test", "status": "pending"},
        ]}))
        .unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].status, TaskStatus::Done);
        assert_eq!(got[1].text, "write code");
    }

    #[test]
    fn parse_rejects_bad_status_and_shape() {
        assert!(parse_task_items(&json!({"tasks": [{"text": "x", "status": "nope"}]})).is_err());
        assert!(parse_task_items(&json!({"nope": []})).is_err());
    }

    #[test]
    fn tasks_returns_latest_snapshot_last_write_wins() {
        let e1 = tasks_event(vec![TaskItem {
            text: "a".into(),
            status: TaskStatus::Active,
        }]);
        let e2 = tasks_event(vec![
            TaskItem {
                text: "a".into(),
                status: TaskStatus::Done,
            },
            TaskItem {
                text: "b".into(),
                status: TaskStatus::Active,
            },
        ]);
        let got = tasks(&[e1, e2]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].status, TaskStatus::Done);
    }

    #[test]
    fn tasks_empty_when_none_published() {
        assert!(tasks(&[]).is_empty());
    }

    #[test]
    fn tasks_event_round_trips_through_serde() {
        let ev = tasks_event(vec![TaskItem {
            text: "x".into(),
            status: TaskStatus::Pending,
        }]);
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
