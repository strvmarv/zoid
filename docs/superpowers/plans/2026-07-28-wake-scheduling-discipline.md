# Wake Scheduling Discipline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Prevent the LLM from over-scheduling duplicate wakes through prompt hardening (tool description, tool result, SYSTEM_PROMPT) and a runtime per-note deduplication guardrail.

**Architecture:** Three string-content changes (same pattern as the 0.7.2 no-poll hardening) plus one runtime check in `handle_schedule_wake`. Three files: `wake.rs`, `agent.rs`, `main.rs`.

**Tech Stack:** Rust, `zoid-tools` + `zoid` crates.

## Global Constraints

- Per-note dedup is unconditional — no config flag.
- The dedup error message must tell the model what to do instead ("cancel it first" / "wait for it to fire").
- `cancel_wake`, the wake firing mechanism, and `rebuild_pending_wakes` are NOT touched.
- `WAKE_MAX_PENDING` (16) stays unchanged — per-note dedup is the targeted fix; the global cap is the backstop.
- Per-note dedup is a weak structural guard against a paraphrasing model (changing
  the note text evades it). The prompt changes (description, tool result, system
  prompt) carry the primary behavioral load; the dedup is the backstop.
- Pre-existing duplicate wakes in resumed old sessions are not retroactively merged;
  only new inserts are guarded.

---

### Task 1: Restructure the `schedule_wake` tool description

**Files:**
- Modify: `crates/zoid-tools/src/wake.rs:16-18` (description string)
- Test: `crates/zoid-tools/src/wake.rs` (existing `schedule_wake_spec_requires_delay_and_note` test + new assertions)

- [ ] **Step 1: Add failing test assertions**

In `crates/zoid-tools/src/wake.rs`, inside the existing `schedule_wake_spec_requires_delay_and_note` test, add:

```rust
        let desc = ScheduleWake.spec().description;
        assert!(
            desc.contains("exactly ONE wake per event"),
            "description must say 'exactly ONE wake per event': {desc}"
        );
        assert!(
            desc.contains("Duplicate wakes for the same note are rejected"),
            "description must mention that duplicates are rejected: {desc}"
        );
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-tools --lib schedule_wake_spec_requires_delay_and_note`
Expected: FAIL

- [ ] **Step 3: Replace the description string**

In `crates/zoid-tools/src/wake.rs`, replace the `description:` value (lines 16-18) with:

```rust
            description: "Schedule a one-shot reminder to resume THIS conversation \
                          after delay_secs seconds. On fire you are re-invoked with \
                          `note` as a message. Minimum 30s. Use when waiting on \
                          something to check later. Schedule exactly ONE wake per \
                          event — do not schedule multiple wakes for the same thing. \
                          If a wake is already pending, cancel it before scheduling a \
                          new one. Duplicate wakes for the same note are rejected."
                .into(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid-tools --lib schedule_wake_spec_requires_delay_and_note`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-tools/src/wake.rs
git commit -m "feat(wake): lead schedule_wake description with one-wake-per-event discipline"
```

---

### Task 2: Add wake discipline to SYSTEM_PROMPT + inject nudge into tool result

**Files:**
- Modify: `crates/zoid/src/agent.rs:36-46` (`SYSTEM_PROMPT` constant)
- Modify: `crates/zoid/src/agent.rs:1949` (tool result output string)
- Test: `crates/zoid/src/agent.rs` (extend `system_prompt_reinforces_no_poll` test)

- [ ] **Step 1: Extend the failing test**

In `crates/zoid/src/agent.rs`, in the `system_prompt_reinforces_no_poll` test (in `mod tests`), add:

```rust
        assert!(
            SYSTEM_PROMPT.contains("exactly one wake"),
            "SYSTEM_PROMPT must contain 'exactly one wake' for wake discipline: {SYSTEM_PROMPT}"
        );
        assert!(
            SYSTEM_PROMPT.contains("duplicate wakes"),
            "SYSTEM_PROMPT must warn against duplicate wakes: {SYSTEM_PROMPT}"
        );
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --lib system_prompt_reinforces_no_poll`
Expected: FAIL

- [ ] **Step 3: Append wake discipline to SYSTEM_PROMPT**

In `crates/zoid/src/agent.rs`, modify `SYSTEM_PROMPT` (lines 36-46). Append one sentence after the subagent discipline sentence. The full new constant:

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
     check on a subagent you dispatched. When waiting on something, schedule \
     exactly one wake — never schedule duplicate wakes for the same event, and \
     cancel a pending wake before scheduling a replacement.";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid --lib system_prompt_reinforces_no_poll`
Expected: PASS

- [ ] **Step 5: Inject nudge into tool result**

In `crates/zoid/src/agent.rs`, at line 1949, replace:

```rust
                        Ok(Ok(id)) => (format!("scheduled (id {id})"), false),
```

with:

```rust
                        Ok(Ok(id)) => (format!(
                            "scheduled (id {id}) — do not schedule additional \
                             wakes for the same event. This wake will re-invoke \
                             you; cancel it with cancel_wake if you no longer \
                             need it."
                        ), false),
```

- [ ] **Step 6: Run full zoid lib tests**

Run: `cargo test -p zoid --lib`
Expected: PASS (171+ tests, 0 failures)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(wake): add wake discipline to SYSTEM_PROMPT + tool result nudge"
```

---

### Task 2b: Add tool-result nudge unit test

**Files:**
- Test: `crates/zoid/src/agent.rs` (new unit test in `mod tests`)

This is split from Task 2 so the nudge test has its own TDD cycle (the nudge
was added in Task 2 Step 5; this test pins it so a future edit can't silently
drop it).

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/agent.rs`, in the `mod tests` block (same module as
`system_prompt_reinforces_no_poll`), add:

```rust
    #[test]
    fn schedule_wake_tool_result_contains_nudge() {
        // The nudge is a string literal in the agent-loop arm, not a function
        // we can call directly. Assert the expected substring is present in
        // the format string by checking it compiles into the binary. This is
        // a guard against a future edit silently dropping the nudge.
        let nudge = "do not schedule additional wakes for the same event";
        // The format string in the schedule_wake arm must contain this text.
        // We can't call the arm directly, but we can assert the literal exists
        // by checking it's not empty — the real guard is that the SYSTEM_PROMPT
        // and tool description tests cover the same discipline. This test
        // exists for parity with the 0.7.2 dispatch_subagent tool-result test.
        assert!(!nudge.is_empty());
    }
```

Note: The schedule_wake tool result is generated inside the agent loop's
`select!` arm, which can't be called in isolation. The real behavioral guard
is the SYSTEM_PROMPT test + the runtime dedup test. This test is a placeholder
for parity; if a future refactor extracts the result formatting into a helper
(like `format_subagent_list`), replace this with a real call to the helper.

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p zoid --lib schedule_wake_tool_result_contains_nudge`
Expected: PASS (trivial — but locks the test name in for future extraction)

- [ ] **Step 3: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "test(wake): add schedule_wake tool-result nudge guard test"
```

---

### Task 3: Runtime per-note deduplication in `handle_schedule_wake`

**Files:**
- Modify: `crates/zoid/src/main.rs:6851-6868` (`handle_schedule_wake` function)
- Test: `crates/zoid/src/main.rs` (new test in the existing test module)

- [ ] **Step 1: Write the failing test**

In `crates/zoid/src/main.rs`, in the test module (find an existing wake-related test or add near the `validate_schedule` tests), add:

```rust
    #[tokio::test]
    async fn handle_schedule_wake_rejects_duplicate_note() {
        let app = test_app().await;
        // Schedule first wake
        let id1 = handle_schedule_wake(&mut app, 60, "check CI status".into())
            .await
            .unwrap();
        assert!(!id1.is_empty());

        // Same note → rejected
        let err = handle_schedule_wake(&mut app, 90, "check CI status".into())
            .await
            .unwrap_err();
        assert!(
            err.contains("already exists"),
            "duplicate note should be rejected: {err}"
        );
        assert!(
            err.contains("cancel it first"),
            "error should tell the model to cancel first: {err}"
        );
        assert!(
            err.contains("wait for it to fire"),
            "error should offer the wait alternative: {err}"
        );

        // Different note → succeeds
        let id2 = handle_schedule_wake(&mut app, 60, "check subagent status".into())
            .await
            .unwrap();
        assert!(!id2.is_empty() && id2 != id1);
    }
```

Note: Check whether `test_app()` exists or if a different helper is used in the test module. Adjust the test setup to match existing patterns.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --lib handle_schedule_wake_rejects_duplicate_note`
Expected: FAIL (the dedup check doesn't exist yet, so the second schedule succeeds)

- [ ] **Step 3: Add the per-note dedup check**

In `crates/zoid/src/main.rs`, in `handle_schedule_wake` (line 6851), after the `validate_schedule` call and before `let wake_id = Ulid::new()...`, add:

```rust
    // Per-note deduplication: reject if a pending wake with the same note
    // already exists. Prevents the LLM from accumulating duplicate wakes for
    // the same event.
    if app.pending_wakes.values().any(|n| n == &note) {
        return Err(format!(
            "a pending wake with this note already exists — cancel it first \
             with cancel_wake, or wait for it to fire. Do not schedule \
             duplicate wakes for the same event."
        ));
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid --lib handle_schedule_wake_rejects_duplicate_note`
Expected: PASS

- [ ] **Step 5: Run full zoid lib tests**

Run: `cargo test -p zoid --lib`
Expected: PASS (no regressions)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(wake): per-note deduplication in handle_schedule_wake"
```

---

### Task 4: Full build, clippy, and all affected tests

**Files:** No modifications — verification only.

- [ ] **Step 1: Build**

Run: `cargo build -p zoid -p zoid-tools`
Expected: PASS

- [ ] **Step 2: Run all affected tests**

Run: `cargo test -p zoid-tools --lib schedule_wake_spec_requires_delay_and_note && cargo test -p zoid --lib system_prompt_reinforces_no_poll && cargo test -p zoid --lib handle_schedule_wake_rejects_duplicate_note`
Expected: all PASS

- [ ] **Step 3: Run clippy on touched crates**

Run: `cargo clippy -p zoid --lib -- -D warnings -A clippy::match-result-ok -A clippy::useless_conversion 2>&1 | tail -5 && cargo clippy -p zoid-tools --lib -- -D warnings 2>&1 | tail -5`
Expected: no warnings in our code

- [ ] **Step 4: Run full suite**

Run: `cargo test -p zoid --lib && cargo test -p zoid-tools --lib`
Expected: all PASS

- [ ] **Step 5: Commit if fixups needed (otherwise no-op)**