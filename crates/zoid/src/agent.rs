//! The terminal-free agent loop: stream a turn, execute any tool calls in the
//! working directory, record everything as events, and re-request until the
//! model stops calling tools (or the iteration cap trips).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::event::{BranchId, Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider, ProviderEvent, ToolCall, ToolSpec};
use zoid_tools::Tool;

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
}

/// The orchestrator (Chat) turn config: main branch, process cwd, Chat prompt.
pub fn chat_turn_config() -> TurnConfig {
    TurnConfig {
        system: SYSTEM_PROMPT.to_string(),
        cwd: PathBuf::from("."),
        branch: BranchId::default(),
    }
}

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 25;

/// UI-facing updates emitted as the turn progresses.
pub enum AgentUpdate {
    /// A new event was persisted; the UI should cache it and redraw.
    Appended(Box<Event>),
    /// The turn is finished (model produced no further tool calls / cap / error).
    TurnComplete,
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
    session: SessionHandle,
    events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<Vec<Event>> {
    let result = run_turn_inner(
        &config, provider, tools, session, events, model, &ui, session_id, now,
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
        for tc in pending {
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
}
