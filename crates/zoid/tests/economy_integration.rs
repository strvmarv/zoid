use std::sync::Arc;
use tokio::sync::mpsc;
use ulid::Ulid;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::economy::token_ledger;
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent, Usage};

fn now() -> i64 {
    0
}

#[tokio::test]
async fn turn_usage_lands_in_ledger() {
    // Fake provider: a little text, then a usage report, then done.
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("hello".into()),
        ProviderEvent::Usage(Usage {
            input_tokens: 120,
            output_tokens: 18,
        }),
        ProviderEvent::Done,
    ]));
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let session = SessionHandle::spawn(tmp.path().to_str().unwrap()).unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage { text: "hi".into() },
    )];
    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        Arc::new(zoid_tools::registry()),
        session.clone(),
        seed,
        "m".into(),
        tx,
        Ulid::new(),
        now,
    )
    .await
    .unwrap();
    while rx.recv().await.is_some() {}

    let events = session.snapshot().await.unwrap();
    let ledger = token_ledger(&events);
    assert_eq!(ledger.input, 120);
    assert_eq!(ledger.output, 18);
    assert_eq!(ledger.total, 138);
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::Usage)));
}
