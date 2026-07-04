//! The terminal-free agent loop: stream a turn, execute any tool calls in the
//! working directory, record everything as events, and re-request until the
//! model stops calling tools (or the iteration cap trips).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use ulid::Ulid;

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
}

/// The orchestrator (Chat) turn config: main branch, process cwd, Chat prompt.
pub fn chat_turn_config() -> TurnConfig {
    TurnConfig {
        system: SYSTEM_PROMPT.to_string(),
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
        policy: zoid_core::assembler::ContextPolicy::default(),
    }
}

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 50;

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
        },
        ChatMsg::ToolResult { name, output, .. } => Message::tool(name, output),
        ChatMsg::Delegated { summary, .. } => Message {
            role: zoid_provider::MsgRole::Assistant,
            content: format!("[delegated subagent] {summary}"),
            tool_calls: vec![],
            tool_name: None,
        },
    }
}

/// Build a completion request from the current event log.
pub fn build_request(
    events: &[Event],
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: Some(system.to_string()),
        messages: conversation(events).into_iter().map(map_msg).collect(),
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
// These 9 params thread the full turn context (turn config, provider, tools,
// session, seed events, model, ui channel, session_id, clock); a params
// struct would add indirection without adding clarity.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_turn(
    config: TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    gate: Arc<dyn ToolGate>,
    session: SessionHandle,
    events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<Vec<Event>> {
    let result = run_turn_inner(
        &config, provider, tools, gate, session, events, model, &ui, session_id, now,
    )
    .await;
    // Best-effort: if the receiver is already gone we still return the inner result.
    let _ = ui.send(AgentUpdate::TurnComplete).await;
    result
}

/// Inner loop — separated so `run_agent_turn` can send `TurnComplete` regardless
/// of whether this returns `Ok` or `Err`.
// Same 9-arg turn context as `run_agent_turn` above; see that comment.
#[allow(clippy::too_many_arguments)]
async fn run_turn_inner(
    config: &TurnConfig,
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    gate: Arc<dyn ToolGate>,
    session: SessionHandle,
    mut events: Vec<Event>,
    model: String,
    ui: &mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<Vec<Event>> {
    let mut iterations: u32 = 0;

    'turn: loop {
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
        while let Some(pe) = prx.recv().await {
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
                    let _ = stream_task.await;
                    break 'turn;
                }
                ProviderEvent::Done => break,
            }
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
            break 'turn;
        }

        // Execute each pending tool in the configured working directory
        // (blocking work off the async runtime), recording its result as an event.
        let cwd_for_exec = config.cwd.clone();
        let mut pending_iter = pending.into_iter();
        while let Some(tc) = pending_iter.next() {
            if let Gate::Deny(reason) = gate.check(&tc) {
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
                continue;
            }

            let kind = tools.iter().find(|t| t.name() == tc.name).map(|t| t.kind());
            crate::zlog!("tool: name={:?} kind={:?}", tc.name, kind);

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
                        }
                        Err(msg) => {
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
                        }
                    }
                }
                Some(zoid_tools::ToolKind::Interactive) if tc.name == "ask_user" => {
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
                    let (rtx, rrx) = oneshot::channel::<Answer>();
                    crate::zlog!(
                        "ask_user: intercepted, sending AskUser (choices={})",
                        choices.len()
                    );
                    let sent = ui
                        .send(AgentUpdate::AskUser {
                            question,
                            choices,
                            reply: rtx,
                        })
                        .await;
                    crate::zlog!("ask_user: send result ok={}, awaiting reply", sent.is_ok());
                    let ans = rrx.await;
                    crate::zlog!("ask_user: reply received ok={}", ans.is_ok());
                    match ans {
                        Ok(ans) => {
                            let output = match ans {
                                Answer::Choice(s) | Answer::FreeText(s) => s,
                                Answer::LetYouDecide => "[let you decide]".to_string(),
                            };
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output,
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                        Err(_) => {
                            // Sender dropped == Esc hard-abort: balanced result, end the turn.
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: "[user aborted]".to_string(),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
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
                            break 'turn;
                        }
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
                }
            }
        }
        record_compactions(&session, &mut events, ui, config, session_id, now).await?;
        // loop: re-request with the tool results now in context
    }

    Ok(events)
}

/// Persist one event and announce it to the UI, keeping the local log in sync.
async fn emit(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    emit_with_tokens(session, events, ui, branch, kind, None, session_id, now).await
}

/// Record `ToolResultCompacted` events for any tool-results the policy says
/// should be compacted given the current log. Idempotent: `plan_compactions`
/// skips already-compacted ids, so calling this each round is safe.
async fn record_compactions(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    for c in zoid_core::compaction::plan_compactions(events, &config.policy) {
        emit(
            session,
            events,
            ui,
            &config.branch,
            EventKind::ToolResultCompacted {
                id: c.id,
                summary: c.summary,
                original_tokens: c.original_tokens,
            },
            session_id,
            now,
        )
        .await?;
    }
    Ok(())
}

/// Persist one event (optionally carrying token usage) and announce it to the
/// UI, keeping the local log in sync.
// 8 args: session, events, ui, branch, kind, tokens, session_id, clock — every
// one load-bearing for what/where this event is tagged and delivered.
#[allow(clippy::too_many_arguments)]
async fn emit_with_tokens(
    session: &SessionHandle,
    events: &mut Vec<Event>,
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
    fn build_request_uses_the_given_system_prompt() {
        let req = build_request(&[], "m", &zoid_tools::registry(), "CUSTOM SYS");
        assert_eq!(req.system.as_deref(), Some("CUSTOM SYS"));
    }

    #[test]
    fn build_request_carries_compacted_summary_into_live_messages() {
        let ts = 1700000000i64;
        let events = vec![
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
        ];

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
}
