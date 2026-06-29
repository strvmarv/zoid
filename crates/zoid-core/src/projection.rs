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
pub fn transcript(events: &[Event]) -> Vec<Turn> {
    events
        .iter()
        .map(|e| match &e.kind {
            EventKind::UserMessage { text } => Turn { role: Role::User, text: text.clone() },
            EventKind::AssistantMessage { text } => Turn { role: Role::Assistant, text: text.clone() },
        })
        .collect()
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
    }
}
