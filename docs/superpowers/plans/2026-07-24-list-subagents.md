# List Running Subagents — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the main chat agent a tool to see which subagents are currently
running, so it doesn't have to discover in-flight work via the "already running"
error. Today the agent has `dispatch_subagent`, `cancel_subagent`, and
`subagent_diff`, but no way to list what's in flight.

**Architecture:** A new `list_subagents` tool (`Emitting` kind, handled in the
agent loop — same pattern as `cancel_subagent`). `SubagentHandle` gains a
`task: String` field so the agent loop can show what each running subagent is
doing. The tool is registered in `chat_tools()` only (not the base `registry()`).

**Tech Stack:** Rust workspace (`zoid-tools` tool definitions; `zoid` agent
loop). `Emitting` tools are spec-only — their `run()` is unreachable; the agent
loop branches on `ToolKind::Emitting` before calling `run()`.

**Spec:** `docs/superpowers/specs/2026-07-24-list-subagents-design.md`

## Global Constraints

- **`SubagentHandle` is `#[derive(Clone)]`** (not `Eq`/`Debug`-only) — adding a
  `String` field is fine for `Clone`.
- **`Emitting` tool pattern** — the tool's `run()` is unreachable. The agent
  loop handles it. Follow the `CancelSubagent` (subagent_kill.rs) pattern
  exactly.
- **Chat-only** — `list_subagents` is NOT in `zoid_tools::registry()` or
  `registry_with_kill()`. It's added to `chat_tools()` only (subagents can't
  dispatch, so they can't list either).
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-tools/src/subagent_list.rs` | New `ListSubagents` tool (spec-only, `Emitting`) | Create |
| `crates/zoid-tools/src/lib.rs` | Add `pub mod subagent_list;` | Modify |
| `crates/zoid/src/agent.rs` | `SubagentHandle` gains `task: String`; new `list_subagents` arm in the Emitting match block; update the `SubagentHandle` construction at the dispatch site | Modify |
| `crates/zoid/src/invoke_skill.rs` | Register `ListSubagents` in `chat_tools()` | Modify |

**Task order:** T1 (tool + struct field + agent loop + registration) — this is
a single small task. All changes are interdependent (the struct field must exist
before the agent loop reads it; the tool must exist before it's registered; the
agent loop arm must exist before the tool is useful). One commit.

---

### Task 1: `ListSubagents` tool + `SubagentHandle.task` + agent loop arm

**Files:**
- Create: `crates/zoid-tools/src/subagent_list.rs`
- Modify: `crates/zoid-tools/src/lib.rs`
- Modify: `crates/zoid/src/agent.rs`
- Modify: `crates/zoid/src/invoke_skill.rs`

- [ ] **Step 1: Create `ListSubagents` tool**

Create `crates/zoid-tools/src/subagent_list.rs`:

```rust
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `list_subagents {}` — an Emitting tool the main Chat agent uses to see which
/// subagents are currently running. The agent loop reads the `in_flight`
/// registry and returns each subagent's id + task. No parameters.
pub struct ListSubagents;

impl Tool for ListSubagents {
    fn name(&self) -> &str {
        "list_subagents"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_subagents".into(),
            description: "List subagents that are currently running. Returns each \
                          subagent's id and task description. Call this to check \
                          in-flight work before dispatching or canceling."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("list_subagents is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(ListSubagents.name(), "list_subagents");
        assert_eq!(ListSubagents.kind(), ToolKind::Emitting);
    }

    #[test]
    fn spec_has_no_required_params() {
        let spec = ListSubagents.spec();
        assert_eq!(spec.name, "list_subagents");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.is_empty(), "list_subagents takes no params");
    }

    #[test]
    fn not_in_base_registry() {
        // Subagents must NOT be able to list subagents (they can't dispatch).
        assert!(
            !crate::registry().iter().any(|t| t.name() == "list_subagents"),
            "list_subagents must be chat-only, never in the subagent registry"
        );
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

Add `pub mod subagent_list;` to `crates/zoid-tools/src/lib.rs` (after
`pub mod subagent_kill;` at line 19):

```rust
pub mod subagent_kill;
pub mod subagent_list;
```

- [ ] **Step 3: Add `task: String` to `SubagentHandle` (agent.rs:100)**

Change:

```rust
#[derive(Clone)]
pub struct SubagentHandle {
    pub cancel: CancellationToken,
    pub hard: CancellationToken,
    pub progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    pub abort_reason: std::sync::Arc<std::sync::Mutex<Option<AbortReason>>>,
}
```

to:

```rust
#[derive(Clone)]
pub struct SubagentHandle {
    pub cancel: CancellationToken,
    pub hard: CancellationToken,
    pub progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    pub abort_reason: std::sync::Arc<std::sync::Mutex<Option<AbortReason>>>,
    /// The task description passed to `dispatch_subagent`. Used by
    /// `list_subagents` to show what each running subagent is doing.
    pub task: String,
}
```

- [ ] **Step 3b: Update all 4 test `SubagentHandle { ... }` literals**

Run: `grep -rn "SubagentHandle\s*{" crates/ | grep -v "fn \|struct \|pub struct\|impl " | grep -v target`

The production site (agent.rs:1617) is updated in Step 4. The 4 test sites
must add `task: String::new(),` (the task value is irrelevant to what they
assert):

- `crates/zoid/src/agent.rs:5240` — `subagent_handle_is_constructible_and_clonable`
- `crates/zoid/src/agent.rs:5256` — `fire_kill_targets_one_or_all` (the `mk` closure)
- `crates/zoid/src/agent.rs:5286` — `fire_kill_preserves_existing_reason`
- `crates/zoid/src/main.rs:6761` — `escalate_force_fires_registered_subagents`

- [ ] **Step 4: Set `task` at the dispatch site (agent.rs:1617)**

The `SubagentHandle` is constructed at agent.rs:1617. The `task` variable is
already in scope (parsed from the tool call args at agent.rs:1521). Add
`task: task.clone(),` to the struct literal:

```rust
                        reg.lock().unwrap().insert(
                            sub_id.clone(),
                            SubagentHandle {
                                cancel: sub_cancel.clone(),
                                hard: sub_hard.clone(),
                                progress: sub_progress.clone(),
                                abort_reason: sub_abort_reason.clone(),
                                task: task.clone(),
                            },
                        );
```

- [ ] **Step 5: Add the `list_subagents` arm in the agent loop**

After the `cancel_subagent` arm closes (agent.rs:1926 — the `}` before the
`Interactive` arm at 1927), add:

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
```

- [ ] **Step 6: Register `ListSubagents` in `chat_tools()` (invoke_skill.rs:95)**

After the `ListAgents` line (invoke_skill.rs:95), add:

```rust
    tools.push(Box::new(zoid_tools::subagent_list::ListSubagents));
```

- [ ] **Step 6b: Extend `chat_tools_includes_dispatch_and_diff` test**

In `crates/zoid/src/invoke_skill.rs`, find the `chat_tools_includes_dispatch_and_diff`
test (around line 203). Add an assertion for `list_subagents`:

```rust
    assert!(names.contains(&"list_subagents"), "chat_tools includes list_subagents");
```

- [ ] **Step 7: Build and test**

Run: `cargo build --workspace`
Expected: success.

Run: `cargo test -p zoid-tools -- subagent_list`
Expected: PASS (3 tests: name_and_kind, spec_has_no_required_params, not_in_base_registry).

Run: `cargo test -p zoid -- subagent`
Expected: PASS (existing subagent tests — `SubagentHandle` construction sites
updated, no regressions).

Run: `cargo test -p zoid -- chat_tools_includes`
Expected: PASS (the `chat_tools_includes_dispatch_and_diff` test — extend it
with a `list_subagents` assertion, see Step 6b).

Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success — full release gate.

- [ ] **Step 7b: Add agent-loop formatting test**

Add a test in `crates/zoid/src/agent.rs` (in the test module near the existing
`subagent_handle_is_constructible_and_clonable` test) that constructs an
`in_flight` map with `SubagentHandle` entries and verifies the formatting:

```rust
#[test]
fn list_subagents_formats_id_and_task() {
    use std::collections::HashMap;
    use tokio_util::sync::CancellationToken;

    let mut map: HashMap<String, SubagentHandle> = HashMap::new();
    map.insert("sub-001".into(), SubagentHandle {
        cancel: CancellationToken::new(),
        hard: CancellationToken::new(),
        progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
        task: "implement the resolver".into(),
    });
    map.insert("sub-002".into(), SubagentHandle {
        cancel: CancellationToken::new(),
        hard: CancellationToken::new(),
        progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
        task: "review the spec".into(),
    });

    // Format the output the same way the agent loop arm does.
    let mut lines = format!("Running subagents ({}):\n", map.len());
    for (id, handle) in map.iter() {
        lines.push_str(&format!("- {id}: {}\n", handle.task));
    }
    let output = lines.trim_end().to_string();

    assert!(output.contains("Running subagents (2)"));
    assert!(output.contains("sub-001: implement the resolver"));
    assert!(output.contains("sub-002: review the spec"));

    // Empty map → "No subagents currently running."
    let empty: HashMap<String, SubagentHandle> = HashMap::new();
    let output = if empty.is_empty() {
        "No subagents currently running.".to_string()
    } else {
        format!("Running subagents ({}):", empty.len())
    };
    assert_eq!(output, "No subagents currently running.");
}
```

Run: `cargo test -p zoid -- list_subagents_formats`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tools/src/subagent_list.rs crates/zoid-tools/src/lib.rs \
       crates/zoid/src/agent.rs crates/zoid/src/invoke_skill.rs
git commit -m "feat: list_subagents tool for in-flight subagent visibility

New Emitting tool (same pattern as cancel_subagent) that reads the
in_flight registry and returns each running subagent's id + task.
SubagentHandle gains task: String field, set at dispatch time.
Registered in chat_tools() only (subagents can't dispatch, so they
can't list either). The agent can now check in-flight work before
dispatching or canceling, instead of discovering it via the 'already
running' error."
```

---

## Self-Review

Run after the task: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
(AGENTS.md release gate). Confirm:
- `list_subagents` tool tests pass (name, kind, no params, not in base registry).
- `SubagentHandle` construction compiles with `task: task.clone()`.
- The agent loop `list_subagents` arm reads `in_flight` and formats id + task.
- `chat_tools()` includes `list_subagents`; `registry()` does not.
- No regressions in existing subagent tests.