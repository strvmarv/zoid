//! Integration: the inline question card contract. `ask_user` and
//! `apply_mode_mapping` are `Interactive` tools the agent loop intercepts by
//! name: it emits `QuestionAsked` (kind = `Ask` / `ModeMapping`) the instant the
//! tool call arrives, suspends on a `oneshot` for the UI's answer, then emits
//! `QuestionAnswered` + a balanced `ToolResult`. Dropping the reply sender
//! (Esc-abort) yields `answer: "[user aborted]"` and an error `ToolResult`.
//!
//! These tests assert the event-log shape (the new contract from this feature);
//! `ask_user.rs` already covers the tool-result routing, and
//! `mode_import_wiring.rs` covers the bin-side materialize-on-Approve flow.

use std::sync::Arc;
use tokio::sync::mpsc;

use zoid::agent::{run_agent_turn, AgentUpdate, Answer};
use zoid::mode_wizard::{ApplyModeMappingTool, ModeImportWizard, ProposeModeMappingTool};
use zoid_core::event::{Event, EventKind, QuestionKind};
use zoid_core::session::SessionHandle;
use zoid_core::wizard::{ScannedFile, UpstreamScan};
use zoid_provider::ProviderEvent;

fn fixed_now() -> i64 {
    0
}

fn seed_events(text: &str) -> Vec<Event> {
    vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: text.into() },
    )]
}

fn scan() -> UpstreamScan {
    UpstreamScan {
        url: "u".into(),
        repo: "o/r".into(),
        resolved_ref: "abc".into(),
        subtree_path: "skills".into(),
        files: vec![ScannedFile {
            upstream_path: "skills/using/SKILL.md".into(),
            sha: "sha-u".into(),
            content: "---\nname: using\ndescription: d\n---\nLOADER\n".into(),
        }],
    }
}

#[tokio::test]
async fn ask_user_emits_question_asked_then_answered() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call(
            "ask_user",
            serde_json::json!({
                "question": "Skip?",
                "choices": ["Skip", "Continue"]
            }),
        ),
        zoid_testkit::text("ok"),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events("hi");
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                let _ = reply.send(Answer::Choice("Skip".into()));
            }
        }
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    let asked = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAsked { kind, question, .. } => {
                Some((kind.clone(), question.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        asked
            .iter()
            .any(|(k, q)| matches!(k, QuestionKind::Ask) && q == "Skip?"),
        "expected QuestionAsked {{ kind: Ask, question: \"Skip?\" }}, got: {asked:?}"
    );

    let answered = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAnswered { answer, .. } => Some(answer.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        answered.iter().any(|a| a == "Skip"),
        "expected QuestionAnswered {{ answer: \"Skip\" }}, got: {answered:?}"
    );

    let results = zoid_testkit::tool_results(events.iter());
    assert!(
        results
            .iter()
            .any(|(n, out, err)| n == "ask_user" && !err && out == "Skip"),
        "expected a non-error ask_user ToolResult with output \"Skip\", got: {results:?}"
    );
}

#[tokio::test]
async fn apply_mode_mapping_emits_question_asked_with_mapping() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call(
            "apply_mode_mapping",
            serde_json::json!({
                "mode_name": "TestMode",
                "mode_description": "test",
                "mode_body": "LOADER",
                "entries": [
                    { "Materialize": { "canonical_path": "mode.md", "source": "skills/using/SKILL.md", "summary": "loader" } }
                ]
            }),
        ),
        zoid_testkit::text("ok"),
        ProviderEvent::Done,
    ]);

    let wiz = Arc::new(ModeImportWizard::new_import(scan()));
    let mut tools = zoid::invoke_skill::chat_tools(
        Arc::new(zoid_core::skill::SkillRegistry::builtin()),
        zoid_tools::KillSlot::new(),
    );
    tools.push(Box::new(ProposeModeMappingTool::new(wiz.clone())));
    tools.push(Box::new(ApplyModeMappingTool::new(wiz.clone())));
    let tools = Arc::new(tools);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events("import");
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);
    let responder = tokio::spawn(async move {
        while let Some(u) = ui_rx.recv().await {
            if let AgentUpdate::AskUser { reply, .. } = u {
                let _ = reply.send(Answer::Choice("Approve".into()));
            }
        }
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    let asked = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAsked { kind, .. } => Some(kind.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        asked
            .iter()
            .any(|k| matches!(k, QuestionKind::ModeMapping { .. })),
        "expected QuestionAsked {{ kind: ModeMapping {{ .. }} }}, got: {asked:?}"
    );

    let answered = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAnswered { answer, .. } => Some(answer.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        answered.iter().any(|a| a == "Approve"),
        "expected QuestionAnswered {{ answer: \"Approve\" }}, got: {answered:?}"
    );

    let results = zoid_testkit::tool_results(events.iter());
    assert!(
        results
            .iter()
            .any(|(n, out, err)| n == "apply_mode_mapping" && !err && out.contains("Approved and materialized")),
        "expected a non-error apply_mode_mapping ToolResult mentioning materialization, got: {results:?}"
    );
}

#[tokio::test]
async fn cancel_path_emits_cancelled_answer() {
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("ask_user", serde_json::json!({ "question": "stop?" })),
        ProviderEvent::Done,
    ]);

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = seed_events("hi");
    session.append(seed[0].clone()).await.unwrap();

    let (ui_tx, mut ui_rx) = mpsc::channel::<AgentUpdate>(64);
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
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        ui_tx,
        ulid::Ulid::new(),
        zoid_companion::CompanionHub::new(),
        fixed_now,
    )
    .await
    .unwrap();
    responder.abort();

    let answered = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QuestionAnswered { answer, .. } => Some(answer.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        answered.iter().any(|a| a == "[user aborted]"),
        "expected QuestionAnswered {{ answer: \"[user aborted]\" }}, got: {answered:?}"
    );

    let results = zoid_testkit::tool_results(events.iter());
    assert!(
        results
            .iter()
            .any(|(n, out, err)| n == "ask_user" && *err && out == "[user aborted]"),
        "expected an error ask_user ToolResult with output \"[user aborted]\", got: {results:?}"
    );
}
