//! Integration: a scripted provider calls propose_mode_mapping then
//! apply_mode_mapping; the loop raises AskUser (Interactive, unified with
//! ask_user); the test simulates the bin's wizard bridge — runs the
//! materializer on "Approve", then sends `Answer::Choice` down the reply
//! channel; the canonical files land in a temp user-global dir; the mode
//! loads as Ready. No real fetch (scan injected via the wizard state); no
//! real model (scripted tool calls).

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use zoid::agent::{run_agent_turn, AgentUpdate, Answer};
use zoid::mode_wizard::{ApplyModeMappingTool, ModeImportWizard, ProposeModeMappingTool};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_core::wizard::{ScannedFile, UpstreamScan};
use zoid_provider::{CompletionRequest, Provider, ProviderEvent};

struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
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
        files: vec![
            ScannedFile {
                upstream_path: "skills/using/SKILL.md".into(),
                sha: "sha-u".into(),
                content: "---\nname: using\ndescription: d\n---\nLOADER\n".into(),
            },
            ScannedFile {
                upstream_path: "skills/brain/SKILL.md".into(),
                sha: "sha-b".into(),
                content: "---\nname: brain\ndescription: d\n---\nBODY\n".into(),
            },
        ],
    }
}

#[tokio::test]
async fn import_wizard_approve_materializes_and_loads() {
    let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                zoid_testkit::tool_call(
                    "apply_mode_mapping",
                    serde_json::json!({
                        "mode_name": "TestMode",
                        "mode_description": "test",
                        "mode_body": "LOADER",
                        "entries": [
                            { "Materialize": { "canonical_path": "mode.md", "source": "skills/using/SKILL.md", "summary": "loader" } },
                            { "Materialize": { "canonical_path": "brain/SKILL.md", "source": "skills/brain/SKILL.md", "summary": "brain" } }
                        ]
                    }),
                ),
                ProviderEvent::Done,
            ],
            vec![zoid_testkit::text("ok"), ProviderEvent::Done],
        ])),
    });

    let wiz = Arc::new(ModeImportWizard::new_import(scan()));
    let mut tools = zoid::invoke_skill::chat_tools(
        Arc::new(zoid_core::skill::SkillRegistry::builtin()),
        zoid_tools::KillSlot::new(),
    );
    tools.push(Box::new(ProposeModeMappingTool::new(wiz.clone())));
    tools.push(Box::new(ApplyModeMappingTool::new(wiz.clone())));
    let tools = Arc::new(tools);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "import".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("testmode");
    let dest_for_task = dest.clone();
    let scan_for_task = wiz.scan.clone();
    // The scripted tool call proposes this mapping; the bin would read it from
    // the event log's latest `QuestionAsked` (kind = ModeMapping). Reconstruct
    // it here from the same args the scripted provider sends.
    let mapping_args = serde_json::json!({
        "mode_name": "TestMode",
        "mode_description": "test",
        "mode_body": "LOADER",
        "entries": [
            { "Materialize": { "canonical_path": "mode.md", "source": "skills/using/SKILL.md", "summary": "loader" } },
            { "Materialize": { "canonical_path": "brain/SKILL.md", "source": "skills/brain/SKILL.md", "summary": "brain" } }
        ]
    });
    let mapping_for_task = zoid::mode_wizard::parse_mapping_args(&mapping_args).unwrap();
    let handle = tokio::spawn(async move {
        while let Some(upd) = rx.recv().await {
            if let AgentUpdate::AskUser {
                question,
                choices,
                reply,
            } = upd
            {
                // Simulate the bin's wizard bridge: a ModeMapping question's
                // detail names the proposed mode. On "Approve", materialize
                // then send `Answer::Choice("Approve")`; the choices for the
                // wizard are ["Approve", "Reject", "Adjust"].
                let _ = (question, choices);
                let res = zoid::mode_wizard::materialize(
                    &mapping_for_task,
                    &scan_for_task,
                    &dest_for_task,
                    "2026-07-05T12:00:00Z",
                );
                let _ = reply.send(if res.is_ok() {
                    Answer::Choice("Approve".into())
                } else {
                    Answer::Choice("Reject".into())
                });
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

    assert!(dest.join("mode.md").is_file());
    assert!(dest.join("brain/SKILL.md").is_file());
    assert!(dest.join(".zoid-provenance.json").is_file());

    let reg = zoid::mode_import::build_mode_registry(
        &zoid::agent::default_profile(),
        &[tmp.path().to_path_buf()],
    );
    let m = reg
        .modes()
        .iter()
        .find(|m| m.name() == "TestMode")
        .expect("TestMode loaded");
    assert!(!m.is_broken());
}
