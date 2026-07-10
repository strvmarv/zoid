//! Drives the terminal-free agent loop against a deterministic multi-turn fake
//! provider and the real tool registry, asserting the persisted event log.

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zoid::agent::{run_agent_turn, run_agent_turn_cancellable, AgentUpdate};
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall};

/// A provider that replays one scripted stream per `stream()` call, in order,
/// and records every received `CompletionRequest` for post-run assertions.
struct ScriptedProvider {
    turns: Mutex<std::collections::VecDeque<Vec<ProviderEvent>>>,
    /// Every request passed to `stream` is cloned here (lock never held across `.await`).
    requests: Mutex<Vec<CompletionRequest>>,
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        // Capture request and pop next script without holding either lock across .await.
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

struct DenyAll;
impl zoid_tools::ToolGate for DenyAll {
    fn check(&self, _c: &zoid_provider::ToolCall) -> zoid_tools::Gate {
        zoid_tools::Gate::Deny("denied by policy".into())
    }
}

#[tokio::test]
async fn agent_loop_runs_tool_then_finishes() {
    // Arrange a write_file in a tempdir so the tool actually executes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let path_str = path.to_str().unwrap().to_string();

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model calls write_file, then ends its turn.
            vec![
                zoid_testkit::tool_call("write", json!({ "path": path_str, "content": "hi" })),
                ProviderEvent::Done,
            ],
            // Turn 2: with the tool result in context, the model replies in text.
            vec![zoid_testkit::text("done"), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    // Use a fixed epoch-0 ULID so the seed always sorts before any Ulid::new()
    // generated inside run_agent_turn (which uses the current wall-clock time).
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "write hi".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    // Drain UI updates so the channel never blocks.
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider.clone(),
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

    let complete = drain.await.unwrap();
    assert!(complete, "loop must emit TurnComplete");

    // The tool actually ran.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hi");

    // The log records: UserMessage, ToolCall, ToolResult, ModelDelta.
    let log = session.snapshot().await.unwrap();
    let kinds: Vec<&EventKind> = log.iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], EventKind::UserMessage { .. }));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, EventKind::ToolCall { name, .. } if name == "write")));
    assert!(kinds.iter().any(|k| matches!(
        k,
        EventKind::ToolResult {
            is_error: false,
            ..
        }
    )));
    assert!(kinds
        .iter()
        .any(|k| matches!(k, EventKind::ModelDelta { text } if text == "done")));

    // The second request must include a Tool message, proving the tool result was fed back.
    let captured = provider.requests.lock().unwrap();
    assert_eq!(
        captured.len(),
        2,
        "expected exactly 2 provider requests (tool-call turn + follow-up)"
    );
    assert!(
        captured[1].messages.iter().any(|m| m.role == MsgRole::Tool),
        "second request must contain a MsgRole::Tool message (tool result fed back to model)"
    );
}

#[tokio::test]
async fn gate_deny_blocks_tool_and_feeds_reason_back() {
    // Arrange a write_file in a tempdir so we can assert it was NOT written.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    let path_str = path.to_str().unwrap().to_string();

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 1: the model calls write_file, then ends its turn.
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "write".into(),
                    args: json!({ "path": path_str, "content": "hi" }),
                }),
                ProviderEvent::Done,
            ],
            // Turn 2: with the (denied) tool result in context, the model replies in text.
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage {
            text: "write hi".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(DenyAll),
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

    let _ = drain.await;

    assert!(
        !path.exists(),
        "denied write_file must not touch the filesystem"
    );
    let denied = events.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ToolResult { output, is_error, .. }
            if *is_error && output.contains("denied by policy"))
    });
    assert!(denied, "a Deny must surface as an error ToolResult");
}

#[tokio::test]
async fn agent_loop_returns_ok_and_emits_turn_complete_on_error_event() {
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            // Provider sends an Error event instead of a normal response.
            vec![ProviderEvent::Error("boom".into())],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::new(),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    // Must return Ok (with the accumulated events) even though the provider sent an Error event.
    let result = run_agent_turn(
        zoid::agent::chat_turn_config(),
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
    .await;
    assert!(
        result.is_ok(),
        "run_agent_turn must return Ok(events) on a provider Error event"
    );

    let complete = drain.await.unwrap();
    assert!(
        complete,
        "TurnComplete must be emitted even on the error path"
    );

    // The event log must contain an AssistantMessage whose text includes the error string.
    let log = session.snapshot().await.unwrap();
    assert!(
        log.iter().any(
            |e| matches!(&e.kind, EventKind::AssistantMessage { text } if text.contains("boom"))
        ),
        "error text must be logged as an AssistantMessage"
    );
}

/// A provider that emits one tool call then holds the stream open (delaying its
/// `Done` far past the turn) so the agent's recv loop is parked on `select!`
/// when the test fires the cancel. Its own `Done` never arrives — the abort
/// path is the only exit, and the agent aborts this task, so the delay is never
/// actually awaited.
struct EmitToolCallThenStall {
    call: ProviderEvent,
}

#[async_trait]
impl Provider for EmitToolCallThenStall {
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> anyhow::Result<()> {
        let _ = sink.send(self.call.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok(())
    }
}

/// A stub Network tool whose `run_async` sleeps, for hard-cancel testing.
struct SlowNetworkTool;
impl zoid_tools::Tool for SlowNetworkTool {
    fn name(&self) -> &str { "slow_network" }
    fn spec(&self) -> zoid_provider::ToolSpec {
        zoid_provider::ToolSpec {
            name: "slow_network".into(),
            description: "test stub".into(),
            parameters: json!({"type":"object","properties":{}}),
        }
    }
    fn run(&self, _: &serde_json::Value, _: &std::path::Path) -> zoid_tools::ToolOutput {
        unreachable!("Network tool: run() never called")
    }
    fn kind(&self) -> zoid_tools::ToolKind { zoid_tools::ToolKind::Network }
    fn run_async(&self, _: &serde_json::Value, _: &std::path::Path)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = zoid_tools::ToolOutput> + Send + '_>>
    {
        Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            zoid_tools::ToolOutput::ok("done")
        })
    }
}

/// A stub Network tool whose `run_async` returns immediately, for the
/// happy-path test (asserts the ToolResult flows back like a Local tool).
struct FastNetworkTool;
impl zoid_tools::Tool for FastNetworkTool {
    fn name(&self) -> &str { "fast_network" }
    fn spec(&self) -> zoid_provider::ToolSpec {
        zoid_provider::ToolSpec {
            name: "fast_network".into(),
            description: "test stub".into(),
            parameters: json!({"type":"object","properties":{}}),
        }
    }
    fn run(&self, _: &serde_json::Value, _: &std::path::Path) -> zoid_tools::ToolOutput {
        unreachable!("Network tool: run() never called")
    }
    fn kind(&self) -> zoid_tools::ToolKind { zoid_tools::ToolKind::Network }
    fn run_async(&self, _: &serde_json::Value, _: &std::path::Path)
        -> std::pin::Pin<Box<dyn std::future::Future<Output = zoid_tools::ToolOutput> + Send + '_>>
    {
        Box::pin(async { zoid_tools::ToolOutput::ok("network-ok") })
    }
}

/// A tool registry containing only the given Network stub (the agent calls it
/// by name; no other tools are needed for these tests).
fn network_tool_registry(stub: Box<dyn zoid_tools::Tool>) -> Arc<Vec<Box<dyn zoid_tools::Tool>>> {
    Arc::new(vec![stub])
}

#[tokio::test]
async fn cancel_mid_stream_drains_pending_tool_calls_without_running_them() {
    // A cancel that lands after a tool call is recorded but before it executes
    // must (a) not run the tool and (b) still balance the call with a skipped
    // ToolResult, so the next request isn't malformed. We fire the cancel only
    // once the ToolCall's `Appended` update is observed — event-ordered, not
    // time-ordered — so the agent is provably parked in the recv `select!` with
    // the call already in `pending`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must_not_exist.txt");
    let path_str = path.to_str().unwrap().to_string();

    let cancel = CancellationToken::new();
    let provider = Arc::new(EmitToolCallThenStall {
        call: zoid_testkit::tool_call(
            "write",
            json!({ "path": path_str, "content": "should never be written" }),
        ),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let cancel_on_toolcall = cancel.clone();
    let drain = tokio::spawn(async move {
        let mut complete = false;
        let mut fired = false;
        while let Some(u) = rx.recv().await {
            if let AgentUpdate::Appended(ev) = &u {
                // The instant the tool call is recorded, request cancellation.
                if !fired && matches!(ev.kind, EventKind::ToolCall { .. }) {
                    cancel_on_toolcall.cancel();
                    fired = true;
                }
            }
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn_cancellable(
        zoid::agent::chat_turn_config(),
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
        cancel,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    let complete = drain.await.unwrap();
    assert!(
        complete,
        "TurnComplete must fire even when the turn is cancelled"
    );

    // The tool must NOT have executed.
    assert!(!path.exists(), "a cancelled tool call must not run");

    // The pending tool call was balanced with a skipped ToolResult.
    let log = session.snapshot().await.unwrap();
    assert!(
        log.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolResult { output, .. } if output == "[skipped: turn aborted]"
        )),
        "cancel must drain pending tool calls with a balanced skipped result"
    );
}

/// A Network tool whose run_async sleeps must be abandonable on a hard-stop:
/// the cancel yields a `[killed: hard-stop]` ToolResult, balanced so the next
/// request isn't malformed.
#[tokio::test]
async fn network_tool_hard_cancel_yields_killed_result() {
    // The hard token (Esc/Ctrl-C analog) fires when the tool call is observed.
    // The Network arm's select! listens on `hard.cancelled()`, not `cancel`.
    let cancel = CancellationToken::new();
    let hard = CancellationToken::new();
    let provider = Arc::new(EmitToolCallThenStall {
        call: zoid_testkit::tool_call("slow_network", json!({})),
    });
    let tools = network_tool_registry(Box::new(SlowNetworkTool));
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let hard_on_toolcall = hard.clone();
    let drain = tokio::spawn(async move {
        let mut complete = false;
        let mut fired = false;
        while let Some(u) = rx.recv().await {
            if let AgentUpdate::Appended(ev) = &u {
                if !fired && matches!(ev.kind, EventKind::ToolCall { .. }) {
                    hard_on_toolcall.cancel();
                    fired = true;
                }
            }
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn_cancellable(
        zoid::agent::chat_turn_config(),
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
        cancel,
        hard,
    )
    .await
    .unwrap();

    let complete = drain.await.unwrap();
    assert!(complete, "TurnComplete must fire even when the turn is cancelled");

    let log = session.snapshot().await.unwrap();
    assert!(
        log.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolResult { output, is_error, .. } if output == "[killed: hard-stop]" && *is_error
        )),
        "network tool hard-stop must yield a [killed: hard-stop] error ToolResult"
    );
}

/// A Network tool whose run_async returns immediately must flow its ToolResult
/// back like a Local tool (the success path).
#[tokio::test]
async fn network_tool_happy_path_flows_tool_result() {
    // No cancel — the tool returns immediately and the turn completes normally.
    // ScriptedProvider is constructed with its struct literal (no `new` ctor):
    // `turns` is a VecDeque of per-turn scripts (one turn here: [ToolCall, Done]);
    // `requests` is the capture sink (empty initially).
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![vec![
            ProviderEvent::ToolCall(ToolCall {
                id: String::new(),
                name: "fast_network".into(),
                args: json!({}),
            }),
            ProviderEvent::Done,
        ]])),
        requests: Mutex::new(vec![]),
    });
    let tools = network_tool_registry(Box::new(FastNetworkTool));
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
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

    let complete = drain.await.unwrap();
    assert!(complete, "TurnComplete must fire after the network tool returns");

    let log = session.snapshot().await.unwrap();
    assert!(
        log.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolResult { output, is_error, .. } if output == "network-ok" && !*is_error
        )),
        "network tool happy path must yield a network-ok ToolResult (got: {log:?})"
    );
}

#[tokio::test]
async fn truncated_event_surfaces_a_warning_and_still_completes() {
    // A Truncated event (model hit the output cap) must log an incomplete-reply
    // warning but NOT abort — the following Done still ends the turn cleanly.
    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![vec![
            zoid_testkit::text("partial ans"),
            ProviderEvent::Truncated,
            ProviderEvent::Done,
        ]])),
        requests: Mutex::new(Vec::new()),
    });
    let provider_probe = provider.clone();
    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            if matches!(u, AgentUpdate::TurnComplete) {
                complete = true;
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
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

    assert!(
        drain.await.unwrap(),
        "TurnComplete must fire after a truncation"
    );

    // Exactly one sub-turn: a truncation must not trigger a spurious re-request.
    assert_eq!(
        provider_probe.requests.lock().unwrap().len(),
        1,
        "truncation must not spawn a second sub-turn"
    );

    let log = session.snapshot().await.unwrap();
    let text_idx = log.iter().position(
        |e| matches!(&e.kind, EventKind::ModelDelta { text } if text.contains("partial ans")),
    );
    let warn_idx = log.iter().position(
        |e| matches!(&e.kind, EventKind::AssistantMessage { text } if text.contains("truncated")),
    );
    assert!(text_idx.is_some(), "the partial reply must be logged");
    assert!(
        warn_idx.is_some(),
        "a Truncated event must surface an incomplete-reply warning"
    );
    assert!(
        text_idx < warn_idx,
        "the truncation warning must come after the partial reply, not before"
    );
}

// --- Gate::Prompt integration tests ---

/// A gate that returns Gate::Prompt for shell calls with a dangerous command,
/// Allow otherwise.
struct PromptGate;
impl zoid_tools::ToolGate for PromptGate {
    fn check(&self, c: &zoid_provider::ToolCall) -> zoid_tools::Gate {
        if c.name == "shell" {
            let cmd = c.args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.contains("rm -rf") {
                return zoid_tools::Gate::Prompt {
                    question: format!("approve? {}", cmd),
                    choices: vec!["approve once".into(), "deny".into()],
                };
            }
        }
        zoid_tools::Gate::Allow
    }
}

#[tokio::test]
async fn gate_prompt_approve_runs_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proof.txt");
    let path_str = path.to_str().unwrap().to_string();
    let cmd = format!("rm -rf /tmp/nonexistent && echo hi > {}", path_str);

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "shell".into(),
                    args: json!({ "command": cmd }),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("done".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::AskUser { reply, .. } => {
                    let _ = reply.send(zoid::agent::Answer::Choice("approve once".into()));
                }
                AgentUpdate::TurnComplete => complete = true,
                _ => {}
            }
        }
        complete
    });

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(PromptGate),
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

    let complete = drain.await.unwrap();
    assert!(complete, "loop must emit TurnComplete");
    assert!(path.exists(), "approved command must have executed");
}

#[tokio::test]
async fn gate_prompt_deny_blocks_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must_not_exist.txt");
    let path_str = path.to_str().unwrap().to_string();
    let cmd = format!("rm -rf /tmp/nonexistent && echo hi > {}", path_str);

    let provider = Arc::new(ScriptedProvider {
        turns: Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                ProviderEvent::ToolCall(ToolCall {
                    id: "".into(),
                    name: "shell".into(),
                    args: json!({ "command": cmd }),
                }),
                ProviderEvent::Done,
            ],
            vec![ProviderEvent::TextDelta("ok".into()), ProviderEvent::Done],
        ])),
        requests: Mutex::new(Vec::new()),
    });

    let tools = Arc::new(zoid_tools::registry());
    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        ulid::Ulid::from(1u128),
        None,
        0,
        EventKind::UserMessage { text: "go".into() },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut complete = false;
        while let Some(u) = rx.recv().await {
            match u {
                AgentUpdate::AskUser { reply, .. } => {
                    let _ = reply.send(zoid::agent::Answer::Choice("deny".into()));
                }
                AgentUpdate::TurnComplete => complete = true,
                _ => {}
            }
        }
        complete
    });

    let events = run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        tools,
        Arc::new(PromptGate),
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

    let _ = drain.await;
    assert!(!path.exists(), "denied command must not execute");
    assert!(events.iter().any(|e| {
        matches!(&e.kind, EventKind::ToolResult { is_error, .. } if *is_error)
    }), "denied prompt must produce an error ToolResult");
}
