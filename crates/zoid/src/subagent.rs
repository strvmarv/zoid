//! The subagent runtime (spec §4.4/§7). Builds a subagent's constructed context
//! (task + relevant code, NEVER session history) and runs it in isolation. The
//! orchestrator (the Chat loop) dispatches one at a time.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::agent_profile::AgentProfile;
use zoid_core::assembler::{assemble_context, ContextPolicy};
use zoid_core::context::{context_window, file_contents, ItemKind};
use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider};
use zoid_tools::Tool;

use crate::agent::{run_agent_turn, tool_specs, AgentUpdate, TurnConfig, WARN_GLYPH};

/// Per-subagent max output tokens (mirrors the Chat loop's budget).
const SUBAGENT_MAX_TOKENS: u32 = 4096;

/// Token ceiling for a subagent's constructed context (≈ half a 64k window,
/// leaving room for the task, tool round-trips, and output).
const SUBAGENT_CONTEXT_CEILING: u64 = 32_000;

/// Default context budget for a dispatched subagent: drop cold items and cap the
/// constructed context so it stays a *precise* slice, not a dump.
pub fn subagent_policy() -> ContextPolicy {
    ContextPolicy {
        token_ceiling: Some(SUBAGENT_CONTEXT_CEILING),
        auto_evict_cold: true,
        compact_threshold: None,
    }
}

/// Build a subagent `CompletionRequest`: the P3 assembler selects the relevant
/// context items from `events`; we keep the included **File** items, resolve
/// their content, and compose a task-focused prompt. Session messages/tool
/// transcripts are intentionally excluded (spec §4.4/§5.4: never session history).
pub fn build_subagent_request(
    task: &str,
    events: &[Event],
    policy: &ContextPolicy,
    profile: &AgentProfile,
    model: &str,
    tools: &[Box<dyn Tool>],
) -> CompletionRequest {
    let window = context_window(events);
    let selection = assemble_context(&window, policy);
    let contents = file_contents(events);

    let mut ctx = String::new();
    for item in selection
        .included
        .iter()
        .filter(|i| i.kind == ItemKind::File)
    {
        if let Some(c) = contents.get(&item.key) {
            ctx.push_str(&format!("\n// {}\n{}\n", item.label, c));
        }
    }

    let user = if ctx.is_empty() {
        format!("Task:\n{task}")
    } else {
        format!("Task:\n{task}\n\nRelevant files:\n{ctx}")
    };

    CompletionRequest {
        model: model.to_string(),
        system: Some(profile.system_prompt.clone()),
        messages: vec![Message::user(user)],
        max_tokens: SUBAGENT_MAX_TOKENS,
        tools: tool_specs(tools),
    }
}

/// The outcome of a dispatched subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}

/// Run `task` as an isolated subagent: build its constructed context (B3), seed
/// it as the first user message on a fresh `subagent:<id>` branch, run the
/// generalized agent loop in `cwd` under `profile`, and distill a
/// `SubagentResult`. Sequential — the caller dispatches one at a time.
///
/// `session_id` tags every event this subagent emits (Plan 2's single global
/// DB is partitioned by session_id; untagged subagent events would get the nil
/// session and Task E1's spend attribution would break).
// 9 params thread the full dispatch context (task, constructed context,
// profile, provider, cwd, model, session + its id, ui channel, clock); a
// params struct would add indirection without adding clarity.
#[allow(clippy::too_many_arguments)]
pub async fn run_subagent(
    task: &str,
    context_events: &[Event],
    profile: &AgentProfile,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    default_model: String,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<SubagentResult> {
    let branch = BranchId(format!("subagent:{}", Ulid::new()));
    let model = profile.model.clone().unwrap_or(default_model);

    // Only the tools this profile allows (fresh registry, filtered by allow-list).
    let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(
        zoid_tools::registry()
            .into_iter()
            .filter(|t| profile.allows(t.name()))
            .collect(),
    );

    // The constructed prompt (task + relevant code) becomes the seed user turn.
    let req = build_subagent_request(
        task,
        context_events,
        &subagent_policy(),
        profile,
        &model,
        &tools,
    );
    let prompt = req.messages[0].content.clone();
    let mut seed = Event::new(
        Ulid::new(),
        None,
        now(),
        EventKind::UserMessage { text: prompt },
    )
    .with_session(session_id);
    seed.branch = branch.clone();
    session.append(seed.clone()).await?;

    let config = TurnConfig {
        system: profile.system_prompt.clone(),
        cwd,
        branch: branch.clone(),
    };
    let produced = run_agent_turn(
        config,
        provider,
        tools,
        std::sync::Arc::new(zoid_tools::AllowAll),
        session,
        vec![seed],
        model,
        ui,
        session_id,
        now,
    )
    .await?;

    // Distill: last non-empty assistant text = summary; an emitted ⚠ = not ok.
    // `conversation()` is branch-aware (Task D1) and skips non-main events, but
    // this subagent's own turn lives entirely on `branch` — rebase a filtered
    // copy onto the default branch so the fold sees it.
    let mut branch_events: Vec<Event> = produced
        .iter()
        .filter(|e| e.branch == branch)
        .cloned()
        .collect();
    for e in &mut branch_events {
        e.branch = BranchId::default();
    }
    let summary = conversation(&branch_events)
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let ok = !summary.starts_with(WARN_GLYPH);

    Ok(SubagentResult {
        branch: branch.0,
        summary,
        ok,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;
    use zoid_core::agent_profile::AgentProfile;
    use zoid_core::event::{Event, EventKind};

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }
    fn call(id: &str, path: &str) -> Event {
        ev(EventKind::ToolCall {
            id: id.into(),
            name: "read_file".into(),
            args: format!(r#"{{"path":"{path}"}}"#),
        })
    }
    fn result(id: &str, out: &str) -> Event {
        ev(EventKind::ToolResult {
            id: id.into(),
            name: "read_file".into(),
            output: out.into(),
            is_error: false,
        })
    }

    #[test]
    fn request_carries_task_and_relevant_file_never_history() {
        let evs = vec![
            ev(EventKind::UserMessage {
                text: "secret chat history".into(),
            }),
            call("c1", "src/ast.rs"),
            result("c1", "fn parse() {}"),
        ];
        let profile = AgentProfile::builtin();
        let tools = zoid_tools::registry();
        let req = build_subagent_request(
            "refactor parse()",
            &evs,
            &subagent_policy(),
            &profile,
            "glm",
            &tools,
        );

        assert_eq!(req.model, "glm");
        assert_eq!(req.system.as_deref(), Some(profile.system_prompt.as_str()));
        assert_eq!(
            req.messages.len(),
            1,
            "subagent gets ONE constructed user message"
        );
        let body = &req.messages[0].content;
        assert!(body.contains("refactor parse()"), "task present");
        assert!(
            body.contains("fn parse() {}"),
            "relevant file content present"
        );
        assert!(body.contains("src/ast.rs"), "file labeled by path");
        // THE SUPERPOWERS INVARIANT: never the session transcript.
        assert!(
            !body.contains("secret chat history"),
            "session history excluded (spec §4.4/§5.4)"
        );
        assert!(!req.tools.is_empty(), "tools advertised");
    }

    #[test]
    fn request_without_files_is_just_the_task() {
        let req = build_subagent_request(
            "do a thing",
            &[],
            &subagent_policy(),
            &AgentProfile::builtin(),
            "glm",
            &zoid_tools::registry(),
        );
        assert!(req.messages[0].content.contains("do a thing"));
    }

    #[test]
    fn subagent_policy_is_bounded_and_evicts_cold() {
        let p = subagent_policy();
        assert!(
            p.auto_evict_cold,
            "cold items dropped from a subagent's context"
        );
        assert!(
            p.token_ceiling.is_some(),
            "subagent context is token-bounded"
        );
    }

    #[tokio::test]
    async fn subagent_runs_constructed_task_and_returns_summary() {
        use std::sync::Arc;
        use tokio::sync::mpsc;
        use zoid_core::session::SessionHandle;
        use zoid_provider::{FakeProvider, ProviderEvent, Usage};

        let provider = Arc::new(FakeProvider::new(vec![
            ProviderEvent::TextDelta("Refactored parse() into two functions.".into()),
            ProviderEvent::Usage(Usage {
                input_tokens: 200,
                output_tokens: 30,
            }),
            ProviderEvent::Done,
        ]));
        let session = SessionHandle::spawn(":memory:").unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let res = run_subagent(
            "refactor parse()",
            &[],
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

        assert!(res.ok, "no error emitted → ok");
        assert!(
            res.summary.contains("Refactored parse()"),
            "summary = subagent's final text"
        );
        assert!(res.branch.starts_with("subagent:"));
        // The subagent's work is persisted on ITS OWN branch.
        let snap = session.snapshot().await.unwrap();
        assert!(snap.iter().any(|e| e.branch.0 == res.branch));
    }
}
