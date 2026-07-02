//! Drives the agent loop with a scripted `update_tasks` call and asserts the
//! loop routes it as an Emitting tool: a faithful `EventKind::Tasks` snapshot
//! is appended (not run through `run_tool`'s defensive error path), and a
//! non-error ack `ToolResult` is fed back to the model.

use std::sync::Arc;
use tokio::sync::mpsc;

use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::ProviderEvent;

fn fixed_now() -> i64 {
    0
}

#[tokio::test]
async fn update_tasks_appends_a_tasks_event_and_acks() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call(
            "update_tasks",
            serde_json::json!({"tasks": [
                {"text": "step one", "status": "active"},
                {"text": "step two", "status": "pending"},
            ]}),
        ),
        zoid_testkit::text("ok"),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "update tasks".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();

    let complete = drain.await.unwrap();
    assert!(complete, "loop must emit TurnComplete");

    // A Tasks event was appended with the two items, faithfully.
    let snapshot = zoid_core::tasks::tasks(&events);
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].status, zoid_core::tasks::TaskStatus::Active);

    // And a non-error ack ToolResult was fed back.
    let acks = zoid_testkit::tool_results(&events);
    assert!(acks
        .iter()
        .any(|(n, out, err)| n == "update_tasks" && !err && out.contains("task")));
}
