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

/// One paged-out span, for the in-context breadcrumb and the audit view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictedSpan {
    pub token_estimate: u64,
    pub topic_hint: String,
}

/// The data an eviction wave renders (transcript) and the model reads (breadcrumb).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionMarker {
    pub spans: Vec<EvictedSpan>,
}

/// What kind of question an inline card represents. Drives rendering + the
/// bin's side-effect on answer (materialize for `ModeMapping` + "Approve").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionKind {
    /// A plain `ask_user` question (free-text or pick-list).
    Ask,
    /// The wizard's mode-mapping approval. The `mapping` rides here so the bin
    /// can materialize on "Approve" without re-parsing it from anywhere.
    ModeMapping {
        mapping: Box<crate::wizard::ModeMapping>,
    },
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
    /// An automatic context-management action: the tool-result with `id` was
    /// compacted to `summary` in the live request. Append-only — the original
    /// `ToolResult` event is retained, so this is reversible. `original_tokens`
    /// is the pre-compaction estimate, kept for the audit view.
    ToolResultCompacted {
        id: String,
        summary: String,
        original_tokens: u64,
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
    /// **Inert.** Marks a prior layer-4 turn-drop compaction. Layer 4
    /// (turn-dropping) was removed — it cascaded and wiped history because the
    /// model's `real_input_tokens` never decreased while the conversation
    /// projection sent the full log, so the planner kept firing until one turn
    /// survived. The variant is kept for backward-compatible deserialization of
    /// old DBs; existing `TurnsDropped` events are now inert metadata that no
    /// projection filters on. `turns_dropped` is how many complete turns were
    /// truncated at the time.
    TurnsDropped {
        turns_dropped: usize,
    },
    /// Whole turns paged to the cold tier. Append-only; the original events are
    /// retained (reversible). Projections skip these ids (minus any later
    /// `TurnsReadmitted`). `marker` backs the in-context breadcrumb.
    TurnsEvicted {
        ids: Vec<Ulid>,
        reclaimed_tokens: u64,
        marker: EvictionMarker,
    },
    /// Undo / recall re-admission: projections stop skipping these ids.
    TurnsReadmitted {
        ids: Vec<Ulid>,
    },
    /// A question the model asked the user via `ask_user` (or `apply_mode_mapping`'s
    /// approval gate). Rendered as an inline card in the conversation. Paired
    /// with a `QuestionAnswered` carrying the same `id`.
    QuestionAsked {
        id: String,
        kind: QuestionKind,
        question: String,
        choices: Vec<String>,
    },
    /// The user's answer to a `QuestionAsked`. `id` matches the question. The
    /// card collapses to a one-line summary after this lands.
    QuestionAnswered {
        id: String,
        answer: String,
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

    #[test]
    fn tool_result_compacted_round_trips_through_serde() {
        let k = EventKind::ToolResultCompacted {
            id: "call_42".into(),
            summary: "… (compacted: 300 more lines, ~2100 tokens elided)".into(),
            original_tokens: 2200,
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }

    #[test]
    fn turns_evicted_round_trips_json() {
        let m = EvictionMarker {
            spans: vec![EvictedSpan {
                token_estimate: 4200,
                topic_hint: "read config".into(),
            }],
        };
        let k = EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
            reclaimed_tokens: 4200,
            marker: m,
        };
        let s = serde_json::to_string(&k).unwrap();
        let back: EventKind = serde_json::from_str(&s).unwrap();
        assert_eq!(k, back);
    }

    #[test]
    fn question_ask_round_trips() {
        let k = EventKind::QuestionAsked {
            id: "call_1".into(),
            kind: QuestionKind::Ask,
            question: "retry or skip?".into(),
            choices: vec!["Retry".into(), "Skip".into()],
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }

    #[test]
    fn question_ask_mode_mapping_round_trips() {
        use crate::wizard::{MappingEntry, ModeMapping};
        let mapping = ModeMapping {
            mode_name: "brainstorm".into(),
            mode_description: "brainstorming mode".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "skills/brainstorming/SKILL.md".into(),
                source: "upstream/SKILL.md".into(),
                summary: "the skill".into(),
            }],
        };
        let k = EventKind::QuestionAsked {
            id: "call_2".into(),
            kind: QuestionKind::ModeMapping {
                mapping: Box::new(mapping.clone()),
            },
            question: "review the mapping".into(),
            choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
        // Confirm the mapping survived the box + serde round-trip.
        match back.kind {
            EventKind::QuestionAsked {
                kind: QuestionKind::ModeMapping { mapping: m },
                ..
            } => assert_eq!(*m, mapping),
            _ => panic!("expected ModeMapping kind"),
        }
    }

    #[test]
    fn question_answered_round_trips() {
        let k = EventKind::QuestionAnswered {
            id: "call_1".into(),
            answer: "Skip".into(),
        };
        let ev = Event::new(Ulid::new(), None, 0, k.clone());
        let json = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, k);
    }
}
