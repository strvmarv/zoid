//! End-to-end at the projection level: a subagent runs on its own sub-branch,
//! the orchestrator's `DelegationResult` (recorded on main) folds into
//! `conversation()` as a single `Delegated` card, and the subagent's own turn
//! output stays HIDDEN from the main conversation.

use std::sync::Arc;

use tokio::sync::mpsc;
use ulid::Ulid;

use zoid::subagent::run_subagent;
use zoid_core::agent_profile::AgentProfile;
use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent};

#[tokio::test]
async fn delegated_result_folds_into_main_conversation() {
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("Added the function.".into()),
        ProviderEvent::Done,
    ]));
    let session = SessionHandle::spawn(":memory:").unwrap();

    // Seed the user turn on main (the request that triggered delegation).
    session
        .append(Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "delegate: add fn".into() }))
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let seed = session.snapshot().await.unwrap();
    let res = run_subagent(
        "add fn",
        &seed,
        &AgentProfile::builtin(),
        provider,
        std::path::PathBuf::from("."),
        "glm".into(),
        session.clone(),
        Ulid::new(),
        tx,
        || 0,
    )
    .await
    .unwrap();

    // Orchestrator records the result on main.
    session
        .append(Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::DelegationResult { branch: res.branch, summary: res.summary, ok: res.ok },
        ))
        .await
        .unwrap();

    let conv = conversation(&session.snapshot().await.unwrap());
    assert_eq!(conv.first(), Some(&ChatMsg::User { text: "delegate: add fn".into(), ts: 0 }));
    assert!(matches!(conv.last(), Some(ChatMsg::Delegated { ok: true, .. })));
    // Subagent work events exist in the log but are NOT in the main conversation.
    assert!(!conv.iter().any(|m| matches!(m, ChatMsg::Assistant { text, .. } if text == "Added the function.")));
}
