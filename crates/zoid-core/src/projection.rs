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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatMsg {
    User(String),
    Assistant { text: String, tool_calls: Vec<ToolCallRef> },
    ToolResult { id: String, name: String, output: String, is_error: bool },
}

/// Fold the event log into ordered `ChatMsg` items. A run of `ModelDelta` plus
/// any `ToolCall`s before the next user/tool-result/assistant boundary collapses
/// into one `Assistant` item; `ToolResult` events become their own items. Pure.
pub fn conversation(events: &[Event]) -> Vec<ChatMsg> {
    let mut out: Vec<ChatMsg> = Vec::new();
    let mut text: Option<String> = None;
    let mut calls: Vec<ToolCallRef> = Vec::new();

    fn flush(text: &mut Option<String>, calls: &mut Vec<ToolCallRef>, out: &mut Vec<ChatMsg>) {
        if text.is_some() || !calls.is_empty() {
            out.push(ChatMsg::Assistant {
                text: text.take().unwrap_or_default(),
                tool_calls: std::mem::take(calls),
            });
        }
    }

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text: t } => {
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::User(t.clone()));
            }
            EventKind::AssistantMessage { text: t } => {
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::Assistant { text: t.clone(), tool_calls: Vec::new() });
            }
            EventKind::ModelDelta { text: t } => {
                text.get_or_insert_with(String::new).push_str(t);
            }
            EventKind::ToolCall { id, name, args } => {
                calls.push(ToolCallRef { id: id.clone(), name: name.clone(), args: args.clone() });
            }
            EventKind::ToolResult { id, name, output, is_error } => {
                // The assistant turn that made the call(s) ends here.
                flush(&mut text, &mut calls, &mut out);
                out.push(ChatMsg::ToolResult {
                    id: id.clone(), name: name.clone(), output: output.clone(), is_error: *is_error,
                });
            }
            EventKind::Usage | EventKind::ContextMutation { .. } => {
                // Economy bookkeeping; not part of the conversation projection.
            }
        }
    }
    flush(&mut text, &mut calls, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use proptest::prelude::*;
    use ulid::Ulid;

    fn user(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::UserMessage { text: text.into() })
    }
    fn delta(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::ModelDelta { text: text.into() })
    }
    fn tcall(id: u128, name: &str, args: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::ToolCall {
            id: "".into(), name: name.into(), args: args.into(),
        })
    }
    fn tres(id: u128, name: &str, output: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::ToolResult {
            id: "".into(), name: name.into(), output: output.into(), is_error: false,
        })
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
        assert_eq!(conv, vec![
            ChatMsg::User("read a".into()),
            ChatMsg::Assistant {
                text: "let me look".into(),
                tool_calls: vec![ToolCallRef { id: "".into(), name: "read_file".into(), args: r#"{"path":"a"}"#.into() }],
            },
            ChatMsg::ToolResult { id: "".into(), name: "read_file".into(), output: "data".into(), is_error: false },
            ChatMsg::Assistant { text: "it says data".into(), tool_calls: vec![] },
        ]);
    }

    #[test]
    fn tool_call_only_turn_has_empty_text() {
        let events = vec![user(1, "go"), tcall(2, "shell", r#"{"command":"ls"}"#), tres(3, "shell", "ok")];
        let conv = conversation(&events);
        assert_eq!(conv[1], ChatMsg::Assistant {
            text: "".into(),
            tool_calls: vec![ToolCallRef { id: "".into(), name: "shell".into(), args: r#"{"command":"ls"}"#.into() }],
        });
    }

    #[test]
    fn tool_result_preserves_id_and_error_through_fold() {
        // Tasks 7-9 rely on `id` and `is_error` surviving the fold (id correlates
        // calls→results; is_error flags failures back to the model and the UI).
        let events = vec![
            user(1, "go"),
            Event::new(Ulid::from(2u128), None, 0, EventKind::ToolCall {
                id: "call_1".into(), name: "shell".into(), args: r#"{"command":"false"}"#.into(),
            }),
            Event::new(Ulid::from(3u128), None, 0, EventKind::ToolResult {
                id: "call_1".into(), name: "shell".into(), output: "boom\n[exit 1]".into(), is_error: true,
            }),
        ];
        let conv = conversation(&events);
        assert_eq!(conv[1], ChatMsg::Assistant {
            text: "".into(),
            tool_calls: vec![ToolCallRef { id: "call_1".into(), name: "shell".into(), args: r#"{"command":"false"}"#.into() }],
        });
        assert_eq!(conv[2], ChatMsg::ToolResult {
            id: "call_1".into(), name: "shell".into(), output: "boom\n[exit 1]".into(), is_error: true,
        });
    }

    #[test]
    fn conversation_ignores_usage_and_mutation() {
        let evs = vec![
            user(1, "hi"),
            Event::new(Ulid::from(2u128), None, 0, EventKind::Usage),
            Event::new(Ulid::from(3u128), None, 0, EventKind::ContextMutation {
                item: "file:a".into(), op: crate::event::MutationOp::Evict,
            }),
            delta(4, "yo"),
        ];
        let msgs = conversation(&evs);
        assert_eq!(msgs, vec![
            ChatMsg::User("hi".into()),
            ChatMsg::Assistant { text: "yo".into(), tool_calls: vec![] },
        ]);
    }

    proptest! {
        #[test]
        fn conversation_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,8}", 0..15)) {
            let events: Vec<Event> = texts.iter().enumerate().map(|(i, t)| user(i as u128 + 1, t)).collect();
            prop_assert_eq!(conversation(&events), conversation(&events));
        }
    }
}
