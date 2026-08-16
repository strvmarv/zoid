use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Machine-readable error category for tool failures. Propagated from
/// `ToolOutput` through `EventKind::ToolResult` to the projection/UI. The
/// model sees a rendered `[error: <kind>]` prefix in the tool-result text;
/// the loop and UI get the enum directly for future retry logic and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorKind {
    /// External service down or returned an error page (web_search DDG
    /// outage, web_fetch non-2xx or unparseable 2xx body).
    BackendUnavailable,
    /// Operation exceeded a time limit (network connect timeout).
    Timeout,
    /// File, path, or resource does not exist.
    NotFound,
    /// Bad arguments from the model (missing arg, wrong type, empty query,
    /// limit < 1, offset past end, bad URL scheme).
    InvalidInput,
    /// OS-level permission failure (write to read-only path, dir read denied).
    PermissionDenied,
    /// Ambiguous or precondition failure (edit: `old_string` ambiguous or not
    /// found).
    Conflict,
    /// The working directory was deleted out from under the agent. Recovery:
    /// call exit_worktree (if in a worktree) or navigate to an existing
    /// directory.
    CwdDeleted,
    /// Unexpected internal error (serialization failure, spawn failure,
    /// anything that doesn't fit above).
    Internal,
}

impl ErrorKind {
    /// Canonical snake_case string for the `[error: <kind>]` prefix.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::BackendUnavailable => "backend_unavailable",
            ErrorKind::Timeout => "timeout",
            ErrorKind::NotFound => "not_found",
            ErrorKind::InvalidInput => "invalid_input",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::Conflict => "conflict",
            ErrorKind::CwdDeleted => "cwd_deleted",
            ErrorKind::Internal => "internal",
        }
    }
}

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
    /// Reasoning/thinking token count (from `Usage.thinking_tokens`).
    pub thinking: u64,
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
    /// A tool-approval gate prompt (dangerous shell command). The card is
    /// UI-only — the model sees the real ToolResult (the tool's output), not
    /// the approval string. The projection does NOT suppress the ToolResult
    /// for this kind, unlike Ask/ModeMapping where the card IS the result.
    Approval,
    /// The `submit_feedback` tool's proposal: the agent's draft report. The
    /// bin seeds the `Feedback` overlay from these fields; the user edits and
    /// confirms. `kind` is the string form ("bug"|"feature"|"general").
    Feedback {
        kind: String,
        title: String,
        body: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Reasoning/thinking text from the model. Ephemeral — pushed to the
    /// in-memory `EventLog` but NOT persisted to SQLite (skipped by
    /// `emit_ephemeral`). Only the final sub-turn's reasoning is kept;
    /// intermediate reasoning from tool-selection sub-turns is discarded.
    /// The projection attaches this to the next `ChatMsg::Assistant`.
    ModelThinking {
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
        #[serde(default)]
        error_kind: Option<ErrorKind>,
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
        subagent_id: String,
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
    /// Live-edge re-assertion marker (spec: re-floor). Records the
    /// cumulative-appended token value at the moment a re-floor fired, so the
    /// interval spans the whole session. Weightless: inert for eviction and
    /// context_window; never projected into a ChatMsg.
    DirectiveReasserted {
        at_cumulative: u64,
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
        rescue: Option<crate::eviction::RescueRationale>,
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
    /// A one-shot wake scheduled by the agent (`schedule_wake` tool). Persisted
    /// so it survives restart; the pending set is the projection of every
    /// WakeScheduled with no matching WakeFired/WakeCancelled. Bookkeeping only —
    /// inert to conversation rendering; the injected UserMessage is what shows.
    WakeScheduled {
        wake_id: String,
        fire_at_ms: i64,
        note: String,
    },
    /// A scheduled wake actually fired (injected its note + spawned a turn).
    /// Written ONLY at injection, so a crash before injection re-fires on reload.
    WakeFired {
        wake_id: String,
    },
    /// A scheduled wake was cancelled before firing (`cancel_wake` tool).
    WakeCancelled {
        wake_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
                thinking: 0,
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
                thinking: 0,
                input: 1,
                output: 2,
                cached: 3
            })
        );
    }

    #[test]
    fn question_kind_feedback_round_trips() {
        let k = QuestionKind::Feedback {
            kind: "bug".into(),
            title: "Crash".into(),
            body: "steps".into(),
        };
        let s = serde_json::to_string(&k).unwrap();
        let back: QuestionKind = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, QuestionKind::Feedback { kind, .. } if kind == "bug"));
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
                error_kind: None,
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
                thinking: 0,
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
                subagent_id: "sub-test".into(),
                branch: "subagent:zz".into(),
                summary: "did it".into(),
                ok: false,
            },
        );
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
    }

    #[test]
    fn delegation_result_with_subagent_id_round_trips() {
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::DelegationResult {
                subagent_id: "sub-01HZTEST".into(),
                branch: "subagent:01HZTEST".into(),
                summary: "did it".into(),
                ok: true,
            },
        );
        let json = serde_json::to_string(&ev).unwrap();
        let restored: Event = serde_json::from_str(&json).unwrap();
        match &restored.kind {
            EventKind::DelegationResult { subagent_id, .. } => {
                assert_eq!(subagent_id, "sub-01HZTEST");
            }
            _ => panic!("expected DelegationResult"),
        }
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
            rescue: None,
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

    #[test]
    fn tool_result_deserializes_without_error_kind() {
        // Legacy JSON from before error_kind was added — must still load.
        let json = r#"{"ToolResult":{"id":"tc1","name":"read","output":"ok","is_error":false}}"#;
        let kind: EventKind = serde_json::from_str(json).unwrap();
        match kind {
            EventKind::ToolResult { id, error_kind, .. } => {
                assert_eq!(id, "tc1");
                assert_eq!(
                    error_kind,
                    None,
                    "legacy ToolResult must deserialize error_kind as None"
                );
            }
            _ => panic!("expected ToolResult"),
        }
    }
}
