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
}

/// Fold the event log into ordered `ChatMsg` items. A run of `ModelDelta` plus
/// any `ToolCall`s before the next user/tool-result/assistant boundary collapses
/// into one `Assistant` item; `ToolResult` events become their own items. Pure.
pub fn conversation(events: &[Event]) -> Vec<ChatMsg> {
    // ACM-1: a tool-result whose id has a later ToolResultCompacted is emitted
    // as its summary (last write wins), both to the live request and the view.
    let mut compacted: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for e in events {
        if let EventKind::ToolResultCompacted { id, summary, .. } = &e.kind {
            compacted.insert(id.as_str(), summary.as_str());
        }
    }

    let mut out: Vec<ChatMsg> = Vec::new();
    let mut text: Option<String> = None;
    let mut calls: Vec<ToolCallRef> = Vec::new();
    // ts of the first event that contributed to the in-progress assistant turn.
    let mut turn_ts: Option<i64> = None;

    fn flush(
        text: &mut Option<String>,
        calls: &mut Vec<ToolCallRef>,
        turn_ts: &mut Option<i64>,
        out: &mut Vec<ChatMsg>,
    ) {
        if text.is_some() || !calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: text.take().unwrap_or_default(),
                tool_calls: std::mem::take(calls),
                ts: turn_ts.take().unwrap_or(0),
            });
        }
        *turn_ts = None;
    }

    for e in events {
        // Subagent work lives on its own branch and never appears in the main
        // conversation; only its folded DelegationResult (on main) surfaces.
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            EventKind::UserMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::User {
                    text: t.clone(),
                    ts: e.ts,
                });
            }
            EventKind::AssistantMessage { text: t } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
                out.push(ChatMsg::Assistant {
                    text: t.clone(),
                    tool_calls: Vec::new(),
                    ts: e.ts,
                });
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
                // The assistant turn that made the call(s) ends here.
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
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
            EventKind::DelegationResult { summary, ok, .. } => {
                flush(&mut text, &mut calls, &mut turn_ts, &mut out);
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
        }
    }
    flush(&mut text, &mut calls, &mut turn_ts, &mut out);
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
    fn conversation_substitutes_compacted_summary() {
        let evs = vec![
            Event::new(Ulid::new(), None, 100, EventKind::UserMessage { text: "go".into() }),
            Event::new(Ulid::new(), None, 200, EventKind::ToolResult { id: "c1".into(), name: "search".into(), output: "HUGE ORIGINAL OUTPUT".into(), is_error: false }),
            Event::new(Ulid::new(), None, 300, EventKind::ToolResultCompacted { id: "c1".into(), summary: "tiny summary".into(), original_tokens: 500 }),
        ];
        let conv = conversation(&evs);
        let tr = conv.iter().find_map(|m| match m {
            ChatMsg::ToolResult { id, output, compacted, .. } if id == "c1" => Some((output.clone(), *compacted)),
            _ => None,
        }).expect("tool result present");
        assert_eq!(tr.0, "tiny summary", "live request must carry the summary, not the dump");
        assert!(tr.1, "must be flagged compacted for the transcript");
    }

    proptest! {
        #[test]
        fn conversation_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,8}", 0..15)) {
            let events: Vec<Event> = texts.iter().enumerate().map(|(i, t)| user(i as u128 + 1, t)).collect();
            prop_assert_eq!(conversation(&events), conversation(&events));
        }
    }
}
