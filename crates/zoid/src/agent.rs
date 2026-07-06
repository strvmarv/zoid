//! The terminal-free agent loop: stream a turn, execute any tool calls in the
//! working directory, record everything as events, and re-request until the
//! model stops calling tools (or the iteration cap trips).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use zoid_core::agent_profile::AgentProfile;
use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider, ProviderEvent, ToolCall, ToolSpec};
use zoid_tools::{Gate, Tool, ToolGate};

/// Warning glyph used in agent-generated error messages; avoids a TUI-layer dep.
/// `pub(crate)` so `subagent.rs` can detect a failed subagent from the same
/// source of truth rather than a drifting sentinel of its own.
pub(crate) const WARN_GLYPH: char = '⚠';

/// System prompt for Chat-mode turns.
pub const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     You can call tools to read, write, edit, and search files and run shell \
     commands in the user's working directory. Use them when helpful.";

/// The default Chat mode profile: the standard zoid system prompt with an
/// unrestricted tool set (empty allow-list = every tool permitted, per
/// `AgentProfile::allows`). The base profile for the `Chat` floor of the mode
/// registry; reproduces pre-mode behavior exactly.
pub fn default_profile() -> AgentProfile {
    AgentProfile {
        name: "Chat".into(),
        description: "General terminal coding assistant.".into(),
        system_prompt: SYSTEM_PROMPT.to_string(),
        tools: vec![], // empty = every tool permitted
        model: None,
    }
}

/// How one agent turn is run: its system prompt, working directory, and the
/// event branch its output is recorded on. Chat uses the main branch + process
/// cwd; a subagent uses its own branch + (optionally) a worktree.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
    /// Context-management policy for this turn. Chat gets it from `[economy]`;
    /// subagents get `subagent_policy()`. Drives automatic tool-result compaction.
    pub policy: zoid_core::assembler::ContextPolicy,
    /// Live eviction band parameters. `disabled()` for subagents/tests.
    pub eviction: zoid_core::eviction::EvictionPolicy,
}

/// The orchestrator (Chat) turn config for an explicit mode profile + skill menu.
/// `system` is the profile's prompt; when `skill_menu` is non-empty it is
/// appended under a header so the model knows what it can `invoke_skill`.
pub fn chat_turn_config_with(profile: &AgentProfile, skill_menu: &str) -> TurnConfig {
    let system = if skill_menu.is_empty() {
        profile.system_prompt.clone()
    } else {
        format!(
            "{}\n\n## Available skills — call invoke_skill(name):\n{}",
            profile.system_prompt, skill_menu
        )
    };
    TurnConfig {
        system,
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
        policy: zoid_core::assembler::ContextPolicy::default(),
        eviction: zoid_core::eviction::EvictionPolicy::disabled(),
    }
}

/// The default Chat turn config: the `default_profile()` with no skill menu.
/// Kept zero-arg for the many callers (tests) that don't exercise modes;
/// byte-identical to the pre-mode behavior.
pub fn chat_turn_config() -> TurnConfig {
    chat_turn_config_with(&default_profile(), "")
}

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 50;
/// Bound on the capacity-error retry (Task 1.7): the hard-bound backstop when
/// the pre-flight estimate under-reads reality and the provider still rejects
/// the request as too large. Each retry forces an eviction wave before
/// re-sending, so this also bounds the number of forced eviction waves per turn.
pub const MAX_CONTEXT_RETRIES: u32 = 3;

/// The user's answer to an `ask_user` prompt.
pub enum Answer {
    Choice(String),
    FreeText(String),
    /// The user chose to let the agent decide (a positive choice, not a cancel).
    LetYouDecide,
}

/// UI-facing updates emitted as the turn progresses.
pub enum AgentUpdate {
    /// A new event was persisted; the UI should cache it and redraw.
    Appended(Box<Event>),
    /// A Local tool is about to run; the UI shows an in-flight spinner until the
    /// matching `ToolResult` is appended (or the turn completes).
    ToolStarted { name: String },
    /// The model asked the user a question; the loop parks until `reply`
    /// resolves. Dropping `reply` (Esc) aborts the turn.
    AskUser {
        question: String,
        choices: Vec<String>,
        reply: oneshot::Sender<Answer>,
    },
    /// The turn is finished (model produced no further tool calls / cap / error).
    TurnComplete,
    /// Live model list fetched for the config model picker, tagged with the
    /// provider id it was fetched for so a stale (superseded) fetch can be
    /// dropped instead of clobbering a newer provider's picker.
    ModelsFetched {
        provider: String,
        models: Vec<String>,
    },
    /// Dynamically-fetched model capabilities (context window, prompt cache,
    /// etc.) from the provider's introspection endpoint. Tagged with the model
    /// id so a stale fetch for a superseded model is dropped.
    ModelInfoFetched {
        model: String,
        info: zoid_provider::model::ModelInfo,
    },
}

/// The tool specs to advertise to the provider.
pub fn tool_specs(tools: &[Box<dyn Tool>]) -> Vec<ToolSpec> {
    tools.iter().map(|t| t.spec()).collect()
}

/// Map a folded `ChatMsg` to a provider `Message`.
fn map_msg(m: ChatMsg) -> Message {
    match m {
        ChatMsg::User { text, .. } => Message::user(text),
        ChatMsg::Assistant {
            text, tool_calls, ..
        } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: text,
            tool_calls: tool_calls
                .into_iter()
                .map(|c| ToolCall {
                    id: c.id,
                    name: c.name,
                    args: serde_json::from_str(&c.args).unwrap_or(serde_json::Value::Null),
                })
                .collect(),
            tool_name: None,
            tool_call_id: None,
        },
        ChatMsg::ToolResult {
            id, name, output, ..
        } => Message::tool_with_call_id(name, id, output),
        ChatMsg::Delegated { summary, .. } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: format!("[delegated subagent] {summary}"),
            tool_calls: vec![],
            tool_name: None,
            tool_call_id: None,
        },
        ChatMsg::Question {
            id,
            kind,
            question: _,
            choices: _,
            state,
            ts: _,
        } => {
            // The card is a UI/persistence concern. The model sees the answer
            // as a ToolResult for the matching id. When the card is Answered,
            // emit it as a tool-result message so the provider request carries
            // the answer. When Open (shouldn't happen in a well-formed request,
            // but defensive), emit an inert assistant message.
            let tool_name = match kind {
                zoid_core::event::QuestionKind::Ask => "ask_user",
                zoid_core::event::QuestionKind::ModeMapping { .. } => "apply_mode_mapping",
            };
            match state {
                zoid_core::projection::QuestionCardState::Answered { answer } => {
                    Message::tool_with_call_id(tool_name, id, answer)
                }
                zoid_core::projection::QuestionCardState::Open { .. } => Message {
                    role: zoid_provider::MsgRole::Assistant,
                    content: String::new(),
                    tool_calls: vec![],
                    tool_name: None,
                    tool_call_id: None,
                },
            }
        }
    }
}

/// Build a completion request from the current event log.
pub fn build_request(
    events: &crate::eventlog::EventLog,
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
) -> CompletionRequest {
    let system = match zoid_core::eviction::eviction_breadcrumb(events.iter()) {
        Some(bc) => format!("{system}\n\n{bc}"),
        None => system.to_string(),
    };
    CompletionRequest {
        model: model.to_string(),
        system: Some(system),
        messages: conversation(events.iter())
            .into_iter()
            .map(map_msg)
            .collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}

/// Run one user-message-to-completion agent turn. `seed_events` is the current
/// log snapshot (including the just-appended user message). Every event this
/// produces is persisted via `session` and announced via `ui`, and returned as
/// the accumulated `Vec<Event>` (seed + everything appended this turn).
///
/// `config` carries the system prompt, working directory, and event branch —
/// generalizing this ONE loop over both Chat (main branch, cwd `.`) and
/// subagents (their own branch + cwd).
///
/// `TurnComplete` is sent on EVERY exit path — including session/IO errors —
/// so the UI never gets stuck in the `streaming` state.
// These 10 params thread the full turn context (turn config, provider, tools,
// session, seed events, model, ui channel, session_id, clock, calibration);
// a params struct would add indirection without adding clarity.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_turn(
    config: TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    gate: Arc<dyn ToolGate>,
    session: SessionHandle,
    events: crate::eventlog::EventLog,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    companion_hub: std::sync::Arc<zoid_companion::CompanionHub>,
    now: fn() -> i64,
) -> Result<crate::eventlog::EventLog> {
    // Not externally cancellable: a token that never fires. The TUI uses
    // `run_agent_turn_cancellable` to wire Esc/Ctrl-C; subagents and tests use
    // this convenience wrapper.
    run_agent_turn_cancellable(
        config,
        provider,
        tools,
        gate,
        session,
        events,
        model,
        ui,
        session_id,
        companion_hub,
        now,
        CancellationToken::new(),
    )
    .await
}

/// Like [`run_agent_turn`] but cancellable: when `cancel` fires (Esc/Ctrl-C from
/// the TUI), the loop stops at the next safe point — draining any un-executed
/// tool calls with a balanced `[skipped: turn aborted]` result so no tool call
/// is left unanswered — and ends the turn cleanly. `TurnComplete` still fires on
/// every exit path.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_turn_cancellable(
    config: TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    gate: Arc<dyn ToolGate>,
    session: SessionHandle,
    events: crate::eventlog::EventLog,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    companion_hub: std::sync::Arc<zoid_companion::CompanionHub>,
    now: fn() -> i64,
    cancel: CancellationToken,
) -> Result<crate::eventlog::EventLog> {
    // Calibration ratio: real_input_tokens / context_window.total_tokens from
    // the last non-cached sub-turn. The chars/4 estimate undercounts 5-7x for
    // code/tool output, so when the provider reports 0 (Ollama cached prompt)
    // we scale the current estimate by this ratio to approximate the real
    // context size. Updated on every sub-turn where the provider reports a
    // non-zero input. Mutable, lives for the turn (across sub-turns).
    let mut calibration_ratio: Option<f64> = None;

    // Compute the context overhead (system prompt + tool specs) once for the
    // turn — it's constant across sub-turns. These are tokens the provider
    // counts against the context ceiling that are not derivable from the event
    // log. The tool-call args are event-derived and counted per-call inside
    // context_window_with; the system prompt and tool schemas are not.
    let overhead = {
        let system_tokens = zoid_core::economy::estimate_tokens(&config.system);
        let tools_tokens: u64 = tools
            .iter()
            .map(|t| {
                let spec = t.spec();
                let spec_str = format!(
                    "{}\n{}\n{}",
                    spec.name,
                    spec.description,
                    serde_json::to_string(&spec.parameters).unwrap_or_default()
                );
                zoid_core::economy::estimate_tokens(&spec_str)
            })
            .sum();
        zoid_core::context::ContextOverhead {
            system_tokens,
            tools_tokens,
        }
    };

    let result = run_turn_inner(
        &config,
        provider,
        tools,
        gate,
        session,
        events,
        model,
        &ui,
        session_id,
        companion_hub,
        now,
        &mut calibration_ratio,
        &overhead,
        &cancel,
    )
    .await;
    // Best-effort: if the receiver is already gone we still return the inner result.
    let _ = ui.send(AgentUpdate::TurnComplete).await;
    result
}

/// Inner loop — separated so `run_agent_turn` can send `TurnComplete` regardless
/// of whether this returns `Ok` or `Err`.
// Same 10-arg turn context as `run_agent_turn` above plus the mutable calibration
// ratio; see that comment.
#[allow(clippy::too_many_arguments)]
async fn run_turn_inner(
    config: &TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    gate: Arc<dyn ToolGate>,
    session: SessionHandle,
    mut events: crate::eventlog::EventLog,
    model: String,
    ui: &mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    companion_hub: std::sync::Arc<zoid_companion::CompanionHub>,
    now: fn() -> i64,
    calibration_ratio: &mut Option<f64>,
    overhead: &zoid_core::context::ContextOverhead,
    cancel: &CancellationToken,
) -> Result<crate::eventlog::EventLog> {
    let turn_start = std::time::Instant::now();
    let mut iterations: u32 = 0;
    let mut context_retries: u32 = 0;
    let mut outcome: &'static str = "completed";

    'turn: loop {
        // Cancelled between sub-turns: nothing is pending here, so end cleanly.
        if cancel.is_cancelled() {
            outcome = "aborted";
            break 'turn;
        }
        // PRE-FLIGHT GATE (spec §3.8): shrink to fit BEFORE building the request.
        preflight_gate(
            &session,
            &mut events,
            ui,
            config,
            session_id,
            now,
            &*calibration_ratio,
            overhead,
        )
        .await?;
        let req = build_request(&events, &model, &tools, &config.system);

        // Stream one model turn. Spawn the provider so a missing terminal Done
        // (truncated stream) can't hang us — we send our own Done after it ends.
        let (ptx, mut prx) = mpsc::channel::<ProviderEvent>(256);
        let p = provider.clone();
        let stream_task = tokio::spawn(async move {
            let _ = p.stream(&req, ptx.clone()).await;
            let _ = ptx.send(ProviderEvent::Done).await;
        });

        let mut turn_usage = zoid_core::event::TokenStat::default();
        let mut pending: Vec<ToolCall> = Vec::new();
        let mut aborted = false;
        loop {
            let pe = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    aborted = true;
                    break;
                }
                maybe = prx.recv() => match maybe {
                    Some(pe) => pe,
                    None => break,
                },
            };
            match pe {
                ProviderEvent::TextDelta(s) => {
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ModelDelta { text: s },
                        session_id,
                        now,
                    )
                    .await?;
                }
                ProviderEvent::ToolCall(tc) => {
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            args: tc.args.to_string(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    pending.push(tc);
                }
                ProviderEvent::Usage(u) => {
                    turn_usage.input += u.input_tokens;
                    turn_usage.output += u.output_tokens;
                    turn_usage.cached += u.cached;
                }
                ProviderEvent::Error(msg) => {
                    let _ = stream_task.await;
                    if zoid_provider::is_context_length_error(&msg)
                        && context_retries < MAX_CONTEXT_RETRIES
                        && config.eviction.enabled
                    {
                        context_retries += 1;
                        // The estimate under-read reality: force a wave toward low_water and retry.
                        let est = zoid_core::context::context_window_with(
                            events.iter(),
                            overhead.clone(),
                        )
                        .total_tokens;
                        let plan = zoid_core::eviction::plan_evictions(
                            events.iter(),
                            &config.eviction,
                            est,
                            &zoid_core::eviction::RecencyScorer,
                        );
                        emit_eviction(&session, &mut events, ui, config, session_id, now, plan)
                            .await?;
                        tracing::warn!(ctx = "provider", "context-length error; forced eviction, retrying ({context_retries}/{MAX_CONTEXT_RETRIES})");
                        continue 'turn;
                    }
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::AssistantMessage {
                            text: format!("{WARN_GLYPH} {msg}"),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::warn!(ctx = "provider", message = msg.as_str(), "turn error");
                    outcome = "error";
                    break 'turn;
                }
                ProviderEvent::Truncated => {
                    // Surface an incomplete-reply warning but do NOT break: the
                    // provider still sends a terminal Done right after.
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::AssistantMessage {
                            text: format!(
                                "{WARN_GLYPH} response truncated — hit the output token cap; \
                                 the reply above is incomplete"
                            ),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                }
                ProviderEvent::Done => break,
            }
        }
        if aborted {
            // Cancelled mid-stream: stop the provider task and balance any tool
            // calls parsed before the cancel so none is left without a
            // ToolResult (the provider protocol requires every call answered
            // before the next request).
            stream_task.abort();
            let _ = stream_task.await;
            for tc in pending.drain(..) {
                emit(
                    &session,
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::ToolResult {
                        id: tc.id,
                        name: tc.name,
                        output: "[skipped: turn aborted]".to_string(),
                        is_error: false,
                    },
                    session_id,
                    now,
                )
                .await?;
            }
            outcome = "aborted";
            break 'turn;
        }
        let _ = stream_task.await;

        // Record the sub-turn's token usage so the economy ledger is live.
        if turn_usage != zoid_core::event::TokenStat::default() {
            emit_with_tokens(
                &session,
                &mut events,
                ui,
                &config.branch,
                EventKind::Usage,
                Some(turn_usage),
                session_id,
                now,
            )
            .await?;
        }

        // Check for tool-result compactions after every sub-turn (not just
        // after tool execution). Without this, text-only responses (the common
        // case) would break out of the loop before reaching record_compactions,
        // and the context window would grow unbounded until the model happens
        // to call a tool.
        record_compactions(
            &session,
            &mut events,
            ui,
            config,
            session_id,
            now,
            if turn_usage.input > 0 {
                Some(turn_usage.input)
            } else {
                None
            },
            calibration_ratio,
            overhead,
        )
        .await?;

        if pending.is_empty() {
            break 'turn; // model answered without tools — turn complete
        }

        iterations += 1;
        if iterations > MAX_TOOL_ITERATIONS {
            emit(
                &session,
                &mut events,
                ui,
                &config.branch,
                EventKind::AssistantMessage {
                    text: format!("{WARN_GLYPH} tool-iteration limit reached"),
                },
                session_id,
                now,
            )
            .await?;
            outcome = "cap";
            break 'turn;
        }

        // Execute each pending tool in the configured working directory
        // (blocking work off the async runtime), recording its result as an event.
        let cwd_for_exec = config.cwd.clone();
        let mut pending_iter = pending.into_iter();
        while let Some(tc) = pending_iter.next() {
            // Cancelled mid-batch: skip this tool and every remaining one with a
            // balanced result, then end the turn (same integrity rule as the
            // ask_user abort path — no tool call left unanswered).
            if cancel.is_cancelled() {
                for rest in std::iter::once(tc).chain(pending_iter.by_ref()) {
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: rest.id,
                            name: rest.name,
                            output: "[skipped: turn aborted]".to_string(),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                }
                outcome = "aborted";
                break 'turn;
            }
            let tool_start = std::time::Instant::now();
            let tool_name = tc.name.clone();

            if let Gate::Deny(reason) = gate.check(&tc) {
                let reason_msg = reason.clone();
                emit(
                    &session,
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::ToolResult {
                        id: tc.id,
                        name: tc.name,
                        output: reason,
                        is_error: true,
                    },
                    session_id,
                    now,
                )
                .await?;
                tracing::info!(
                    kind = "tool",
                    name = tool_name.as_str(),
                    ms = tool_start.elapsed().as_millis() as u64,
                    ok = false,
                    "tool executed"
                );
                let ctx = format!("tool {tool_name}");
                tracing::warn!(
                    ctx = ctx.as_str(),
                    message = reason_msg.as_str(),
                    "tool failed"
                );
                continue;
            }

            let kind = tools.iter().find(|t| t.name() == tc.name).map(|t| t.kind());
            tracing::debug!("tool: name={:?} kind={:?}", tc.name, kind);

            match kind {
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "update_tasks" => {
                    match zoid_core::tasks::parse_task_items(&tc.args) {
                        Ok(items) => {
                            let n = items.len();
                            let active = items
                                .iter()
                                .filter(|i| i.status == zoid_core::tasks::TaskStatus::Active)
                                .count();
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::Tasks { items },
                                session_id,
                                now,
                            )
                            .await?;
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: format!("{n} tasks · {active} active"),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            tracing::info!(
                                kind = "tool",
                                name = tool_name.as_str(),
                                ms = tool_start.elapsed().as_millis() as u64,
                                ok = true,
                                "tool executed"
                            );
                        }
                        Err(msg) => {
                            let tool_msg = msg.clone();
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: msg,
                                    is_error: true,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            tracing::info!(
                                kind = "tool",
                                name = tool_name.as_str(),
                                ms = tool_start.elapsed().as_millis() as u64,
                                ok = false,
                                "tool executed"
                            );
                            let ctx = format!("tool {tool_name}");
                            tracing::warn!(
                                ctx = ctx.as_str(),
                                message = tool_msg.as_str(),
                                "tool failed"
                            );
                        }
                    }
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "recall" => {
                    let query = tc
                        .args
                        .get("query")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let limit = tc.args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                    let hits = session
                        .recall(query, session_id, limit)
                        .await
                        .unwrap_or_default();
                    // Re-admit any currently-evicted originals so they re-enter the projection.
                    let live_evicted = zoid_core::eviction::evicted_ids(events.iter());
                    let readmit: Vec<Ulid> = hits
                        .iter()
                        .map(|e| e.id)
                        .filter(|id| live_evicted.contains(id))
                        .collect();
                    if !readmit.is_empty() {
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::TurnsReadmitted { ids: readmit },
                            session_id,
                            now,
                        )
                        .await?;
                    }
                    let rendered = render_recalled(&hits);
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: if rendered.is_empty() {
                                "[recall: no matches]".into()
                            } else {
                                rendered
                            },
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "recall",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "tool executed"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "show" => {
                    let html = tc
                        .args
                        .get("html")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let (output, is_error) = companion_show(&companion_hub, html);
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output,
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "show",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = !is_error,
                        "tool executed"
                    );
                }
                Some(zoid_tools::ToolKind::Interactive)
                    if tc.name == "ask_user" || tc.name == "apply_mode_mapping" =>
                {
                    let (question, choices) = if tc.name == "ask_user" {
                        let question = tc
                            .args
                            .get("question")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let choices = tc
                            .args
                            .get("choices")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|c| c.as_str().map(String::from))
                                    .collect::<Vec<String>>()
                            })
                            .unwrap_or_default();
                        // Emit QuestionAsked so the card renders inline immediately.
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::QuestionAsked {
                                id: tc.id.clone(),
                                kind: zoid_core::event::QuestionKind::Ask,
                                question: question.clone(),
                                choices: choices.clone(),
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        (question, choices)
                    } else {
                        // apply_mode_mapping
                        let mapping = match crate::mode_wizard::parse_mapping_args(&tc.args) {
                            Ok(m) => m,
                            Err(reason) => {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: tc.id.clone(),
                                        name: tc.name.clone(),
                                        output: format!(
                                            "apply_mode_mapping: {reason}. Re-propose with valid args."
                                        ),
                                        is_error: true,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                                continue;
                            }
                        };
                        let detail = crate::mode_wizard::detailed_approval_summary(&mapping);
                        let choices = vec!["Approve".into(), "Reject".into(), "Adjust".into()];
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::QuestionAsked {
                                id: tc.id.clone(),
                                kind: zoid_core::event::QuestionKind::ModeMapping {
                                    mapping: Box::new(mapping),
                                },
                                question: detail.clone(),
                                choices: choices.clone(),
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        (detail, choices)
                    };
                    let (rtx, rrx) = oneshot::channel::<Answer>();
                    tracing::debug!(
                        "{}: intercepted, sending AskUser (choices={})",
                        tc.name,
                        choices.len()
                    );
                    let sent = ui
                        .send(AgentUpdate::AskUser {
                            question,
                            choices,
                            reply: rtx,
                        })
                        .await;
                    tracing::debug!(
                        "{}: send result ok={}, awaiting reply",
                        tc.name,
                        sent.is_ok()
                    );
                    let ans = rrx.await;
                    tracing::debug!("{}: reply received ok={}", tc.name, ans.is_ok());
                    let output = match ans {
                        Ok(Answer::Choice(s) | Answer::FreeText(s)) => s,
                        Ok(Answer::LetYouDecide) => "[let you decide]".to_string(),
                        Err(_) => "[user aborted]".to_string(),
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAnswered {
                            id: tc.id.clone(),
                            answer: output.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    let is_error = if tc.name == "apply_mode_mapping" {
                        output == "[user aborted]" || output == "Reject"
                    } else {
                        output == "[user aborted]"
                    };
                    // For apply_mode_mapping, enrich the tool result so the
                    // model knows the materialization already happened (no
                    // need to ask the user to confirm again).
                    let tool_output = if tc.name == "apply_mode_mapping" && !is_error {
                        match output.as_str() {
                            "Approve" => "Approved and materialized. The mode files have been \
                                written to disk and the mode registry reloaded. No further \
                                confirmation needed — the import is complete."
                                .to_string(),
                            "Adjust" => "User requested adjustments. Re-propose the mapping \
                                with the user's feedback."
                                .to_string(),
                            _ => output.clone(),
                        }
                    } else {
                        output.clone()
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: tool_output,
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = !is_error,
                        "tool executed"
                    );
                    if is_error {
                        // Drain any remaining batched tool calls so none is
                        // left without a matching ToolResult (the provider's
                        // tool-call protocol requires every call to be
                        // answered before the next request).
                        for rest in pending_iter.by_ref() {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: rest.id,
                                    name: rest.name,
                                    output: "[skipped: turn aborted]".to_string(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                        outcome = "aborted";
                        break 'turn;
                    }
                }
                _ => {
                    // Local tools (the default): run in the working directory.
                    let _ = ui
                        .send(AgentUpdate::ToolStarted {
                            name: tc.name.clone(),
                        })
                        .await;
                    let tools_for_exec = tools.clone();
                    let name = tc.name.clone();
                    let args = tc.args.clone();
                    let cwd = cwd_for_exec.clone();
                    let out = tokio::task::spawn_blocking(move || {
                        zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd)
                    })
                    .await?;
                    let tool_ok = !out.is_error;
                    let tool_fail_msg = out.is_error.then(|| out.text.clone());
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: out.text,
                            is_error: out.is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = tool_ok,
                        "tool executed"
                    );
                    if let Some(msg) = tool_fail_msg {
                        let ctx = format!("tool {tool_name}");
                        tracing::warn!(ctx = ctx.as_str(), message = msg.as_str(), "tool failed");
                    }
                }
            }
        }
        record_compactions(
            &session,
            &mut events,
            ui,
            config,
            session_id,
            now,
            if turn_usage.input > 0 {
                Some(turn_usage.input)
            } else {
                None
            },
            calibration_ratio,
            overhead,
        )
        .await?;
        // loop: re-request with the tool results now in context
    }

    tracing::info!(
        kind = "turn",
        model = %model,
        iterations = iterations as u64,
        ms = turn_start.elapsed().as_millis() as u64,
        outcome = outcome,
        "turn complete"
    );

    Ok(events)
}

/// Persist one event and announce it to the UI, keeping the local log in sync.
async fn emit(
    session: &SessionHandle,
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    emit_with_tokens(session, events, ui, branch, kind, None, session_id, now).await
}

/// Result of a `show` tool call: publish the card when the companion is enabled,
/// otherwise return a no-op ack. Never errors (returns `is_error = false`).
pub(crate) fn companion_show(hub: &zoid_companion::CompanionHub, html: String) -> (String, bool) {
    if hub.is_enabled() {
        hub.publish_card(html);
        ("card shown in companion".to_string(), false)
    } else {
        (
            "Companion is disabled; enable it from the command palette to view cards.".to_string(),
            false,
        )
    }
}

/// Render recalled events into readable text for the recall tool-result.
fn render_recalled(events: &[Event]) -> String {
    let mut out = String::new();
    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => out.push_str(&format!("[user] {text}\n")),
            EventKind::AssistantMessage { text } => out.push_str(&format!("[assistant] {text}\n")),
            EventKind::ToolResult { name, output, .. } => {
                out.push_str(&format!("[{name}] {output}\n"))
            }
            _ => {}
        }
    }
    out.trim_end().to_string()
}

/// Record `ToolResultCompacted` events for any tool-results the policy says
/// should be compacted given the current log. Idempotent: `plan_compactions`
/// skips already-compacted ids, so calling this each round is safe.
///
/// **Calibration:** when the provider reports a non-zero `real_input_tokens`,
/// we learn the ratio `real_input / context_window.total_tokens` and store it
/// in `calibration_ratio`. The chars/3 estimate is closer to the real tokenizer
/// ratio than the old chars/4, but it's still an estimate; this ratio lets us
/// fine-tune on cached sub-turns (where the provider reports 0): we scale the
/// current estimate by the last known ratio.
// Pre-existing 9-arg signature (predates the companion feature); a refactor is
// out of scope for companion lifecycle wiring, so the lint is suppressed here
// rather than reshaping unrelated agent-loop plumbing.
#[allow(clippy::too_many_arguments)]
async fn record_compactions(
    session: &SessionHandle,
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
    real_input_tokens: Option<u64>,
    calibration_ratio: &mut Option<f64>,
    overhead: &zoid_core::context::ContextOverhead,
) -> Result<()> {
    // When the provider reports a non-zero input, learn the calibration ratio:
    // real tokens / estimated tokens. This is the ground-truth correction factor
    // for the chars/3 estimate.
    let effective_tokens = real_input_tokens.filter(|&t| t > 0);
    if let Some(real) = effective_tokens {
        let window = zoid_core::context::context_window_with(events.iter(), overhead.clone());
        if window.total_tokens > 0 {
            *calibration_ratio = Some(real as f64 / window.total_tokens as f64);
        }
    }
    // When cached (0), pass None — plan_compactions scales the estimate by the
    // calibration ratio if one has been learned, else uses the raw estimate.
    let plan = zoid_core::compaction::plan_compactions(
        events.iter(),
        &config.policy,
        effective_tokens,
        *calibration_ratio,
        overhead,
    );
    for c in &plan.compactions {
        emit(
            session,
            events,
            ui,
            &config.branch,
            EventKind::ToolResultCompacted {
                id: c.id.clone(),
                summary: c.summary.clone(),
                original_tokens: c.original_tokens,
            },
            session_id,
            now,
        )
        .await?;
    }
    Ok(())
}

/// Bias applied to the pre-flight estimate (the chars/3 estimate under-reads
/// code/tool output). Push the estimate up so the gate fires early, not late.
const OVERCOUNT_BIAS: f64 = 1.15;

/// Run the cheap correctness levers BEFORE the request is built (spec §3.8, C1):
/// (1) compact tool results, (2) evict oldest turns to `low_water`, (3) if near
/// hard capacity, evict harder toward the safety floor. Emits `ToolResultCompacted`
/// / `TurnsEvicted` events (append-only). No-op when `config.eviction.enabled` is
/// false (subagents/tests) — byte-identical to pre-ACM behavior.
#[allow(clippy::too_many_arguments)]
async fn preflight_gate(
    session: &SessionHandle,
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
    calibration_ratio: &Option<f64>,
    overhead: &zoid_core::context::ContextOverhead,
) -> Result<()> {
    let policy = &config.eviction;
    if !policy.enabled {
        return Ok(());
    }
    let band = policy.band();

    let estimate = |events: &crate::eventlog::EventLog| -> u64 {
        let raw =
            zoid_core::context::context_window_with(events.iter(), overhead.clone()).total_tokens;
        let scaled = match calibration_ratio {
            Some(r) if *r > 0.0 => (raw as f64 * r) as u64,
            _ => raw,
        };
        (scaled as f64 * OVERCOUNT_BIAS) as u64
    };

    // Compute the estimate once and refresh it only after a step actually mutates
    // `events` (each `estimate` re-walks the whole log). Behavior is identical to
    // recomputing on every check — every check still sees the current state.
    let mut est = estimate(events);

    // (1) Compaction first (largest-first; spec §3.9 rule 2). Reuse plan_compactions
    // with the band's high_water as the threshold.
    if est >= band.high_water {
        let gate_policy = zoid_core::assembler::ContextPolicy {
            compact_threshold: Some(band.high_water),
            ..config.policy
        };
        let plan = zoid_core::compaction::plan_compactions(
            events.iter(),
            &gate_policy,
            None,
            *calibration_ratio,
            overhead,
        );
        let compacted = !plan.compactions.is_empty();
        for c in &plan.compactions {
            emit(
                session,
                events,
                ui,
                &config.branch,
                EventKind::ToolResultCompacted {
                    id: c.id.clone(),
                    summary: c.summary.clone(),
                    original_tokens: c.original_tokens,
                },
                session_id,
                now,
            )
            .await?;
        }
        if compacted {
            est = estimate(events);
        }
    }

    // (2) Eviction to low_water.
    if est >= band.high_water {
        let plan = zoid_core::eviction::plan_evictions(
            events.iter(),
            policy,
            est,
            &zoid_core::eviction::RecencyScorer,
        );
        emit_eviction(session, events, ui, config, session_id, now, plan).await?;
        est = estimate(events);
    }

    // (3) Hard floor: if still near capacity, evict harder toward the safety margin.
    let hard = policy
        .capacity
        .saturating_sub(zoid_core::band::CAPACITY_SAFETY_MARGIN);
    if est >= hard {
        // Re-run with the same policy; low_water already targets below capacity.
        let plan = zoid_core::eviction::plan_evictions(
            events.iter(),
            policy,
            est,
            &zoid_core::eviction::RecencyScorer,
        );
        emit_eviction(session, events, ui, config, session_id, now, plan).await?;
    }
    Ok(())
}

/// Emit one `TurnsEvicted` event carrying the plan's spans (or nothing if empty).
#[allow(clippy::too_many_arguments)]
async fn emit_eviction(
    session: &SessionHandle,
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
    plan: zoid_core::eviction::EvictionPlan,
) -> Result<()> {
    if plan.turns.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::new();
    let mut reclaimed = 0u64;
    let mut spans = Vec::new();
    for t in plan.turns {
        reclaimed += t.token_estimate;
        spans.push(zoid_core::event::EvictedSpan {
            token_estimate: t.token_estimate,
            topic_hint: t.topic_hint,
        });
        ids.extend(t.ids);
    }
    emit(
        session,
        events,
        ui,
        &config.branch,
        EventKind::TurnsEvicted {
            ids,
            reclaimed_tokens: reclaimed,
            marker: zoid_core::event::EvictionMarker { spans },
        },
        session_id,
        now,
    )
    .await?;
    Ok(())
}

/// Persist one event (optionally carrying token usage) and announce it to the
/// UI, keeping the local log in sync.
// 8 args: session, events, ui, branch, kind, tokens, session_id, clock — every
// one load-bearing for what/where this event is tagged and delivered.
#[allow(clippy::too_many_arguments)]
async fn emit_with_tokens(
    session: &SessionHandle,
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    tokens: Option<zoid_core::event::TokenStat>,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind).with_session(session_id);
    ev.branch = branch.clone();
    ev.tokens = tokens;
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::event::BranchId;
    use zoid_provider::MsgRole;

    #[test]
    fn chat_turn_config_is_main_branch_cwd_dot() {
        let c = chat_turn_config();
        assert_eq!(c.branch, BranchId::default());
        assert_eq!(c.cwd, std::path::PathBuf::from("."));
        assert_eq!(c.system, SYSTEM_PROMPT);
    }

    #[test]
    fn companion_show_publishes_when_enabled_and_acks_when_disabled() {
        use zoid_companion::CompanionHub;
        let hub = CompanionHub::new();

        // Disabled: no publish, distinct ack.
        let (out, err) = super::companion_show(&hub, "<b>x</b>".into());
        assert!(!err);
        assert!(out.contains("disabled"), "got: {out}");
        assert!(hub.current().card.is_none());

        // Enabled: publishes the card.
        hub.set_enabled(true);
        let (out, err) = super::companion_show(&hub, "<b>y</b>".into());
        assert!(!err);
        assert_eq!(out, "card shown in companion");
        assert_eq!(hub.current().card.as_deref(), Some("<b>y</b>"));
    }

    #[test]
    fn build_request_uses_the_given_system_prompt() {
        let req = build_request(
            &crate::eventlog::EventLog::new(),
            "m",
            &zoid_tools::registry(),
            "CUSTOM SYS",
        );
        assert_eq!(req.system.as_deref(), Some("CUSTOM SYS"));
    }

    #[test]
    fn build_request_appends_breadcrumb_when_evicted() {
        use zoid_core::event::{EvictedSpan, EvictionMarker};
        let events = crate::eventlog::EventLog::from_vec(vec![
            Event::new(
                Ulid::from(1u128),
                None,
                1,
                EventKind::UserMessage { text: "hi".into() },
            ),
            Event::new(
                Ulid::from(9u128),
                None,
                9,
                EventKind::TurnsEvicted {
                    ids: vec![Ulid::from(1u128)],
                    reclaimed_tokens: 4200,
                    marker: EvictionMarker {
                        spans: vec![EvictedSpan {
                            token_estimate: 4200,
                            topic_hint: "setup".into(),
                        }],
                    },
                },
            ),
        ]);
        let req = build_request(&events, "m", &zoid_tools::registry(), "SYS");
        let sys = req.system.unwrap();
        assert!(sys.starts_with("SYS"));
        assert!(sys.contains("recall"));
    }

    #[test]
    fn default_profile_carries_system_prompt_and_allows_all_tools() {
        let p = default_profile();
        assert_eq!(p.name, "Chat");
        assert_eq!(p.system_prompt, SYSTEM_PROMPT);
        assert!(p.tools.is_empty(), "empty allow-list = all tools permitted");
        assert!(p.allows("invoke_skill"));
        assert!(p.allows("write_file"));
    }

    #[test]
    fn build_request_carries_compacted_summary_into_live_messages() {
        let ts = 1700000000i64;
        let events = crate::eventlog::EventLog::from_vec(vec![
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::UserMessage {
                    text: "test".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolCall {
                    id: "c1".into(),
                    name: "shell".into(),
                    args: "{}".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolResult {
                    id: "c1".into(),
                    name: "shell".into(),
                    output: "HUGE ORIGINAL DUMP".into(),
                    is_error: false,
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolResultCompacted {
                    id: "c1".into(),
                    summary: "compacted summary".into(),
                    original_tokens: 500,
                },
            ),
        ]);

        let req = build_request(&events, "test-model", &zoid_tools::registry(), "SYS");

        // Find the tool message in the built request
        let tool_msg = req
            .messages
            .iter()
            .find(|m| m.role == MsgRole::Tool && m.tool_name.as_deref() == Some("shell"))
            .expect("tool message should be present");

        // Assert the content is the compacted summary, not the original dump
        assert_eq!(tool_msg.content, "compacted summary");
        assert!(!tool_msg.content.contains("HUGE ORIGINAL DUMP"));
    }

    #[test]
    fn chat_turn_config_with_embeds_menu_in_system() {
        let p = default_profile();
        let cfg = chat_turn_config_with(&p, "- spike-plan: do the thing");
        assert!(cfg.system.starts_with(SYSTEM_PROMPT));
        assert!(cfg.system.contains("## Available skills"));
        assert!(cfg.system.contains("- spike-plan: do the thing"));
    }

    #[test]
    fn chat_turn_config_with_empty_menu_is_just_prompt() {
        let p = default_profile();
        let cfg = chat_turn_config_with(&p, "");
        assert_eq!(cfg.system, SYSTEM_PROMPT);
    }

    #[test]
    fn zero_arg_chat_turn_config_matches_default_profile_no_menu() {
        // The zero-arg convenience must stay byte-identical to the old behavior.
        assert_eq!(chat_turn_config().system, SYSTEM_PROMPT);
    }

    #[tokio::test]
    async fn preflight_gate_evicts_before_send() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        // 8 fat turns, target tiny so the gate must evict.
        let big = "x".repeat(3000);
        let mut seed = Vec::new();
        for i in 0..8u128 {
            seed.push(Event::new(
                Ulid::from(i * 2 + 1),
                None,
                (i * 2 + 1) as i64,
                EventKind::UserMessage { text: big.clone() },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: "ok".into() },
            ));
        }
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            zoid_provider::ProviderEvent::TextDelta("done".into()),
            zoid_provider::ProviderEvent::Done,
        ]));
        let mut cfg = chat_turn_config();
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 3_000,
            band_headroom_pct: 20,
            recent_n: 2,
            max_output: None,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} }); // drain UI updates
        let out = run_agent_turn(
            cfg,
            provider,
            std::sync::Arc::new(zoid_tools::registry()),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();
        assert!(
            out.iter()
                .any(|e| matches!(e.kind, EventKind::TurnsEvicted { .. })),
            "gate must evict pre-flight"
        );
        // and the surviving conversation is under the seed size
        assert!(zoid_core::projection::conversation(out.iter()).len() < 16);
    }

    // Test double: replays a different script per stream() call (retry / multi-request turns).
    struct SequencedProvider {
        scripts: std::sync::Mutex<std::collections::VecDeque<Vec<zoid_provider::ProviderEvent>>>,
    }
    impl SequencedProvider {
        fn new(scripts: Vec<Vec<zoid_provider::ProviderEvent>>) -> Self {
            Self {
                scripts: std::sync::Mutex::new(scripts.into_iter().collect()),
            }
        }
    }
    #[async_trait::async_trait]
    impl zoid_provider::Provider for SequencedProvider {
        async fn stream(
            &self,
            _req: &zoid_provider::CompletionRequest,
            sink: tokio::sync::mpsc::Sender<zoid_provider::ProviderEvent>,
        ) -> anyhow::Result<()> {
            let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
            for ev in script {
                if sink.send(ev).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn context_length_error_is_retried_not_surfaced() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "hi".into() },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        // First stream errors with a context-length message; the retry completes.
        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![ProviderEvent::Error(
                "prompt is too long: exceeds context window".into(),
            )],
            vec![
                ProviderEvent::TextDelta("recovered".into()),
                ProviderEvent::Done,
            ],
        ]));
        let mut cfg = chat_turn_config();
        // enabled so the retry arm is active; band huge so the preflight gate itself evicts nothing
        // — this isolates the capacity-error retry path.
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 900_000,
            band_headroom_pct: 20,
            recent_n: 4,
            max_output: None,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            cfg,
            provider,
            std::sync::Arc::new(zoid_tools::registry()),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();
        // The context error was retried, not surfaced as a ⚠ message …
        assert!(!out.iter().any(|e| matches!(&e.kind, EventKind::AssistantMessage { text } if text.starts_with(WARN_GLYPH))), "context error must not surface");
        // … and the retry reached the second, successful stream.
        assert!(
            out.iter()
                .any(|e| matches!(&e.kind, EventKind::ModelDelta { text } if text == "recovered")),
            "retry must reach the successful stream"
        );
    }

    #[tokio::test]
    async fn recall_tool_readmits_and_returns_content() {
        use serde_json::json;
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind, EvictionMarker};
        use zoid_core::projection::{conversation, ChatMsg};
        use zoid_provider::{ProviderEvent, ToolCall};
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        // Seed: an evicted user turn (indexed in the store at append) + a recent turn + the marker.
        let e1 = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "configure the vector backend".into(),
            },
        );
        let e2 = Event::new(
            Ulid::from(2u128),
            None,
            2,
            EventKind::UserMessage {
                text: "recent question".into(),
            },
        );
        let evicted = Event::new(
            Ulid::from(9u128),
            None,
            9,
            EventKind::TurnsEvicted {
                ids: vec![Ulid::from(1u128)],
                reclaimed_tokens: 10,
                marker: EvictionMarker { spans: vec![] },
            },
        );
        for e in [&e1, &e2, &evicted] {
            session.append(e.clone()).await.unwrap();
        }
        let seed = vec![e1.clone(), e2.clone(), evicted.clone()];
        // Initially the evicted turn is NOT in the projection.
        assert!(!conversation(&seed)
            .iter()
            .any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("vector backend"))));

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "r1".into(),
                    name: "recall".into(),
                    args: json!({"query": "vector"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("thanks".into()),
                ProviderEvent::Done,
            ],
        ]));
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(std::sync::Arc::new(
            zoid_core::skill::SkillRegistry::builtin(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            chat_turn_config(),
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        // Re-admission event for the evicted id …
        assert!(out.iter().any(|e| matches!(&e.kind, EventKind::TurnsReadmitted { ids } if ids.contains(&Ulid::from(1u128)))));
        // … the recall ToolResult carries the retrieved content …
        assert!(out.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { name, output, .. } if name == "recall" && output.contains("vector backend"))));
        // … and the turn is back in the projection.
        assert!(conversation(out.iter())
            .iter()
            .any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("vector backend"))));
    }

    #[tokio::test]
    async fn recall_tool_reports_no_matches_and_readmits_nothing() {
        use serde_json::json;
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "configure the vector backend".into(),
            },
        );
        let e2 = Event::new(
            Ulid::from(2u128),
            None,
            2,
            EventKind::UserMessage {
                text: "recent question".into(),
            },
        );
        for e in [&e1, &e2] {
            session.append(e.clone()).await.unwrap();
        }
        let seed = vec![e1.clone(), e2.clone()];

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "r1".into(),
                    name: "recall".into(),
                    args: json!({"query": "nonexistent_term_xyz"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("thanks".into()),
                ProviderEvent::Done,
            ],
        ]));
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(std::sync::Arc::new(
            zoid_core::skill::SkillRegistry::builtin(),
        )));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            chat_turn_config(),
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        // No matches → the tool result says so …
        assert!(out.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { name, output, .. } if name == "recall" && output == "[recall: no matches]")));
        // … and nothing is re-admitted.
        assert!(!out
            .iter()
            .any(|e| matches!(&e.kind, EventKind::TurnsReadmitted { .. })));
    }

    #[tokio::test]
    async fn evict_then_recall_round_trips() {
        use serde_json::json;
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_core::eviction::EvictionPolicy;
        use zoid_core::projection::{conversation, ChatMsg};
        use zoid_provider::{FakeProvider, ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let big = "x".repeat(3000); // ~1000 tokens per turn
                                    // Oldest turn carries a distinctive searchable token.
        let mut seed = vec![
            Event::new(
                Ulid::from(1u128),
                None,
                1,
                EventKind::UserMessage {
                    text: format!("zephyrbackend {big}"),
                },
            ),
            Event::new(
                Ulid::from(2u128),
                None,
                2,
                EventKind::AssistantMessage { text: "ok".into() },
            ),
        ];
        for i in 1..8u128 {
            seed.push(Event::new(
                Ulid::from(i * 2 + 1),
                None,
                (i * 2 + 1) as i64,
                EventKind::UserMessage { text: big.clone() },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: "ok".into() },
            ));
        }
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let policy = EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 3_000,
            band_headroom_pct: 20,
            recent_n: 2,
            max_output: None,
        };
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(std::sync::Arc::new(
            zoid_core::skill::SkillRegistry::builtin(),
        )));

        // TURN 1 — the pre-flight gate evicts the oldest turns.
        let mut cfg1 = chat_turn_config();
        cfg1.eviction = policy;
        let p1 = std::sync::Arc::new(FakeProvider::new(vec![
            ProviderEvent::TextDelta("ack".into()),
            ProviderEvent::Done,
        ]));
        let (tx1, mut rx1) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx1.recv().await.is_some() {} });
        let out1 = run_agent_turn(
            cfg1,
            p1,
            tools.clone(),
            std::sync::Arc::new(zoid_tools::AllowAll),
            session.clone(),
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx1,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();
        assert!(
            out1.iter()
                .any(|e| matches!(e.kind, EventKind::TurnsEvicted { .. })),
            "turn 1 must evict"
        );
        assert!(
            !conversation(out1.iter())
                .iter()
                .any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("zephyrbackend"))),
            "evicted turn gone from projection"
        );

        // TURN 2 — the model recalls the evicted content.
        let mut cfg2 = chat_turn_config();
        cfg2.eviction = policy;
        let p2 = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "r1".into(),
                    name: "recall".into(),
                    args: json!({"query": "zephyrbackend"}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("got it".into()),
                ProviderEvent::Done,
            ],
        ]));
        let (tx2, mut rx2) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx2.recv().await.is_some() {} });
        let out2 = run_agent_turn(
            cfg2,
            p2,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            out1,
            "m".into(),
            tx2,
            Ulid::from(0u128),
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();
        assert!(out2.iter().any(|e| matches!(&e.kind, EventKind::TurnsReadmitted { ids } if ids.contains(&Ulid::from(1u128)))), "recall re-admits the evicted turn");
        assert!(out2.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { name, output, .. } if name == "recall" && output.contains("zephyrbackend"))), "recall result carries content");
    }
}

#[cfg(test)]
mod tool_call_id_threading_tests {
    use super::*;
    use zoid_core::event::{Event, EventKind};
    use zoid_provider::MsgRole;

    #[test]
    fn build_request_threads_tool_result_id_into_tool_call_id() {
        let ts = 1700000000i64;
        let events = crate::eventlog::EventLog::from_vec(vec![
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::UserMessage { text: "hi".into() },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolCall {
                    id: "call-7".into(),
                    name: "read_file".into(),
                    args: "{}".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolResult {
                    id: "call-7".into(),
                    name: "read_file".into(),
                    output: "ok".into(),
                    is_error: false,
                },
            ),
        ]);

        let req = build_request(&events, "m", &[], "sys");

        let tool_msg = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MsgRole::Tool)
            .expect("tool message should be present");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call-7"));
    }
}
