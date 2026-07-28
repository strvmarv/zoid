# Subagent No-Poll Prompt Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Solidify the LLM's "do not poll subagents" discipline through four prompt-text changes — tool description, tool result, system prompt, and list_subagents output — with no runtime/architecture change.

**Architecture:** All four changes are string-content edits to the exact text the model receives. One small refactor extracts the `list_subagents` formatting into a shared helper so the test verifies the real output (including the new reminder) rather than a duplicated reconstruction. Two files touched: `crates/zoid-tools/src/subagent_dispatch.rs` and `crates/zoid/src/agent.rs`.

**Tech Stack:** Rust, `zoid-tools` + `zoid` crates, existing test harness (`#[tokio::test]`, `SequencedProvider`).

## Global Constraints

- All four changes are unconditional — no config flags, no gating.
- `list_subagents` must return data + reminder, NOT a refusal/error (decided: soft nudge).
- The `dispatch_subagent` tool result must keep the `{"subagent_id": "..."}` JSON prefix (some models/tests may parse it); the directive follows after an em-dash.
- The `DelegationResult` event payload and its `[delegated subagent] {summary}` chat folding are NOT touched.
- `wrap_reassertion` mechanics are NOT touched — it already re-states `SYSTEM_PROMPT` verbatim, so adding the sentence to `SYSTEM_PROMPT` is the only wiring.
- `cancel_subagent` tool is NOT touched (canceling is a legitimate action, not polling).

---

### Task 1: Restructure the `dispatch_subagent` tool description

**Files:**
- Modify: `crates/zoid-tools/src/subagent_dispatch.rs:15-22` (description string)
- Test: `crates/zoid-tools/src/subagent_dispatch.rs` (existing `dispatch_subagent_spec_and_kind` test + new assertion)

**Interfaces:**
- Consumes: nothing
- Produces: the restructured `ToolSpec.description` string that the model sees in its tool list

- [ ] **Step 1: Add the failing test assertion**

In `crates/zoid-tools/src/subagent_dispatch.rs`, inside the existing `dispatch_subagent_spec_and_kind` test (around line 49-64), add an assertion that the description leads with "Fire-and-forget" and contains "never call list_subagents":

```rust
        let desc = DispatchSubagent.spec().description;
        assert!(
            desc.starts_with("Fire-and-forget"),
            "description must lead with 'Fire-and-forget' so the no-poll rule is \
             the first thing the model reads, not buried mid-paragraph: {desc}"
        );
        assert!(
            desc.contains("never call list_subagents"),
            "description must explicitly name list_subagents as a do-not-call: {desc}"
        );
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid-tools --lib dispatch_subagent_spec_and_kind`
Expected: FAIL — `description must lead with 'Fire-and-forget'`

- [ ] **Step 3: Replace the description string**

In `crates/zoid-tools/src/subagent_dispatch.rs`, replace the `description:` value in the `spec()` method (lines 15-22) with:

```rust
            description: "Fire-and-forget: dispatch a subagent to execute a task in \
                          isolation, then STOP. The result arrives later as a \
                          DelegationResult event that re-invokes you automatically — \
                          never poll for status, never call list_subagents to check \
                          progress, and do not edit files in the main worktree while a \
                          subagent runs (they share the working directory unless \
                          worktree: true). Returns the subagent ID immediately. Up to \
                          max_concurrent subagents (default 3) may run simultaneously — \
                          additional dispatches are queued and start when a slot frees. \
                          Use worktree: true for file isolation when subagents might \
                          edit the same files."
                .into(),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid-tools --lib dispatch_subagent_spec_and_kind`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/subagent_dispatch.rs
git commit -m "feat(subagent): lead dispatch_subagent description with fire-and-forget rule"
```

---

### Task 2: Add subagent discipline sentence to `SYSTEM_PROMPT`

**Files:**
- Modify: `crates/zoid/src/agent.rs:36-43` (`SYSTEM_PROMPT` constant)
- Test: `crates/zoid/src/agent.rs` (new unit test `system_prompt_reinforces_no_poll`)

**Interfaces:**
- Consumes: nothing
- Produces: the updated `SYSTEM_PROMPT` string that `wrap_reassertion` re-states periodically, and that `default_profile()` uses as the Chat mode system prompt

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/agent.rs`, in the `#[cfg(test)] mod tests` block (line 3131 — this module has `use super::*;` so `SYSTEM_PROMPT` is in scope; do NOT use `guardrail_types_tests` whose explicit import list lacks it), add:

```rust
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
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid --lib system_prompt_reinforces_no_poll`
Expected: FAIL — `SYSTEM_PROMPT must contain 'fire-and-forget'`

- [ ] **Step 3: Append the discipline sentence to `SYSTEM_PROMPT`**

In `crates/zoid/src/agent.rs`, modify the `SYSTEM_PROMPT` constant (lines 36-43). Append one sentence after the existing closing line. The full new constant:

```rust
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
     check on a subagent you dispatched.";
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid --lib system_prompt_reinforces_no_poll`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): add no-poll discipline to SYSTEM_PROMPT for re-assertion"
```

---

### Task 3: Inject positive directive into the `dispatch_subagent` tool result

**Files:**
- Modify: `crates/zoid/src/agent.rs:1740` (tool result output string)
- Test: `crates/zoid/src/agent.rs:4794` (existing `dispatch_subagent_returns_id_as_tool_result` test)

**Interfaces:**
- Consumes: the `sub_id` variable already in scope at the dispatch arm
- Produces: the tool-result string the model receives immediately after dispatching a subagent

- [ ] **Step 1: Update the existing test to assert the directive is present**

In `crates/zoid/src/agent.rs`, in the `dispatch_subagent_returns_id_as_tool_result` test (line 4858-4867), add assertions that the output contains the directive. Replace the `match &tool_result.kind { ... }` body:

```rust
        match &tool_result.kind {
            EventKind::ToolResult { output, is_error, .. } => {
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid --lib dispatch_subagent_returns_id_as_tool_result`
Expected: FAIL — `result must carry the no-poll directive`

- [ ] **Step 3: Replace the tool result output string**

In `crates/zoid/src/agent.rs`, at line 1740, replace:

```rust
                            output: format!("{{\"subagent_id\": \"{sub_id}\"}}"),
```

with:

```rust
                            output: format!(
                                "{{\"subagent_id\": \"{sub_id}\"}} — Subagent {sub_id} is \
                                 running in isolation. You will be re-invoked automatically \
                                 with its result; do NOT call list_subagents or otherwise \
                                 check on it. End your turn now and await the result."
                            ),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid --lib dispatch_subagent_returns_id_as_tool_result`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): inject no-poll directive into dispatch tool result"
```

---

### Task 4: Extract `format_subagent_list` helper and append soft reminder

**Files:**
- Modify: `crates/zoid/src/agent.rs:2010-2025` (extract formatting into helper, call it)
- Modify: `crates/zoid/src/agent.rs:5496-5538` (rewrite `list_subagents_formats_id_and_task` test to call the helper)
- Test: the rewritten test

**Interfaces:**
- Consumes: `SubagentHandle` (already defined in this file), `HashMap<String, SubagentHandle>`
- Produces: `fn format_subagent_list(map: &HashMap<String, SubagentHandle>) -> String` — a pure function that both the agent-loop `list_subagents` arm and the test call

This task does two things in one: (a) extract the inline formatting into a named helper so the test exercises the real code, and (b) append the soft reminder to the non-empty output. They're one task because the extraction is the enabling refactor for testing the reminder — splitting them would leave the test still duplicating logic.

- [ ] **Step 1: Write the failing test (rewrite to call the helper)**

In `crates/zoid/src/agent.rs`, replace the entire `list_subagents_formats_id_and_task` test (lines 5496-5538) with:

```rust
    #[test]
    fn list_subagents_formats_id_and_task() {
        use std::collections::HashMap;
        use tokio_util::sync::CancellationToken;

        let mut map: HashMap<String, SubagentHandle> = HashMap::new();
        map.insert("sub-001".into(), SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
            task: "implement the resolver".into(),
            agent: "delegate".into(),
        });
        map.insert("sub-002".into(), SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
            task: "review the spec".into(),
            agent: "reviewer".into(),
        });

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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p zoid --lib list_subagents_formats_id_and_task`
Expected: FAIL — `cannot find function format_subagent_list` (it doesn't exist yet)

- [ ] **Step 3: Add the `format_subagent_list` helper function**

In `crates/zoid/src/agent.rs`, add this function at module level — immediately after `fire_subagent_kill` (around line 141), which is the existing module-level helper this one parallels. Do NOT place it inside `run_agent_turn_cancellable` (line 2010 is a match arm inside that function's body; a module-level `fn` cannot go there):

```rust
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
        let agent = if handle.agent.is_empty() { "delegate" } else { &handle.agent };
        lines.push_str(&format!("- {id} [{agent}]: {}\n", handle.task));
    }
    lines.push_str(
        "\nReminder: subagents are fire-and-forget. You will be re-invoked with \
         each result automatically — do not poll or call this tool repeatedly \
         to check progress. End your turn and await the DelegationResult.",
    );
    lines.trim_end().to_string()
}
```

- [ ] **Step 4: Replace the inline formatting in the agent-loop arm with a call to the helper**

In `crates/zoid/src/agent.rs`, replace the `list_subagents` arm's body (lines 2010-2025):

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "list_subagents" => {
                    let output = if let Some(reg) = &config.in_flight {
                        let map = reg.lock().unwrap();
                        format_subagent_list(&map)
                    } else {
                        "No subagents currently running.".to_string()
                    };
```

Keep the rest of the arm (the `emit(...)` call) unchanged.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p zoid --lib list_subagents_formats_id_and_task`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(subagent): extract format_subagent_list helper, append no-poll reminder"
```

---

### Task 5: Full build, clippy, and all affected tests

**Files:**
- No modifications — verification only

**Interfaces:**
- Consumes: all four prior tasks
- Produces: confirmation that the full crate compiles and all tests pass

- [ ] **Step 1: Build the workspace**

Run: `cargo build -p zoid -p zoid-tools`
Expected: PASS (no compile errors)

- [ ] **Step 2: Run all affected tests**

Run: `cargo test -p zoid-tools --lib dispatch_subagent_spec_and_kind && cargo test -p zoid --lib system_prompt_reinforces_no_poll && cargo test -p zoid --lib dispatch_subagent_returns_id_as_tool_result && cargo test -p zoid --lib list_subagents_formats_id_and_task`
Expected: all four PASS

- [ ] **Step 3: Run clippy on the touched crates**

Run: `cargo clippy -p zoid -p zoid-tools -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Run the broader zoid test suite to catch regressions**

Run: `cargo test -p zoid --lib`
Expected: PASS (no regressions from the string changes or the helper extraction)

- [ ] **Step 5: Commit if any fixups were needed**

If steps 1-4 required any fixup edits, commit them:

```bash
git add -A
git commit -m "fix: post-verification fixups for no-poll prompt hardening"
```

If no fixups were needed, this step is a no-op — do not create an empty commit.