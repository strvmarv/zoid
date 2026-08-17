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
use zoid_core::projection::ChatMsg;
use zoid_core::session::SessionHandle;
use zoid_provider::{
    CompletionRequest, Message, Provider, ProviderEvent, ThinkingMode, ToolCall, ToolSpec,
};
use zoid_tools::{Gate, Tool, ToolGate};

/// Warning glyph used in agent-generated error messages; avoids a TUI-layer dep.
/// `pub(crate)` so `subagent.rs` can detect a failed subagent from the same
/// source of truth rather than a drifting sentinel of its own.
pub(crate) const WARN_GLYPH: char = '⚠';

/// Multiplier for how many vector recall candidates to fetch beyond the caller's
/// `limit`. The in-memory embedding index is session-agnostic, so some hits
/// belong to other sessions and are dropped by the per-session filter; fetching
/// a multiple keeps enough session-matching hits to fill `limit`. 4× covers
/// roughly even dilution across ~4 active sessions; beyond that the vector
/// contribution thins and recall leans on the (fully session-scoped) FTS side —
/// a graceful quality taper, not a correctness cliff. Picked, not tuned.
const VECTOR_OVERFETCH: usize = 4;

/// System prompt for Chat-mode turns.
pub const SYSTEM_PROMPT: &str =
    "You are zoid, a terminal coding assistant. Be concise and precise. \
     You can call tools to read, write, edit, and search files and run shell \
     commands in the user's working directory. Use them when helpful. \
     Brief single-line narration alongside tool calls is good. But when a task \
     is done, do NOT reframe or re-explain the whole effort in long paragraphs: \
     close with a short recap — a few lines or a tight list of what changed and \
     any next step. Don't restate what the tool calls and diffs already showed. \
     Subagents are fire-and-forget: dispatch, then end your turn and await the \
     DelegationResult event — never poll for status or call list_subagents to \
     check on a subagent you dispatched. When waiting on something, schedule \
     exactly one wake — never schedule duplicate wakes for the same event, and \
     cancel a pending wake before scheduling a replacement. \
     If the working directory has an AGENTS.md file, read it before touching \
     anything — it carries project-specific rules (test commands, release \
     flow, constraints) you must follow.";

/// Wrap the system prompt as a standing, tail-injected reminder. The pre/post
/// framing is the only added text; `system` is verbatim (zero drift). The
/// "NOT a signal that anything is complete" clause guards against a mid-loop
/// re-floor being misread as "the task is done now".
pub fn wrap_reassertion(system: &str) -> String {
    format!(
        "[Standing reminder — your operating instructions below are still in \
         effect. This is a periodic re-statement, NOT a change of task and NOT \
         a signal that anything is complete. Do not alter what you are doing in \
         response to seeing this; continue the current work and keep following \
         these instructions:]\n\n{system}\n\n[End of reminder — resume the task in progress.]"
    )
}

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

/// Why a subagent was aborted. Shared across all three firers (timeout supervisor,
/// kill tool, Esc) via a single first-writer-wins slot in `SubagentHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// No-progress (idle) timeout tripped.
    IdleTimeout,
    /// Absolute wall-clock ceiling tripped.
    Ceiling,
    /// Cancelled by the orchestrator kill tool or the user's Esc.
    Killed,
}

impl AbortReason {
    /// Short human label for the failure summary.
    pub fn label(&self) -> &'static str {
        match self {
            AbortReason::IdleTimeout => "idle timeout",
            AbortReason::Ceiling => "hard timeout",
            AbortReason::Killed => "killed",
        }
    }
}

/// Live handle to one in-flight subagent: how to stop it and how it reports why.
/// Held in the registry (`App.in_flight` / `TurnConfig.in_flight`) so the timeout
/// supervisor, the kill tool, and Esc can all reach the same tokens.
#[derive(Clone)]
pub struct SubagentHandle {
    /// Graceful cancel (reserved; parity with the main turn).
    pub cancel: CancellationToken,
    /// Force-kill this subagent (drains + kills its shell via the turn loop).
    pub hard: CancellationToken,
    /// Heartbeat: last-progress epoch ms, bumped per iteration by the turn loop.
    pub progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// First-writer-wins abort reason, set by whichever firer trips first.
    pub abort_reason: std::sync::Arc<std::sync::Mutex<Option<AbortReason>>>,
    /// The task description passed to `dispatch_subagent`. Used by
    /// `list_subagents` to show what each running subagent is doing.
    pub task: String,
    /// The agent profile name used for this subagent (e.g. "delegate").
    pub agent: String,
}

/// Fire the `hard` token (and record `Killed`, first-writer-wins) for one
/// subagent by id, or for ALL in-flight subagents when `target` is `None`.
/// Returns how many handles were fired. Shared by the `cancel_subagent` tool
/// handler and the Esc escalation path.
pub fn fire_subagent_kill(
    reg: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SubagentHandle>>>,
    target: Option<&str>,
) -> usize {
    let reg = reg.lock().unwrap();
    let handles: Vec<&SubagentHandle> = match target {
        Some(id) => reg.get(id).into_iter().collect(),
        None => reg.values().collect(),
    };
    let mut fired = 0usize;
    for h in handles {
        {
            let mut slot = h.abort_reason.lock().unwrap();
            if slot.is_none() {
                *slot = Some(AbortReason::Killed);
            }
        }
        h.hard.cancel();
        fired += 1;
    }
    fired
}

/// Format the `list_subagents` tool output from the in-flight registry. Pure
/// function shared by the agent-loop arm and the unit test so the test
/// exercises the real formatting (including the no-poll reminder) rather than
/// a duplicated reconstruction. Empty registry → a plain "no subagents" line
/// with no reminder; non-empty → one line per subagent + a fire-and-forget
/// reminder appended to weaken the poll-reward loop.
fn format_subagent_list(map: &std::collections::HashMap<String, SubagentHandle>) -> String {
    if map.is_empty() {
        return "No subagents currently running.".to_string();
    }
    let mut lines = format!("Running subagents ({}):\n", map.len());
    for (id, handle) in map.iter() {
        let agent = if handle.agent.is_empty() {
            "delegate"
        } else {
            &handle.agent
        };
        lines.push_str(&format!("- {id} [{agent}]: {}\n", handle.task));
    }
    lines.push_str(
        "\nReminder: subagents are fire-and-forget. You will be re-invoked with \
         each result automatically — do not poll or call this tool repeatedly \
         to check progress. End your turn and await the DelegationResult.",
    );
    lines.trim_end().to_string()
}

/// How one agent turn is run: its system prompt, working directory, and the
/// event branch its output is recorded on. Chat uses the main branch + process
/// cwd; a subagent uses its own branch + (optionally) a worktree.
#[derive(Clone)]
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
    /// Context-management policy for this turn. Chat gets it from `[economy]`;
    /// subagents get `subagent_policy()`. Drives automatic tool-result compaction.
    pub policy: zoid_core::assembler::ContextPolicy,
    /// Live eviction band parameters. `disabled()` for subagents/tests.
    pub eviction: zoid_core::eviction::EvictionPolicy,
    /// Connected MCP servers whose tools this turn may call. `None` for
    /// subagents and tests (no MCP). Carried here (not as a fn parameter) so
    /// the turn-function signatures are unchanged.
    pub mcp: Option<std::sync::Arc<zoid_mcp::McpManager>>,
    /// In-memory embedding index for hybrid recall (None = FTS-only). Present
    /// only when built with `local-embed` and `[embed] enabled = true`.
    pub embed: Option<std::sync::Arc<std::sync::RwLock<zoid_core::embed_index::EmbeddingIndex>>>,
    /// The embedder used to embed the recall query. Paired with `embed`.
    pub embedder: Option<std::sync::Arc<dyn zoid_core::retrieval::Embedder>>,
    /// Thinking mode for this turn. Resolved from config + model capability
    /// in spawn_turn. Defaults to Off.
    pub thinking: ThinkingMode,
    /// Approval config for gate selection. Subagents use the blacklist with
    /// interactive=false (auto-deny) unless yolo.
    pub approval: zoid_core::config::ApprovalConfig,
    /// Shared kill slot for the `shell` tool's process group. A hard-stop
    /// SIGKILLs whatever pgid the running shell published here. Defaults to a
    /// fresh (unshared) slot for subagents/tests; the chat turn shares the same
    /// slot given to the chat tool list (see spawn_turn).
    pub kill: zoid_tools::KillSlot,
    /// Hard cap on tool-call sub-turns. The main chat loop uses
    /// MAX_TOOL_ITERATIONS (1000); subagents override this to a tighter bound
    /// so a confused headless agent stops fast. None = MAX_TOOL_ITERATIONS.
    pub max_iterations: Option<u32>,
    /// Shared in-flight subagent registry (id → live handle) for the
    /// sequential-dispatch guard and the guardrail firers. None when
    /// dispatch_subagent is disabled or for subagent turns.
    pub in_flight:
        Option<std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SubagentHandle>>>>,
    /// Token budget between system-prompt re-assertions (0 = disabled).
    /// Chat gets it from `[economy]`; subagents/tests default off.
    pub reassert_interval: u64,
    /// Heartbeat slot the turn loop bumps each iteration so a subagent's
    /// `WakeTimer` can detect a stalled `await`. `None` for the main chat turn.
    pub progress: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// Idle-timeout for a subagent dispatched from THIS turn (chat only). `None`
    /// = disabled. Consumed at the dispatch site; ignored by subagent turns.
    pub subagent_idle: Option<std::time::Duration>,
    /// Absolute-ceiling for a subagent dispatched from THIS turn. `None` = off.
    pub subagent_ceiling: Option<std::time::Duration>,
    /// The agent profile registry for `dispatch_subagent` name resolution.
    /// `None` for subagent turns (subagents can't dispatch) and tests.
    pub agents: Option<std::sync::Arc<zoid_core::agent_profile::AgentRegistry>>,
    /// The model's actual context window (tokens). Used by the hard-ceiling
    /// compaction pass in `preflight_gate` — the live-fetched value (from
    /// `fetch_model_info` / `ModelInfoFetched`), not the static table's
    /// conservative default. 0 = unknown → hard-ceiling pass is skipped.
    pub context_window: u64,
    /// Max concurrent subagents (global pool). 0 = unlimited. Default 3.
    /// Wired in `spawn_turn` from `app.config.subagent.max_concurrent` so the
    /// pool check in `dispatch_subagent` honors the user's config.
    pub max_concurrent: usize,
    /// The merged registry, used to resolve per-(provider, model) caps for this
    /// turn AND for any subagent it dispatches. `Registry::default()` (empty) in
    /// tests → conservative 32k fallback.
    pub reg: std::sync::Arc<zoid_model::Registry>,
    /// The provider id this turn runs under (e.g. "opencode-go"). Empty in tests.
    pub provider_id: String,
}

// Manual `Debug`: `embed`/`embedder` hold a trait object (`dyn Embedder`) and
// an index type that don't implement `Debug`, so they can't be part of a
// `#[derive(Debug)]`. Every other field is printed normally; those two are
// summarized as present/absent, matching how `mcp` would render.
impl std::fmt::Debug for TurnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnConfig")
            .field("system", &self.system)
            .field("cwd", &self.cwd)
            .field("branch", &self.branch)
            .field("policy", &self.policy)
            .field("eviction", &self.eviction)
            .field("mcp", &self.mcp.is_some())
            .field("embed", &self.embed.is_some())
            .field("embedder", &self.embedder.is_some())
            .field("thinking", &self.thinking)
            .field("approval", &self.approval)
            .field("kill", &self.kill)
            .field("max_iterations", &self.max_iterations)
            .field("in_flight", &self.in_flight.is_some())
            .field("reassert_interval", &self.reassert_interval)
            .field("progress", &self.progress.is_some())
            .field("subagent_idle", &self.subagent_idle)
            .field("subagent_ceiling", &self.subagent_ceiling)
            .field("agents", &self.agents.is_some())
            .field("reg", &self.reg)
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

/// Resolve the `agent` argument from a `dispatch_subagent` tool call against the
/// registry. Absent/empty `agent` defaults to `"delegate"`. Returns the cloned
/// `AgentProfile` to dispatch with, or an `Err` (listing available agents) for an
/// unknown name so the dispatch site can emit a self-correcting ToolResult.
pub fn resolve_agent_for_dispatch(
    args: &serde_json::Value,
    registry: std::sync::Arc<zoid_core::agent_profile::AgentRegistry>,
) -> Result<(zoid_core::agent_profile::AgentProfile, String), String> {
    let agent_name = args
        .get("agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("delegate")
        .to_string();
    match registry.get(&agent_name) {
        Some(profile) => Ok((profile.clone(), agent_name)),
        None => Err(format!(
            "dispatch_subagent: unknown agent '{agent_name}'. Available: {}",
            registry.names().join(", ")
        )),
    }
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
        mcp: None,
        embed: None,
        embedder: None,
        thinking: ThinkingMode::Off,
        approval: zoid_core::config::ApprovalConfig::default(),
        kill: zoid_tools::KillSlot::new(),
        max_iterations: None,
        in_flight: None,
        reassert_interval: 0,
        progress: None,
        subagent_idle: None,
        subagent_ceiling: None,
        agents: None,
        context_window: 0,
        max_concurrent: 3,
        reg: std::sync::Arc::new(zoid_model::Registry::default()),
        provider_id: String::new(),
    }
}

/// The default Chat turn config: the `default_profile()` with no skill menu.
/// Kept zero-arg for the many callers (tests) that don't exercise modes;
/// byte-identical to the pre-mode behavior.
pub fn chat_turn_config() -> TurnConfig {
    chat_turn_config_with(&default_profile(), "")
}

/// Max tool rounds per user message before the loop force-ends (safety leash).
pub const MAX_TOOL_ITERATIONS: u32 = 1000;
/// Bound on the capacity-error retry (Task 1.7): the hard-bound backstop when
/// the pre-flight estimate under-reads reality and the provider still rejects
/// the request as too large. Each retry forces an eviction wave before
/// re-sending, so this also bounds the number of forced eviction waves per turn.
pub const MAX_CONTEXT_RETRIES: u32 = 3;

/// Ensure a tool call has a stable, unique internal id. Some providers
/// (Ollama native) emit tool calls with an empty id; propagating "" would make
/// every tool result share one id and collide in id-keyed machinery
/// (cumulative_appended, projection, compaction, eviction). Synthesize a unique
/// Ulid when empty; internal only — not sent on the provider wire.
pub(crate) fn ensure_tool_call_id(id: String) -> String {
    if id.is_empty() {
        Ulid::new().to_string()
    } else {
        id
    }
}

/// The user's answer to an `ask_user` prompt.
#[allow(clippy::large_enum_variant)]
pub enum Answer {
    Choice(String),
    FreeText(String),
    /// The user chose to let the agent decide (a positive choice, not a cancel).
    LetYouDecide,
    /// The `submit_feedback` tool's confirmed report (built by the bin from
    /// the edited `FeedbackState` + diagnostics). Carries the report back to
    /// the loop so it can submit without shared state.
    Feedback(zoid_core::feedback::FeedbackReport),
}

/// Reply payload for a worktree relocation: the new absolute cwd + optional
/// warning (or an error string). Shared by `WorktreeRequested` and
/// `handle_worktree_request`.
pub type WorktreeReply =
    tokio::sync::oneshot::Sender<Result<(std::path::PathBuf, Option<String>), String>>;

/// A request from the `enter_worktree` / `exit_worktree` Emitting tools (or
/// the `:worktree` user commands) to relocate the session cwd. Ephemeral —
/// travels via `AgentUpdate`, never persisted to SQLite (spec: chat-worktree-
/// design, "Signal type").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeAction {
    /// Create and enter a worktree named `name`.
    Enter { name: String },
    /// Exit the current worktree, restoring the prior cwd.
    Exit,
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
    /// A subagent was dispatched (via the dispatch_subagent tool). The UI tracks
    /// it as in-flight until its DelegationResult arrives.
    SubagentStarted {
        id: String,
        task: String,
        agent: String,
    },
    /// A `dispatch_subagent` call was queued because the global pool is full.
    /// Carries everything the main loop needs to spawn the subagent once a
    /// slot opens (the resolved profile, the parent's cwd for worktree
    /// creation, and the `want_worktree` flag parsed at dispatch time). The
    /// main loop pushes this onto `App.queued_subagents` and drains it on
    /// each `DelegationResult`.
    SubagentQueued {
        tool_call_id: String,
        task: String,
        agent: String,
        resolved_profile: zoid_core::agent_profile::AgentProfile,
        resolved_name: String,
        want_worktree: bool,
        cwd: PathBuf,
    },
    /// An ephemeral, UI-only diff for an edit/write tool call. Carries the
    /// computed `FileDiff` to the TUI's in-memory cache; never persisted and
    /// never sent to the model. Keyed by tool-call id.
    EditDiff {
        id: String,
        diff: zoid_tools::FileDiff,
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
    /// Automated compaction is running (before a burst of ToolResultCompacted events).
    CompactionStarted,
    /// Automated compaction finished (after the burst).
    CompactionComplete,
    /// A re-floor fired: the system prompt was re-asserted at the live edge.
    DirectiveReasserted { at_cumulative: u64 },
    /// The current session was taken over by another instance (the heartbeat
    /// detected another process claimed the row). The bin cancels the in-flight
    /// turn, sets `yielded`, and surfaces a hint. Spec §2.4. Bin-only — subagents
    /// never heartbeat and never emit this.
    SessionTakenOver,
    /// A feedback submit finished; the bin updates the overlay's status line.
    FeedbackOutcome(anyhow::Result<zoid_core::feedback::SubmitOutcome>),
    /// A completed plugin fetch, ready to materialize on the main loop.
    PluginScan {
        id: String,
        origin: String,
        /// `--mode`/`--skills` override from the `:plugin install` invocation,
        /// carried across the async fetch so `apply_plugin_scan` can flip the
        /// freshly-resolved manifest's kind before `build_plan`.
        over: crate::plugin_install::KindOverride,
        /// The resolved manifest folded into `Ok` alongside the scan: the
        /// catalog manifest is fetched/parsed/validated inside the spawned
        /// task, so a manifest-stage failure must be representable as
        /// `Err(String)` and travel back through the SAME message that
        /// clears the `installing_plugin` guard.
        res: Result<
            (
                zoid_plugin::manifest::PluginManifest,
                zoid_core::wizard::UpstreamScan,
            ),
            String,
        >,
    },
    /// The agent (or user via `:worktree`) requested a worktree relocation.
    /// `reply` carries the new absolute cwd + optional warning (or an error)
    /// back to the awaiting turn so its in-flight tool execution repoints
    /// atomically (WT-1/WT-2). The warning is non-None when a branch with
    /// unmerged commits is retained on exit.
    WorktreeRequested {
        action: WorktreeAction,
        reply: WorktreeReply,
    },
    /// The wake watcher's timer elapsed; the main loop should drain any wakes
    /// whose `fire_at_ms <= now` (inject if idle, else defer to TurnComplete).
    WakeDue,
    /// `schedule_wake` request → main loop validates + persists; replies Ok(wake_id) or Err(msg).
    ScheduleWake {
        delay_secs: u64,
        note: String,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    /// `cancel_wake` request → main loop cancels one/all; replies Ok(summary) or Err(msg).
    CancelWake {
        id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    /// The async plugin catalog index load finished (fresh cache, network
    /// fetch, or a stale-cache fallback). Drives both `:plugin catalog`
    /// (populates the overlay) and `:plugin list` (prints rows once resolved;
    /// no blocking fetch on the main loop either way).
    CatalogLoaded(Result<Vec<crate::catalog::CatalogEntry>, String>),
    /// A confirm-time fetch of an mcp plugin's `<id>.toml` finished. Tagged with
    /// `id` so a stale fetch (user navigated to another row) is dropped — same
    /// stale-drop discipline as `ModelsFetched`/`PluginScan`. Populates the
    /// already-open confirm; it does NOT install.
    McpManifestFetched {
        id: String,
        res: Result<zoid_plugin::manifest::PluginManifest, String>,
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
            // Approval-gate questions are UI-only: the real ToolResult
            // carries the tool's output. Map the card to an inert assistant
            // message so the model doesn't see a spurious tool result with
            // the approval string as its content.
            if matches!(kind, zoid_core::event::QuestionKind::Approval) {
                return Message {
                    role: zoid_provider::MsgRole::Assistant,
                    content: String::new(),
                    tool_calls: vec![],
                    tool_name: None,
                    tool_call_id: None,
                };
            }
            let tool_name = match kind {
                zoid_core::event::QuestionKind::Ask => "ask_user",
                zoid_core::event::QuestionKind::ModeMapping { .. } => "apply_mode_mapping",
                zoid_core::event::QuestionKind::Approval => "shell",
                zoid_core::event::QuestionKind::Feedback { .. } => "submit_feedback",
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
        ChatMsg::Evicted { .. } => Message {
            // Defense-in-depth: build_request_with_thinking filters Evicted out
            // before map_msg runs, so this arm should never fire in production.
            // Emit an inert assistant message in case a future caller forgets the
            // filter — never a tool-result, which would violate alternation.
            role: zoid_provider::MsgRole::Assistant,
            content: String::new(),
            tool_calls: vec![],
            tool_name: None,
            tool_call_id: None,
        },
    }
}

/// Build a completion request from the current event log.
pub fn build_request(
    events: &crate::eventlog::EventLog,
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
    reassert: Option<String>,
) -> CompletionRequest {
    build_request_with_thinking(
        events,
        model,
        zoid_provider::model::model_info(model),
        tools,
        system,
        ThinkingMode::Off,
        reassert,
        &zoid_core::event::BranchId::default(),
    )
}

pub fn build_request_with_thinking(
    events: &crate::eventlog::EventLog,
    model: &str,
    model_info: zoid_provider::model::ModelInfo,
    tools: &[Box<dyn Tool>],
    system: &str,
    thinking: ThinkingMode,
    reassert: Option<String>,
    active_branch: &zoid_core::event::BranchId,
) -> CompletionRequest {
    let system = match zoid_core::eviction::eviction_breadcrumb(events.iter()) {
        Some(bc) => format!("{system}\n\n{bc}"),
        None => system.to_string(),
    };
    let max_tokens = match thinking {
        ThinkingMode::Off => {
            // Even with thinking disabled in the request, thinking-capable models
            // may produce internal reasoning tokens that count against the output
            // budget. A 4096 budget can be exhausted by reasoning before the tool
            // call JSON completes, producing truncated/malformed arguments. Bump
            // the budget for thinking-capable models.
            if model_info.thinking != zoid_provider::model::ThinkingSupport::None {
                8192
            } else {
                4096
            }
        }
        ThinkingMode::Auto | ThinkingMode::Effort(_) => {
            if model_info.max_output > 0 {
                (model_info.max_output as u32).min(16384)
            } else {
                16384
            }
        }
    };
    CompletionRequest {
        model: model.to_string(),
        model_info,
        system: Some(system),
        messages: zoid_core::projection::conversation_for_branch(events.iter(), active_branch)
            .into_iter()
            .filter(|m| !matches!(m, zoid_core::projection::ChatMsg::Evicted { .. }))
            .map(map_msg)
            .collect(),
        max_tokens,
        tools: tool_specs(tools),
        thinking,
        reassert,
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
        CancellationToken::new(), // graceful (never fires here)
        CancellationToken::new(), // hard (never fires here)
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
    hard: CancellationToken,
) -> Result<crate::eventlog::EventLog> {
    // Calibration ratio: real_input_tokens / context_window.total_tokens from
    // the last non-cached sub-turn. The chars/4 estimate undercounts 5-7x for
    // code/tool output, so on cache-hit turns (where the Ollama provider
    // reconstructs input from the previous sub-turn's size) we scale the
    // current estimate by this ratio to approximate the real context size.
    // Updated on every sub-turn where the provider reports a non-zero input.
    // Mutable, lives for the turn (across sub-turns).
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
        &hard,
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
    hard: &CancellationToken,
) -> Result<crate::eventlog::EventLog> {
    let turn_start = std::time::Instant::now();
    let mut iterations: u32 = 0;
    let mut context_retries: u32 = 0;
    let mut outcome: &'static str = "completed";
    // Whether the model produced any user-visible output (streamed text or a
    // tool call) at any point this turn. When a turn ends cleanly having produced
    // nothing — the signature of an empty upstream completion (e.g. a degraded
    // model returning 200 with `content:""` and `done_reason:"stop"`) — we surface
    // a ⚠ message instead of ending silently and leaving the UI to snap back to
    // idle with no explanation.
    let mut turn_produced_content = false;
    // Working directory for tool execution. TURN-scoped on purpose, not
    // sub-turn-scoped: `enter_worktree`/`exit_worktree` repoint this mid-turn
    // and the relocation must survive the sub-turn boundary. Re-initializing it
    // from the frozen `config.cwd` snapshot on every sub-turn handed the shell a
    // DELETED worktree path after `exit_worktree` (ENOENT — `current_dir` chdir's
    // before exec); see tests/worktree_wt2_exit_cwd.rs. Cross-TURN state is
    // carried by the main loop's `spawn_turn`, which rebuilds `config.cwd` from
    // `app.active_worktree` — that path only runs between turns, never between
    // sub-turns, which is the gap this scoping closes.
    let mut cwd_for_exec = config.cwd.clone();

    'turn: loop {
        // Cancelled between sub-turns: nothing is pending here, so end cleanly.
        if cancel.is_cancelled() || hard.is_cancelled() {
            outcome = "aborted";
            break 'turn;
        }
        // Decide re-floor FIRST so its ephemeral tokens are in the preflight size
        // (spec: re-floor, S2/S3 ordering — decide → size → preflight → build).
        let will_reassert = config.reassert_interval > 0
            && zoid_core::reassert::reassertion_due(events.iter(), config.reassert_interval);
        let reassert_text = will_reassert.then(|| wrap_reassertion(&config.system));

        let mut overhead_now = overhead.clone();
        if let Some(t) = &reassert_text {
            overhead_now.system_tokens += zoid_core::economy::estimate_tokens(t);
        }

        // PRE-FLIGHT GATE (spec §3.8): shrink to fit BEFORE building the request.
        let model_ctx = config.context_window;
        preflight_gate(
            &session,
            &mut events,
            ui,
            config,
            session_id,
            now,
            &*calibration_ratio,
            &overhead_now,
            model_ctx,
        )
        .await?;
        let model_info = config.reg.model_info(&config.provider_id, &model);
        let req = build_request_with_thinking(
            &events,
            &model,
            model_info,
            &tools,
            &config.system,
            config.thinking,
            reassert_text.clone(),
            &config.branch,
        );

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
        let mut thinking_buf: String = String::new();
        let mut aborted = false;
        loop {
            let pe = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    aborted = true;
                    break;
                }
                _ = hard.cancelled() => {
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
                    turn_produced_content = true;
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
                ProviderEvent::ToolCall(mut tc) => {
                    tc.id = ensure_tool_call_id(std::mem::take(&mut tc.id));
                    turn_produced_content = true;
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
                    turn_usage.thinking += u.thinking_tokens;
                }
                ProviderEvent::Error(msg) => {
                    let _ = stream_task.await;
                    if zoid_provider::is_context_length_error(&msg)
                        && context_retries < MAX_CONTEXT_RETRIES
                        && config.eviction.enabled
                    {
                        context_retries += 1;
                        // The estimate under-read reality: force a wave toward low_water and retry.
                        // Use the same scaled estimate as preflight_gate (raw ×
                        // calibration_ratio × OVERCOUNT_BIAS) and pass the scale so
                        // plan_evictions' reclaimed accumulator matches.
                        let raw = zoid_core::context::context_window_with(
                            events.iter(),
                            overhead.clone(),
                        )
                        .total_tokens;
                        let scale = match *calibration_ratio {
                            Some(r) if r > 0.0 => r * OVERCOUNT_BIAS,
                            _ => OVERCOUNT_BIAS,
                        };
                        let est = (raw as f64 * scale) as u64;
                        let plan = zoid_core::eviction::plan_evictions(
                            events.iter(),
                            &config.eviction,
                            est,
                            &zoid_core::eviction::RecencyScorer,
                            &zoid_core::eviction::GoalContext::default(),
                            scale,
                        );
                        emit_eviction(&session, &mut events, ui, config, session_id, now, plan)
                            .await?;
                        tracing::warn!(ctx = "provider", "context-length error; forced eviction, retrying ({context_retries}/{MAX_CONTEXT_RETRIES})");
                        log_turn_warn(&session, "warn", session_id, &format!("context-length error; forced eviction, retrying ({context_retries}/{MAX_CONTEXT_RETRIES})"), None).await;
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
                    log_turn_warn(&session, "warn", session_id, &msg, None).await;
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
                ProviderEvent::ThinkingDelta(s) => {
                    thinking_buf.push_str(&s);
                }
                ProviderEvent::ThinkingSignature(_) => {
                    // Accumulated for future Phase 3 replay; not rendered or persisted.
                }
            }
        }
        if aborted {
            // Cancelled mid-stream: stop the provider task and balance any tool
            // calls parsed before the cancel so none is left without a
            // ToolResult (the provider protocol requires every call answered
            // before the next request).
            stream_task.abort();
            let _ = stream_task.await;
            thinking_buf.clear();
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

        // Marker only on a clean stream close: the context-length `continue
        // 'turn` above and the error/abort `break 'turn` paths all exit before
        // this point, so a rejected re-floor does not burn its interval.
        if will_reassert {
            let at = zoid_core::reassert::cumulative_appended(events.iter());
            emit(
                &session,
                &mut events,
                ui,
                &config.branch,
                EventKind::DirectiveReasserted { at_cumulative: at },
                session_id,
                now,
            )
            .await?;
            let _ = ui
                .send(AgentUpdate::DirectiveReasserted { at_cumulative: at })
                .await;
            tracing::info!(kind = "reassert", at, "re-floor fired");
        }

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
            will_reassert,
        )
        .await?;

        if pending.is_empty() {
            // Final answer: flush reasoning as ephemeral ModelThinking.
            if !thinking_buf.is_empty() {
                emit_ephemeral(
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::ModelThinking {
                        text: std::mem::take(&mut thinking_buf),
                    },
                    session_id,
                    now,
                )
                .await?;
            }
            // The turn ended without ever producing streamed text or a tool call:
            // an empty upstream completion. Surface it so the UI doesn't just snap
            // back to idle with no explanation (spec: no silent turns).
            if !turn_produced_content {
                emit(
                    &session,
                    &mut events,
                    ui,
                    &config.branch,
                    EventKind::AssistantMessage {
                        text: format!(
                            "{WARN_GLYPH} model returned an empty response — the provider sent no \
                             content. The upstream model may be degraded; try again or switch models."
                        ),
                    },
                    session_id,
                    now,
                )
                .await?;
                outcome = "empty";
            }
            break 'turn; // model answered without tools — turn complete
        }

        // Intermediate sub-turn (tool calls): discard reasoning — it helped
        // the model select tools but isn't useful to the user and would
        // clutter the history + context.
        thinking_buf.clear();

        iterations += 1;
        if let Some(p) = &config.progress {
            p.store(now(), std::sync::atomic::Ordering::Relaxed);
        }
        let cap = config.max_iterations.unwrap_or(MAX_TOOL_ITERATIONS);
        if iterations > cap {
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

        // Execute each pending tool in the turn's current working directory
        // (blocking work off the async runtime), recording its result as an event.
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

            match gate.check(&tc) {
                Gate::Allow => { /* fall through to dispatch below */ }
                Gate::Deny(reason) => {
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
                    log_turn_warn(&session, "warn", session_id, &reason_msg, None).await;
                    continue;
                }
                Gate::Prompt { question, choices } => {
                    // Reuse the ask_user park-and-await path: emit a
                    // QuestionAsked event, send AgentUpdate::AskUser, await
                    // the reply. Approve → fall through to dispatch. Deny →
                    // error ToolResult + continue. Esc → abort the turn.
                    // QuestionKind::Approval distinguishes this from ask_user
                    // so the projection does NOT suppress the real ToolResult
                    // — the model sees the tool's actual output, not the
                    // approval string.
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::QuestionAsked {
                            id: tc.id.clone(),
                            kind: zoid_core::event::QuestionKind::Approval,
                            question: question.clone(),
                            choices: choices.clone(),
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    let (rtx, rrx) = oneshot::channel::<Answer>();
                    let sent = ui
                        .send(AgentUpdate::AskUser {
                            question,
                            choices,
                            reply: rtx,
                        })
                        .await;
                    if sent.is_err() {
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output: "[user aborted]".to_string(),
                                is_error: true,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        outcome = "aborted";
                        break 'turn;
                    }
                    let ans = rrx.await;
                    let (output, is_error, approved) = match ans {
                        Ok(Answer::Choice(s)) => {
                            let approved = s == "approve once";
                            if approved {
                                (s, false, true)
                            } else {
                                (s, true, false)
                            }
                        }
                        Ok(Answer::FreeText(s)) => (s, false, true),
                        Ok(Answer::LetYouDecide) => ("[let you decide]".to_string(), false, true),
                        Ok(Answer::Feedback(_)) => (String::new(), true, false), // unreachable for approvals
                        Err(_) => ("[user aborted]".to_string(), true, false),
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
                    if !approved {
                        let is_aborted = is_error && output == "[user aborted]";
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
                        if is_aborted {
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
                        continue;
                    }
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "tool approved"
                    );
                }
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
                            log_turn_warn(&session, "warn", session_id, &tool_msg, None).await;
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

                    // FTS candidates (existing path) → ids.
                    let fts_events = session
                        .recall(query.clone(), session_id, limit)
                        .await
                        .unwrap_or_default();
                    let fts_ids: Vec<Ulid> = fts_events.iter().map(|e| e.id).collect();

                    // The in-memory index is session-agnostic, so a vector hit may
                    // belong to another session and get dropped by the session
                    // filter in `events_by_ids` below. Over-fetch vector candidates
                    // (and merge to the same larger bound) so enough survive the
                    // filter to still fill `limit` in a multi-session DB.
                    let vfetch = limit.saturating_mul(VECTOR_OVERFETCH).max(limit);

                    // Vector candidates — only when BOTH the index and embedder are present.
                    let vec_ids: Vec<Ulid> = match (&config.embed, &config.embedder) {
                        (Some(index), Some(emb)) => {
                            use zoid_core::retrieval::CandidateSource;
                            let vs =
                                zoid_core::retrieval::VectorSource::new(emb.clone(), index.clone());
                            let q = query.clone();
                            tokio::task::spawn_blocking(move || vs.candidates(&q, vfetch))
                                .await
                                .unwrap_or_default()
                                .into_iter()
                                .map(|c| c.event_id)
                                .collect()
                        }
                        _ => Vec::new(),
                    };

                    let hits: Vec<Event> = if vec_ids.is_empty() {
                        // Pure-FTS fast path, byte-identical to pre-feature behavior.
                        fts_events
                    } else {
                        let merged = zoid_core::hybrid::hybrid_recall(&fts_ids, &vec_ids, vfetch);
                        let mut evs = session
                            .events_by_ids(merged.clone(), session_id)
                            .await
                            .unwrap_or_default();
                        evs.sort_by_key(|e| {
                            merged
                                .iter()
                                .position(|id| *id == e.id)
                                .unwrap_or(usize::MAX)
                        });
                        // Session filter may have dropped cross-session vector hits;
                        // trim the session-present survivors back to the requested limit.
                        evs.truncate(limit);
                        evs
                    };
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
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "dispatch_subagent" => {
                    // Parse task + resolve agent profile + want_worktree BEFORE the
                    // pool check: the queue path needs all of them so the main loop
                    // can spawn without re-resolving (Task 4 Step 3). cwd is the
                    // parent's CURRENT `cwd_for_exec` (worktree created at spawn).
                    let task = tc
                        .args
                        .get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if task.trim().is_empty() {
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output: "dispatch_subagent: 'task' is required".into(),
                                is_error: true,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        continue;
                    }
                    let want_worktree = tc
                        .args
                        .get("worktree")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    // Resolve the agent profile by name (default "delegate").
                    let (profile, resolved_agent_name) = match &config.agents {
                        Some(reg) => match resolve_agent_for_dispatch(&tc.args, reg.clone()) {
                            Ok((p, name)) => (p, name),
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
                                continue;
                            }
                        },
                        // No registry available (subagent turn) → fall back to builtin.
                        None => (
                            zoid_core::agent_profile::AgentProfile::builtin(),
                            "delegate".to_string(),
                        ),
                    };
                    // Pool check: if the global in-flight set is at capacity, queue
                    // the dispatch instead of spawning. Returns a non-error "queued"
                    // tool result and signals the main loop to enqueue the subagent;
                    // the next `DelegationResult` drains the queue. `max_concurrent`
                    // of 0 means unlimited (never queue).
                    if let Some(set) = &config.in_flight {
                        let n = set.lock().unwrap().len();
                        if n >= config.max_concurrent && config.max_concurrent > 0 {
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    output: format!(
                                        "subagent queued ({n} running, max {})",
                                        config.max_concurrent
                                    ),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                            // Signal the main loop to queue the subagent. Carries
                            // the resolved profile + parent cwd so the main loop
                            // can spawn without re-resolving.
                            let _ = ui
                                .send(AgentUpdate::SubagentQueued {
                                    tool_call_id: tc.id.clone(),
                                    task: task.clone(),
                                    agent: resolved_agent_name.clone(),
                                    resolved_profile: profile.clone(),
                                    resolved_name: resolved_agent_name.clone(),
                                    want_worktree,
                                    cwd: cwd_for_exec.clone(),
                                })
                                .await;
                            continue;
                        }
                    }
                    let sub_ulid = Ulid::new();
                    let sub_id = format!("sub-{sub_ulid}");

                    // Notify the UI so it tracks the subagent as in-flight.
                    let _ = ui
                        .send(AgentUpdate::SubagentStarted {
                            id: sub_id.clone(),
                            task: task.clone(),
                            agent: resolved_agent_name.clone(),
                        })
                        .await;

                    let wt = if want_worktree && std::path::Path::new(".git").exists() {
                        crate::worktree::create_worktree(
                            std::path::Path::new("."),
                            &format!("sub-{sub_ulid}"),
                        )
                        .ok()
                    } else {
                        None
                    };
                    // A subagent without its own worktree inherits the parent's
                    // CURRENT cwd, not the turn's opening snapshot: after an
                    // `enter_worktree` earlier in this turn `config.cwd` still
                    // points at the main checkout (so the subagent's commits
                    // would land on the parent branch), and after an
                    // `exit_worktree` it points at a deleted directory.
                    let cwd = wt
                        .as_ref()
                        .map(|w| {
                            std::fs::canonicalize(w.path())
                                .unwrap_or_else(|_| w.path().to_path_buf())
                        })
                        .unwrap_or_else(|| cwd_for_exec.clone());

                    // Create the guardrail tokens + heartbeat for this subagent and
                    // register a handle BEFORE spawning, so a fast-completing
                    // subagent can't emit DelegationResult (which removes the ID)
                    // before we insert it. Tokens are created here but not fired
                    // until the WakeTimer + firers are wired (Task 4).
                    let sub_cancel = CancellationToken::new();
                    let sub_hard = CancellationToken::new();
                    let sub_progress =
                        std::sync::Arc::new(std::sync::atomic::AtomicI64::new(now()));
                    let sub_abort_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
                    if let Some(reg) = &config.in_flight {
                        reg.lock().unwrap().insert(
                            sub_id.clone(),
                            SubagentHandle {
                                cancel: sub_cancel.clone(),
                                hard: sub_hard.clone(),
                                progress: sub_progress.clone(),
                                abort_reason: sub_abort_reason.clone(),
                                task: task.clone(),
                                agent: resolved_agent_name.clone(),
                            },
                        );
                    }

                    crate::spawn_subagent::spawn_subagent(
                        task,
                        profile,
                        events.snapshot(),
                        provider.clone(),
                        cwd,
                        model.clone(),
                        config.thinking,
                        session.clone(),
                        session_id,
                        ui.clone(),
                        now,
                        sub_id.clone(),
                        wt,
                        config.approval.clone(),
                        sub_cancel,
                        sub_hard,
                        sub_progress,
                        sub_abort_reason,
                        config.subagent_idle,
                        config.subagent_ceiling,
                        config.reg.clone(),
                        config.provider_id.clone(),
                    );

                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: format!(
                                "{{\"subagent_id\": \"{sub_id}\"}} — Subagent {sub_id} is \
                                 running in isolation. You will be re-invoked automatically \
                                 with its result; do NOT call list_subagents or otherwise \
                                 check on it. End your turn now and await the result."
                            ),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "dispatch_subagent",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "subagent dispatched"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "enter_worktree" => {
                    let name = tc
                        .args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.trim().is_empty() {
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id,
                                name: tc.name,
                                output: "enter_worktree: 'name' is required".into(),
                                is_error: true,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        continue;
                    }
                    // Synchronous relocation: send a reply channel and await the
                    // main loop's new absolute cwd so THIS turn's subsequent tool
                    // calls run in the worktree (WT-1). No optimistic result.
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Enter { name: name.clone() },
                            reply: tx,
                        })
                        .await;
                    match rx.await {
                        Ok(Ok((new_cwd, _warn))) => {
                            cwd_for_exec = new_cwd;
                            emit(
                                &session,
                                &mut events,
                                ui,
                                &config.branch,
                                EventKind::ToolResult {
                                    id: tc.id,
                                    name: tc.name,
                                    output: format!("{{\"worktree\": \"{name}\"}}"),
                                    is_error: false,
                                },
                                session_id,
                                now,
                            )
                            .await?;
                        }
                        other => {
                            let msg = match other {
                                Ok(Err(m)) => m,
                                _ => "worktree switch failed (no reply)".to_string(),
                            };
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
                    tracing::info!(
                        kind = "tool",
                        name = "enter_worktree",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "worktree enter requested"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "exit_worktree" => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui
                        .send(AgentUpdate::WorktreeRequested {
                            action: WorktreeAction::Exit,
                            reply: tx,
                        })
                        .await;
                    match rx.await {
                        Ok(Ok((new_cwd, warn))) => {
                            cwd_for_exec = new_cwd;
                            let output = warn.unwrap_or_else(|| "exited worktree".into());
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
                        other => {
                            let msg = match other {
                                Ok(Err(m)) => m,
                                _ => "worktree exit failed (no reply)".to_string(),
                            };
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
                    tracing::info!(
                        kind = "tool",
                        name = "exit_worktree",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "worktree exit requested"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "schedule_wake" => {
                    let delay_secs = tc
                        .args
                        .get("delay_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let note = tc
                        .args
                        .get("note")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui
                        .send(AgentUpdate::ScheduleWake {
                            delay_secs,
                            note,
                            reply: tx,
                        })
                        .await;
                    let (output, is_error) = match rx.await {
                        Ok(Ok(id)) => (
                            format!(
                                "scheduled (id {id}) — do not schedule additional \
                             wakes for the same event. This wake will re-invoke \
                             you; cancel it with cancel_wake if you no longer \
                             need it."
                            ),
                            false,
                        ),
                        Ok(Err(e)) => (e, true),
                        Err(_) => ("schedule_wake failed (no reply)".to_string(), true),
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
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "schedule_wake",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "wake scheduled"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "cancel_wake" => {
                    let id = tc
                        .args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui.send(AgentUpdate::CancelWake { id, reply: tx }).await;
                    let (output, is_error) = match rx.await {
                        Ok(Ok(msg)) => (msg, false),
                        Ok(Err(e)) => (e, true),
                        Err(_) => ("cancel_wake failed (no reply)".to_string(), true),
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
                            is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = "cancel_wake",
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = true,
                        "wake cancel requested"
                    );
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "cancel_subagent" => {
                    let target = tc
                        .args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let fired = if let Some(reg) = &config.in_flight {
                        fire_subagent_kill(reg, target.as_deref())
                    } else {
                        0
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: format!("{{\"cancelled\": {fired}}}"),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "list_subagents" => {
                    let output = if let Some(reg) = &config.in_flight {
                        let map = reg.lock().unwrap();
                        format_subagent_list(&map)
                    } else {
                        "No subagents currently running.".to_string()
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
                Some(zoid_tools::ToolKind::Interactive)
                    if tc.name == "ask_user"
                        || tc.name == "apply_mode_mapping"
                        || tc.name == "submit_feedback" =>
                {
                    // submit_feedback: parse + validate, emit QuestionAsked, park,
                    // submit the confirmed report. Separate from ask_user/
                    // apply_mode_mapping because the reply carries a FeedbackReport.
                    if tc.name == "submit_feedback" {
                        let (kind, title, body) = match parse_feedback_args(&tc.args) {
                            Some(v) => v,
                            None => {
                                emit(
                                    &session,
                                    &mut events,
                                    ui,
                                    &config.branch,
                                    EventKind::ToolResult {
                                        id: tc.id.clone(),
                                        name: tc.name.clone(),
                                        output: "submit_feedback: invalid args. kind must be \
                                            bug|feature|general; title and body must be non-empty."
                                            .into(),
                                        is_error: true,
                                    },
                                    session_id,
                                    now,
                                )
                                .await?;
                                continue;
                            }
                        };
                        let question = format!("Submit {} feedback?", kind.display());
                        let choices = vec!["Submit".into(), "Cancel".into()];
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::QuestionAsked {
                                id: tc.id.clone(),
                                kind: zoid_core::event::QuestionKind::Feedback {
                                    kind: kind_str(kind).to_string(),
                                    title: title.clone(),
                                    body: body.clone(),
                                },
                                question: question.clone(),
                                choices: choices.clone(),
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        let (rtx, rrx) = oneshot::channel::<Answer>();
                        let _ = ui
                            .send(AgentUpdate::AskUser {
                                question,
                                choices,
                                reply: rtx,
                            })
                            .await;
                        let ans = rrx.await;
                        let output = match ans {
                            Ok(Answer::Feedback(report)) => {
                                let api = zoid_core::feedback::HttpFeedbackApi::new();
                                match report.submit_via(&api).await {
                                    Ok(zoid_core::feedback::SubmitOutcome::Created {
                                        url,
                                        number,
                                    }) => format!("Created issue #{}: {}", number, url),
                                    Ok(zoid_core::feedback::SubmitOutcome::BrowserFallback {
                                        url,
                                    }) => format!(
                                        "No GitHub token available — opened your browser at {}. \
                                        The user must finish submitting there.",
                                        url
                                    ),
                                    Err(e) => format!("Failed to submit feedback: {e}"),
                                }
                            }
                            _ => "User declined to submit feedback.".to_string(),
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
                        emit(
                            &session,
                            &mut events,
                            ui,
                            &config.branch,
                            EventKind::ToolResult {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                output,
                                is_error: false,
                            },
                            session_id,
                            now,
                        )
                        .await?;
                        continue;
                    }
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
                        Ok(Answer::Feedback(_)) => String::new(), // unreachable for ask_user/apply_mode_mapping
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
                Some(zoid_tools::ToolKind::Mcp) => {
                    let _ = ui
                        .send(AgentUpdate::ToolStarted {
                            name: tc.name.clone(),
                        })
                        .await;
                    let out = match config.mcp.as_ref() {
                        Some(m) => call_or_abandon(cancel, m.call_tool(&tc.name, &tc.args)).await,
                        None => Some(zoid_tools::ToolOutput::err(format!(
                            "mcp tool '{}' requested but no MCP manager is active",
                            tc.name
                        ))),
                    };
                    let out = match out {
                        Some(o) => o,
                        None => {
                            // Graceful cancel abandoned the call: answer it + drain
                            // the rest of the batch so no tool_use is unbalanced.
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
                    };
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
                        log_turn_warn(&session, "warn", session_id, &msg, None).await;
                    }
                }
                Some(zoid_tools::ToolKind::Network) => {
                    let _ = ui
                        .send(AgentUpdate::ToolStarted {
                            name: tc.name.clone(),
                        })
                        .await;
                    let tools_for_async = tools.clone();
                    let name = tc.name.clone();
                    let args = tc.args.clone();
                    let cwd = cwd_for_exec.clone();
                    let out = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            // Graceful cancel during a Network tool: return a
                            // non-fatal skip so the batch drain can end the turn.
                            zoid_tools::ToolOutput::err("[skipped: turn aborted]")
                        }
                        _ = hard.cancelled() => {
                            zoid_tools::ToolOutput::err("[killed: hard-stop]")
                        }
                        o = async move {
                            match tools_for_async.iter().find(|t| t.name() == name) {
                                Some(t) => t.run_async(&args, &cwd).await,
                                None => zoid_tools::ToolOutput::err(format!("unknown tool: {name}")),
                            }
                        } => o,
                    };
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
                    if hard.is_cancelled() || cancel.is_cancelled() {
                        // Hard-stop or graceful-cancel mid-batch: answer every
                        // remaining call so no tool_use is left without a
                        // tool_result, then end.
                        // Mirrors the Local arm's drain (search/spawn_blocking is
                        // detached; here the async future is dropped — the reqwest
                        // connection is abandoned mid-flight).
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
                        log_turn_warn(&session, "warn", session_id, &msg, None).await;
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
                    let mut exec = tokio::task::spawn_blocking(move || {
                        zoid_tools::run_tool(&tools_for_exec, &name, &args, &cwd)
                    });
                    let mut out = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            // Graceful cancel during a Local tool: kill the
                            // shell's process group (same as hard) and return a
                            // non-fatal skip. The blocking task is detached.
                            config.kill.kill();
                            zoid_tools::ToolOutput::err("[skipped: turn aborted]")
                        }
                        _ = hard.cancelled() => {
                            // Force-kill the shell's process group (sticky kill:
                            // also reaps a child that registers a moment later).
                            // We do NOT await `exec` — control returns immediately.
                            // The detached spawn_blocking finishes on its own:
                            // near-instantly for a killed shell, or in the
                            // background for a non-killable local tool (search /
                            // read_file / subagent_diff), which is the spec's
                            // "abandon-wait" behavior. `exec` is dropped here,
                            // detaching (not cancelling) the blocking task.
                            config.kill.kill();
                            zoid_tools::ToolOutput::err("[killed: hard-stop]")
                        }
                        joined = &mut exec => joined?,
                    };
                    let tool_ok = !out.is_error;
                    let tool_fail_msg = out.is_error.then(|| out.text.clone());
                    // Ephemeral UI-only diff (edit/write). Sent BEFORE the emit
                    // (which moves tc.id/out.text). Never persisted; the
                    // ToolResult event below still stores only out.text. MAIN
                    // branch only — a subagent's edits happen in a worktree and
                    // never render in the main transcript, so caching them would
                    // only let a subagent burst evict the main agent's visible
                    // diffs from the bounded cache (M6). `config.branch` is in
                    // scope here (used by the `emit` just below).
                    if config.branch == BranchId::default() {
                        if let Some(diff) = out.diff.take() {
                            let _ = ui
                                .send(AgentUpdate::EditDiff {
                                    id: tc.id.clone(),
                                    diff,
                                })
                                .await;
                        }
                    }
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
                    if hard.is_cancelled() || cancel.is_cancelled() {
                        // Hard-stop or graceful-cancel mid-batch: answer every
                        // remaining call so no tool_use is left without a
                        // tool_result, then end.
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
                        log_turn_warn(&session, "warn", session_id, &msg, None).await;
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
            will_reassert,
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

/// Race an async tool call against a cancellation token. Returns `Some(output)`
/// if the call finishes first, or `None` if `cancel` fires first (the caller
/// then synthesizes a balanced `[skipped: turn aborted]` result). Used to make
/// an in-flight MCP call abandonable on a graceful cancel (first Esc).
async fn call_or_abandon<F>(cancel: &CancellationToken, fut: F) -> Option<zoid_tools::ToolOutput>
where
    F: std::future::Future<Output = zoid_tools::ToolOutput>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        out = fut => Some(out),
    }
}

/// Persist one event and announce it to the UI, keeping the local log in sync.
/// Push an ephemeral event to the in-memory log + UI, skipping SQLite.
/// Used for `ModelThinking` — reasoning text that survives only for the
/// current process lifetime, not persisted across restarts.
async fn emit_ephemeral(
    events: &mut crate::eventlog::EventLog,
    ui: &mpsc::Sender<AgentUpdate>,
    branch: &BranchId,
    kind: EventKind,
    session_id: Ulid,
    now: fn() -> i64,
) -> Result<()> {
    let mut ev = Event::new(Ulid::new(), None, now(), kind).with_session(session_id);
    ev.branch = branch.clone();
    events.push(ev.clone());
    let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    Ok(())
}

/// Write a turn-scoped log entry to the `logs` table. Called alongside
/// `tracing::warn!` at key warn sites in the agent loop so the entry carries
/// `session_id` for contextual debugging. Non-fatal: a write failure is
/// silently dropped (the `tracing::warn!` already captured it in the ring
/// buffer). `event_id` is optional — not every warn has a specific triggering
/// event.
#[allow(clippy::too_many_arguments)]
async fn log_turn_warn(
    session: &SessionHandle,
    level: &str,
    session_id: Ulid,
    message: &str,
    fields: Option<String>,
) {
    let row = zoid_core::store::LogRow {
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        level: level.into(),
        scope: "turn".into(),
        session_id: Some(session_id.to_string()),
        event_id: None,
        message: message.into(),
        fields,
    };
    let _ = session.write_log(row).await;
}

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
/// current estimate by the last known ratio. `skip_calibration` (set on a
/// re-floor sub-turn) suppresses the update: the request carried the extra
/// ephemeral reassert-reminder copy of the system prompt, so the real input
/// count includes tokens not accounted for by `overhead`/`context_window_with`
/// and would poison the ratio.
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
    skip_calibration: bool,
) -> Result<()> {
    // When the provider reports a non-zero input, learn the calibration ratio:
    // real tokens / estimated tokens. This is the ground-truth correction factor
    // for the chars/3 estimate. Skipped on re-floor sub-turns (see doc comment).
    let effective_tokens = real_input_tokens.filter(|&t| t > 0);
    if !skip_calibration {
        if let Some(real) = effective_tokens {
            let window = zoid_core::context::context_window_with(events.iter(), overhead.clone());
            if window.total_tokens > 0 {
                *calibration_ratio = Some(real as f64 / window.total_tokens as f64);
            }
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
    if !plan.compactions.is_empty() {
        let _ = ui.send(AgentUpdate::CompactionStarted).await;
    }
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
    if !plan.compactions.is_empty() {
        let _ = ui.send(AgentUpdate::CompactionComplete).await;
    }
    Ok(())
}

/// Bias applied to the pre-flight estimate (the chars/3 estimate under-reads
/// code/tool output). Push the estimate up so the gate fires early, not late.
const OVERCOUNT_BIAS: f64 = 1.15;

/// Candidate ids for the relevance read. Over-approximates the real candidate set
/// (avoids replicating `group_turns`) BUT excludes already-evicted ids: those
/// turns are `protected`, so `plan_evictions` never looks up their vectors —
/// reading them would be pure waste.
///
/// **Bounded:** Capped to the first `EMBEDDABLE_ID_CAP` non-evicted events from the
/// log. The real candidate set in `plan_evictions` is the non-protected turns
/// (old enough to evict, not recent-N) — at most ~band_size turns (~40-80
/// events). 500 ids gives generous headroom while preventing a 10k-event session
/// from deserializing ~15 MB of vectors that `plan_evictions` will never look up.
fn embeddable_event_ids(events: &crate::eventlog::EventLog) -> Vec<Ulid> {
    let evicted = zoid_core::eviction::evicted_ids(events.iter());
    events
        .iter()
        .map(|e| e.id)
        .filter(|id| !evicted.contains(id))
        .take(EMBEDDABLE_ID_CAP)
        .collect()
}

/// Max number of event ids to read vectors for in one eviction pass. The candidate
/// set in `plan_evictions` is bounded by the band (~40 turns ≈ 80 events); this
/// cap is generous over-approximation that keeps the hot-path read bounded on
/// long sessions.
const EMBEDDABLE_ID_CAP: usize = 500;

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
    model_context_window: u64,
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

    // The scale factor converting raw per-turn token estimates (chars/3) into
    // the same units as `est` (raw × calibration_ratio × OVERCOUNT_BIAS). Passed
    // to plan_evictions so its reclaimed accumulator matches current_tokens.
    let scale = match calibration_ratio {
        Some(r) if *r > 0.0 => r * OVERCOUNT_BIAS,
        _ => OVERCOUNT_BIAS,
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
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionStarted).await;
        }
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
            let _ = ui.send(AgentUpdate::CompactionComplete).await;
        }
        if compacted {
            est = estimate(events);
        }
    }

    // Relevance rescue context — built only when a wave will fire and the
    // embedder is present. Otherwise default (recency-only, unchanged).
    let goal_ctx: zoid_core::eviction::GoalContext = if est >= band.high_water {
        if let Some(emb) = &config.embedder {
            let text = zoid_core::eviction::goal_text(
                &events.iter().collect::<Vec<_>>(),
                zoid_core::eviction::GOAL_WINDOW_MSGS,
            );
            if text.is_empty() {
                Default::default()
            } else {
                let model = emb.model_id().to_string();
                let goal_text = text.clone();
                let goal = {
                    let emb = emb.clone();
                    tokio::task::spawn_blocking(move || {
                        emb.embed(&[text.as_str()])
                            .ok()
                            .and_then(|mut v| v.pop())
                            .unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default()
                };
                if goal.is_empty() {
                    Default::default()
                } else {
                    let ids = embeddable_event_ids(events);
                    let vecs = session.vectors_by_ids(model, ids).await.unwrap_or_default();
                    zoid_core::eviction::GoalContext {
                        goal,
                        vecs,
                        weight: zoid_core::eviction::resolve_rescue_weight(
                            config.eviction.rescue_weight,
                        ),
                        goal_text,
                    }
                }
            }
        } else {
            Default::default()
        }
    } else {
        Default::default()
    };
    if !goal_ctx.goal.is_empty() {
        tracing::info!(
            candidates = goal_ctx.vecs.len(),
            weight = goal_ctx.weight,
            "eviction relevance rescue active"
        );
    }

    // (2) Eviction to low_water.
    if est >= band.high_water {
        let plan = zoid_core::eviction::plan_evictions(
            events.iter(),
            policy,
            est,
            &zoid_core::eviction::RecencyScorer,
            &goal_ctx,
            scale,
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
            &goal_ctx,
            scale,
        );
        emit_eviction(session, events, ui, config, session_id, now, plan).await?;
    }

    // Hard-ceiling safety net: if the estimate still exceeds the model's
    // actual context window, force-compact the largest uncompacted tool
    // results. This catches the case where a single tool result (e.g.
    // reading a 10K-line file) pushes context past the limit in one sub-turn,
    // bypassing the soft-threshold compaction above.
    est = estimate(events);
    if est > model_context_window && model_context_window > 0 {
        let plan = zoid_core::compaction::plan_compactions_for_overflow(
            events.iter(),
            model_context_window,
            overhead,
            *calibration_ratio,
        );
        let compacted = !plan.compactions.is_empty();
        if compacted {
            let _ = ui.send(AgentUpdate::CompactionStarted).await;
        }
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
            let _ = ui.send(AgentUpdate::CompactionComplete).await;
        }
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
    let zoid_core::eviction::EvictionPlan { turns, rescue } = plan;
    if turns.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::new();
    let mut reclaimed = 0u64;
    let mut spans = Vec::new();
    for t in turns {
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
            rescue,
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

/// Parse + validate the `submit_feedback` tool-call args.
/// Returns `(kind, title, body)` on success, or `None` on any validation failure.
fn parse_feedback_args(
    args: &serde_json::Value,
) -> Option<(zoid_core::feedback::FeedbackKind, String, String)> {
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .and_then(zoid_core::feedback::FeedbackKind::parse)?;
    let title = args.get("title").and_then(|v| v.as_str())?;
    let body = args.get("body").and_then(|v| v.as_str())?;
    if title.trim().is_empty() || body.trim().is_empty() {
        return None;
    }
    Some((kind, title.to_string(), body.to_string()))
}

fn kind_str(k: zoid_core::feedback::FeedbackKind) -> &'static str {
    match k {
        zoid_core::feedback::FeedbackKind::Bug => "bug",
        zoid_core::feedback::FeedbackKind::FeatureRequest => "feature",
        zoid_core::feedback::FeedbackKind::General => "general",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::event::BranchId;
    use zoid_provider::MsgRole;

    #[test]
    fn system_prompt_reinforces_no_poll() {
        assert!(
            SYSTEM_PROMPT.contains("fire-and-forget"),
            "SYSTEM_PROMPT must contain 'fire-and-forget' so wrap_reassertion \
             periodically reinforces the no-poll rule: {SYSTEM_PROMPT}"
        );
        assert!(
            SYSTEM_PROMPT.contains("never poll"),
            "SYSTEM_PROMPT must contain 'never poll' so the periodic re-assertion \
             carries the no-poll discipline: {SYSTEM_PROMPT}"
        );
        assert!(
            SYSTEM_PROMPT.contains("exactly one wake"),
            "SYSTEM_PROMPT must contain 'exactly one wake' for wake discipline: {SYSTEM_PROMPT}"
        );
        assert!(
            SYSTEM_PROMPT.contains("duplicate wakes"),
            "SYSTEM_PROMPT must warn against duplicate wakes: {SYSTEM_PROMPT}"
        );
    }

    #[test]
    fn submit_feedback_parse_validates_kind_title_body() {
        let ok = parse_feedback_args(&serde_json::json!({"kind":"bug","title":"t","body":"b"}));
        assert!(ok.is_some());
        let bad_kind = parse_feedback_args(&serde_json::json!({"kind":"x","title":"t","body":"b"}));
        assert!(bad_kind.is_none());
        let empty_title =
            parse_feedback_args(&serde_json::json!({"kind":"bug","title":"","body":"b"}));
        assert!(empty_title.is_none());
        let empty_body =
            parse_feedback_args(&serde_json::json!({"kind":"bug","title":"t","body":""}));
        assert!(empty_body.is_none());
    }

    #[tokio::test]
    async fn call_or_abandon_yields_none_when_cancel_wins() {
        let cancel = CancellationToken::new();
        cancel.cancel(); // already cancelled → abandon immediately
        let out = call_or_abandon(&cancel, std::future::pending::<zoid_tools::ToolOutput>()).await;
        assert!(out.is_none(), "a cancelled token must abandon the call");
    }

    #[tokio::test]
    async fn call_or_abandon_yields_result_when_future_completes() {
        let cancel = CancellationToken::new(); // never fired
        let out = call_or_abandon(&cancel, async { zoid_tools::ToolOutput::ok("done") }).await;
        assert_eq!(out.expect("future should win").text, "done");
    }

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
            None,
        );
        assert_eq!(req.system.as_deref(), Some("CUSTOM SYS"));
    }

    #[test]
    fn subagent_branch_request_carries_the_seed_turn_not_empty() {
        use zoid_core::event::{BranchId, Event, EventKind};
        let sub = BranchId("subagent:zz9".into());
        let mut seed = Event::new(
            ulid::Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "do the subagent task".into(),
            },
        );
        seed.branch = sub.clone();
        let events = crate::eventlog::EventLog::from_vec(vec![seed]);
        // Regression guard (HTTP 400): the turn loop rebuilds the request from
        // the event log, but the projection filters to the default branch — a
        // subagent's whole transcript lives on `subagent:<id>`, so the naive
        // build yielded an EMPTY messages array and the provider (zai/glm-5.2,
        // Ollama) rejected it. Building for the active branch keeps the seed.
        let sub_req = build_request_with_thinking(
            &events,
            "m",
            zoid_provider::model::model_info("m"),
            &zoid_tools::registry(),
            "SYS",
            ThinkingMode::Off,
            None,
            &sub,
        );
        assert_eq!(
            sub_req.messages.len(),
            1,
            "subagent request must carry its seed user turn (else HTTP 400)"
        );
        // The main-branch build still excludes it — proving this is branch
        // selection, not a blanket "include everything" that would leak a
        // subagent's turns into the main conversation.
        let main_req = build_request(&events, "m", &zoid_tools::registry(), "SYS", None);
        assert!(
            main_req.messages.is_empty(),
            "main-branch build must not include subagent-branch events"
        );
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
                    rescue: None,
                },
            ),
        ]);
        let req = build_request(&events, "m", &zoid_tools::registry(), "SYS", None);
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
        assert!(p.allows("write"));
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

        let req = build_request(&events, "test-model", &zoid_tools::registry(), "SYS", None);

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

    #[test]
    fn wrap_reassertion_frames_prompt_as_standing_reminder() {
        let out = wrap_reassertion("BEHAVIORAL RULES");
        assert!(out.contains("BEHAVIORAL RULES"));
        assert!(out.contains("NOT a signal that anything is complete"));
        assert!(out.contains("resume the task"));
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
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
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

    /// Test helper: drive `run_agent_turn` with a FakeProvider yielding "done"+Done,
    /// a drained UI channel, and a seed-only EventLog. Returns the `out` events so
    /// callers can inspect `TurnsEvicted` markers.
    async fn run_gate_only(
        cfg: TurnConfig,
        session: zoid_core::session::SessionHandle,
        seed: Vec<Event>,
    ) -> crate::eventlog::EventLog {
        use zoid_provider::ProviderEvent;
        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            ProviderEvent::TextDelta("done".into()),
            ProviderEvent::Done,
        ]));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        run_agent_turn(
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
        .unwrap()
    }

    #[tokio::test]
    async fn preflight_rescues_relevant_old_turn_over_newer_offgoal() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_core::retrieval::{Embedder, FakeEmbedder};

        let fat = "x".repeat(3000); // ~1000 est tokens, on the assistant side
                                    // user ids 1,3,5,7,9,11,13,15. recent_n=2 → 13,15 protected; goal (3 recent
                                    // user msgs) = ids 15,13,11. On-goal set {1,11,13,15}; off-goal {3,5,7,9}.
        let goalish = "alpha beta gamma delta";
        let offgoal = "zulu yankee xray whiskey";
        let utext = |uid: u128| -> String {
            if matches!(uid, 1 | 11 | 13 | 15) {
                format!("{goalish} n{uid}")
            } else {
                format!("{offgoal} n{uid}")
            }
        };
        let mut seed = Vec::new();
        for i in 0..8u128 {
            let uid = i * 2 + 1;
            seed.push(Event::new(
                Ulid::from(uid),
                None,
                uid as i64,
                EventKind::UserMessage { text: utext(uid) },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: fat.clone() },
            ));
        }
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        // Seed cached vectors for the candidate user events (model "fake").
        let emb = FakeEmbedder::new(16);
        for uid in [1u128, 3, 5, 7, 9, 11] {
            let v = emb.embed(&[utext(uid).as_str()]).unwrap().remove(0);
            session
                .write_embedding(Ulid::from(uid), "fake".into(), v)
                .await
                .unwrap();
        }

        let mut cfg = chat_turn_config();
        // context_target 9_910 (not the plan's 5_000) is calibrated so the reclaim
        // quota evicts exactly 4 of the 6 candidate turns, leaving 2 survivors —
        // enough room for the rescue to keep id 1 and drop a newer off-goal turn.
        // At 5_000, all 6 candidates are evicted (reclaim > 6 turns), so no rescue
        // is possible. The margin: est≈11,801, high_water=9,910, low_water=7,928,
        // reclaim≈3,873 ≈ 4 turns @ ~950 tokens/turn (with OVERCOUNT_BIAS).
        // NB: bumped from 9_500 → 9_910 after the expanded system prompt added
        // ~411 estimated tokens, which shifted the eviction math.
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 9_910,
            band_headroom_pct: 20,
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
        cfg.embedder = Some(std::sync::Arc::new(FakeEmbedder::new(16)));

        let out = run_gate_only(cfg, session, seed).await;
        let evicted: Vec<Ulid> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        assert!(!evicted.is_empty(), "a wave fired");
        assert!(
            !evicted.contains(&Ulid::from(1u128)),
            "on-goal old turn rescued"
        );
        assert!(
            evicted.contains(&Ulid::from(3u128)),
            "a newer off-goal turn dropped instead"
        );
    }

    #[tokio::test]
    async fn preflight_rescue_weight_zero_is_pure_recency() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_core::retrieval::{Embedder, FakeEmbedder};

        let fat = "x".repeat(3000);
        let goalish = "alpha beta gamma delta";
        let offgoal = "zulu yankee xray whiskey";
        let utext = |uid: u128| -> String {
            if matches!(uid, 1 | 11 | 13 | 15) {
                format!("{goalish} n{uid}")
            } else {
                format!("{offgoal} n{uid}")
            }
        };
        let mut seed = Vec::new();
        for i in 0..8u128 {
            let uid = i * 2 + 1;
            seed.push(Event::new(
                Ulid::from(uid),
                None,
                uid as i64,
                EventKind::UserMessage { text: utext(uid) },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: fat.clone() },
            ));
        }
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let emb = FakeEmbedder::new(16);
        for uid in [1u128, 3, 5, 7, 9, 11] {
            let v = emb.embed(&[utext(uid).as_str()]).unwrap().remove(0);
            session
                .write_embedding(Ulid::from(uid), "fake".into(), v)
                .await
                .unwrap();
        }

        let mut cfg = chat_turn_config();
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 9_910,
            band_headroom_pct: 20,
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: Some(0.0), // weight 0 ⇒ pure recency, no rescue
        };
        cfg.embedder = Some(std::sync::Arc::new(FakeEmbedder::new(16)));

        let out = run_gate_only(cfg, session, seed).await;
        let evicted: Vec<Ulid> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();

        assert!(!evicted.is_empty(), "a wave fired");
        assert!(
            evicted.contains(&Ulid::from(1u128)),
            "weight 0 ⇒ oldest evicted (no rescue)"
        );
    }

    #[tokio::test]
    async fn preflight_without_embedder_evicts_the_old_turn() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        // Same seed shape, but cfg.embedder = None → recency-only → oldest (id 1) evicted.
        let fat = "x".repeat(3000);
        let mut seed = Vec::new();
        for i in 0..8u128 {
            let uid = i * 2 + 1;
            seed.push(Event::new(
                Ulid::from(uid),
                None,
                uid as i64,
                EventKind::UserMessage {
                    text: format!("msg n{uid}"),
                },
            ));
            seed.push(Event::new(
                Ulid::from(i * 2 + 2),
                None,
                (i * 2 + 2) as i64,
                EventKind::AssistantMessage { text: fat.clone() },
            ));
        }
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let mut cfg = chat_turn_config();
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 5_000,
            band_headroom_pct: 20,
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
        cfg.embedder = None;
        let out = run_gate_only(cfg, session, seed).await;
        let evicted: Vec<Ulid> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(
            evicted.contains(&Ulid::from(1u128)),
            "no embedder ⇒ recency evicts oldest"
        );
    }

    #[tokio::test]
    async fn empty_completion_surfaces_a_warning_not_silent_idle() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;
        // Reproduces the upstream failure where Ollama Cloud's glm-5.2 returns a
        // 200 with empty content and `done_reason:"stop"` — the provider stream
        // yields ONLY Done (no TextDelta / ToolCall / Usage / Error). Before the
        // guard, the turn ended with nothing to show and the UI went silently
        // back to idle. The turn must now surface a ⚠ empty-response message.
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "are you there?".into(),
            },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let provider =
            std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![ProviderEvent::Done]));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            chat_turn_config(),
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
            out.iter().any(|e| matches!(
                &e.kind,
                EventKind::AssistantMessage { text }
                    if text.starts_with(WARN_GLYPH) && text.contains("empty response")
            )),
            "an empty completion must surface a ⚠ empty-response message, not a silent idle"
        );
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

    // Test double: records the `reassert` field of every request it receives
    // (so a test can assert whether the re-floor reminder was injected),
    // while replaying a fixed scripted response.
    struct RecordingProvider {
        scripted: Vec<zoid_provider::ProviderEvent>,
        seen_reassert: std::sync::Mutex<Vec<Option<String>>>,
    }
    impl RecordingProvider {
        fn new(scripted: Vec<zoid_provider::ProviderEvent>) -> Self {
            Self {
                scripted,
                seen_reassert: std::sync::Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait::async_trait]
    impl zoid_provider::Provider for RecordingProvider {
        async fn stream(
            &self,
            req: &zoid_provider::CompletionRequest,
            sink: tokio::sync::mpsc::Sender<zoid_provider::ProviderEvent>,
        ) -> anyhow::Result<()> {
            self.seen_reassert
                .lock()
                .unwrap()
                .push(req.reassert.clone());
            for ev in &self.scripted {
                if sink.send(ev.clone()).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn re_floor_fires_and_persists_marker_on_success() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;
        // Seed a log whose cumulative_appended (estimated tokens) already
        // exceeds a small reassert_interval: a 3000-char user message is
        // ~1000 estimated tokens (chars/3).
        let big = "x".repeat(3000);
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: big },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let provider = std::sync::Arc::new(RecordingProvider::new(vec![
            ProviderEvent::TextDelta("done".into()),
            ProviderEvent::Done,
        ]));
        let mut cfg = chat_turn_config();
        cfg.reassert_interval = 500; // well under the seeded ~1000 estimated tokens
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            cfg,
            provider.clone(),
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

        let seen = provider.seen_reassert.lock().unwrap();
        assert!(
            seen.iter().any(|r| r.is_some()),
            "the request sent to the provider must carry a reassert reminder"
        );
        assert!(
            out.iter()
                .any(|e| matches!(&e.kind, EventKind::DirectiveReasserted { .. })),
            "a successful re-floor sub-turn must persist a DirectiveReasserted marker"
        );
    }

    /// Task 10 / B1 regression guard: a long synthetic session that grows PAST
    /// `context_target` with BOTH eviction (marks-but-keeps) and compaction
    /// body-clears (#6b, `clear_compacted_bodies` — the case that reopened B1;
    /// eviction markers alone are not enough to prove liveness) active, and a
    /// small `reassert_interval`.
    ///
    /// The PRIMARY, mutation-sensitive assertion is monotonicity: at every
    /// point where #6b clears compacted tool-result bodies, `cumulative_appended`
    /// must NOT DECREASE (a body-clear empties the `ToolResult.output`, and the
    /// `original_tokens` carried by the paired `ToolResultCompacted` is what
    /// keeps the count from collapsing to 0). If `cumulative_appended` reverts
    /// to the buggy pre-fix form that drops the `original_tokens` substitution,
    /// clearing a compacted body drops the count and THIS assertion fails —
    /// which is exactly the invariant that reopened B1. (Proven mutation-
    /// sensitive: see the FIX section of task-10-report.md.)
    ///
    /// The liveness checks (re-floor keeps firing past `context_target`, every
    /// fired request carried the reminder) are kept as SECONDARY assertions —
    /// they alone are satisfied by each turn's own fresh user message + tool
    /// result crossing the interval, so they do not by themselves pin
    /// monotonicity of already-compacted history.
    #[tokio::test]
    async fn re_floor_keeps_firing_in_steady_state_with_compaction() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let mut events = crate::eventlog::EventLog::new();

        let mut cfg = chat_turn_config();
        cfg.reassert_interval = 2_000; // small: fires often relative to per-turn growth
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 8_000, // small: exceeded within a handful of turns
            band_headroom_pct: 20,
            min_protected_turns: 4,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };

        let provider = std::sync::Arc::new(RecordingProvider::new(vec![
            ProviderEvent::TextDelta("ok".into()),
            ProviderEvent::Done,
        ]));

        const N_TURNS: usize = 24;
        let mut fires = 0usize;
        let mut fires_after_target_exceeded = 0usize;
        let mut past_target_seen = false;
        // True once at least one #6b body-clear actually emptied a compacted
        // body during the run — proves the monotonicity assertion below is not
        // vacuous (it exercised the ToolResultCompacted.original_tokens path).
        let mut observed_a_body_clear = false;

        for i in 0..N_TURNS {
            let seq = (i as i64) * 2 + 1;
            // A big user message plus a big tool-result each turn — the tool
            // result is what compaction (largest-first, ToolResult-only)
            // actually targets.
            let user_ev = Event::new(
                Ulid::new(),
                None,
                seq,
                EventKind::UserMessage {
                    text: format!("turn {i}: {}", "u".repeat(1500)),
                },
            );
            // Multi-line output: compaction's `compact_tool_output` only shrinks
            // bodies with more than COMPACT_HEAD_LINES (8) lines, so a single
            // 3000-char line would never compact. ~200 lines forces a real
            // ToolResultCompacted (with original_tokens) whose body the #6b
            // clear below then empties — exercising the monotonicity path.
            let tool_output: String = (0..200)
                .map(|n| format!("line {n:03}: {}", "t".repeat(30)))
                .collect::<Vec<_>>()
                .join("\n");
            let tool_ev = Event::new(
                Ulid::new(),
                None,
                seq + 1,
                EventKind::ToolResult {
                    id: format!("tool-{i}"),
                    name: "bash".into(),
                    output: tool_output,
                    is_error: false,
                },
            );
            session.append(user_ev.clone()).await.unwrap();
            session.append(tool_ev.clone()).await.unwrap();
            events.push(user_ev);
            events.push(tool_ev);

            let out = run_agent_turn(
                cfg.clone(),
                provider.clone(),
                std::sync::Arc::new(zoid_tools::registry()),
                std::sync::Arc::new(zoid_tools::AllowAll),
                session.clone(),
                events.clone(),
                "m".into(),
                {
                    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                    tokio::spawn(async move { while rx.recv().await.is_some() {} });
                    tx
                },
                Ulid::new(),
                zoid_companion::CompanionHub::new(),
                || 0,
            )
            .await
            .unwrap();

            let turn_fires = out
                .iter()
                .filter(|e| matches!(&e.kind, EventKind::DirectiveReasserted { .. }))
                .count();
            fires += turn_fires;

            let over_target =
                zoid_core::reassert::cumulative_appended(out.iter()) >= cfg.eviction.context_target;
            if over_target {
                past_target_seen = true;
            }
            if past_target_seen {
                fires_after_target_exceeded += turn_fires;
            }

            events = out;

            // PRIMARY assertion — monotonicity across the #6b body-clear.
            // Capture cumulative_appended immediately BEFORE and AFTER the
            // resume-path body-clear. Emptying a compacted ToolResult.output
            // must be fully offset by the paired ToolResultCompacted's
            // original_tokens, so the count must NEVER DECREASE. Under the
            // pre-fix bug (original_tokens substitution dropped), the cleared
            // body counts as 0 and this fails — the exact regression B1.
            let before = zoid_core::reassert::cumulative_appended(events.iter());
            // Did any compacted-but-still-full body exist that this clear will empty?
            let had_full_compacted_body = {
                let compacted: std::collections::HashSet<&str> = events
                    .iter()
                    .filter_map(|e| match &e.kind {
                        EventKind::ToolResultCompacted { id, .. } => Some(id.as_str()),
                        _ => None,
                    })
                    .collect();
                events.iter().any(|e| {
                    matches!(&e.kind,
                    EventKind::ToolResult { id, output, .. }
                        if compacted.contains(id.as_str()) && !output.is_empty())
                })
            };
            // Simulate a mid-session resume (main.rs's #6b path): body-clear
            // every compacted tool result, not just leave the eviction/compaction
            // markers in place. This is the exact case that reopened B1.
            events.clear_compacted_bodies();
            let after = zoid_core::reassert::cumulative_appended(events.iter());
            assert!(
                after >= before,
                "cumulative_appended must NOT decrease across a #6b compaction \
                 body-clear (turn {i}): before={before}, after={after} — a cleared \
                 compacted body must still count at its original_tokens (B1 monotonicity)"
            );
            if had_full_compacted_body {
                observed_a_body_clear = true;
            }
        }

        // The monotonicity assertion is only a real regression guard if a
        // body-clear actually happened during the run.
        assert!(
            observed_a_body_clear,
            "test is vacuous: no compacted tool-result body was ever cleared — \
             compaction never fired, so monotonicity-across-body-clear was untested"
        );
        // Secondary (liveness) assertions.
        assert!(
            fires >= 4,
            "re-floor must fire repeatedly over a long session (fired {fires} times)"
        );
        assert!(
            fires_after_target_exceeded >= 2,
            "re-floor must keep firing PAST context_target under eviction+compaction, \
             not go dormant (fired {fires_after_target_exceeded} times after target exceeded)"
        );
        assert!(
            out_carried_reminder(&provider),
            "every fired re-floor request must have carried the reassert reminder"
        );
    }

    /// Helper: true iff at least one request the provider saw actually carried
    /// a non-empty reassert reminder (mirrors the assertion already proven in
    /// `re_floor_fires_and_persists_marker_on_success`, factored out for reuse).
    fn out_carried_reminder(provider: &RecordingProvider) -> bool {
        provider
            .seen_reassert
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.as_deref().is_some_and(|s| !s.is_empty()))
    }

    /// Negative path (Task 10 step 1b): cumulative_appended is below the
    /// configured interval, so the re-floor must not fire — the request must
    /// carry no reminder, and no `DirectiveReasserted` marker may be persisted.
    #[tokio::test]
    async fn re_floor_does_not_fire_below_interval() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;
        // A short user message: well under any reasonable interval.
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "hi there".into(),
            },
        )];
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        let provider = std::sync::Arc::new(RecordingProvider::new(vec![
            ProviderEvent::TextDelta("done".into()),
            ProviderEvent::Done,
        ]));
        let mut cfg = chat_turn_config();
        cfg.reassert_interval = 5_000; // far above the ~3-token seeded message
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            cfg,
            provider.clone(),
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

        let seen = provider.seen_reassert.lock().unwrap();
        assert!(
            seen.iter().all(|r| r.is_none()),
            "below the interval, no request may carry a reassert reminder"
        );
        drop(seen);
        assert!(
            !out.iter()
                .any(|e| matches!(&e.kind, EventKind::DirectiveReasserted { .. })),
            "below the interval, no DirectiveReasserted marker may be persisted"
        );
    }

    /// Negative path (Task 10 step 1b, S2 retry-safety pin): cumulative_appended
    /// is ABOVE the interval, but the provider's stream errors with a
    /// context-length message and eviction is enabled, so the turn loop takes
    /// the `continue 'turn` retry path instead of a clean Done. The marker must
    /// NOT be persisted — a rejected re-floor sub-turn must not burn the
    /// interval, or a real re-floor could permanently starve behind a retry
    /// loop. Task 9 proved this only by control-flow inspection; this pins it.
    #[tokio::test]
    async fn re_floor_marker_not_persisted_when_turn_errors() {
        use ulid::Ulid;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::ProviderEvent;

        let big = "x".repeat(3000); // ~1000 estimated tokens, above the interval below
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: big },
        )];
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }
        assert!(zoid_provider::is_context_length_error(
            "prompt is too long: exceeds context window"
        ));
        // Every scripted stream (initial send + every retry) errors with a
        // context-length message: eviction is enabled, so the loop keeps
        // taking the `continue 'turn` retry path and never reaches a clean
        // Done. This isolates "the turn never cleanly completes" without
        // relying on a bounded retry count.
        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![ProviderEvent::Error(
                "prompt is too long: exceeds context window".into(),
            )];
            8
        ]));
        let mut cfg = chat_turn_config();
        cfg.reassert_interval = 500; // well under the seeded ~1000 estimated tokens
        cfg.eviction = zoid_core::eviction::EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 900_000,
            band_headroom_pct: 20,
            min_protected_turns: 4,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
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

        assert!(
            !out.iter()
                .any(|e| matches!(&e.kind, EventKind::DirectiveReasserted { .. })),
            "a turn that never cleanly completes must not persist a DirectiveReasserted \
             marker — the retry-safety property (interval must not be burned by a \
             rejected sub-turn)"
        );
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
            min_protected_turns: 4,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
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
                rescue: None,
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
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
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

    /// Facet A (multi-session over-fetch): the in-memory index is
    /// session-agnostic, so vector hits from OTHER sessions can crowd out the
    /// current session's own hit. Over-fetching (`VECTOR_OVERFETCH` × limit)
    /// before the per-session `events_by_ids` filter keeps the session's
    /// semantic hit alive.
    ///
    /// Setup: five foreign-session vectors sit exactly at the query vector
    /// (cosine 1.0); the current session's target vector is orthogonal, so it
    /// ranks 6th. With `limit = 5`, a naive top-5 vector scan returns only the
    /// five foreign hits — all dropped by the session filter → the target never
    /// surfaces. Over-fetching to `5 × VECTOR_OVERFETCH` pulls the target into
    /// range, and the session filter keeps it. (`limit = 5` also leaves room for
    /// the recall tool-call's own FTS self-match, which renders empty.)
    #[tokio::test]
    async fn recall_overfetches_past_cross_session_vectors() {
        use serde_json::json;
        use std::sync::{Arc, RwLock};
        use ulid::Ulid;
        use zoid_core::embed_index::EmbeddingIndex;
        use zoid_core::event::{Event, EventKind};
        use zoid_core::retrieval::{Embedder, FakeEmbedder};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let sid_a = Ulid::from(100u128); // the recall (current) session
        let sid_b = Ulid::from(200u128); // a foreign session sharing the DB

        // Target lives in session A; its text shares no token with the query, so
        // session FTS does not find it — only the vector path can surface it.
        let target = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "zzz nomatch target body".into(),
            },
        )
        .with_session(sid_a);
        // Five foreign events in session B (ids 2..=6).
        let foreign: Vec<Event> = (2u128..=6)
            .map(|i| {
                Event::new(
                    Ulid::from(i),
                    None,
                    1,
                    EventKind::UserMessage {
                        text: format!("foreign {i}"),
                    },
                )
                .with_session(sid_b)
            })
            .collect();
        session.append(target.clone()).await.unwrap();
        for e in &foreign {
            session.append(e.clone()).await.unwrap();
        }

        // Build the session-agnostic ring directly (vectors decoupled from event
        // text, so FTS and the vector scan disagree — the whole point): the five
        // foreign vectors ARE the query vector (cosine 1.0); the target's is
        // orthogonal, so it ranks last.
        let fake: Arc<dyn Embedder> = Arc::new(FakeEmbedder::new(16));
        let qv = fake.embed(&["alpha"]).unwrap().remove(0);
        let tv = fake.embed(&["beta"]).unwrap().remove(0);
        let mut ring = EmbeddingIndex::new(16, 100);
        for i in 2u128..=6 {
            ring.append(Ulid::from(i), &qv);
        }
        ring.append(Ulid::from(1u128), &tv); // target ranks below the 5 foreign
        let index = Arc::new(RwLock::new(ring));

        let mut cfg = chat_turn_config();
        cfg.embed = Some(index);
        cfg.embedder = Some(fake);

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "r1".into(),
                    name: "recall".into(),
                    args: json!({"query": "alpha", "limit": 5}),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("ok".into()), ProviderEvent::Done],
        ]));
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let seed = std::iter::once(target.clone())
            .chain(foreign.clone())
            .collect();
        let out = run_agent_turn(
            cfg,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(seed),
            "m".into(),
            tx,
            sid_a,
            zoid_companion::CompanionHub::new(),
            || 0,
        )
        .await
        .unwrap();

        // The recall ToolResult carries the TARGET's content: over-fetch pulled it
        // past the five higher-ranked foreign-session vectors, and the session
        // filter kept it. Without over-fetch, a top-5 vector scan returns only the
        // five foreign hits (all dropped by the session filter) → target absent.
        assert!(
            out.iter().any(|e| matches!(&e.kind,
                EventKind::ToolResult { name, output, .. }
                    if name == "recall" && output.contains("zzz nomatch target"))),
            "over-fetch must surface the current session's semantic hit despite cross-session vectors"
        );
    }

    /// Task 9a graceful-degradation guarantee: a `TurnConfig` built via
    /// `chat_turn_config()` has `embed`/`embedder` both `None` (no
    /// `local-embed` wiring yet), so recall must stay byte-identical to the
    /// pure-FTS path. Body copied from `recall_tool_readmits_and_returns_content`.
    #[tokio::test]
    async fn recall_stays_fts_only_when_embed_config_absent() {
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
                rescue: None,
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

        let config = chat_turn_config();
        assert!(config.embed.is_none(), "embed must default to None");
        assert!(config.embedder.is_none(), "embedder must default to None");

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
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = run_agent_turn(
            config,
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
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
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
            min_protected_turns: 2,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));

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
    #[tokio::test]
    async fn dispatch_subagent_returns_id_as_tool_result() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "dispatch a subagent".into(),
            },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "do something", "worktree": false}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("ok dispatched".into()),
                ProviderEvent::Done,
            ],
        ]));

        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
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

        let tool_result = out
            .iter()
            .find(|e| {
                matches!(
                    &e.kind,
                    EventKind::ToolResult { name, .. } if name == "dispatch_subagent"
                )
            })
            .expect("dispatch_subagent tool result must be emitted");
        match &tool_result.kind {
            EventKind::ToolResult {
                output, is_error, ..
            } => {
                assert!(!*is_error, "dispatch should not error");
                assert!(
                    output.contains("sub-"),
                    "result must contain subagent ID: got {output}"
                );
                assert!(
                    output.contains("do NOT call list_subagents"),
                    "result must carry the no-poll directive: got {output}"
                );
                assert!(
                    output.contains("End your turn now"),
                    "result must give the positive action (end turn): got {output}"
                );
            }
            _ => panic!(),
        }
    }
    /// An unknown `agent` name against a populated registry emits an error
    /// ToolResult listing available agents, and no subagent is spawned.
    #[tokio::test]
    async fn dispatch_with_unknown_agent_emits_error_listing_available() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "dispatch a subagent".into(),
            },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "x", "agent": "nope"}),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("ok".into()), ProviderEvent::Done],
        ]));

        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            reg.clone(),
            zoid_tools::KillSlot::new(),
        ));
        // Capture any SubagentStarted updates to assert none are sent.
        let started = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started_cap = started.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            while let Some(upd) = rx.recv().await {
                if let AgentUpdate::SubagentStarted { id, task, agent } = upd {
                    started_cap.lock().unwrap().push((id, task, agent));
                }
            }
        });

        let mut config = chat_turn_config();
        config.agents = Some(reg.clone());
        let out = run_agent_turn(
            config,
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

        let tool_result = out
            .iter()
            .find(|e| {
                matches!(
                    &e.kind,
                    EventKind::ToolResult { name, .. } if name == "dispatch_subagent"
                )
            })
            .expect("dispatch_subagent tool result must be emitted");
        match &tool_result.kind {
            EventKind::ToolResult {
                output, is_error, ..
            } => {
                assert!(*is_error, "unknown agent must produce an error ToolResult");
                assert!(
                    output.contains("unknown agent 'nope'"),
                    "error must name the unknown agent: got {output}"
                );
                assert!(
                    output.contains("delegate"),
                    "error must list available agents (delegate): got {output}"
                );
            }
            _ => panic!(),
        }
        // Give the drainer a tick to flush, then assert no subagent was started.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let started = started.lock().unwrap();
        assert!(
            started.is_empty(),
            "no SubagentStarted should be emitted for an unknown agent: got {started:?}"
        );
    }
    #[tokio::test]
    async fn dispatch_two_subagents_second_is_rejected() {
        use serde_json::json;
        use zoid_core::event::{Event, EventKind};
        use zoid_provider::{ProviderEvent, ToolCall};

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "dispatch two subagents".into(),
            },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(SequencedProvider::new(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "d1".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task one", "worktree": false}),
                }),
                ProviderEvent::ToolCall(ToolCall {
                    id: "d2".into(),
                    name: "dispatch_subagent".into(),
                    args: json!({"task": "task two", "worktree": false}),
                }),
                ProviderEvent::Done,
            ],
            vec![
                ProviderEvent::TextDelta("both dispatched".into()),
                ProviderEvent::Done,
            ],
        ]));

        let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(
            std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()),
            std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin()),
            zoid_tools::KillSlot::new(),
        ));
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let mut config = chat_turn_config();
        let shared: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, SubagentHandle>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        config.in_flight = Some(shared.clone());
        // Concurrency: with the default pool size (3), two dispatches both
        // succeed — the single-in-flight guard is gone. The pool now bounds
        // concurrency, so a SECOND dispatch while ONE is running is allowed
        // (not rejected). Set the pool to 1 to keep the sequential-rejection
        // semantics this test was built to check.
        config.max_concurrent = 1;

        let out = run_agent_turn(
            config,
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

        let results: Vec<(String, bool)> = out
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolResult {
                    name,
                    output,
                    is_error,
                    ..
                } if name == "dispatch_subagent" => Some((output.clone(), *is_error)),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2, "two dispatch tool results");
        assert!(
            !results[0].1,
            "first dispatch should succeed: {:?}",
            results[0]
        );
        // With max_concurrent = 1, the second dispatch hits the pool cap and is
        // queued (not an error — a non-error "queued" tool result). The
        // rejection-when-full behavior now returns a queued response instead
        // of an error; the SubagentQueued event carries it to the main loop.
        assert!(
            !results[1].1,
            "second dispatch at capacity is queued (non-error), not rejected: {:?}",
            results[1]
        );
        assert!(
            results[1].0.starts_with("subagent queued"),
            "second dispatch tool result should announce it was queued: {:?}",
            results[1]
        );
    }

    #[tokio::test]
    async fn mcp_kind_tool_routes_to_manager_and_errors_cleanly() {
        // A manager with a configured-but-unconnected server: calling its tool
        // must surface a ToolOutput error (never panic, never hit the Local path).
        let mgr = std::sync::Arc::new(zoid_mcp::McpManager::new());
        // No servers connected => any mcp tool name is unknown.
        let out = mgr.call_tool("srv__thing", &serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.text.contains("unknown mcp tool"));
    }

    #[tokio::test]
    async fn turn_loop_routes_mcp_tool_call_to_mcp_manager() {
        use ulid::Ulid;
        use zoid_provider::ProviderEvent;

        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        let seed = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "call the mcp tool".into(),
            },
        )];
        for e in &seed {
            session.append(e.clone()).await.unwrap();
        }

        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "c1".into(),
                name: "srv__missing".into(),
                args: serde_json::json!({}),
            }),
            ProviderEvent::Done,
        ]));

        let tools: Vec<Box<dyn Tool>> = vec![Box::new(zoid_mcp::McpTool::new(
            "srv__missing".into(),
            String::new(),
            serde_json::json!({"type": "object"}),
        ))];

        let mut cfg = chat_turn_config();
        cfg.mcp = Some(std::sync::Arc::new(zoid_mcp::McpManager::new()));

        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} }); // drain UI updates

        let out = run_agent_turn(
            cfg,
            provider,
            std::sync::Arc::new(tools),
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

        let result = out.iter().find_map(|e| match &e.kind {
            EventKind::ToolResult {
                name,
                output,
                is_error,
                ..
            } if name == "srv__missing" => Some((output.clone(), *is_error)),
            _ => None,
        });
        let (output, is_error) = result.expect("expected a ToolResult for the mcp tool call");
        assert!(is_error, "unknown mcp tool must surface as an error");
        assert!(
            output.contains("unknown mcp tool"),
            "expected 'unknown mcp tool' in output, got: {output}"
        );
    }

    #[tokio::test]
    async fn hard_cancel_kills_running_local_shell_and_balances() {
        use ulid::Ulid;
        use zoid_core::event::EventKind;
        use zoid_provider::{ProviderEvent, ToolCall};
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        // The model asks to run a long shell command, then would continue.
        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "shell".into(),
                args: serde_json::json!({ "command": "sleep 30" }),
            }),
            ProviderEvent::Done,
        ]));
        // Shared kill slot wired into both the tool list and the config.
        let kill = zoid_tools::KillSlot::new();
        let tools = std::sync::Arc::new(zoid_tools::registry_with_kill(kill.clone()));
        let mut cfg = chat_turn_config();
        cfg.kill = kill.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let graceful = CancellationToken::new();
        let hard = CancellationToken::new();
        // Fire hard shortly after the turn starts running the tool.
        let hard2 = hard.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            hard2.cancel();
        });
        let started = std::time::Instant::now();
        let out = run_agent_turn_cancellable(
            cfg,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(vec![]),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
            graceful,
            hard,
        )
        .await
        .unwrap();
        // The turn must end well before the 30s sleep would finish.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "hard-stop must not wait for the shell command"
        );
        // The tool call is answered with a killed result (balance preserved).
        let killed = out.iter().any(|e| {
            matches!(
                &e.kind,
                EventKind::ToolResult { id, output, .. }
                    if id == "call-1" && output.contains("[killed")
            )
        });
        assert!(
            killed,
            "the interrupted shell call must get a [killed] result"
        );
    }

    #[tokio::test]
    async fn hard_cancel_mid_batch_balances_remaining_calls() {
        use ulid::Ulid;
        use zoid_core::event::EventKind;
        use zoid_provider::{ProviderEvent, ToolCall};
        let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
        // Two tool calls arrive in one batch: the shell runs first (killed by
        // the hard-stop), the second must be drained with [skipped: turn aborted]
        // so no tool_use is left without a tool_result (balance invariant).
        let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![
            ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "shell".into(),
                args: serde_json::json!({ "command": "sleep 30" }),
            }),
            ProviderEvent::ToolCall(ToolCall {
                id: "call-2".into(),
                name: "read".into(),
                args: serde_json::json!({ "path": "Cargo.toml" }),
            }),
            ProviderEvent::Done,
        ]));
        let kill = zoid_tools::KillSlot::new();
        let tools = std::sync::Arc::new(zoid_tools::registry_with_kill(kill.clone()));
        let mut cfg = chat_turn_config();
        cfg.kill = kill.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let graceful = CancellationToken::new();
        let hard = CancellationToken::new();
        let hard2 = hard.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            hard2.cancel();
        });
        let started = std::time::Instant::now();
        let out = run_agent_turn_cancellable(
            cfg,
            provider,
            tools,
            std::sync::Arc::new(zoid_tools::AllowAll),
            session,
            crate::eventlog::EventLog::from_vec(vec![]),
            "m".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            || 0,
            graceful,
            hard,
        )
        .await
        .unwrap();
        // (a) The turn must not wait out the 30s sleep.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "hard-stop must not wait for the shell command"
        );
        // (b) The running shell call is answered with a killed result.
        let killed = out.iter().any(|e| {
            matches!(
                &e.kind,
                EventKind::ToolResult { id, output, .. }
                    if id == "call-1" && output.contains("[killed")
            )
        });
        assert!(
            killed,
            "the interrupted shell call must get a [killed] result"
        );
        // (c) The un-run second call is drained (this is the mid-batch drain path).
        let skipped = out.iter().any(|e| {
            matches!(
                &e.kind,
                EventKind::ToolResult { id, output, .. }
                    if id == "call-2" && output.contains("[skipped")
            )
        });
        assert!(
            skipped,
            "the remaining batched call must get a [skipped] result"
        );
    }

    #[test]
    fn resolve_agent_name_defaults_to_delegate_when_absent() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        // No "agent" key → default "delegate".
        let resolved = resolve_agent_for_dispatch(&serde_json::json!({}), reg.clone());
        let (profile, name) = resolved.expect("absent agent should resolve to delegate");
        assert_eq!(name, "delegate");
        assert_eq!(profile.name, "delegate");
    }

    #[test]
    fn resolve_agent_name_known_returns_that_profile() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        // "delegate" is always known.
        let resolved =
            resolve_agent_for_dispatch(&serde_json::json!({ "agent": "delegate" }), reg.clone());
        let (profile, name) = resolved.unwrap();
        assert_eq!(name, "delegate");
        assert_eq!(profile.name, "delegate");
    }

    #[test]
    fn resolve_agent_name_unknown_returns_err_listing_available() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let resolved =
            resolve_agent_for_dispatch(&serde_json::json!({ "agent": "typo-name" }), reg.clone());
        let err = resolved.expect_err("unknown agent should be Err");
        assert!(err.contains("unknown agent 'typo-name'"));
        assert!(
            err.contains("delegate"),
            "error should list available agents"
        );
    }

    #[test]
    fn resolve_agent_name_empty_string_defaults_to_delegate() {
        let reg = std::sync::Arc::new(zoid_core::agent_profile::AgentRegistry::builtin());
        let resolved = resolve_agent_for_dispatch(&serde_json::json!({ "agent": "" }), reg.clone());
        let (profile, name) = resolved.expect("empty agent string should resolve to delegate");
        assert_eq!(name, "delegate");
        assert_eq!(profile.name, "delegate");
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
                    name: "read".into(),
                    args: "{}".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                ts,
                EventKind::ToolResult {
                    id: "call-7".into(),
                    name: "read".into(),
                    output: "ok".into(),
                    is_error: false,
                },
            ),
        ]);

        let req = build_request(&events, "m", &[], "sys", None);

        let tool_msg = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MsgRole::Tool)
            .expect("tool message should be present");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call-7"));
    }

    #[test]
    fn ensure_tool_call_id_fills_empty_and_preserves_present() {
        // Present id is preserved verbatim.
        assert_eq!(ensure_tool_call_id("call_abc".to_string()), "call_abc");
        // Empty id becomes a non-empty, unique value each call.
        let a = ensure_tool_call_id(String::new());
        let b = ensure_tool_call_id(String::new());
        assert!(!a.is_empty() && !b.is_empty(), "empty id must be filled");
        assert_ne!(a, b, "two empty ids must not collide");
    }
}

#[cfg(test)]
mod guardrail_types_tests {
    use super::{format_subagent_list, AbortReason, SubagentHandle};
    use std::sync::atomic::AtomicI64;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn abort_reason_labels_are_stable() {
        assert_eq!(AbortReason::IdleTimeout.label(), "idle timeout");
        assert_eq!(AbortReason::Ceiling.label(), "hard timeout");
        assert_eq!(AbortReason::Killed.label(), "killed");
    }

    #[test]
    fn subagent_handle_is_constructible_and_clonable() {
        let h = SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
            task: String::new(),
            agent: String::new(),
        };
        let h2 = h.clone();
        h.hard.cancel();
        assert!(h2.hard.is_cancelled(), "clone shares the same token");
    }

    #[test]
    fn fire_kill_targets_one_or_all() {
        use super::fire_subagent_kill;
        use std::collections::HashMap;

        let mk = || SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
            task: String::new(),
            agent: String::new(),
        };
        let a = mk();
        let b = mk();
        let mut map = HashMap::new();
        map.insert("sub-a".to_string(), a.clone());
        map.insert("sub-b".to_string(), b.clone());
        let reg = Arc::new(Mutex::new(map));

        // Target one.
        let n = fire_subagent_kill(&reg, Some("sub-a"));
        assert_eq!(n, 1);
        assert!(a.hard.is_cancelled());
        assert_eq!(
            *a.abort_reason.lock().unwrap(),
            Some(super::AbortReason::Killed)
        );
        assert!(!b.hard.is_cancelled(), "untargeted subagent untouched");

        // Target all.
        let n = fire_subagent_kill(&reg, None);
        assert_eq!(n, 2, "None fires every registered subagent");
        assert!(b.hard.is_cancelled());
    }

    #[test]
    fn fire_kill_preserves_existing_reason() {
        use super::fire_subagent_kill;
        use std::collections::HashMap;
        let h = SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(Some(super::AbortReason::IdleTimeout))),
            task: String::new(),
            agent: String::new(),
        };
        let mut map = HashMap::new();
        map.insert("sub-a".to_string(), h.clone());
        let reg = Arc::new(Mutex::new(map));
        let n = fire_subagent_kill(&reg, None);
        assert_eq!(n, 1, "still fires the handle");
        assert!(h.hard.is_cancelled(), "hard token still fired");
        assert_eq!(
            *h.abort_reason.lock().unwrap(),
            Some(super::AbortReason::IdleTimeout),
            "a reason set by the timeout supervisor must NOT be overwritten by Killed"
        );
    }

    #[test]
    fn list_subagents_formats_id_and_task() {
        use std::collections::HashMap;
        use tokio_util::sync::CancellationToken;

        let mut map: HashMap<String, SubagentHandle> = HashMap::new();
        map.insert(
            "sub-001".into(),
            SubagentHandle {
                cancel: CancellationToken::new(),
                hard: CancellationToken::new(),
                progress: Arc::new(AtomicI64::new(0)),
                abort_reason: Arc::new(Mutex::new(None)),
                task: "implement the resolver".into(),
                agent: "delegate".into(),
            },
        );
        map.insert(
            "sub-002".into(),
            SubagentHandle {
                cancel: CancellationToken::new(),
                hard: CancellationToken::new(),
                progress: Arc::new(AtomicI64::new(0)),
                abort_reason: Arc::new(Mutex::new(None)),
                task: "review the spec".into(),
                agent: "reviewer".into(),
            },
        );

        // Non-empty: data + reminder
        let output = format_subagent_list(&map);
        assert!(output.contains("Running subagents (2)"));
        assert!(output.contains("sub-001 [delegate]: implement the resolver"));
        assert!(output.contains("sub-002 [reviewer]: review the spec"));
        assert!(
            output.contains("fire-and-forget"),
            "non-empty output must carry the no-poll reminder: {output}"
        );
        assert!(
            output.contains("do not poll"),
            "non-empty output must tell the model not to poll: {output}"
        );

        // Empty: no reminder
        let empty: HashMap<String, SubagentHandle> = HashMap::new();
        let output = format_subagent_list(&empty);
        assert_eq!(output, "No subagents currently running.");
        assert!(
            !output.contains("fire-and-forget"),
            "empty output must not carry the reminder: {output}"
        );
    }
}
