use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BranchId(pub String);

impl Default for BranchId {
    fn default() -> Self {
        BranchId("main".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenStat {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ModelDelta { text: String },
    /// A tool the model asked to call. `args` is the raw JSON arguments (stored
    /// as a string so `EventKind` keeps `Eq`).
    ToolCall { id: String, name: String, args: String },
    /// The result of running a `ToolCall`. `output` is the tool's text output.
    ToolResult { id: String, name: String, output: String, is_error: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: Ulid,
    pub parent: Option<Ulid>,
    pub branch: BranchId,
    pub ts: i64,
    pub kind: EventKind,
    pub tokens: Option<TokenStat>,
}

impl Event {
    pub fn new(id: Ulid, parent: Option<Ulid>, ts: i64, kind: EventKind) -> Self {
        Event { id, parent, branch: BranchId::default(), ts, kind, tokens: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_round_trips() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let parent = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        let ev = Event {
            id,
            parent: Some(parent),
            branch: BranchId("feature".to_string()),
            ts: 1_700_000_000,
            kind: EventKind::AssistantMessage { text: "hello".into() },
            tokens: Some(TokenStat { input: 1, output: 2, cached: 3 }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        assert_eq!(back.parent, Some(parent));
        assert_eq!(back.tokens, Some(TokenStat { input: 1, output: 2, cached: 3 }));
    }

    #[test]
    fn event_new_defaults() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ev = Event::new(id, None, 1_700_000_000, EventKind::UserMessage { text: "hi".into() });
        assert_eq!(ev.branch, BranchId::default());
        assert_eq!(ev.tokens, None);
        assert_eq!(ev.parent, None);
    }

    #[test]
    fn model_delta_round_trips() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ev = Event::new(id, None, 42, EventKind::ModelDelta { text: "tok".into() });
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        assert!(matches!(back.kind, EventKind::ModelDelta { .. }));
    }

    #[test]
    fn tool_events_round_trip() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let call = Event::new(id, None, 1, EventKind::ToolCall {
            id: "c1".into(), name: "read_file".into(), args: r#"{"path":"a"}"#.into(),
        });
        let res = Event::new(id, None, 2, EventKind::ToolResult {
            id: "c1".into(), name: "read_file".into(), output: "data".into(), is_error: false,
        });
        for ev in [call, res] {
            let json = serde_json::to_string(&ev).unwrap();
            let back: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(ev, back);
        }
    }
}
