//! The subagent runtime (spec §4.4/§7). Builds a subagent's constructed context
//! (task + relevant code, NEVER session history) and runs it in isolation. The
//! orchestrator (the Chat loop) dispatches one at a time.

use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use zoid_core::agent_profile::AgentProfile;
use zoid_core::assembler::{assemble_context, ContextPolicy};
use zoid_core::context::{context_window, file_contents, ItemKind};
use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider};
use zoid_tools::Tool;

use crate::agent::{run_agent_turn_cancellable, tool_specs, AgentUpdate, TurnConfig, WARN_GLYPH};

/// Per-subagent max output tokens (mirrors the Chat loop's budget).
const SUBAGENT_MAX_TOKENS: u32 = 4096;

/// Hard cap on a subagent's tool-call iterations. 100 covers a realistic
/// multi-file read-edit-test-debug cycle with several retries; beyond that
/// the subagent is almost certainly stuck in a loop.
const SUBAGENT_MAX_ITERATIONS: u32 = 100;

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
    events: &crate::eventlog::EventLog,
    policy: &ContextPolicy,
    profile: &AgentProfile,
    model: &str,
    tools: &[Box<dyn Tool>],
    thinking: zoid_provider::ThinkingMode,
) -> CompletionRequest {
    let window = context_window(events.iter());
    let selection = assemble_context(&window, policy);
    let contents = file_contents(events.iter());

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
        // The subagent's constructed context request is never streamed — it's
        // only used to extract the seed user message — so its `model_info` is
        // never read. Use the conservative default to avoid a registry lookup
        // (no registry/provider is in scope here).
        model_info: zoid_provider::model::DEFAULT_MODEL_INFO,
        system: Some(profile.system_prompt.clone()),
        messages: vec![Message::user(user)],
        max_tokens: SUBAGENT_MAX_TOKENS,
        tools: tool_specs(tools),
        thinking,
        reassert: None,
    }
}

/// The outcome of a dispatched subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentResult {
    pub id: String,
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
    context_events: &crate::eventlog::EventLog,
    profile: &AgentProfile,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    default_model: String,
    thinking: zoid_provider::ThinkingMode,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
    id: String,
    approval: zoid_core::config::ApprovalConfig,
    cancel: CancellationToken,
    hard: CancellationToken,
    progress: Arc<AtomicI64>,
    reg: std::sync::Arc<zoid_model::Registry>,
    provider_id: String,
) -> Result<SubagentResult> {
    let sub_ulid = id.strip_prefix("sub-").unwrap_or(&id).to_string();
    let branch = BranchId(format!("subagent:{sub_ulid}"));
    let model = profile.model.clone().unwrap_or(default_model);

    // Only the tools this profile allows (fresh registry, filtered by allow-list).
    let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(
        zoid_tools::registry()
            .into_iter()
            .filter(|t| profile.allows(t.name()))
            .filter(|t| t.kind() != zoid_tools::ToolKind::Interactive)
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
        thinking,
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
        policy: subagent_policy(),
        eviction: zoid_core::eviction::EvictionPolicy::disabled(),
        mcp: None,
        embed: None,
        embedder: None,
        thinking,
        approval: approval.clone(),
        kill: zoid_tools::KillSlot::new(),
        max_iterations: Some(SUBAGENT_MAX_ITERATIONS),
        in_flight: None,
        reassert_interval: 0,
        progress: Some(progress.clone()),
        subagent_idle: None,
        subagent_ceiling: None,
        agents: None,
        context_window: 0, // subagents: eviction disabled, hard-ceiling pass skipped
        max_concurrent: 3, // subagents never dispatch; pool size is irrelevant
        reg,
        provider_id,
    };
    // Subagents have no session-scoped companion (the `show` tool is chat-only
    // and is never in the subagent tool registry), so this hub is never
    // published to; it only satisfies the turn-loop's signature.
    let companion_hub = zoid_companion::CompanionHub::new();
    // Subagents are headless — they can't answer a prompt, so the blacklist
    // gate auto-denies dangerous matches instead of prompting.
    let gate: std::sync::Arc<dyn zoid_tools::ToolGate> = if approval.yolo {
        std::sync::Arc::new(zoid_tools::AllowAll)
    } else {
        std::sync::Arc::new(zoid_tools::BlacklistGate::new(
            approval.shell_danger.clone(),
            approval.shell_allow.clone(),
            false, // interactive = false → Gate::Deny, not Gate::Prompt
        ))
    };
    let produced = run_agent_turn_cancellable(
        config,
        provider,
        tools,
        gate,
        session,
        crate::eventlog::EventLog::from_vec(vec![seed]),
        model,
        ui,
        session_id,
        companion_hub,
        now,
        cancel,
        hard,
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
    let (summary, ok) = distill(&branch_events);

    Ok(SubagentResult {
        id,
        branch: branch.0,
        summary,
        ok,
    })
}

/// Structural tool-execution report for a subagent's own branch events.
/// Pure (no I/O); drives `distill`'s `ok` flag + advisory notes.
struct ExecReport {
    tool_call_count: usize,
    /// `ToolCall` ids that never produced a matching `ToolResult`, in
    /// first-seen order, de-duplicated.
    orphan_ids: Vec<String>,
}

/// A `ToolCall` whose id has no matching `ToolResult` is "claimed but never
/// executed". Subagents carry only paired tools (read/write/edit/grep/glob/
/// ls/shell — see `AgentProfile::builtin`), so an orphan is a genuine anomaly.
fn verify_execution(branch_events: &[Event]) -> ExecReport {
    use std::collections::HashSet;
    let mut call_ids: Vec<String> = Vec::new();
    let mut result_ids: HashSet<String> = HashSet::new();
    for e in branch_events {
        match &e.kind {
            EventKind::ToolCall { id, .. } => call_ids.push(id.clone()),
            EventKind::ToolResult { id, .. } => {
                result_ids.insert(id.clone());
            }
            _ => {}
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let orphan_ids = call_ids
        .iter()
        .filter(|id| !result_ids.contains(*id))
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect();
    ExecReport {
        tool_call_count: call_ids.len(),
        orphan_ids,
    }
}

/// Distill a subagent's branch events into a summary + ok flag.
/// - summary = last non-empty assistant text, or a warn-glyph placeholder.
/// - ok = summary doesn't start with warn glyph AND no errored tool results AND no orphan calls.
/// - orphan tool calls (produced but never executed) flip ok to false.
/// - zero tool calls issues an advisory note only (never flips ok).
fn distill(branch_events: &[Event]) -> (String, bool) {
    let mut summary = conversation(branch_events)
        .iter()
        .rev()
        .find_map(|m| match m {
            ChatMsg::Assistant { text, .. } if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("{WARN_GLYPH} subagent produced no output"));

    let has_errors = branch_events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::ToolResult { is_error: true, .. }));

    let report = verify_execution(branch_events);
    let has_orphans = !report.orphan_ids.is_empty();

    let ok = !summary.starts_with(WARN_GLYPH) && !has_errors && !has_orphans;

    if has_errors && !summary.starts_with(WARN_GLYPH) {
        summary = format!("{summary}\n\n{WARN_GLYPH} one or more tool calls errored");
    }
    if has_orphans {
        summary = format!(
            "{summary}\n\n{WARN_GLYPH} {} tool call(s) produced no result: {}",
            report.orphan_ids.len(),
            report.orphan_ids.join(", ")
        );
    }
    if report.tool_call_count == 0 {
        summary = format!(
            "{summary}\n\nnote: subagent emitted no tool calls — if this task \
             required file or shell changes, its results are unverified"
        );
    }
    (summary, ok)
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
    // Helper: an assistant text event (canonical assistant-text variant).
    fn assistant(text: &str) -> Event {
        ev(EventKind::AssistantMessage { text: text.into() })
    }

    #[test]
    fn request_carries_task_and_relevant_file_never_history() {
        let evs = crate::eventlog::EventLog::from_vec(vec![
            ev(EventKind::UserMessage {
                text: "secret chat history".into(),
            }),
            call("c1", "src/ast.rs"),
            result("c1", "fn parse() {}"),
        ]);
        let profile = AgentProfile::builtin();
        let tools = zoid_tools::registry();
        let req = build_subagent_request(
            "refactor parse()",
            &evs,
            &subagent_policy(),
            &profile,
            "glm",
            &tools,
            zoid_provider::ThinkingMode::Off,
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
    fn assembled_tools_exclude_interactive_ask_user() {
        // Mirrors the exact filter chain `run_subagent` uses to build its tool
        // set: a headless subagent cannot answer an `ask_user` prompt and
        // would hang forever awaiting a reply, so Interactive tools are
        // dropped before the request is ever built.
        let profile = AgentProfile::builtin();
        let tools: Vec<Box<dyn Tool>> = zoid_tools::registry()
            .into_iter()
            .filter(|t| profile.allows(t.name()))
            .filter(|t| t.kind() != zoid_tools::ToolKind::Interactive)
            .collect();
        assert!(
            !tools.iter().any(|t| t.name() == "ask_user"),
            "ask_user must be filtered out of a subagent's tool set"
        );
        assert!(
            !tools.iter().any(|t| t.name() == "submit_feedback"),
            "submit_feedback must be filtered out of a subagent's tool set"
        );

        let req = build_subagent_request(
            "do a thing",
            &crate::eventlog::EventLog::new(),
            &subagent_policy(),
            &profile,
            "glm",
            &tools,
            zoid_provider::ThinkingMode::Off,
        );
        assert!(
            !req.tools.iter().any(|s| s.name == "ask_user"),
            "ask_user must not be advertised to the provider for a subagent"
        );
        assert!(
            !req.tools.iter().any(|s| s.name == "submit_feedback"),
            "submit_feedback must not be advertised to the provider for a subagent"
        );
    }

    #[test]
    fn request_without_files_is_just_the_task() {
        let req = build_subagent_request(
            "do a thing",
            &crate::eventlog::EventLog::new(),
            &subagent_policy(),
            &AgentProfile::builtin(),
            "glm",
            &zoid_tools::registry(),
            zoid_provider::ThinkingMode::Off,
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

    #[test]
    fn distill_empty_summary_is_failure() {
        // No non-empty assistant text → summary has warn glyph, ok = false.
        let evs = vec![ev(EventKind::ToolResult {
            id: "t1".into(),
            name: "read".into(),
            output: "some output".into(),
            is_error: false,
        })];
        let (summary, ok) = distill(&evs);
        assert!(!ok, "empty summary must be failure");
        assert!(
            summary.starts_with(WARN_GLYPH),
            "summary must have warn glyph"
        );
        assert!(
            summary.contains("no output"),
            "summary must explain: {summary}"
        );
    }

    #[test]
    fn distill_errored_tool_is_failure() {
        // A summary exists but a tool result errored → ok = false.
        let evs = vec![
            ev(EventKind::AssistantMessage {
                text: "done".into(),
            }),
            ev(EventKind::ToolResult {
                id: "t1".into(),
                name: "write".into(),
                output: "permission denied".into(),
                is_error: true,
            }),
        ];
        let (summary, ok) = distill(&evs);
        assert!(!ok, "errored tool must be failure");
        assert!(
            summary.contains("errored"),
            "summary must note the error: {summary}"
        );
    }

    #[test]
    fn distill_normal_output_is_success() {
        let evs = vec![ev(EventKind::AssistantMessage {
            text: "refactored successfully".into(),
        })];
        let (summary, ok) = distill(&evs);
        assert!(ok, "normal output must be success");
        assert!(summary.contains("refactored successfully"));
    }

    #[test]
    fn distill_orphan_call_forces_not_ok() {
        let evs = vec![assistant("did the thing"), call("c1", "a.rs")]; // no result
        let (summary, ok) = distill(&evs);
        assert!(!ok, "orphan tool call must force ok=false");
        assert!(
            summary.contains("produced no result") && summary.contains("c1"),
            "summary must name the orphan id: {summary}"
        );
    }

    #[test]
    fn distill_zero_calls_stays_ok_with_advisory() {
        // KEY false-positive-safety test: a legitimate text-only subagent.
        let evs = vec![assistant("here is your summary")];
        let (summary, ok) = distill(&evs);
        assert!(
            ok,
            "zero tool calls must NOT flip ok for a text-only subagent"
        );
        assert!(
            summary.contains("emitted no tool calls"),
            "advisory note must be present: {summary}"
        );
    }

    #[test]
    fn distill_paired_calls_stay_ok() {
        let evs = vec![assistant("done"), call("c1", "a.rs"), result("c1", "ok")];
        let (_summary, ok) = distill(&evs);
        assert!(ok, "a healthy subagent with paired calls stays ok");
    }

    #[test]
    fn distill_errored_result_still_not_ok() {
        // Regression guard: existing behavior preserved.
        let evs = vec![
            assistant("tried"),
            call("c1", "a.rs"),
            ev(EventKind::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "boom".into(),
                is_error: true,
            }),
        ];
        let (summary, ok) = distill(&evs);
        assert!(!ok);
        assert!(
            summary.contains("errored"),
            "keeps existing errored note: {summary}"
        );
    }

    #[test]
    fn verify_execution_flags_orphan_call() {
        let evs = vec![call("c1", "a.rs"), result("c1", "ok"), call("c2", "b.rs")];
        let r = verify_execution(&evs);
        assert_eq!(r.tool_call_count, 2);
        assert_eq!(r.orphan_ids, vec!["c2".to_string()]);
    }

    #[test]
    fn verify_execution_no_orphans_when_all_paired() {
        let evs = vec![call("c1", "a.rs"), result("c1", "ok")];
        let r = verify_execution(&evs);
        assert_eq!(r.tool_call_count, 1);
        assert!(r.orphan_ids.is_empty());
    }

    #[test]
    fn verify_execution_counts_zero_calls() {
        let evs = vec![ev(EventKind::UserMessage { text: "hi".into() })];
        let r = verify_execution(&evs);
        assert_eq!(r.tool_call_count, 0);
        assert!(r.orphan_ids.is_empty());
    }

    #[test]
    fn verify_execution_dedups_orphan_ids() {
        // Same call id emitted twice with no result — reported once.
        let evs = vec![call("c1", "a.rs"), call("c1", "a.rs")];
        let r = verify_execution(&evs);
        assert_eq!(r.tool_call_count, 2);
        assert_eq!(r.orphan_ids, vec!["c1".to_string()]);
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
                cached: 0,
                thinking_tokens: 0,
            }),
            ProviderEvent::Done,
        ]));
        let session = SessionHandle::spawn(":memory:").unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let res = run_subagent(
            "refactor parse()",
            &crate::eventlog::EventLog::new(),
            &AgentProfile::builtin(),
            provider,
            std::path::PathBuf::from("."),
            "glm".into(),
            zoid_provider::ThinkingMode::Off,
            session.clone(),
            Ulid::new(),
            tx,
            || 0,
            "sub-test".into(),
            zoid_core::config::ApprovalConfig::default(),
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
            std::sync::Arc::new(zoid_model::Registry::default()),
            String::new(),
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

    #[test]
    fn distill_orphan_and_errored_compose() {
        let evs = vec![
            assistant("tried"),
            call("c1", "a.rs"),
            ev(EventKind::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "boom".into(),
                is_error: true,
            }),
            call("c2", "b.rs"), // orphan — no matching result
        ];
        let (summary, ok) = distill(&evs);
        assert!(!ok, "errored + orphan must be not-ok");
        assert!(
            summary.contains("errored"),
            "errored note present: {summary}"
        );
        assert!(
            summary.contains("produced no result") && summary.contains("c2"),
            "orphan note names c2: {summary}"
        );
    }

    #[test]
    fn assembled_tools_exclude_emitting() {
        // Mirrors assembled_tools_exclude_interactive_ask_user's construction of
        // the subagent tool set, then asserts no Emitting tools are present:
        // verify_execution's orphan check would false-positive on a tool that
        // emits its own events instead of a paired ToolResult.
        let profile = AgentProfile::builtin();
        let tools: Vec<Box<dyn Tool>> = zoid_tools::registry()
            .into_iter()
            .filter(|t| profile.allows(t.name()))
            .filter(|t| t.kind() != zoid_tools::ToolKind::Interactive)
            .collect();
        for t in &tools {
            assert_ne!(
                t.kind(),
                zoid_tools::ToolKind::Emitting,
                "subagent profile must contain no Emitting tools (would break verify_execution): {}",
                t.name()
            );
        }
    }

    #[test]
    fn subagent_max_iterations_is_100() {
        assert_eq!(
            SUBAGENT_MAX_ITERATIONS, 100,
            "iteration limit must be 100 (was 25, too tight for realistic cycles)"
        );
    }
}
