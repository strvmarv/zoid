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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MutationOp {
    Pin,
    Unpin,
    Evict,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    ModelDelta {
        text: String,
    },
    /// A tool the model asked to call. `args` is the raw JSON arguments (stored
    /// as a string so `EventKind` keeps `Eq`).
    ToolCall {
        id: String,
        name: String,
        args: String,
    },
    /// The result of running a `ToolCall`. `output` is the tool's text output.
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    /// A turn's token usage. The numbers live in `Event.tokens`; this variant
    /// is the carrier so the economy projections can sum real counts. Ignored
    /// by the conversation projection.
    Usage,
    /// A manual or automatic change to the context window, targeting a
    /// `ContextItem` by its stable `key`.
    ContextMutation {
        item: String,
        op: MutationOp,
    },
    /// A finished subagent's outcome, recorded on the MAIN branch. `branch`
    /// names the subagent's sub-branch; `summary` is its closing report.
    DelegationResult {
        branch: String,
        summary: String,
        ok: bool,
    },
    /// The model's full task-list snapshot (last-write-wins). Rendered in the
    /// rail; never inlined into the conversation transcript. Faithful — no
    /// cardinality rules enforced here.
    Tasks {
        items: Vec<crate::tasks::TaskItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: Ulid,
    pub parent: Option<Ulid>,
    pub branch: BranchId,
    pub session_id: Ulid,
    pub ts: i64,
    pub kind: EventKind,
    pub tokens: Option<TokenStat>,
}

impl Event {
    pub fn new(id: Ulid, parent: Option<Ulid>, ts: i64, kind: EventKind) -> Self {
        Event {
            id,
            parent,
            branch: BranchId::default(),
            session_id: Ulid::from(0u128),
            ts,
            kind,
            tokens: None,
        }
    }

    /// Tag this event with its owning session (bin wiring; core stays session-agnostic).
    pub fn with_session(mut self, session_id: Ulid) -> Self {
        self.session_id = session_id;
        self
    }

    pub fn with_tokens(mut self, tokens: TokenStat) -> Self {
        self.tokens = Some(tokens);
        self
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
            session_id: Ulid::from(0u128),
            ts: 1_700_000_000,
            kind: EventKind::AssistantMessage {
                text: "hello".into(),
            },
            tokens: Some(TokenStat {
                input: 1,
                output: 2,
                cached: 3,
            }),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        assert_eq!(back.parent, Some(parent));
        assert_eq!(
            back.tokens,
            Some(TokenStat {
                input: 1,
                output: 2,
                cached: 3
            })
        );
    }

    #[test]
    fn event_new_defaults() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ev = Event::new(
            id,
            None,
            1_700_000_000,
            EventKind::UserMessage { text: "hi".into() },
        );
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
        let call = Event::new(
            id,
            None,
            1,
            EventKind::ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: r#"{"path":"a"}"#.into(),
            },
        );
        let res = Event::new(
            id,
            None,
            2,
            EventKind::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "data".into(),
                is_error: false,
            },
        );
        for ev in [call, res] {
            let json = serde_json::to_string(&ev).unwrap();
            let back: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(ev, back);
        }
    }

    #[test]
    fn usage_and_mutation_round_trip() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let usage = Event {
            id,
            parent: None,
            branch: BranchId::default(),
            session_id: Ulid::from(0u128),
            ts: 5,
            kind: EventKind::Usage,
            tokens: Some(TokenStat {
                input: 100,
                output: 40,
                cached: 10,
            }),
        };
        let mutation = Event::new(
            id,
            None,
            6,
            EventKind::ContextMutation {
                item: "file:src/a.rs".into(),
                op: MutationOp::Pin,
            },
        );
        for ev in [usage, mutation] {
            let json = serde_json::to_string(&ev).unwrap();
            assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
        }
    }

    #[test]
    fn delegation_result_round_trips() {
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::DelegationResult {
                branch: "subagent:zz".into(),
                summary: "did it".into(),
                ok: false,
            },
        );
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
    }

    #[test]
    fn session_id_round_trips_and_defaults() {
        let id = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        // new() defaults to the zero sentinel …
        let ev = Event::new(id, None, 1, EventKind::UserMessage { text: "hi".into() });
        assert_eq!(ev.session_id, Ulid::from(0u128));
        // … and the builder sets it, surviving a JSON round-trip.
        let sid = Ulid::from(7u128);
        let ev = ev.with_session(sid);
        let back: Event = serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(back.session_id, sid);
    }
}
