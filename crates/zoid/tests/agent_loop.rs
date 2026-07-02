//! Drives the terminal-free agent loop against a deterministic multi-turn fake
//! provider and the real tool registry, asserting the persisted event log.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall};

/// A provider that replays one scripted stream per `stream()` call, in order,
/// and records every received `CompletionRequest` for post-run assertions.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
    /// Every request passed to `stream` is cloned here (lock never held across `.await`).
    requests: Mutex<Vec<CompletionRequest>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        // Capture request and pop next script without holding either lock across .await.
        self.requests.lock().unwrap().push(req.clone());
        let script = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![ProviderEvent::Done]);
        for ev in script {
            if sink.send(ev).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

fn fixed_now() -> i64 {
    0
}

struct DenyAll;
impl zoid_tools::ToolGate for DenyAll {
    fn check(&self, _c: &zoid_provider::ToolCall) -> zoid_tools::Gate {
        zoid_tools::Gate::Deny("denied by policy".into())
    }
}

#[tokio::test]
async fn agent_loop_runs_tool_then_finishes() {
    // Arrange a write_file in a tempdir so the tool actually executes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let path_str = path.to_str().unwrap().to_string();

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model calls write_file, then ends its turn.
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "write_file".into(),
                    args: json!({ "path": path_str, "content": "hi" }),
                }),
                ProviderEvent::Done,
            ],
            // Turn 2: with the tool result in context, the model replies in text.
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    // Use a fixed epoch-0 ULID so the seed always sorts before any Ulid::new()
    // generated inside run_agent_turn (which uses the current wall-clock time).
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "write hi".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    // Drain UI updates so the channel never blocks.
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider.clone(),
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
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

    // The tool actually ran.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");

    // The log records: UserMessage, ToolCall, ToolResult, ModelDelta.
    let log = session.snapshot().await.unwrap();
    let kinds: Vec<&EventKind> = log.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], EventKind::UserMessage { .. }));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, EventKind::ToolCall { name, .. } if name == "write_file")));
    assert!(kinds.iter().any(|k| matches!(
        k,
        EventKind::ToolResult {
            is_error: false,
            ..
        }
    )));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, EventKind::ModelDelta { text } if text == "done")));

    // The second request must include a Tool message, proving the tool result was fed back.
    let captured = provider.requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "expected exactly 2 provider requests (tool-call turn + follow-up)"
    );
    assert!(
        captured[1].messages.iter().any(|m| m.role == MsgRole::Tool),
        "second request must contain a MsgRole::Tool message (tool result fed back to model)"
    );
}

#[tokio::test]
async fn gate_deny_blocks_tool_and_feeds_reason_back() {
    // Arrange a write_file in a tempdir so we can assert it was NOT written.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let path_str = path.to_str().unwrap().to_string();

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model calls write_file, then ends its turn.
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "write_file".into(),
                    args: json!({ "path": path_str, "content": "hi" }),
                }),
                ProviderEvent::Done,
            ],
            // Turn 2: with the (denied) tool result in context, the model replies in text.
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "write hi".into(),
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
        Arc::new(DenyAll),
        session.clone(),
        seed,
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await
    .unwrap();

    let _ = drain.await;

    assert!(
        !path.exists(),
        "denied write_file must not touch the filesystem"
    );
    let denied = events.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ToolResult { output, is_error, .. }
            if *is_error && output.contains("denied by policy"))
    });
    assert!(denied, "a Deny must surface as an error ToolResult");
}

#[tokio::test]
async fn agent_loop_returns_ok_and_emits_turn_complete_on_error_event() {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Provider sends an Error event instead of a normal response.
            vec![ProviderEvent::Error("boom".into())],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::new(),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
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

    // Must return Ok (with the accumulated events) even though the provider sent an Error event.
    let result = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        seed,
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        fixed_now,
    )
    .await;
    assert!(
        result.is_ok(),
        "run_agent_turn must return Ok(events) on a provider Error event"
    );

    let complete = drain.await.unwrap();
    assert!(
        complete,
        "TurnComplete must be emitted even on the error path"
    );

    // The event log must contain an AssistantMessage whose text includes the error string.
    let log = session.snapshot().await.unwrap();
    assert!(
        log.iter().any(
            |e| matches!(&e.kind, EventKind::AssistantMessage { text } if text.contains("boom"))
        ),
        "error text must be logged as an AssistantMessage"
    );
}
