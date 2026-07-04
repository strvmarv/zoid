//! Wiring proof for the mode/skill runtime spike: a scripted `invoke_skill` call
//! must have its skill body recorded as a non-error ToolResult AND fed back into
//! the next provider request as a Tool message. Deterministic — no real model.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::run_agent_turn;
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::skill::SkillRegistry;
use zoid_provider::{CompletionRequest, MsgRole, Provider, ProviderEvent};

/// Replays one scripted stream per `stream()` call and captures every request.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
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

#[tokio::test]
async fn invoke_skill_body_flows_back_into_the_loop() {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model loads the spike-plan skill, then ends its stream.
            vec![
                zoid_testkit::tool_call("invoke_skill", serde_json::json!({ "name": "spike-plan" })),
                ProviderEvent::Done,
            ],
            // Turn 2: with the skill body in context, the model replies in text.
            vec![zoid_testkit::text("planned"), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let skills = Arc::new(SkillRegistry::builtin());
    let tools = Arc::new(zoid::invoke_skill::chat_tools(skills.clone()));

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "plan and implement the spike task".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    run_agent_turn(
        zoid::agent::chat_turn_config_with(&zoid::agent::default_profile(), &skills.menu()),
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
    drain.await.unwrap();

    // 1) The loop recorded a non-error ToolResult for invoke_skill carrying the body.
    let log = session.snapshot().await.unwrap();
    let body_result = log.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult {
            name,
            output,
            is_error,
            ..
        } if name == "invoke_skill" => Some((output.clone(), *is_error)),
        _ => None,
    });
    let (output, is_error) = body_result.expect("expected an invoke_skill ToolResult");
    assert!(!is_error, "invoke_skill should succeed for a known skill");
    assert!(
        output.contains("spike-implement"),
        "the returned body must be spike-plan's (which chains to spike-implement)"
    );

    // 2) The skill body was fed back into the second provider request as a Tool message.
    let captured = provider.requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected a tool-call turn + a follow-up turn");
    assert!(
        captured[1]
            .messages
            .iter()
            .any(|m| m.role == MsgRole::Tool && m.content.contains("spike-implement")),
        "second request must carry the skill body back as a Tool message"
    );
}
