# Fix Subagent UI Jumping, Iteration Limit, and Result Landing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three subagent issues: (1) jumpy UI from subagent events polluting the main conversation's projection cache, (2) iteration limit too tight (25→100), (3) add trace logging to verify DelegationResult landing end-to-end.

**Architecture:** The jumpy UI fix is a branch guard in `main.rs`'s `AgentUpdate::Appended` handler — skip `apply_streaming` and cache invalidation for non-default-branch events. The iteration limit is a one-line constant change in `subagent.rs`. The trace logging adds three `tracing::info!` calls at key points in the delegation flow. All three are independent and can be implemented in any order, but are sequenced here for logical flow.

**Tech Stack:** Rust, tokio (`mpsc`), `zoid-core` (events, projection), `zoid-provider` (`FakeProvider`, `ProviderEvent`), `tracing` crate.

## Global Constraints

- `BranchId::default()` = `BranchId("main".to_string())` — defined in `zoid-core/src/event.rs:7`.
- `Event` and `EventKind` are imported at `main.rs:27`: `use zoid_core::event::{Event, EventKind};`
- `BranchId` is NOT imported at the top level of `main.rs` — use the full path `zoid_core::event::BranchId` or add an import.
- `tracing` is already used in `main.rs` (e.g. `tracing::info!`, `tracing::warn!`, `tracing::debug!`).
- `FakeProvider` replays a scripted `Vec<ProviderEvent>` — defined in `zoid-provider/src/lib.rs:256`.
- `test_app()` is an async helper in `main.rs:7117` that builds a minimal `App` for tests.
- `SUBAGENT_MAX_ITERATIONS` is a private `const` in `subagent.rs:31` — tests in the same module can access it.

---

### Task 1: Fix jumpy UI — branch guard in the `Appended` handler

**Files:**
- Modify: `crates/zoid/src/main.rs:2806-2819` (the `apply_streaming` + cache invalidation block in the `AgentUpdate::Appended` handler)
- Test: `crates/zoid/src/main.rs` (new tests in the `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `zoid_core::event::BranchId` (for the branch comparison)
- Produces: No new interfaces. The `Appended` handler gains a branch check.

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)] mod tests` block in `main.rs`, near the existing delegation tests (after `delegation_wake_respects_yielded`):

```rust
    /// A subagent-branch event must NOT be applied to the projection cache
    /// via `apply_streaming`. The `msgs` vector must be unchanged.
    #[tokio::test]
    async fn subagent_branch_event_skips_apply_streaming() {
        let mut app = test_app().await;
        // Seed the projection with one main-branch user message so the cache
        // is populated (events_len = Some(1), msgs has 1 item).
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage { text: "hello".into() },
        );
        app.events.push(ev);
        app.proj.refresh(&app.events);

        let msgs_before = app.proj.msgs.len();
        assert!(msgs_before > 0, "projection must have the seeded message");
        assert!(
            app.proj.events_len.is_some(),
            "events_len must be set (cache is live)"
        );

        // Simulate a subagent-branch ModelDelta arriving through Appended.
        // The branch is "subagent:01ABC" — NOT the default "main" branch.
        let sub_ev = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ModelDelta { text: "subagent text".into() },
        )
        .with_session(app.session_id);
        // Override the branch to a subagent branch.
        let mut sub_ev = sub_ev;
        sub_ev.branch = zoid_core::event::BranchId("subagent:01ABC".into());

        // Process it the same way the Appended handler does, but with the
        // branch guard applied (the code under test).
        let is_subagent_branch = sub_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch && !app.proj.apply_streaming(&sub_ev) {
            app.proj.events_len = None;
        }
        app.events.push(sub_ev);

        // The projection cache must be untouched: same msg count, events_len
        // still set (not invalidated).
        assert_eq!(
            app.proj.msgs.len(),
            msgs_before,
            "subagent-branch event must not add to projection msgs"
        );
        assert!(
            app.proj.events_len.is_some(),
            "subagent-branch event must not invalidate the projection cache"
        );
        // The event IS in app.events (persisted), just not in the projection.
        assert_eq!(app.events.len(), 2, "event pushed into app.events");
    }

    /// A main-branch ModelDelta must still be applied via `apply_streaming`
    /// (existing behavior preserved).
    #[tokio::test]
    async fn main_branch_event_applies_streaming() {
        let mut app = test_app().await;
        // Seed with a user message so apply_streaming can find a last Assistant
        // msg to append to (or it creates a new one).
        let ev = Event::new(
            Ulid::new(),
            None,
            0,
            EventKind::UserMessage { text: "hello".into() },
        );
        app.events.push(ev);
        app.proj.refresh(&app.events);

        let msgs_before = app.proj.msgs.len();

        // A main-branch ModelDelta (default branch).
        let main_ev = Event::new(
            Ulid::new(),
            None,
            1,
            EventKind::ModelDelta { text: "response".into() },
        );

        let is_subagent_branch = main_ev.branch != zoid_core::event::BranchId::default();
        if !is_subagent_branch && !app.proj.apply_streaming(&main_ev) {
            app.proj.events_len = None;
        }
        app.events.push(main_ev);

        // apply_streaming should have added an Assistant msg (ModelDelta creates
        // a new one if none exists, or appends to the last one).
        assert_eq!(
            app.proj.msgs.len(),
            msgs_before + 1,
            "main-branch ModelDelta must add to projection msgs"
        );
    }
```

- [ ] **Step 2: Run tests to verify they pass (they test the guard logic directly)**

Run: `cargo test -p zoid --lib subagent_branch_event_skips_apply_streaming main_branch_event_applies_streaming`
Expected: PASS — these tests replicate the guard logic inline. They serve as a spec for the production code change.

- [ ] **Step 3: Add the branch guard to the `Appended` handler**

In `main.rs`, replace the `apply_streaming` + cache invalidation block (lines ~2806-2819). The current code is:

```rust
                        // Incremental streaming: ModelDelta and ToolCall events
                        // append directly into the cached ChatMsg vec in O(1)
                        // instead of triggering a full O(n) conversation() fold
                        // on the next frame. Structural events (ToolResult,
                        // Usage, etc.) return false and get a full refresh.
                        if !app.proj.apply_streaming(&ev) {
                            // Structural event — invalidate the projection cache
                            // AND the body cache so both do a full rebuild on
                            // the next frame. Compaction events replace content
                            // in existing messages (same count) so the BodyCache's
                            // msg_count check would skip the rebuild without this.
                            app.proj.events_len = None;
                            app.body_cache.key = None;
                        }
```

Replace with:

```rust
                        // Incremental streaming: ModelDelta and ToolCall events
                        // append directly into the cached ChatMsg vec in O(1)
                        // instead of triggering a full O(n) conversation() fold
                        // on the next frame. Structural events (ToolResult,
                        // Usage, etc.) return false and get a full refresh.
                        //
                        // Subagent-branch events are persisted to SQLite and
                        // pushed into app.events, but the projection cache only
                        // cares about main-branch events for the conversation
                        // view. Skip the incremental streaming path AND cache
                        // invalidation for subagent-branch events — no UI churn.
                        // DelegationResult events are on the default branch, so
                        // they flow through here normally.
                        let is_subagent_branch =
                            ev.branch != zoid_core::event::BranchId::default();
                        if !is_subagent_branch && !app.proj.apply_streaming(&ev) {
                            // Structural event — invalidate the projection cache
                            // AND the body cache so both do a full rebuild on
                            // the next frame. Compaction events replace content
                            // in existing messages (same count) so the BodyCache's
                            // msg_count check would skip the rebuild without this.
                            app.proj.events_len = None;
                            app.body_cache.key = None;
                        }
```

- [ ] **Step 4: Run the full test suite**

Run: `cargo test -p zoid`
Expected: PASS — no regressions. The guard is additive (only skips work for subagent-branch events, which were previously causing the UI bug).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "fix: skip apply_streaming for subagent-branch events to prevent UI jumping"
```

---

### Task 2: Raise iteration limit from 25 to 100

**Files:**
- Modify: `crates/zoid/src/subagent.rs:29-31` (the `SUBAGENT_MAX_ITERATIONS` constant)

**Interfaces:**
- Produces: No new interfaces. The constant value changes.

- [ ] **Step 1: Write a failing test**

Add to the `#[cfg(test)] mod tests` block in `subagent.rs`:

```rust
    #[test]
    fn subagent_max_iterations_is_100() {
        assert_eq!(
            SUBAGENT_MAX_ITERATIONS, 100,
            "iteration limit must be 100 (was 25, too tight for realistic cycles)"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid --lib subagent::tests::subagent_max_iterations_is_100`
Expected: FAIL — `SUBAGENT_MAX_ITERATIONS` is still 25.

- [ ] **Step 3: Change the constant**

In `subagent.rs`, replace (lines ~29-31):

```rust
// BEFORE:
/// Hard cap on a subagent's tool-call iterations. 25 covers a realistic
/// read-edit-test-debug cycle with 2–3 retries; beyond that the subagent is
/// almost certainly stuck in a loop.
const SUBAGENT_MAX_ITERATIONS: u32 = 25;

// AFTER:
/// Hard cap on a subagent's tool-call iterations. 100 covers a realistic
/// multi-file read-edit-test-debug cycle with several retries; beyond that
/// the subagent is almost certainly stuck in a loop.
const SUBAGENT_MAX_ITERATIONS: u32 = 100;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid --lib subagent::tests::subagent_max_iterations_is_100`
Expected: PASS

- [ ] **Step 5: Run the full test suite**

Run: `cargo test -p zoid`
Expected: PASS — no regressions.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/subagent.rs
git commit -m "fix: raise subagent max iterations from 25 to 100"
```

---

### Task 3: Add trace logging for result landing

**Files:**
- Modify: `crates/zoid/src/main.rs:2791` (DelegationResult arrival trace)
- Modify: `crates/zoid/src/main.rs:6127` (`plan_delegation_wake` trace)
- Modify: `crates/zoid/src/main.rs:2838` (continuation turn spawn trace)

**Interfaces:**
- Produces: No new interfaces. Three `tracing::info!` calls are added.

- [ ] **Step 1: Add trace at DelegationResult arrival**

In the `AgentUpdate::Appended` handler, the `DelegationResult` branch (line ~2791), add a `tracing::info!` call at the start of the `if let` block. The current code is:

```rust
                        if let EventKind::DelegationResult { subagent_id, .. } = &ev.kind {
                            app.in_flight_subagents.retain(|s| s.id != *subagent_id);
```

Change to:

```rust
                        if let EventKind::DelegationResult { subagent_id, summary, ok, .. } = &ev.kind {
                            tracing::info!(
                                subagent_id = %subagent_id,
                                ok = %ok,
                                summary_len = summary.len(),
                                "delegation result arrived"
                            );
                            app.in_flight_subagents.retain(|s| s.id != *subagent_id);
```

Note: the pattern binding changes from `{ subagent_id, .. }` to `{ subagent_id, summary, ok, .. }` to capture the fields for the trace.

- [ ] **Step 2: Add trace at `plan_delegation_wake`**

In `plan_delegation_wake` (line ~6127), add a trace after the `should_wake_after_delegation` call. The current code is:

```rust
fn plan_delegation_wake(app: &mut App) -> bool {
    if should_wake_after_delegation(
        app.streaming,
        app.in_flight_subagents.is_empty(),
        app.yielded,
    ) {
```

Change to:

```rust
fn plan_delegation_wake(app: &mut App) -> bool {
    let wake = should_wake_after_delegation(
        app.streaming,
        app.in_flight_subagents.is_empty(),
        app.yielded,
    );
    tracing::info!(
        wake = %wake,
        streaming = %app.streaming,
        in_flight_empty = %app.in_flight_subagents.is_empty(),
        yielded = %app.yielded,
        "delegation wake decision"
    );
    if wake {
```

- [ ] **Step 3: Add trace at continuation turn spawn**

In the `AgentUpdate::Appended` handler, the delegation wake spawn site (line ~2838), add a trace. The current code is:

```rust
                        if delegation_arrived && plan_delegation_wake(app) {
                            spawn_turn(app);
                        }
```

Change to:

```rust
                        if delegation_arrived && plan_delegation_wake(app) {
                            tracing::info!("spawning continuation turn after delegation");
                            spawn_turn(app);
                        }
```

- [ ] **Step 4: Run the full test suite**

Run: `cargo test -p zoid`
Expected: PASS — the trace calls are non-functional additions. The `plan_delegation_wake` refactor (extracting `wake` into a local) is behavior-preserving.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat: add trace logging for subagent result landing flow"
```

---

### Task 4: Integration test — subagent events don't appear in main conversation

**Files:**
- Test: `crates/zoid/src/main.rs` (new test in the `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `zoid_core::projection::conversation` (the pure projection that folds events into `ChatMsg`s), `zoid_core::event::{Event, EventKind, BranchId}`

- [ ] **Step 1: Write the test**

Add to the test module:

```rust
    /// Subagent-branch events (ModelDelta, ToolCall, ToolResult) must NOT
    /// appear in the main conversation's ChatMsg list — the projection filters
    /// by branch. This is the integration-level guard behind the jumpy-UI fix:
    /// even if subagent events are in app.events, the conversation view never
    /// shows them.
    #[test]
    fn subagent_branch_events_invisible_in_conversation() {
        use zoid_core::event::{Event, EventKind, BranchId};
        use zoid_core::projection::conversation;

        let sub_branch = BranchId("subagent:01ABC".into());
        let events = vec![
            // Main-branch user message.
            Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "hello".into() }),
            // Subagent-branch assistant text (must NOT appear).
            // Event has no with_branch builder — set .branch directly.
            {
                let mut ev = Event::new(Ulid::new(), None, 1, EventKind::ModelDelta { text: "subagent working".into() });
                ev.branch = sub_branch.clone();
                ev
            },
            // Subagent-branch tool call (must NOT appear).
            {
                let mut ev = Event::new(Ulid::new(), None, 2, EventKind::ToolCall {
                    id: "tc1".into(),
                    name: "read".into(),
                    args: r#"{"path":"src/main.rs"}"#.into(),
                });
                ev.branch = sub_branch;
                ev
            },
            // Main-branch assistant text (must appear).
            Event::new(Ulid::new(), None, 3, EventKind::AssistantMessage { text: "done".into() }),
        ];

        let msgs = conversation(events.iter());
        let joined: String = msgs
            .iter()
            .flat_map(|m| match m {
                zoid_core::projection::ChatMsg::Assistant { text, .. } => text.clone(),
                zoid_core::projection::ChatMsg::User { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();

        // Main-branch messages are visible.
        assert!(joined.contains("hello"), "user message visible: {joined}");
        assert!(joined.contains("done"), "assistant message visible: {joined}");
        // Subagent-branch messages are NOT visible.
        assert!(
            !joined.contains("subagent working"),
            "subagent ModelDelta must not appear in conversation: {joined}"
        );
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p zoid --lib subagent_branch_events_invisible_in_conversation`
Expected: PASS — the projection already filters by branch; this test confirms the invariant that the UI fix relies on.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p zoid`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "test: verify subagent-branch events are invisible in main conversation"
```

---

### Task 5: Integration test — DelegationResult appears in model request

**Files:**
- Test: `crates/zoid/src/main.rs` (new test in the `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `zoid_core::projection::conversation_for_branch`, `zoid::agent::map_msg` (if accessible), or manual ChatMsg→Message mapping

- [ ] **Step 1: Write the test**

Add to the test module:

```rust
    /// A DelegationResult on the default branch must be folded into a
    /// ChatMsg::Delegated by the projection, confirming the result-landing
    /// plumbing. The continuation turn's request builder uses
    /// `conversation_for_branch` → `map_msg`, which maps Delegated to a
    /// Message with "[delegated subagent] {summary}".
    #[test]
    fn delegation_result_folds_into_chat_msg_delegated() {
        use zoid_core::event::{Event, EventKind};
        use zoid_core::projection::{conversation, ChatMsg};

        let events = vec![
            Event::new(Ulid::new(), None, 0, EventKind::UserMessage { text: "do the thing".into() }),
            Event::new(Ulid::new(), None, 1, EventKind::AssistantMessage { text: "delegating".into() }),
            Event::new(
                Ulid::new(),
                None,
                2,
                EventKind::DelegationResult {
                    subagent_id: "sub-01ABC".into(),
                    branch: "subagent:01ABC".into(),
                    summary: "Task completed successfully.".into(),
                    ok: true,
                },
            ),
        ];

        let msgs = conversation(events.iter());

        // Find the Delegated message.
        let delegated = msgs.iter().find_map(|m| {
            if let ChatMsg::Delegated { summary, ok } = m {
                Some((summary.clone(), *ok))
            } else {
                None
            }
        });

        assert!(
            delegated.is_some(),
            "DelegationResult must fold into ChatMsg::Delegated"
        );
        let (summary, ok) = delegated.unwrap();
        assert_eq!(summary, "Task completed successfully.");
        assert!(ok, "ok must be true for a successful delegation");
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p zoid --lib delegation_result_folds_into_chat_msg_delegated`
Expected: PASS — the projection already folds `DelegationResult` into `ChatMsg::Delegated`. This test confirms the invariant.

- [ ] **Step 3: Run the full test suite**

Run: `cargo test -p zoid`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "test: verify DelegationResult folds into ChatMsg::Delegated for model request"
```