//! The terminal-free agent loop: stream a turn, execute any tool calls in the
//! working directory, record everything as events, and re-request until the
//! model stops calling tools (or the iteration cap trips).

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use ulid::Ulid;

use zoid_core::event::{Event, EventKind};
use zoid_core::projection::{conversation, ChatMsg};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, Message, Provider, ProviderEvent, ToolCall, ToolSpec};
use zoid_tools::Tool;

/// System prompt for Chat-mode turns.
pub const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     You can call tools to read, write, edit, and search files and run shell \
     commands in the user's working directory. Use them when helpful.";

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 25;

/// UI-facing updates emitted as the turn progresses.
pub enum AgentUpdate {
    /// A new event was persisted; the UI should cache it and redraw.
    Appended(Event),
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
        ChatMsg::User(text) => Message::user(text),
        ChatMsg::Assistant { text, tool_calls } => Message {
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
    }
}

/// Build a completion request from the current event log.
pub fn build_request(events: &[Event], model: &str, tools: &[Box<dyn Tool>]) -> CompletionRequest {
    CompletionRequest {
        model: model.to_string(),
        system: Some(SYSTEM_PROMPT.to_string()),
        messages: conversation(events).into_iter().map(map_msg).collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}

/// Run one user-message-to-completion agent turn. `seed_events` is the current
/// log snapshot (including the just-appended user message). Every event this
/// produces is persisted via `session` and announced via `ui`.
pub async fn run_agent_turn(
    provider: Arc<dyn Provider>,
    tools: Arc<Vec<Box<dyn Tool>>>,
    session: SessionHandle,
    mut events: Vec<Event>,
    model: String,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
) -> Result<()> {
    let mut iterations: u32 = 0;

    'turn: loop {
        let req = build_request(&events, &model, &tools);

        // Stream one model turn. Spawn the provider so a missing terminal Done
        // (truncated stream) can't hang us — we send our own Done after it ends.
        let (ptx, mut prx) = mpsc::channel::<ProviderEvent>(256);
        let p = provider.clone();
        let stream_task = tokio::spawn(async move {
            let _ = p.stream(&req, ptx.clone()).await;
            let _ = ptx.send(ProviderEvent::Done).await;
        });

        let mut pending: Vec<ToolCall> = Vec::new();
        while let Some(pe) = prx.recv().await {
            match pe {
                ProviderEvent::TextDelta(s) => {
                    emit(&session, &mut events, &ui, EventKind::ModelDelta { text: s }, now).await?;
                }
                ProviderEvent::ToolCall(tc) => {
                    emit(
                        &session,
                        &mut events,
                        &ui,
                        EventKind::ToolCall { id: tc.id.clone(), name: tc.name.clone(), args: tc.args.to_string() },
                        now,
                    )
                    .await?;
                    pending.push(tc);
                }
                ProviderEvent::Usage(_) => { /* token ledger lands in P3 */ }
                ProviderEvent::Error(msg) => {
                    emit(
                        &session,
                        &mut events,
                        &ui,
                        EventKind::AssistantMessage {
                            text: format!("{} {msg}", zoid_tui::tokens::glyph::WARNING),
                        },
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

        if pending.is_empty() {
            break 'turn; // model answered without tools — turn complete
        }

        iterations += 1;
        if iterations > MAX_TOOL_ITERATIONS {
            emit(
                &session,
                &mut events,
                &ui,
                EventKind::AssistantMessage {
                    text: format!("{} tool-iteration limit reached", zoid_tui::tokens::glyph::WARNING),
                },
                now,
            )
            .await?;
            break 'turn;
        }

        // Execute each pending tool in the working directory (blocking work off
        // the async runtime), recording its result as an event.
        for tc in pending {
            let tools_for_exec = tools.clone();
            let name = tc.name.clone();
            let args = tc.args.clone();
            let out = tokio::task::spawn_blocking(move || {
                zoid_tools::run_tool(&tools_for_exec, &name, &args)
            })
            .await?;
            emit(
                &session,
                &mut events,
                &ui,
                EventKind::ToolResult { id: tc.id, name: tc.name, output: out.text, is_error: out.is_error },
                now,
            )
            .await?;
        }
        // loop: re-request with the tool results now in context
    }

    let _ = ui.send(AgentUpdate::TurnComplete).await;
    Ok(())
}

/// Persist one event and announce it to the UI, keeping the local log in sync.
async fn emit(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    kind: EventKind,
    now: fn() -> i64,
) -> Result<()> {
    let ev = Event::new(Ulid::new(), None, now(), kind);
    session.append(ev.clone()).await?;
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(ev)).await;
    Ok(())
}
