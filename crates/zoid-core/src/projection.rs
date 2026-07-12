use crate::event::{Event, EventKind};

/// A reference to a tool call as folded from the log (args kept as raw JSON
/// text, matching `EventKind::ToolCall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// A conversation item: the tool-aware projection consumed by both the renderer
/// and the provider request builder. An assistant item carries any tool calls it
/// made in the same turn; tool results are their own items, in log order.
///
/// `ts` is the originating event's wall-clock stamp (epoch millis, supplied by
/// the bin — the core stays clock-free). For a folded assistant turn it's the ts
/// of the first event that contributed to the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMsg {
    User {
        text: String,
        ts: i64,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallRef>,
        ts: i64,
        /// Reasoning/thinking text from the final sub-turn (from a preceding
        /// `ModelThinking` event). `None` when the model didn't reason.
        thinking: Option<String>,
    },
    ToolResult {
        id: String,
        name: String,
        output: String,
        is_error: bool,
        /// Set when a `ToolResultCompacted` event replaced this result's body
        /// with a summary. The transcript marks it; the live request carries it.
        compacted: bool,
        ts: i64,
    },
    /// A folded subagent delegation — rendered as a collapsible card (① zoom).
    Delegated {
        summary: String,
        ok: bool,
    },
    /// An inline question card (from `EventKind::QuestionAsked` + optional
    /// matching `QuestionAnswered`). `Open` means no answer yet — the card is
    /// live and captures input; `build_conversation` fills the cursor from
    /// `ShellState.question` at render time. `Answered` means the card has
    /// collapsed to a one-line summary.
    Question {
        id: String,
        kind: crate::event::QuestionKind,
        question: String,
        choices: Vec<String>,
        state: QuestionCardState,
        ts: i64,
    },
}

/// What the card renders as. The projection decides Open vs Answered from the
/// event log; `build_conversation` overwrites the `Open` cursor with
/// `ShellState.question`'s live values before rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionCardState {
    /// No matching `QuestionAnswered` yet — the card is live (captures input).
    /// `selected`/`free_text` are placeholder defaults from the projection;
    /// `build_conversation` overwrites them with `ShellState.question`'s live
    /// cursor before rendering.
    Open { selected: usize, free_text: String },
    /// `QuestionAnswered` has landed — the card is collapsed to a one-line summary.
    Answered { answer: String },
}

/// Fold the event log into ordered `ChatMsg` items. A run of `ModelDelta` plus
/// any `ToolCall`s before the next user/tool-result/assistant boundary collapses
/// into one `Assistant` item; `ToolResult` events become their own items. Pure.
///
/// `TurnsDropped` markers do NOT filter the conversation: the transcript pane
/// and the model request both see the full history. Turn-dropping is a
/// `context_window`-only concern (the economy/compaction view), never a
/// transcript or request concern — filtering here would silently wipe the
/// visible history and the model's context, which is what compaction was
/// doing. (See `context.rs::context_window` for the window-scoped filter.)
pub fn conversation<'a>(events: impl IntoIterator<Item = &'a Event>) -> Vec<ChatMsg> {
    conversation_for_branch(events, &crate::event::BranchId::default())
}

/// Like [`conversation`], but in addition to the default (main) branch it also
/// keeps events on `active`. The agent turn loop uses this when rebuilding the
/// provider request: a subagent's entire transcript lives on its own
/// `subagent:<id>` branch, so filtering to the default branch alone yields an
/// EMPTY message list — the provider then receives a system-message-only body
/// and rejects it with HTTP 400. Passing the subagent's branch as `active`
/// makes the subagent's own turns visible to itself.
///
/// When `active` IS the default branch (the main chat), this is byte-identical
/// to filtering to the default branch only — subagent branches stay excluded
/// from the main conversation.
pub fn conversation_for_branch<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    active: &crate::event::BranchId,
) -> Vec<ChatMsg> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    let evicted = crate::eviction::evicted_ids(events.iter().copied());

    // ACM-1: a tool-result whose id has a later ToolResultCompacted is emitted
    // as its summary (last write wins), both to the live request and the view.
    let mut compacted: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for e in visible {
        if let EventKind::ToolResultCompacted { id, summary, .. } = &e.kind {
            compacted.insert(id.as_str(), summary.as_str());
        }
    }

    // Pair QuestionAsked → QuestionAnswered by id, and record which tool-result
    // ids belong to a question (so they can be suppressed from the view — the
    // card is the human-facing record; the ToolResult stays in the log for the
    // model/projections/compaction but is hidden from the conversation).
    let mut answered: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut question_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in visible {
        match &e.kind {
            EventKind::QuestionAsked { id, kind, .. } => {
                // Approval-gate questions do NOT suppress the real ToolResult
                // -- the model needs the tool actual output, not the approval
                // string. Only Ask/ModeMapping suppress it.
                if !matches!(kind, crate::event::QuestionKind::Approval) {
                    question_ids.insert(id.as_str());
                }
            }
            EventKind::QuestionAnswered { id, answer } => {
                answered.insert(id.as_str(), answer.as_str());
            }
            _ => {}
        }
    }

    let mut out: Vec<ChatMsg> = Vec::new();
    let mut text: Option<String> = None;
    let mut calls: Vec<ToolCallRef> = Vec::new();
    // ts of the first event that contributed to the in-progress assistant turn.
    let mut turn_ts: Option<i64> = None;
    let mut pending_thinking: Option<String> = None;

    fn flush(
        text: &mut Option<String>,
        calls: &mut Vec<ToolCallRef>,
        turn_ts: &mut Option<i64>,
        out: &mut Vec<ChatMsg>,
        thinking: Option<String>,
    ) {
        if text.is_some() || !calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: text.take().unwrap_or_default(),
                tool_calls: std::mem::take(calls),
                ts: turn_ts.take().unwrap_or(0),
                thinking,
            });
        }
        *turn_ts = None;
    }

    for e in visible {
        if evicted.contains(&e.id) {
            continue;
        }
        if e.branch != crate::event::BranchId::default() && e.branch != *active {
            continue;
        }
        match &e.kind {
            EventKind::UserMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                out.push(ChatMsg::User {
                    text: t.clone(),
                    ts: e.ts,
                });
            }
            EventKind::AssistantMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                out.push(ChatMsg::Assistant {
                    thinking: None,
                    text: t.clone(),
                    tool_calls: Vec::new(),
                    ts: e.ts,
                });
            }
            EventKind::ModelThinking { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                pending_thinking = Some(t.clone());
            }
            EventKind::ModelDelta { text: t } => {
                turn_ts.get_or_insert(e.ts);
                text.get_or_insert_with(String::new).push_str(t);
            }
            EventKind::ToolCall { id, name, args } => {
                turn_ts.get_or_insert(e.ts);
                calls.push(ToolCallRef {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
            }
            EventKind::ToolResult {
                id,
                name,
                output,
                is_error,
            } => {
                // Suppress the tool-result line when a QuestionAsked owns this
                // id — the card is the human-facing record.
                if question_ids.contains(id.as_str()) {
                    // The assistant turn that made the call(s) still ends here.
                    flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                    continue;
                }
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                let (output, was_compacted) = match compacted.get(id.as_str()) {
                    Some(sum) => ((*sum).to_string(), true),
                    None => (output.clone(), false),
                };
                out.push(ChatMsg::ToolResult {
                    id: id.clone(),
                    name: name.clone(),
                    output,
                    is_error: *is_error,
                    compacted: was_compacted,
                    ts: e.ts,
                });
            }
            EventKind::QuestionAsked {
                id,
                kind,
                question,
                choices,
            } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                let state = match answered.get(id.as_str()) {
                    Some(ans) => QuestionCardState::Answered {
                        answer: (*ans).to_string(),
                    },
                    None => QuestionCardState::Open {
                        selected: 0,
                        free_text: String::new(),
                    },
                };
                out.push(ChatMsg::Question {
                    id: id.clone(),
                    kind: kind.clone(),
                    question: question.clone(),
                    choices: choices.clone(),
                    state,
                    ts: e.ts,
                });
            }
            EventKind::QuestionAnswered { .. } => {
                // Folded into the matching QuestionAsked card above; not a
                // standalone conversation item.
            }
            EventKind::DelegationResult { summary, ok, .. } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
                out.push(ChatMsg::Delegated {
                    summary: summary.clone(),
                    ok: *ok,
                });
            }
            EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. } => {
                // Economy bookkeeping; folded elsewhere, not a raw conversation item.
            }
            EventKind::Tasks { .. } => {
                // Rail-only snapshot; never inlined into the conversation transcript.
            }
            EventKind::TurnsDropped { .. } => {
                // Metadata marker; not a conversation item.
            }
            EventKind::TurnsEvicted { .. }
            | EventKind::TurnsReadmitted { .. }
            | EventKind::DirectiveReasserted { .. } => {
                // Metadata marker; not a conversation item. (Out of scope: rendering
                // the in-context breadcrumb / recall filtering is a later slice.)
            }
            EventKind::WakeScheduled { .. }
            | EventKind::WakeFired { .. }
            | EventKind::WakeCancelled { .. } => { /* bookkeeping: no conversation row */ }
        }
    }
    flush(&mut text, &mut calls, &mut turn_ts, &mut out, pending_thinking.take());
    // If thinking was the last event with no following assistant message,
    // emit a standalone assistant message with empty text + the thinking.
    if let Some(thinking) = pending_thinking.take() {
        if text.is_none() && calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: String::new(),
                tool_calls: Vec::new(),
                ts: turn_ts.unwrap_or(0),
                thinking: Some(thinking),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use proptest::prelude::*;
    use ulid::Ulid;

    fn user(id: u128, text: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::UserMessage { text: text.into() },
        )
    }
    fn delta(id: u128, text: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::ModelDelta { text: text.into() },
        )
    }
    fn tcall(id: u128, name: &str, args: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::ToolCall {
                id: "".into(),
                name: name.into(),
                args: args.into(),
            },
        )
    }
    fn tres(id: u128, name: &str, output: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::ToolResult {
                id: "".into(),
                name: name.into(),
                output: output.into(),
                is_error: false,
            },
        )
    }

    #[test]
    fn conversation_folds_text_calls_results_in_order() {
        let events = vec![
            user(1, "read a"),
            delta(2, "let me "),
            delta(3, "look"),
            tcall(4, "read_file", r#"{"path":"a"}"#),
            tres(5, "read_file", "data"),
            delta(6, "it says data"),
        ];
        let conv = conversation(&events);
        assert_eq!(
            conv,
            vec![
                ChatMsg::User {
                    text: "read a".into(),
                    ts: 0
                },
                ChatMsg::Assistant {
                        thinking: None,
                    text: "let me look".into(),
                    tool_calls: vec![ToolCallRef {
                        id: "".into(),
                        name: "read_file".into(),
                        args: r#"{"path":"a"}"#.into()
                    }],
                    ts: 0,
                },
                ChatMsg::ToolResult {
                    id: "".into(),
                    name: "read_file".into(),
                    output: "data".into(),
                    is_error: false,
                    compacted: false,
                    ts: 0
                },
                ChatMsg::Assistant {
                    thinking: None,
                    text: "it says data".into(),
                    tool_calls: vec![],
                    ts: 0
                },
            ]
        );
    }

    #[test]
    fn tool_call_only_turn_has_empty_text() {
        let events = vec![
            user(1, "go"),
            tcall(2, "shell", r#"{"command":"ls"}"#),
            tres(3, "shell", "ok"),
        ];
        let conv = conversation(&events);
        assert_eq!(
            conv[1],
            ChatMsg::Assistant {
                    thinking: None,
                text: "".into(),
                tool_calls: vec![ToolCallRef {
                    id: "".into(),
                    name: "shell".into(),
                    args: r#"{"command":"ls"}"#.into()
                }],
                ts: 0,
            }
        );
    }

    #[test]
    fn tool_result_preserves_id_and_error_through_fold() {
        // Tasks 7-9 rely on `id` and `is_error` surviving the fold (id correlates
        // calls→results; is_error flags failures back to the model and the UI).
        let events = vec![
            user(1, "go"),
            Event::new(
                Ulid::from(2u128),
                None,
                0,
                EventKind::ToolCall {
                    id: "call_1".into(),
                    name: "shell".into(),
                    args: r#"{"command":"false"}"#.into(),
                },
            ),
            Event::new(
                Ulid::from(3u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "call_1".into(),
                    name: "shell".into(),
                    output: "boom\n[exit 1]".into(),
                    is_error: true,
                },
            ),
        ];
        let conv = conversation(&events);
        assert_eq!(
            conv[1],
            ChatMsg::Assistant {
                    thinking: None,
                text: "".into(),
                tool_calls: vec![ToolCallRef {
                    id: "call_1".into(),
                    name: "shell".into(),
                    args: r#"{"command":"false"}"#.into()
                }],
                ts: 0,
            }
        );
        assert_eq!(
            conv[2],
            ChatMsg::ToolResult {
                id: "call_1".into(),
                name: "shell".into(),
                output: "boom\n[exit 1]".into(),
                is_error: true,
                compacted: false,
                ts: 0,
            }
        );
    }

    #[test]
    fn conversation_ignores_usage_and_mutation() {
        let evs = vec![
            user(1, "hi"),
            Event::new(Ulid::from(2u128), None, 0, EventKind::Usage),
            Event::new(
                Ulid::from(3u128),
                None,
                0,
                EventKind::ContextMutation {
                    item: "file:a".into(),
                    op: crate::event::MutationOp::Evict,
                },
            ),
            delta(4, "yo"),
        ];
        let msgs = conversation(&evs);
        assert_eq!(
            msgs,
            vec![
                ChatMsg::User {
                    text: "hi".into(),
                    ts: 0
                },
                ChatMsg::Assistant {
                    thinking: None,
                    text: "yo".into(),
                    tool_calls: vec![],
                    ts: 0
                },
            ]
        );
    }

    #[test]
    fn ts_propagates_and_folded_turn_uses_first_event() {
        // User carries its own ts; a folded assistant turn (delta+call) carries
        // the ts of the FIRST contributing event; the tool result its own.
        let ev = |id: u128, ts: i64, kind| Event::new(Ulid::from(id), None, ts, kind);
        let events = vec![
            ev(1, 100, EventKind::UserMessage { text: "go".into() }),
            ev(
                2,
                200,
                EventKind::ModelDelta {
                    text: "on it".into(),
                },
            ),
            ev(
                3,
                250,
                EventKind::ToolCall {
                    id: "c".into(),
                    name: "shell".into(),
                    args: "{}".into(),
                },
            ),
            ev(
                4,
                300,
                EventKind::ToolResult {
                    id: "c".into(),
                    name: "shell".into(),
                    output: "ok".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        assert!(matches!(conv[0], ChatMsg::User { ts: 100, .. }));
        assert!(
            matches!(conv[1], ChatMsg::Assistant { ts: 200, .. }),
            "folded turn uses first event ts"
        );
        assert!(matches!(conv[2], ChatMsg::ToolResult { ts: 300, .. }));
    }

    #[test]
    fn conversation_skips_subagent_branch_and_folds_result() {
        use crate::event::BranchId;
        let mut work = Event::new(
            Ulid::from(10u128),
            None,
            0,
            EventKind::ModelDelta {
                text: "subagent thinking".into(),
            },
        );
        work.branch = BranchId("subagent:ax3".into());
        let result = Event::new(
            Ulid::from(11u128),
            None,
            0,
            EventKind::DelegationResult {
                subagent_id: "sub-ax3".into(),
                branch: "subagent:ax3".into(),
                summary: "Refactored parse()".into(),
                ok: true,
            },
        );
        let evs = vec![user(1, "delegate this"), work, result];
        let conv = conversation(&evs);
        assert_eq!(
            conv,
            vec![
                ChatMsg::User {
                    text: "delegate this".into(),
                    ts: 0
                },
                ChatMsg::Delegated {
                    summary: "Refactored parse()".into(),
                    ok: true
                },
            ]
        );
    }

    #[test]
    fn conversation_for_branch_includes_active_subagent_branch() {
        use crate::event::BranchId;
        let sub = BranchId("subagent:ax3".into());
        let mut seed = user(1, "do the task");
        seed.branch = sub.clone();
        // The default conversation() still excludes non-main-branch events, so
        // a subagent's transcript never leaks into the MAIN conversation …
        assert!(
            conversation(&[seed.clone()]).is_empty(),
            "default conversation must still exclude subagent-branch events"
        );
        // … but conversation_for_branch keeps the active branch, so when a
        // subagent rebuilds its OWN provider request it sees its seed turn.
        // Without this the request carries only the system message → HTTP 400.
        let msgs = conversation_for_branch(&[seed], &sub);
        assert_eq!(
            msgs,
            vec![ChatMsg::User {
                text: "do the task".into(),
                ts: 0
            }]
        );
    }

    #[test]
    fn conversation_substitutes_compacted_summary() {
        let evs = vec![
            Event::new(
                Ulid::new(),
                None,
                100,
                EventKind::UserMessage { text: "go".into() },
            ),
            Event::new(
                Ulid::new(),
                None,
                200,
                EventKind::ToolResult {
                    id: "c1".into(),
                    name: "search".into(),
                    output: "HUGE ORIGINAL OUTPUT".into(),
                    is_error: false,
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                300,
                EventKind::ToolResultCompacted {
                    id: "c1".into(),
                    summary: "tiny summary".into(),
                    original_tokens: 500,
                },
            ),
        ];
        let conv = conversation(&evs);
        let tr = conv
            .iter()
            .find_map(|m| match m {
                ChatMsg::ToolResult {
                    id,
                    output,
                    compacted,
                    ..
                } if id == "c1" => Some((output.clone(), *compacted)),
                _ => None,
            })
            .expect("tool result present");
        assert_eq!(
            tr.0, "tiny summary",
            "live request must carry the summary, not the dump"
        );
        assert!(tr.1, "must be flagged compacted for the transcript");
    }

    #[test]
    fn conversation_does_not_filter_turns_dropped() {
        // TurnsDropped is now inert metadata — it must NOT filter the
        // conversation (transcript + model request). All turns stay visible.
        let ev = |id: u128, ts: i64, kind| Event::new(Ulid::from(id), None, ts, kind);
        let events = vec![
            ev(
                1,
                100,
                EventKind::UserMessage {
                    text: "turn 0".into(),
                },
            ),
            ev(
                2,
                200,
                EventKind::ModelDelta {
                    text: "reply 0".into(),
                },
            ),
            ev(
                3,
                300,
                EventKind::UserMessage {
                    text: "turn 1".into(),
                },
            ),
            ev(
                4,
                400,
                EventKind::ModelDelta {
                    text: "reply 1".into(),
                },
            ),
            // Marker claiming turns 0-1 were dropped.
            ev(5, 450, EventKind::TurnsDropped { turns_dropped: 2 }),
            ev(
                6,
                500,
                EventKind::UserMessage {
                    text: "turn 2".into(),
                },
            ),
            ev(
                7,
                600,
                EventKind::ModelDelta {
                    text: "reply 2".into(),
                },
            ),
        ];
        let conv = conversation(&events);
        // All three turns must be present — the marker does NOT filter.
        let texts: Vec<&str> = conv
            .iter()
            .filter_map(|m| match m {
                ChatMsg::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["turn 0", "turn 1", "turn 2"]);
    }

    proptest! {
        #[test]
        fn conversation_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,8}", 0..15)) {
            let events: Vec<Event> = texts.iter().enumerate().map(|(i, t)| user(i as u128 + 1, t)).collect();
            prop_assert_eq!(conversation(&events), conversation(&events));
        }
    }

    #[test]
    fn conversation_skips_evicted_turns() {
        use crate::event::{Event, EventKind, EvictionMarker};
        use ulid::Ulid;
        let mk = |id: u128, k| Event::new(Ulid::from(id), None, id as i64, k);
        let events = vec![
            mk(1, EventKind::UserMessage { text: "old".into() }),
            mk(
                2,
                EventKind::AssistantMessage {
                    text: "old-reply".into(),
                },
            ),
            mk(3, EventKind::UserMessage { text: "new".into() }),
            mk(
                9,
                EventKind::TurnsEvicted {
                    ids: vec![Ulid::from(1u128), Ulid::from(2u128)],
                    reclaimed_tokens: 5,
                    marker: EvictionMarker { spans: vec![] },
                },
            ),
        ];
        let msgs = conversation(&events);
        assert_eq!(msgs.len(), 1); // only the "new" user message survives
        assert!(matches!(&msgs[0], ChatMsg::User { text, .. } if text == "new"));
    }

    use crate::event::QuestionKind;
    use crate::wizard::{MappingEntry, ModeMapping};

    fn q_asked(id: u128, qid: &str, question: &str, choices: &[&str]) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::QuestionAsked {
                id: qid.into(),
                kind: QuestionKind::Ask,
                question: question.into(),
                choices: choices.iter().map(|s| s.to_string()).collect(),
            },
        )
    }

    fn q_answered(id: u128, qid: &str, answer: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::QuestionAnswered {
                id: qid.into(),
                answer: answer.into(),
            },
        )
    }

    #[test]
    fn question_asked_alone_folds_to_open_card() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry or skip?", &["Retry", "Skip"]),
        ];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question {
                state: QuestionCardState::Open { .. },
                ..
            })
        ));
    }

    #[test]
    fn question_asked_then_answered_folds_to_answered_card() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry or skip?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
        ];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question { state: QuestionCardState::Answered { answer }, .. }) if answer == "Skip"
        ));
    }

    #[test]
    fn tool_result_matching_question_asked_is_suppressed() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
            Event::new(
                Ulid::from(4u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "c1".into(),
                    name: "ask_user".into(),
                    output: "Skip".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        assert!(
            !conv
                .iter()
                .any(|m| matches!(m, ChatMsg::ToolResult { id, .. } if id == "c1")),
            "ToolResult matching a QuestionAsked must be suppressed"
        );
    }

    fn feedback_q_asked(id: u128, qid: &str, kind: &str, title: &str, body: &str) -> Event {
        Event::new(
            Ulid::from(id),
            None,
            0,
            EventKind::QuestionAsked {
                id: qid.into(),
                kind: QuestionKind::Feedback {
                    kind: kind.into(),
                    title: title.into(),
                    body: body.into(),
                },
                question: format!("Submit {kind} feedback?"),
                choices: vec!["Submit".into(), "Cancel".into()],
            },
        )
    }

    #[test]
    fn feedback_question_suppresses_paired_tool_result() {
        let events = vec![
            user(1, "report a bug"),
            feedback_q_asked(2, "fb1", "bug", "Crash", "steps"),
            q_answered(3, "fb1", "Created issue #7"),
            Event::new(
                Ulid::from(4u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "fb1".into(),
                    name: "submit_feedback".into(),
                    output: "Created issue #7".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        assert!(
            !conv
                .iter()
                .any(|m| matches!(m, ChatMsg::ToolResult { id, .. } if id == "fb1")),
            "ToolResult matching a Feedback QuestionAsked must be suppressed"
        );
    }

    #[test]
    fn unrelated_tool_result_still_renders() {
        let events = vec![
            user(1, "go"),
            q_asked(2, "c1", "retry?", &["Retry", "Skip"]),
            q_answered(3, "c1", "Skip"),
            Event::new(
                Ulid::from(4u128),
                None,
                0,
                EventKind::ToolResult {
                    id: "c2".into(),
                    name: "read_file".into(),
                    output: "data".into(),
                    is_error: false,
                },
            ),
        ];
        let conv = conversation(&events);
        assert!(
            conv.iter()
                .any(|m| matches!(m, ChatMsg::ToolResult { id, .. } if id == "c2")),
            "unrelated ToolResult must still render"
        );
    }

    #[test]
    fn mode_mapping_question_folds_to_open_card() {
        let mapping = ModeMapping {
            mode_name: "brainstorm".into(),
            mode_description: "".into(),
            mode_body: "".into(),
            entries: vec![MappingEntry::Materialize {
                canonical_path: "skills/brainstorming/SKILL.md".into(),
                source: "up/SKILL.md".into(),
                summary: "skill".into(),
            }],
        };
        let events = vec![Event::new(
            Ulid::from(1u128),
            None,
            0,
            EventKind::QuestionAsked {
                id: "c1".into(),
                kind: QuestionKind::ModeMapping {
                    mapping: Box::new(mapping),
                },
                question: "review".into(),
                choices: vec!["Approve".into(), "Reject".into(), "Adjust".into()],
            },
        )];
        let conv = conversation(&events);
        assert!(matches!(
            conv.last(),
            Some(ChatMsg::Question {
                state: QuestionCardState::Open { .. },
                ..
            })
        ));
    }
}
