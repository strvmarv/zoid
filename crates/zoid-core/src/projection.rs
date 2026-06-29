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
        }
    }
    flush(&mut pending, &mut turns);
    turns
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
    }
}
