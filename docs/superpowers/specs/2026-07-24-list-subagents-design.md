# List Running Subagents — Design

> **Status:** DESIGN APPROVED (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** Visibility into in-flight subagents for the main agent.

---

## 1. Goal & scope

Give the main chat agent a tool to see which subagents are currently running, so it doesn't have to guess or discover via the "already running" error. Today the agent has `dispatch_subagent`, `cancel_subagent`, and `subagent_diff`, but no way to list what's in flight.

**In scope:**
- A new `list_subagents` tool (`Emitting` kind, handled in the agent loop — same pattern as `dispatch_subagent`/`cancel_subagent`).
- `task: String` field added to `SubagentHandle` so the agent loop can show what each running subagent is doing.
- Tool registered in `chat_tools()` only (not the base `registry()` — subagents can't dispatch, so they can't list either).

**Out of scope:**
- Progress/heartbeat display (the `progress` atomic is available on `SubagentHandle` but not surfaced in this slice).
- Polling or push notifications — the agent calls `list_subagents` when it wants to check.
- Subagent agent-name in the output (the `in_flight` map is keyed by id, not by agent profile).

---

## 2. Architecture

### 2.1 `SubagentHandle` gains `task: String` (agent.rs:100)

```rust
#[derive(Clone)]
pub struct SubagentHandle {
    pub cancel: CancellationToken,
    pub hard: CancellationToken,
    pub progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    pub abort_reason: std::sync::Arc<std::sync::Mutex<Option<AbortReason>>>,
    pub task: String,  // NEW — set at dispatch time, read by list_subagents
}
```

Set at the insertion site (agent.rs:1601-1609) where the handle is constructed. The `task` variable is already in scope (parsed from the tool call args at agent.rs:1501).

### 2.2 `ListSubagents` tool (crates/zoid-tools/src/subagent_list.rs)

A spec-only `Emitting` tool — same structure as `CancelSubagent` (subagent_kill.rs). Its `run()` is unreachable; the agent loop branches on `Emitting` before calling `run()`.

```rust
pub struct ListSubagents;

impl Tool for ListSubagent {
    fn name(&self) -> &str { "list_subagents" }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_subagents".into(),
            description: "List subagents that are currently running. Returns each \
                          subagent's id and task description. Call this to check \
                          in-flight work before dispatching or canceling."
                .into(),
            parameters: json!({"type": "object", "properties": {}, "required": []}),
        }
    }
    fn kind(&self) -> ToolKind { ToolKind::Emitting }
    fn run(&self, _: &Value, _: &Path) -> ToolOutput {
        ToolOutput::err("list_subagents is executed by the agent loop")
    }
}
```

### 2.3 Agent loop handler (agent.rs, Emitting match block)

New arm after `cancel_subagent` (agent.rs:1911):

```rust
Some(zoid_tools::ToolKind::Emitting) if tc.name == "list_subagents" => {
    let output = if let Some(reg) = &config.in_flight {
        let map = reg.lock().unwrap();
        if map.is_empty() {
            "No subagents currently running.".to_string()
        } else {
            let mut lines = format!("Running subagents ({}):\n", map.len());
            for (id, handle) in map.iter() {
                lines.push_str(&format!("- {id}: {}\n", handle.task));
            }
            lines.trim_end().to_string()
        }
    } else {
        "No subagents currently running.".to_string()
    };
    emit(
        &session, &mut events, ui, &config.branch,
        EventKind::ToolResult {
            id: tc.id, name: tc.name, output, is_error: false,
        },
        session_id, now,
    ).await?;
}
```

### 2.4 Tool registration (invoke_skill.rs:95)

Add to `chat_tools()`, alongside `ListAgents`:

```rust
tools.push(Box::new(zoid_tools::subagent_list::ListSubagents));
```

Not in `zoid_tools::registry()` — subagents never see it (same gate as `cancel_subagent`).

---

## 3. Testing

- **Tool spec test:** name, kind (`Emitting`), empty parameters, not in base `registry()` (mirrors `subagent_kill.rs` tests).
- **Agent loop test:** dispatch a subagent (or mock `in_flight` with a handle), call `list_subagents`, assert the output contains the id + task. Empty `in_flight` ⇒ "No subagents currently running."

---

## 4. Cross-crate impact

- `SubagentHandle` is in `zoid` (agent.rs) — adding a field breaks every `SubagentHandle { ... }` literal. There's one construction site (agent.rs:1603) and any test constructions. The `task` variable is already in scope at the construction site.
- `ListSubagents` is a new file in `zoid-tools` — add `pub mod subagent_list;` to `lib.rs` and the tool to `chat_tools()`.
- `cargo build --workspace && cargo test --workspace` after each task.