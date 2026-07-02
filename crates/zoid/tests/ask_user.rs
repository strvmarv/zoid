//! Drives the agent loop with a scripted `ask_user` call and asserts the loop
//! routes it as an Interactive tool: it suspends on a `oneshot`, an inline
//! auto-responder (drains `ui_rx`) answers, and the answer becomes the
//! `ToolResult` fed back to the model. A dropped reply sender (Esc-abort)
//! must still leave a balanced `ToolResult` and end the turn.
//!
//! The auto-responder lives HERE (not in `zoid-testkit`) because
//! `AgentUpdate`/`Answer` are `zoid`-crate types, not `zoid-core`/`zoid-provider`.

use std::sync::Arc;
use tokio::sync::mpsc;

use zoid::agent::{run_agent_turn, AgentUpdate, Answer};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::ProviderEvent;

fn fixed_now() -> i64 {
    0
}

fn seed_events() -> Vec<Event> {
    vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "which db should we use?".into(),
        },
    )]
}

#[tokio::test]
async fn ask_user_answer_becomes_the_tool_result() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call(
            "ask_user",
            serde_json::json!({
                "question": "Which DB?",
                "choices": ["postgres", "sqlite"]
            }),
        ),
        zoid_testkit::text("using it"),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events();
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);

    // Responder: answer the first AskUser with a Choice, ignore other updates.
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                let _ = reply.send(Answer::Choice("postgres".into()));
            }
        }
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    let results = zoid_testkit::tool_results(&events);
    assert!(
        results
            .iter()
            .any(|(n, out, err)| n == "ask_user" && !err && out == "postgres"),
        "expected a non-error ask_user ToolResult with output \"postgres\", got: {results:?}"
    );
}

#[tokio::test]
async fn ask_user_dropped_sender_aborts_turn_with_balanced_result() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("ask_user", serde_json::json!({ "question": "stop?" })),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events();
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);

    // Responder DROPS the reply sender (models Esc-abort).
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                drop(reply);
            }
        }
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    // The turn ended, and the pending ask_user call has a balanced
    // "[user aborted]" result — no dangling ToolCall with no result.
    let results = zoid_testkit::tool_results(&events);
    assert!(
        results
            .iter()
            .any(|(n, out, _)| n == "ask_user" && out == "[user aborted]"),
        "expected a balanced [user aborted] ask_user ToolResult, got: {results:?}"
    );
}

#[tokio::test]
async fn abort_drains_remaining_batched_tool_calls() {
    // A single model turn batches TWO tool calls before Done, with ask_user
    // first: [ask_user, read_file]. Both ToolCalls land in one `pending`
    // batch. Aborting ask_user must not leave read_file's ToolCall dangling.
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("ask_user", serde_json::json!({ "question": "stop?" })),
        zoid_testkit::tool_call("read_file", serde_json::json!({ "path": "whatever.txt" })),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events();
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);

    // Responder DROPS the reply sender (models Esc-abort).
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                drop(reply);
            }
        }
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    let results = zoid_testkit::tool_results(&events);

    assert!(
        results
            .iter()
            .any(|(n, out, _)| n == "ask_user" && out == "[user aborted]"),
        "expected a balanced [user aborted] ask_user ToolResult, got: {results:?}"
    );
    assert!(
        results
            .iter()
            .any(|(n, out, err)| n == "read_file" && !err && out == "[skipped: turn aborted]"),
        "expected read_file to be drained with a balanced [skipped: turn aborted] result, got: {results:?}"
    );

    let tool_call_count = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ToolCall { .. }))
        .count();
    let tool_result_count = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ToolResult { .. }))
        .count();
    assert_eq!(
        tool_call_count, tool_result_count,
        "every ToolCall must have a matching ToolResult (no dangling calls)"
    );
}
