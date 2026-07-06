//! Wiring proof: the agent loop intercepts apply_mode_mapping (Approving),
//! raises ModeMappingApproval, and on "Approve" emits a non-error ToolResult.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid::mode_wizard::{ApplyModeMappingTool, ModeImportWizard, ProposeModeMappingTool};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::wizard::{ScannedFile, UpstreamScan};
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

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

fn scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "abc".into(),
        subtree_path: "skills".into(),
        files: vec![ScannedFile {
            upstream_path: "skills/a/SKILL.md".into(),
            sha: "sha-a".into(),
            content: "---\nname: a\ndescription: d\n---\nBODY\n".into(),
        }],
    }
}

#[tokio::test]
async fn apply_mode_mapping_raises_approval_and_approve_emits_tool_result() {
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call(
                    "apply_mode_mapping",
                    serde_json::json!({
                        "mode_name": "M",
                        "mode_description": "d",
                        "mode_body": "",
                        "entries": [{
                            "Materialize": {
                                "canonical_path": "a/SKILL.md",
                                "source": "skills/a/SKILL.md",
                                "summary": "a"
                            }
                        }]
                    }),
                ),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("done"), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let wiz = Arc::new(ModeImportWizard::new_import(scan()));
    let mut tools =
        zoid::invoke_skill::chat_tools(Arc::new(zoid_core::skill::SkillRegistry::builtin()));
    tools.push(Box::new(ProposeModeMappingTool::new(wiz.clone())));
    tools.push(Box::new(ApplyModeMappingTool::new(wiz.clone())));
    let tools = Arc::new(tools);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "import the mode".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let approvals = Arc::new(Mutex::new(Vec::<zoid_core::wizard::ModeMapping>::new()));
    let approvals_for_task = approvals.clone();
    let handle = tokio::spawn(async move {
        while let Some(upd) = rx.recv().await {
            if let AgentUpdate::ModeMappingApproval { mapping, reply, .. } = upd {
                approvals_for_task.lock().unwrap().push(mapping);
                let _ = reply.send("Approve".to_string());
            }
        }
    });

    run_agent_turn(
        zoid::agent::chat_turn_config_with(&zoid::agent::default_profile(), ""),
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    handle.await.unwrap();

    let captured_len = {
        let captured = approvals.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].mode_name, "M");
        captured.len()
    };
    let _ = captured_len;

    let log = session.snapshot().await.unwrap();
    let result = log.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult { name, is_error, .. } if name == "apply_mode_mapping" => {
            Some(*is_error)
        }
        _ => None,
    });
    assert_eq!(result, Some(false), "approve => non-error tool result");
}
