# Subagent Dispatch Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `dispatch_subagent` and `subagent_diff` tools to zoid so the model can spawn subagents during a turn and retrieve their diffs, making SDD functional without `:delegate`.

**Architecture:** Two new tools in `zoid-tools`: `dispatch_subagent` (Emitting — the agent loop spawns `run_subagent` via `tokio::spawn` and returns the subagent ID immediately) and `subagent_diff` (Local — runs `git diff` synchronously). The `DelegationResult` event gains a `subagent_id` field for correlation. The single `app.delegating: bool` becomes an in-flight set. Skill files in the fork are updated to reference the tools.

**Tech Stack:** Rust (tokio, anyhow, serde, ulid), git, markdown.

## Global Constraints

- `dispatch_subagent` is `Emitting` kind — `run()` never called; the agent loop executes it inline.
- `subagent_diff` is `Local` kind — `run()` executes synchronously.
- Both tools are chat-only: added to `chat_tools()`, NOT to the base `registry()` that subagents receive.
- Subagents cannot call `dispatch_subagent` (no nested dispatch).
- `:delegate` stays for the user; it refuses if any subagent is in flight.
- `run_subagent`'s context construction and eviction policy are unchanged.
- The `DelegationResult` event gains `subagent_id: String` — all existing callers must populate it.
- Commit to the zoid repo (`~/source/zoid`) `main` branch. One commit per task.

---

## Task 1: Add `subagent_id` to `DelegationResult` and `SubagentResult`

**Files:**
- Modify: `crates/zoid-core/src/event.rs` (the `DelegationResult` variant + its test)
- Modify: `crates/zoid/src/subagent.rs` (the `SubagentResult` struct + `run_subagent` return)

**Interfaces:**
- Produces: `DelegationResult { subagent_id, branch, summary, ok }` and `SubagentResult { id, branch, summary, ok }`.

- [ ] **Step 1: Write the failing test**

Add a test in `crates/zoid-core/src/event.rs` that constructs a `DelegationResult` with `subagent_id` and verifies it round-trips through serde:

```rust
#[test]
fn delegation_result_with_subagent_id_round_trips() {
    let ev = Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::DelegationResult {
            subagent_id: "sub-01HZTEST".into(),
            branch: "subagent:01HZTEST".into(),
            summary: "did it".into(),
            ok: true,
        },
    );
    let json = serde_json::to_string(&ev).unwrap();
    let restored: Event = serde_json::from_str(&json).unwrap();
    match &restored.kind {
        EventKind::DelegationResult { subagent_id, .. } => {
            assert_eq!(subagent_id, "sub-01HZTEST");
        }
        _ => panic!("expected DelegationResult"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/source/zoid && cargo test -p zoid-core delegation_result_with_subagent_id -- --nocapture 2>&1 | tail -5
```
Expected: compile error — `DelegationResult` has no `subagent_id` field.

- [ ] **Step 3: Add `subagent_id` to the `DelegationResult` variant**

In `crates/zoid-core/src/event.rs`, find the `DelegationResult` variant:

```rust
    DelegationResult {
        branch: String,
        summary: String,
        ok: bool,
    },
```

Replace with:

```rust
    DelegationResult {
        subagent_id: String,
        branch: String,
        summary: String,
        ok: bool,
    },
```

- [ ] **Step 4: Fix the existing `delegation_result_round_trips` test**

In `crates/zoid-core/src/event.rs`, find the existing test `delegation_result_round_trips` and add the `subagent_id` field:

```rust
    fn delegation_result_round_trips() {
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::DelegationResult {
                subagent_id: "sub-test".into(),
                branch: "subagent:zz".into(),
                summary: "did it".into(),
                ok: false,
            },
        );
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(ev, serde_json::from_str::<Event>(&json).unwrap());
    }
```

- [ ] **Step 5: Add `id` to `SubagentResult`**

In `crates/zoid/src/subagent.rs`, find the struct:

```rust
pub struct SubagentResult {
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}
```

Replace with:

```rust
pub struct SubagentResult {
    pub id: String,
    pub branch: String,
    pub summary: String,
    pub ok: bool,
}
```

- [ ] **Step 6: Populate `id` in `run_subagent`'s return**

In `crates/zoid/src/subagent.rs`, find the `Ok(SubagentResult { ... })` at the end of `run_subagent`. The `branch` is `branch.0.clone()`. The `id` should be a `sub-` prefixed ULID. Find where the branch is created:

```rust
    let branch = BranchId(format!("subagent:{}", Ulid::new()));
```

Replace with (extract the ULID so both `id` and `branch` share it):

```rust
    let sub_ulid = Ulid::new();
    let sub_id = format!("sub-{sub_ulid}");
    let branch = BranchId(format!("subagent:{sub_ulid}"));
```

Then find the return at the end of `run_subagent`:

```rust
    Ok(SubagentResult {
        branch: branch.0,
        summary,
        ok,
    })
```

Replace with:

```rust
    Ok(SubagentResult {
        id: sub_id,
        branch: branch.0,
        summary,
        ok,
    })
```

- [ ] **Step 7: Fix all other `SubagentResult` constructions in tests**

In `crates/zoid/src/subagent.rs` tests, find any `SubagentResult { ... }` construction and add `id:`. Search:

```bash
cd ~/source/zoid && grep -n "SubagentResult {" crates/zoid/src/subagent.rs
```

Add `id: "sub-test".into(),` to each.

- [ ] **Step 8: Fix `start_delegation` in main.rs**

In `crates/zoid/src/main.rs`, find the `start_delegation` function's result handling:

```rust
        let (branch, summary, ok) = match res {
            Ok(r) => (r.branch, r.summary, r.ok),
            Err(e) => (String::new(), format!("delegation failed: {e}"), false),
        };
```

Replace with:

```rust
        let (subagent_id, branch, summary, ok) = match res {
            Ok(r) => (r.id, r.branch, r.summary, r.ok),
            Err(e) => (
                String::new(),
                String::new(),
                format!("delegation failed: {e}"),
                false,
            ),
        };
```

And find the `DelegationResult` event construction below it:

```rust
        let ev = Event::new(
            Ulid::new(),
            None,
            now_ms(),
            EventKind::DelegationResult {
                branch,
                summary,
                ok,
            },
        )
        .with_session(session_id);
```

Replace with:

```rust
        let ev = Event::new(
            Ulid::new(),
            None,
            now_ms(),
            EventKind::DelegationResult {
                subagent_id,
                branch,
                summary,
                ok,
            },
        )
        .with_session(session_id);
```

- [ ] **Step 9: Fix any other `DelegationResult` constructions**

Search for all `DelegationResult` constructions across the codebase:

```bash
cd ~/source/zoid && grep -rn "DelegationResult {" crates/ --include="*.rs" | grep -v "EventKind::DelegationResult" | grep -v "test"
```

Fix each to include `subagent_id`. Also fix test constructions:

```bash
cd ~/source/zoid && grep -rn "DelegationResult {" crates/ --include="*.rs"
```

- [ ] **Step 10: Run all tests to verify they pass**

```bash
cd ~/source/zoid && cargo test -p zoid-core delegation_result 2>&1 | tail -5
cd ~/source/zoid && cargo test -p zoid subagent 2>&1 | tail -5
cd ~/source/zoid && cargo build 2>&1 | tail -5
```
Expected: all pass, build succeeds.

- [ ] **Step 11: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "feat: add subagent_id to DelegationResult and SubagentResult"
```

---

## Task 2: Create the `dispatch_subagent` tool

**Files:**
- Create: `crates/zoid-tools/src/subagent_dispatch.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add module + register in `registry()`)

**Interfaces:**
- Produces: a `DispatchSubagent` tool struct, `Emitting` kind, with `task`/`worktree`/`model` args.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-tools/src/subagent_dispatch.rs` with the test first:

```rust
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct DispatchSubagent;

impl Tool for DispatchSubagent {
    fn name(&self) -> &str {
        "dispatch_subagent"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "dispatch_subagent".into(),
            description: "Dispatch a subagent to execute a task in isolation. Returns the subagent's \
                          ID immediately; the result arrives later as a DelegationResult event. Use \
                          worktree: true for file isolation when subagents might edit the same files."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The task description for the subagent" },
                    "worktree": { "type": "boolean", "description": "Isolate in a git worktree (default: false)", "default": false },
                    "model": { "type": "string", "description": "Model override; omit to inherit the session model" }
                },
                "required": ["task"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("dispatch_subagent is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_subagent_spec_and_kind() {
        assert_eq!(DispatchSubagent.name(), "dispatch_subagent");
        assert_eq!(DispatchSubagent.spec().name, "dispatch_subagent");
        assert_eq!(DispatchSubagent.kind(), ToolKind::Emitting);
        let params = DispatchSubagent.spec().parameters;
        assert_eq!(params["required"][0], "task");
        assert!(params["properties"]["worktree"]["default"].is_boolean());
        assert!(params["properties"]["model"].is_object());
    }
}
```

- [ ] **Step 2: Register the module and tool**

In `crates/zoid-tools/src/lib.rs`, add the module declaration (after `pub mod search;`):

```rust
pub mod subagent_dispatch;
```

And in `registry()`, add the tool. But note: `dispatch_subagent` is **chat-only** — it should NOT be in the base `registry()` that subagents receive. It gets added in `chat_tools()` instead. So do NOT add it to `registry()`. Only add the module declaration.

- [ ] **Step 3: Run test to verify it passes**

```bash
cd ~/source/zoid && cargo test -p zoid-tools subagent_dispatch 2>&1 | tail -5
```
Expected: PASS

- [ ] **Step 4: Add a test that the base registry excludes dispatch_subagent**

In `crates/zoid-tools/src/lib.rs` tests, add:

```rust
#[test]
fn registry_excludes_chat_only_tools() {
    let reg = registry();
    assert!(!reg.iter().any(|t| t.name() == "dispatch_subagent"), "dispatch_subagent must not be in base registry (subagents can't dispatch)");
    assert!(!reg.iter().any(|t| t.name() == "subagent_diff"), "subagent_diff must not be in base registry");
}
```

- [ ] **Step 5: Run the test**

```bash
cd ~/source/zoid && cargo test -p zoid-tools registry_excludes_chat_only -- --nocapture 2>&1 | tail -5
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "feat: add dispatch_subagent tool (Emitting, chat-only)"
```

---

## Task 3: Create the `subagent_diff` tool

**Files:**
- Create: `crates/zoid-tools/src/subagent_diff.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add module declaration)

**Interfaces:**
- Produces: a `SubagentDiff` tool struct, `Local` kind, with `subagent_id` arg. Runs `git log` + `git diff` for the subagent's branch.

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-tools/src/subagent_diff.rs`:

```rust
use crate::{Tool, ToolKind, ToolOutput, str_arg};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use zoid_provider::ToolSpec;

pub struct SubagentDiff;

impl Tool for SubagentDiff {
    fn name(&self) -> &str {
        "subagent_diff"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_diff".into(),
            description: "Retrieve the git diff for a completed subagent's branch. Returns the \
                          commit list, stat summary, and full diff. Use after a DelegationResult \
                          event arrives to review what the subagent changed."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subagent_id": { "type": "string", "description": "The subagent ID returned by dispatch_subagent (e.g. 'sub-01HZ...')" }
                },
                "required": ["subagent_id"]
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Local
    }
    fn run(&self, args: &Value, cwd: &Path) -> ToolOutput {
        let id = match str_arg(args, "subagent_id") {
            Ok(s) => s,
            Err(e) => return e,
        };
        // The subagent ID is "sub-<ULID>"; the branch is "subagent:<ULID>".
        // Strip the "sub-" prefix and build the branch ref.
        let ulid = id.strip_prefix("sub-").unwrap_or(&id);
        let branch = format!("subagent:{ulid}");

        // Verify the branch exists.
        let verify = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &branch])
            .current_dir(cwd)
            .output();
        match verify {
            Ok(o) if !o.status.success() => {
                return ToolOutput::err(format!(
                    "subagent {id} history not found — it may have been cleaned up."
                ));
            }
            Err(e) => {
                return ToolOutput::err(format!("git rev-parse failed: {e}"));
            }
            _ => {}
        }

        // Gather the diff: commit list + stat + full diff.
        // Use merge-base to diff only what the subagent committed, not working-tree changes.
        let merge_base = Command::new("git")
            .args(["merge-base", "HEAD", &branch])
            .current_dir(cwd)
            .output();
        let base = match merge_base {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => branch.clone(), // fall back to diffing the branch itself
        };
        let range = format!("{base}..{branch}");
        let log = Command::new("git")
            .args(["log", "--oneline", &range])
            .current_dir(cwd)
            .output();
        let stat = Command::new("git")
            .args(["diff", "--stat", &range])
            .current_dir(cwd)
            .output();
        let diff = Command::new("git")
            .args(["diff", "-U10", &range])
            .current_dir(cwd)
            .output();

        let mut out = String::new();
        if let Ok(o) = log {
            out.push_str("## Commits\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
            out.push('\n');
        }
        if let Ok(o) = stat {
            out.push_str("## Files changed\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
            out.push('\n');
        }
        if let Ok(o) = diff {
            out.push_str("## Diff\n");
            out.push_str(&String::from_utf8_lossy(&o.stdout));
        }
        if out.trim().is_empty() {
            ToolOutput::ok(format!("subagent {id} — no changes on branch {branch}"))
        } else {
            ToolOutput::ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_diff_spec_and_kind() {
        assert_eq!(SubagentDiff.name(), "subagent_diff");
        assert_eq!(SubagentDiff.spec().name, "subagent_diff");
        assert_eq!(SubagentDiff.kind(), ToolKind::Local);
        let params = SubagentDiff.spec().parameters;
        assert_eq!(params["required"][0], "subagent_id");
    }

    #[test]
    fn subagent_diff_missing_id_is_error() {
        let out = SubagentDiff.run(&json!({}), Path::new("."));
        assert!(out.is_error);
        assert!(out.text.contains("subagent_id"));
    }

    #[test]
    fn subagent_diff_nonexistent_branch_is_error() {
        let out = SubagentDiff.run(
            &json!({"subagent_id": "sub-NONEXISTENT123456"}),
            Path::new("."),
        );
        assert!(out.is_error);
        assert!(out.text.contains("not found"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/zoid-tools/src/lib.rs`, add the module declaration (after `pub mod subagent_dispatch;`):

```rust
pub mod subagent_diff;
```

Do NOT add it to `registry()` — it's chat-only, added in `chat_tools()`.

- [ ] **Step 3: Run tests to verify they pass**

```bash
cd ~/source/zoid && cargo test -p zoid-tools subagent_diff 2>&1 | tail -10
```
Expected: spec/kind test passes, missing-id test passes, nonexistent-branch test passes (if in a git repo).

- [ ] **Step 4: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "feat: add subagent_diff tool (Local, git diff retrieval)"
```

---

## Task 4: Wire both tools into `chat_tools`

**Files:**
- Modify: `crates/zoid/src/invoke_skill.rs` (add both tools to `chat_tools()`)

**Interfaces:**
- Consumes: `DispatchSubagent` and `SubagentDiff` from Tasks 2-3.
- Produces: both tools available to the model during chat turns.

- [ ] **Step 1: Add both tools to `chat_tools()`**

In `crates/zoid/src/invoke_skill.rs`, find `chat_tools()`:

```rust
pub fn chat_tools(skills: Arc<SkillRegistry>) -> Vec<Box<dyn Tool>> {
    let mut tools = zoid_tools::registry();
    tools.push(Box::new(InvokeSkillTool::new(skills)));
    // `recall` is always offered in chat ...
    tools.push(Box::new(zoid_tools::recall::Recall));
    // `show` renders an HTML card ...
    tools.push(Box::new(zoid_tools::show::Show));
    tools
}
```

Replace with:

```rust
pub fn chat_tools(skills: Arc<SkillRegistry>) -> Vec<Box<dyn Tool>> {
    let mut tools = zoid_tools::registry();
    tools.push(Box::new(InvokeSkillTool::new(skills)));
    // `recall` is always offered in chat ...
    tools.push(Box::new(zoid_tools::recall::Recall));
    // `show` renders an HTML card ...
    tools.push(Box::new(zoid_tools::show::Show));
    // `dispatch_subagent` lets the model spawn subagents during a turn (chat-only;
    // never in the subagent registry — subagents can't spawn subagents).
    tools.push(Box::new(zoid_tools::subagent_dispatch::DispatchSubagent));
    // `subagent_diff` retrieves a completed subagent's diff for review.
    tools.push(Box::new(zoid_tools::subagent_diff::SubagentDiff));
    tools
}
```

- [ ] **Step 2: Verify the build**

```bash
cd ~/source/zoid && cargo build 2>&1 | tail -5
```
Expected: build succeeds.

- [ ] **Step 3: Verify the tools are in the chat tool set**

```bash
cd ~/source/zoid && cargo test -p zoid chat_tools -- --nocapture 2>&1 | tail -10
```

Add a quick test in `crates/zoid/src/invoke_skill.rs` tests if none exists for tool presence:

```rust
#[test]
fn chat_tools_includes_dispatch_and_diff() {
    let tools = chat_tools(std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin()));
    assert!(tools.iter().any(|t| t.name() == "dispatch_subagent"), "dispatch_subagent in chat_tools");
    assert!(tools.iter().any(|t| t.name() == "subagent_diff"), "subagent_diff in chat_tools");
}
```

- [ ] **Step 4: Run the test**

```bash
cd ~/source/zoid && cargo test -p zoid chat_tools_includes 2>&1 | tail -5
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "feat: wire dispatch_subagent and subagent_diff into chat_tools"
```

---

## Task 5: Add the `dispatch_subagent` Emitting arm to the agent loop

**Files:**
- Modify: `crates/zoid/src/agent.rs` (new `Emitting` arm in `run_turn_inner`)
- Modify: `crates/zoid/src/subagent.rs` (accept an optional `id` parameter in `run_subagent`)

**Interfaces:**
- Consumes: `run_subagent`, `create_worktree`, `AgentProfile::builtin()` from existing code.
- Produces: when the model calls `dispatch_subagent`, the loop spawns the subagent and returns the ID as a tool result.

This is the core task. The Emitting arm needs to:
1. Parse `task`, `worktree`, `model` from the tool call args.
2. Generate a subagent ID.
3. If `worktree: true`, create a worktree (absolute path).
4. Spawn `run_subagent` via `tokio::spawn` (fire-and-forget), passing the generated ID so the `DelegationResult` event carries the same ID the in-flight set tracks.
5. Emit a `ToolResult` with the subagent ID immediately.
6. The spawned task emits `DelegationResult` when it completes (same as `start_delegation`).

**Subagent ID sharing:** Both `dispatch_subagent` (Task 5) and `start_delegation` (Task 6) must use the SAME ID for the in-flight set and the `DelegationResult` event. The fix: `run_subagent` accepts an `id: String` parameter and uses it instead of generating its own. This way the caller controls the ID.

- [ ] **Step 0: Modify `run_subagent` to accept an `id` parameter**

In `crates/zoid/src/subagent.rs`, change `run_subagent`'s signature to accept an `id: String` parameter. Find:

```rust
pub async fn run_subagent(
    task: &str,
    context_events: &crate::eventlog::EventLog,
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
```

Replace with (add `id: String` parameter, use it for the branch):

```rust
pub async fn run_subagent(
    task: &str,
    context_events: &crate::eventlog::EventLog,
    profile: &AgentProfile,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    default_model: String,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
    id: String,
) -> Result<SubagentResult> {
    let sub_ulid = id.strip_prefix("sub-").unwrap_or(&id).to_string();
    let branch = BranchId(format!("subagent:{sub_ulid}"));
```

And the return:

```rust
    Ok(SubagentResult {
        id: id,
        branch: branch.0,
        summary,
        ok,
    })
```

Fix all callers of `run_subagent` (in `subagent.rs` tests and `main.rs`'s `start_delegation`) to pass an `id` argument. In tests: `id: "sub-test".into()`. In `start_delegation`: `id: sub_id.clone()` (see Task 6).

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/agent.rs` tests, add a test that dispatches a subagent via the tool and verifies a `ToolResult` with a subagent ID is emitted:

```rust
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
        EventKind::UserMessage { text: "dispatch a subagent".into() },
    )];
    for e in &seed {
        session.append(e.clone()).await.unwrap();
    }

    // The model calls dispatch_subagent, then on the next sub-turn says "done".
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

    // The tool result must contain a subagent ID.
    let tool_result = out.iter().find(|e|
        matches!(&e.kind, EventKind::ToolResult { name, .. } if name == "dispatch_subagent")
    ).expect("dispatch_subagent tool result must be emitted");
    match &tool_result.kind {
        EventKind::ToolResult { output, is_error, .. } => {
            assert!(!*is_error, "dispatch should not error");
            assert!(output.contains("sub-"), "result must contain subagent ID: got {output}");
        }
        _ => panic!(),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/source/zoid && cargo test -p zoid dispatch_subagent_returns_id -- --nocapture 2>&1 | tail -10
```
Expected: FAIL — no Emitting arm for `dispatch_subagent` exists; the tool falls through to the Local arm which calls `run()` and returns the error "dispatch_subagent is executed by the agent loop".

- [ ] **Step 3: Add the Emitting arm**

In `crates/zoid/src/agent.rs`, find the `show` Emitting arm:

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "show" => {
```

After the entire `show` arm (its closing `}`), add the `dispatch_subagent` arm:

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "dispatch_subagent" => {
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
                    let model_override = tc
                        .args
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    let sub_ulid = Ulid::new();
                    let sub_id = format!("sub-{sub_ulid}");

                    // Worktree isolation (optional).
                    let wt = if want_worktree && std::path::Path::new(".git").exists() {
                        match crate::worktree::create_worktree(
                            std::path::Path::new("."),
                            &format!("sub-{sub_ulid}"),
                        ) {
                            Ok(w) => Some(w),
                            Err(_) => None,
                        }
                    } else {
                        None
                    };
                    let cwd = wt
                        .as_ref()
                        .map(|w| {
                            std::fs::canonicalize(w.path()).unwrap_or_else(|_| w.path().to_path_buf())
                        })
                        .unwrap_or_else(|| config.cwd.clone());

                    let provider = provider.clone();
                    let session = session.clone();
                    let seed = events.snapshot();
                    let sub_model = model_override.unwrap_or_else(|| model.clone());
                    let sub_ui = ui.clone();
                    let sub_session_id = session_id;
                    tokio::spawn(async move {
                        let res = crate::subagent::run_subagent(
                            &task,
                            &seed,
                            &zoid_core::agent_profile::AgentProfile::builtin(),
                            provider,
                            cwd,
                            sub_model,
                            session.clone(),
                            sub_session_id,
                            sub_ui.clone(),
                            now,
                            sub_id.clone(),
                        )
                        .await;
                        drop(wt);

                        let (subagent_id, branch, summary, ok) = match res {
                            Ok(r) => (r.id, r.branch, r.summary, r.ok),
                            Err(e) => (
                                String::new(),
                                String::new(),
                                format!("subagent failed: {e}"),
                                false,
                            ),
                        };
                        let ev = Event::new(
                            Ulid::new(),
                            None,
                            now(),
                            EventKind::DelegationResult {
                                subagent_id,
                                branch,
                                summary,
                                ok,
                            },
                        )
                        .with_session(sub_session_id);
                        let _ = session.append(ev.clone()).await;
                        let _ = sub_ui
                            .send(crate::agent::AgentUpdate::Appended(Box::new(ev)))
                            .await;
                    });

                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: format!("{{\"subagent_id\": \"{sub_id}\"}}"),
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
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cd ~/source/zoid && cargo test -p zoid dispatch_subagent_returns_id -- --nocapture 2>&1 | tail -10
```
Expected: PASS

- [ ] **Step 5: Run the full test suite**

```bash
cd ~/source/zoid && cargo test -p zoid 2>&1 | tail -10
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "feat: add dispatch_subagent Emitting arm to agent loop"
```

---

## Task 6: Replace `app.delegating: bool` with in-flight subagent tracking

**Files:**
- Modify: `crates/zoid/src/main.rs` (replace the field + all references)

**Interfaces:**
- Consumes: the `DelegationResult` event from Task 1.
- Produces: concurrent subagent tracking via a `HashSet<String>` of in-flight IDs.

- [ ] **Step 1: Replace the field**

In `crates/zoid/src/main.rs`, find the `delegating: bool` field in `struct App`:

```rust
    delegating: bool,
```

Replace with:

```rust
    in_flight_subagents: std::collections::HashSet<String>,
```

- [ ] **Step 2: Fix the initializer**

Find `delegating: false,` in the `App { ... }` construction (there are two — one in `main()` and one in `test_app()`). Replace both with:

```rust
        in_flight_subagents: std::collections::HashSet::new(),
```

- [ ] **Step 3: Fix all `app.delegating` references**

Search for every use of `app.delegating`:

```bash
cd ~/source/zoid && grep -n "app.delegating\|\.delegating" crates/zoid/src/main.rs
```

Replace each according to these rules:

1. **`app.shell.busy = app.streaming || app.delegating;`** →
   ```rust
   app.shell.busy = app.streaming || !app.in_flight_subagents.is_empty();
   ```

2. **`app.delegating = false;`** (in the `DelegationResult` handler in `run()`) →
   ```rust
   app.in_flight_subagents.clear();
   ```
   (Actually, this should remove only the matching ID, but since we don't have the ID at this point in the event handler yet — we do: the event carries `subagent_id` now. Find the handler:)
   
   Find:
   ```rust
   if matches!(ev.kind, EventKind::DelegationResult { .. }) {
       app.delegating = false;
       app.shell.status_hint = None;
   }
   ```
   Replace with:
   ```rust
   if let EventKind::DelegationResult { subagent_id, .. } = &ev.kind {
       app.in_flight_subagents.remove(subagent_id);
       if app.in_flight_subagents.is_empty() {
           app.shell.status_hint = None;
       }
   }
   ```

3. **`app.streaming || app.delegating`** (in motion_tick guard, Submit guard, SessionPick guard, NewSession guard, start_delegation) → replace all with `app.streaming || !app.in_flight_subagents.is_empty()`.

4. **`app.delegating = true;`** (in `start_delegation`) → `start_delegation` must use the SAME subagent ID that `run_subagent` returns, so the in-flight set matches the `DelegationResult` event. The cleanest approach: generate the ID in `start_delegation`, pass it to `run_subagent` (which needs to accept an `id: String` parameter — see Step 4 below), and insert it into the in-flight set. See Step 4 for the full code.

5. **Tests that set `app.delegating = true;`** → replace with:
   ```rust
   app.in_flight_subagents.insert("sub-test".into());
   ```

6. **Tests that check `app.delegating`** → replace with:
   ```rust
   assert!(!app.in_flight_subagents.is_empty(), "...");
   ```

- [ ] **Step 4: Fix `start_delegation` to use the in-flight set**

In `start_delegation`, the guard:

```rust
    if app.streaming || app.delegating {
        app.shell.status_hint = Some("busy · one subagent at a time".into());
        return;
    }
```

Replace with:

```rust
    if app.streaming || !app.in_flight_subagents.is_empty() {
        app.shell.status_hint = Some("busy · subagents running".into());
        return;
    }
```

And replace `app.delegating = true;` with:

```rust
    let sub_ulid = Ulid::new();
    let sub_id = format!("sub-{sub_ulid}");
    app.in_flight_subagents.insert(sub_id.clone());
```

Then update the worktree name and event to use the same ULID. Find the worktree creation:

```rust
    let wt = if Path::new(".git").exists() {
        match zoid::worktree::create_worktree(Path::new("."), &format!("sub-{}", Ulid::new())) {
```

Replace `Ulid::new()` with `sub_ulid`:

```rust
    let wt = if Path::new(".git").exists() {
        match zoid::worktree::create_worktree(Path::new("."), &format!("sub-{sub_ulid}")) {
```

And after the spawned task completes, the `DelegationResult` handler in `run()` already removes the ID (Step 3.2 above).

- [ ] **Step 5: Update the status hint**

Find the status hint set in `start_delegation`:

```rust
    app.shell.status_hint = Some(format!("{} delegating…", zoid_tui::tokens::glyph::RUNNING));
```

Replace with:

```rust
    app.shell.status_hint = Some(format!("{} {} subagent running…", zoid_tui::tokens::glyph::RUNNING, app.in_flight_subagents.len()));
```

- [ ] **Step 6: Build and run tests**

```bash
cd ~/source/zoid && cargo build 2>&1 | tail -5
cd ~/source/zoid && cargo test -p zoid 2>&1 | tail -10
```
Expected: build succeeds, all tests pass.

- [ ] **Step 7: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "refactor: replace delegating bool with in-flight subagent set"
```

---

## Task 7: Update SDD skill to reference `dispatch_subagent`

**Files:**
- Modify: `~/source/superpowers/skills/subagent-driven-development/SKILL.md`
- Modify: `~/source/superpowers/skills/dispatching-parallel-agents/SKILL.md`

**Interfaces:**
- None — skill documentation.

- [ ] **Step 1: Update the SDD process flowchart**

In `~/source/superpowers/skills/subagent-driven-development/SKILL.md`, find the `digraph process` block. Replace these node labels:

Find:
```
        "Dispatch implementer subagent (./implementer-prompt.md)" [shape=box];
```
→
```
        "Dispatch implementer (dispatch_subagent tool)" [shape=box];
```

Find all edge labels referencing `"Write diff file, dispatch task reviewer (gilfoyle + ./task-reviewer-prompt.md)"` — these stay as-is (the reviewer dispatch also uses `dispatch_subagent` but the flowchart node name doesn't need to change since the arm already says "gilfoyle").

Find:
```
    "More tasks remain?" -> "Dispatch final code reviewer (gilfoyle-tech-reviewer + ../requesting-code-review/code-reviewer.md)" [label="no"];
```
→
```
    "More tasks remain?" -> "Dispatch final reviewer (dispatch_subagent + gilfoyle)" [label="no"];
```

Find:
```
    "Dispatch final code reviewer (gilfoyle-tech-reviewer + ../requesting-code-review/code-reviewer.md)" -> "Use superpowers:finishing-a-development-branch";
```
→
```
    "Dispatch final reviewer (dispatch_subagent + gilfoyle)" -> "Use superpowers:finishing-a-development-branch";
```

- [ ] **Step 2: Update the Red Flags section**

Find the Red Flag:

```markdown
- Dispatch multiple implementation subagents in parallel (conflicts)
```

Replace with:

```markdown
- Dispatch multiple subagents with `worktree: false` that edit the same files — they will conflict. Use `worktree: true` for isolation, or dispatch sequentially for tasks touching shared files.
```

- [ ] **Step 3: Update the Example Workflow**

Find the `## Example Workflow` section. Replace the entire section with:

```markdown
## Example Workflow

```
You: I'm using Subagent-Driven Development to execute this plan.

[Read plan file once: docs/superpowers/plans/feature-plan.md]
[Create todos for all tasks]

Task 1: Hook installation script

[Run task-brief for Task 1; dispatch_subagent with task brief path in the task argument, worktree: true]

Subagent: "Before I begin - should the hook be installed at user or system level?"

You: "User level (~/.config/superpowers/hooks/)"

[DelegationResult event arrives with subagent_id, summary, ok=true]
[subagent_diff with the subagent_id to review the diff]

Task reviewer (dispatch_subagent with gilfoyle persona + task-reviewer-prompt):
  Spec ✅ - all requirements met, nothing extra.
  Strengths: Good test coverage, clean. Issues: None. Task quality: Approved.

[Mark Task 1 complete]

Task 2: Recovery modes

[dispatch_subagent with Task 2 brief, worktree: true]

[DelegationResult arrives]
[subagent_diff review]
Task reviewer: Spec ❌:
  - Missing: Progress reporting (spec says "report every 100 items")
  - Extra: Added --json flag (not requested)
  Issues (Important): Magic number (100)

[dispatch_subagent with fix task + all findings]
[DelegationResult arrives]
[subagent_diff re-review]
Task reviewer: Spec ✅. Task quality: Approved.

[Mark Task 2 complete]

...

[After all tasks]
[dispatch_subagent with gilfoyle + code-reviewer.md for final whole-branch review]
Final reviewer: All requirements met, ready to merge

Done!
```
```

- [ ] **Step 4: Update the File Handoffs section**

In the `## File Handoffs` section, after the existing bullets, add this paragraph:

```markdown

**Dispatching subagents:** The controller dispatches via the `dispatch_subagent` tool with the task brief path in the `task` argument. The subagent's summary arrives as a `DelegationResult` event carrying the `subagent_id`. Call `subagent_diff` with the `subagent_id` to retrieve the diff for review. The `DelegationResult` event's `subagent_id` matches the ID returned by `dispatch_subagent`.
```

- [ ] **Step 5: Update dispatching-parallel-agents**

In `~/source/superpowers/skills/dispatching-parallel-agents/SKILL.md`, find the `### 3. Dispatch in Parallel` section. Replace the code block:

Find:
```text
Subagent (general-purpose): "Fix agent-tool-abort.test.ts failures"
Subagent (general-purpose): "Fix batch-completion-behavior.test.ts failures"
Subagent (general-purpose): "Fix tool-approval-race-conditions.test.ts failures"
# All three run concurrently.
```

Replace with:
```text
dispatch_subagent(task: "Fix agent-tool-abort.test.ts failures", worktree: true)
dispatch_subagent(task: "Fix batch-completion-behavior.test.ts failures", worktree: true)
dispatch_subagent(task: "Fix tool-approval-race-conditions.test.ts failures", worktree: true)
# All three run concurrently; results arrive as DelegationResult events.
```

And in the `### 4. Review and Integrate` section, add after the existing bullets:

```markdown
- Call `subagent_diff` for each completed subagent to review its changes
- Verify no two subagents edited the same files (worktree isolation prevents this, but verify)
```

- [ ] **Step 6: Commit to the fork**

```bash
cd ~/source/superpowers
git add skills/subagent-driven-development/SKILL.md skills/dispatching-parallel-agents/SKILL.md
git commit -m "feat: update SDD and parallel-agents skills to use dispatch_subagent tool"
```

---

## Task 8: Integration test — concurrent dispatch

**Files:**
- Modify: `crates/zoid/src/agent.rs` (tests module)

**Interfaces:**
- Consumes: all prior tasks.

- [ ] **Step 1: Write the test**

In `crates/zoid/src/agent.rs` tests, add:

```rust
#[tokio::test]
async fn dispatch_two_subagents_concurrently() {
    use serde_json::json;
    use zoid_core::event::{Event, EventKind};
    use zoid_provider::{ProviderEvent, ToolCall};

    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::from(1u128),
        None,
        1,
        EventKind::UserMessage { text: "dispatch two subagents".into() },
    )];
    for e in &seed {
        session.append(e.clone()).await.unwrap();
    }

    // The model dispatches two subagents in one sub-turn, then says "done".
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

    // Both tool results must contain distinct subagent IDs.
    let ids: Vec<String> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult { name, output, .. } if name == "dispatch_subagent" => {
                Some(output.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "two dispatch tool results");
    assert_ne!(ids[0], ids[1], "distinct subagent IDs");
}
```

- [ ] **Step 2: Run the test**

```bash
cd ~/source/zoid && cargo test -p zoid dispatch_two_subagents_concurrently -- --nocapture 2>&1 | tail -10
```
Expected: PASS

- [ ] **Step 3: Commit**

```bash
cd ~/source/zoid
git add -A
git commit -m "test: concurrent subagent dispatch integration test"
```

---

## Task 9: Push both repos and re-import

**Files:**
- None (git operations).

- [ ] **Step 1: Push zoid**

```bash
cd ~/source/zoid && git push origin main
```

- [ ] **Step 2: Push the fork**

```bash
cd ~/source/superpowers && git push origin main
```

- [ ] **Step 3: Re-import the mode**

In the zoid TUI: `:mode update superpowers`

- [ ] **Step 4: Smoke test**

In a fresh zoid session, switch to Superpowers mode and verify the model can call `dispatch_subagent`:

```bash
# Verify the tool is advertised
cd ~/source/zoid && cargo test -p zoid chat_tools_includes -- --nocapture 2>&1 | tail -5
```
Expected: PASS

- [ ] **Step 5: Commit any remaining changes**

```bash
cd ~/source/zoid && git status
```