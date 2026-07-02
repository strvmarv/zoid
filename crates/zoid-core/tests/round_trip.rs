use tempfile::tempdir;
use ulid::Ulid;
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::store::EventStore;

#[test]
fn session_persists_and_replays_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.db");
    let path = path.to_str().unwrap();

    // First session: append two turns.
    let e1 = Event::new(
        Ulid::from(1u128),
        None,
        10,
        EventKind::UserMessage {
            text: "hello".into(),
        },
    );
    let e2 = Event::new(
        Ulid::from(2u128),
        Some(Ulid::from(1u128)),
        20,
        EventKind::AssistantMessage {
            text: "hi there".into(),
        },
    );
    {
        let store = EventStore::open(path).unwrap();
        store.append(&e1).unwrap();
        store.append(&e2).unwrap();
    } // store dropped — connection closed.

    // Second session: reopen, load, fold. Same events, same conversation.
    let store = EventStore::open(path).unwrap();
    let events = store.load_all().unwrap();
    assert_eq!(events, vec![e1, e2]);

    let msgs = conversation(&events);
    assert_eq!(
        msgs,
        vec![
            ChatMsg::User {
                text: "hello".into(),
                ts: 10
            },
            ChatMsg::Assistant {
                text: "hi there".into(),
                tool_calls: vec![],
                ts: 20
            },
        ]
    );
}
