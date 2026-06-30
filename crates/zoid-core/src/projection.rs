use crate::event::{Event, EventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// The Transcript projection: a pure fold over the event log into ordered turns.
/// A run of consecutive `ModelDelta` events collapses into a single assistant
/// `Turn` (concatenated text); `UserMessage`/`AssistantMessage` each map to one
/// turn. Pure: no I/O, no clock.
pub fn transcript(events: &[Event]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut pending: Option<String> = None;

    fn flush(pending: &mut Option<String>, turns: &mut Vec<Turn>) {
        if let Some(text) = pending.take() {
            turns.push(Turn { role: Role::Assistant, text });
        }
    }

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => {
                flush(&mut pending, &mut turns);
                turns.push(Turn { role: Role::User, text: text.clone() });
            }
            EventKind::AssistantMessage { text } => {
                flush(&mut pending, &mut turns);
                turns.push(Turn { role: Role::Assistant, text: text.clone() });
            }
            EventKind::ModelDelta { text } => {
                pending.get_or_insert_with(String::new).push_str(text);
            }
            // Tool events do not appear in the text-only transcript.
            EventKind::ToolCall { .. } | EventKind::ToolResult { .. } => {}
        }
    }
    flush(&mut pending, &mut turns);
    turns
}

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
    fn asst(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::AssistantMessage { text: text.into() })
    }
    fn delta(id: u128, text: &str) -> Event {
        Event::new(Ulid::from(id), None, 0, EventKind::ModelDelta { text: text.into() })
    }

    #[test]
    fn consecutive_deltas_fold_into_one_assistant_turn() {
        let events = vec![user(1, "hi"), delta(2, "he"), delta(3, "ll"), delta(4, "o")];
        let turns = transcript(&events);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], Turn { role: Role::User, text: "hi".into() });
        assert_eq!(turns[1], Turn { role: Role::Assistant, text: "hello".into() });
    }

    #[test]
    fn delta_run_ends_at_next_user_message() {
        let events = vec![user(1, "a"), delta(2, "x"), delta(3, "y"), user(4, "b"), delta(5, "z")];
        let turns = transcript(&events);
        assert_eq!(turns, vec![
            Turn { role: Role::User, text: "a".into() },
            Turn { role: Role::Assistant, text: "xy".into() },
            Turn { role: Role::User, text: "b".into() },
            Turn { role: Role::Assistant, text: "z".into() },
        ]);
    }

    #[test]
    fn assistant_message_and_delta_run_are_separate_turns() {
        let events = vec![asst(1, "full"), delta(2, "d1"), delta(3, "d2")];
        let turns = transcript(&events);
        assert_eq!(turns, vec![
            Turn { role: Role::Assistant, text: "full".into() },
            Turn { role: Role::Assistant, text: "d1d2".into() },
        ]);
    }

    #[test]
    fn maps_events_to_turns_in_order() {
        let events = vec![user(1, "q"), asst(2, "a")];
        let turns = transcript(&events);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], Turn { role: Role::User, text: "q".into() });
        assert_eq!(turns[1], Turn { role: Role::Assistant, text: "a".into() });
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

    proptest! {
        #[test]
        fn transcript_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,12}", 0..20)) {
            let events: Vec<Event> = texts.iter().enumerate()
                .map(|(i, t)| user(i as u128 + 1, t))
                .collect();
            prop_assert_eq!(transcript(&events), transcript(&events));
            prop_assert_eq!(transcript(&events).len(), events.len());
        }

        #[test]
        fn delta_fold_is_deterministic(frags in proptest::collection::vec("[a-z]{0,6}", 0..30)) {
            let events: Vec<Event> = frags.iter().enumerate()
                .map(|(i, t)| delta(i as u128 + 1, t))
                .collect();
            let once = transcript(&events);
            prop_assert_eq!(&once, &transcript(&events));
            // A non-empty delta run folds to exactly one assistant turn.
            if !events.is_empty() {
                prop_assert_eq!(once.len(), 1);
                prop_assert_eq!(&once[0].text, &frags.concat());
            }
        }

        #[test]
        fn conversation_is_deterministic(texts in proptest::collection::vec("[a-z ]{0,8}", 0..15)) {
            let events: Vec<Event> = texts.iter().enumerate().map(|(i, t)| user(i as u128 + 1, t)).collect();
            prop_assert_eq!(conversation(&events), conversation(&events));
        }
    }
}
