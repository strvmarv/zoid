//! Drives the terminal-free agent loop against a deterministic multi-turn fake
//! provider and the real tool registry, asserting the persisted event log.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Provider, ProviderEvent, ToolCall};

/// A provider that replays one scripted stream per `stream()` call, in order.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(&self, _req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> anyhow::Result<()> {
        let script = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| vec![ProviderEvent::Done]);
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
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(ulid::Ulid::new(), None, 0, EventKind::UserMessage { text: "write hi".into() })];
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

    run_agent_turn(provider, tools, session.clone(), seed, "fake".into(), tx, fixed_now)
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
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ToolCall { name, .. } if name == "write_file")));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ToolResult { is_error: false, .. })));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::ModelDelta { text } if text == "done")));
}
