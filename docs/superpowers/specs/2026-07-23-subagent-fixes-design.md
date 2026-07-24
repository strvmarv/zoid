# Fix Subagent UI Jumping, Iteration Limit, and Result Landing

## Problem

Three issues with subagent execution and display:

1. **Jumpy UI while running:** The subagent shares the main agent's `ui` channel
   (`ui.clone()` at `agent.rs:1559`). Every subagent event (ModelDelta, ToolCall,
   ToolResult) flows through `AgentUpdate::Appended` to `main.rs:2789`, where
   `apply_streaming()` is called. But `apply_streaming` (line 1292) doesn't check the
   event's branch — it blindly appends subagent ModelDelta/ToolCall events to the main
   conversation's projection cache. The subagent's streaming text and tool calls flash
   into the main conversation pane, then disappear on the next full rebuild (which
   filters by branch). This causes visible flickering/jumping.

2. **Iteration limit too tight:** `SUBAGENT_MAX_ITERATIONS = 25` in `subagent.rs` is
   too few for realistic read-edit-test-debug cycles. A single `read` + `edit` +
   `shell` round-trip is 3 iterations; with retries, 25 is easily exhausted.

3. **Result doesn't seem to land:** The plumbing exists (DelegationResult on the
   default branch → folded into `ChatMsg::Delegated` → mapped to a `Message` for the
   model → `plan_delegation_wake` fires a continuation turn). But the jumpy UI and
   iteration-limit aborts may mask or prevent a useful result from surfacing. Add
   trace logging to confirm the end-to-end flow after the first two fixes.

## Design

### §1 Fix jumpy UI — guard in the `Appended` handler

In `main.rs`'s `AgentUpdate::Appended` handler (line ~2789), add a branch check before
the `apply_streaming` call and projection cache invalidation:

```rust
AgentUpdate::Appended(ev) => {
    let mut delegation_arrived = false;
    if let EventKind::DelegationResult { subagent_id, .. } = &ev.kind {
        // ... existing DelegationResult handling unchanged ...
        delegation_arrived = true;
    }
    // ... existing ToolResult / tool_complete handling unchanged ...

    // Subagent-branch events are persisted to SQLite and pushed into
    // app.events, but the projection cache only cares about main-branch
    // events for the conversation view. Skip the incremental streaming path
    // AND cache invalidation for subagent-branch events — no UI churn.
    let is_subagent_branch = ev.branch != zoid_core::event::BranchId::default();

    if !is_subagent_branch && !app.proj.apply_streaming(&ev) {
        // Main-branch structural event — invalidate caches for a full rebuild.
        app.proj.events_len = None;
        app.body_cache.key = None;
    }

    // ... existing compaction_id handling unchanged ...
    app.events.push(*ev);
    // ... existing delegation wake handling unchanged ...
}
```

Key points:

- `apply_streaming` is only called for main-branch events (`!is_subagent_branch`).
- Cache invalidation (`events_len = None`, `body_cache.key = None`) is only triggered
  for main-branch structural events.
- Subagent events are still pushed into `app.events` (needed for the full event log,
  recall, embeddings) but don't touch the projection. No UI churn.
- `DelegationResult` events are on the default branch (`Event::new` defaults to
  `BranchId::default()`), so they are NOT subagent-branch events — they flow through
  the normal path and appear in the conversation as `ChatMsg::Delegated`.

Why not fix in `apply_streaming` itself? The `Appended` handler is the right place
because it has the full `ev` (with branch) and makes the decision about whether to
touch the projection at all. Guarding in `apply_streaming` would still trigger cache
invalidation (the `!apply_streaming(...)` branch) for subagent events, causing a
full rebuild on every subagent structural event — less churn than the current bug,
but still unnecessary work. Guarding at the handler avoids both paths.

### §2 Raise iteration limit to 100

In `subagent.rs`:

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

### §3 Trace logging for result landing

Add targeted `tracing::info!` calls at three points in the delegation flow to confirm
end-to-end behavior:

**a. DelegationResult arrival** (`main.rs`, `Appended` handler, `DelegationResult`
branch):

```rust
if let EventKind::DelegationResult { subagent_id, summary, ok, .. } = &ev.kind {
    tracing::info!(
        subagent_id = %subagent_id,
        ok = %ok,
        summary_len = summary.len(),
        "delegation result arrived"
    );
    // ... existing handling unchanged ...
}
```

**b. Wake decision** (`plan_delegation_wake`):

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
    // ... existing logic unchanged ...
}
```

**c. Continuation turn spawn** (at the spawn site, line ~2839):

```rust
if delegation_arrived && plan_delegation_wake(app) {
    tracing::info!("spawning continuation turn after delegation");
    spawn_turn(app);
}
```

These traces confirm: the result arrives, the wake decision fires, and the
continuation turn spawns. If the model still doesn't act on the result after these
fixes, the issue is in the model's behavior, not the plumbing.

### §4 What is not touched

- **The `ui` channel sharing** — the subagent still shares `ui.clone()`. This is
  correct: the `DelegationResult` event needs to flow through it to reach `main.rs`.
  The fix is filtering at the handler, not splitting channels.
- **`apply_streaming` itself** — not modified. The guard is in the `Appended`
  handler, which is the right place (it has the `ev` and can check the branch).
- **The subagent's event emission** — `run_agent_turn_cancellable` still emits events
  on the `subagent:<id>` branch. No change.
- **`plan_delegation_wake` / `should_wake_after_delegation` logic** — unchanged, just
  traced.
- **The worktree guard, abort supervisor, idle/ceiling timers** — unchanged.
- **`distill` / `verify_execution`** — unchanged.
- **`spawn_subagent`** — unchanged (the DelegationResult emission, branch handling,
  worktree commit/discard logic all stay as-is).

### §5 Testing

**Unit tests in `main.rs`:**

- A subagent-branch `ModelDelta` event processed through the `Appended` handler does
  NOT call `apply_streaming` — verify the projection cache's `msgs` vector is
  unchanged after the event.
- A subagent-branch `ToolCall` event processed through the `Appended` handler does NOT
  invalidate the projection cache (`events_len` stays `Some(...)`).
- A main-branch `ModelDelta` event still triggers `apply_streaming` (existing behavior
  preserved).
- A `DelegationResult` event (default branch) still triggers the normal path —
  `apply_streaming` or cache invalidation fires, and `delegation_arrived` is set.

**Unit tests in `subagent.rs`:**

- `SUBAGENT_MAX_ITERATIONS` is 100 (constant value check).
- A subagent that runs 50 tool-call iterations does not hit the cap (integration-level,
  using a FakeProvider that emits tool calls then a final text response on iteration 50).

**Integration tests:**

- A subagent dispatch where the subagent produces ModelDelta + ToolCall events —
  verify none of them appear in the main conversation's `ChatMsg` list (projection
  filtered by branch, no UI churn).
- A subagent that completes successfully — verify `DelegationResult` arrives on the
  default branch, `plan_delegation_wake` fires, and the continuation turn's request
  includes the `ChatMsg::Delegated` → `Message` with `[delegated subagent] {summary}`.
- A subagent that hits the iteration cap (100) — verify it aborts with the correct
  summary and `ok: false`.

### §6 Edge cases

- **DelegationResult on default branch:** Not a subagent-branch event — flows through
  `apply_streaming` / cache invalidation normally. The `ChatMsg::Delegated` card
  appears in the conversation. Correct.
- **Multiple subagents in flight:** Each has its own branch (`subagent:<id>`). All are
  filtered by the `is_subagent_branch` check. No UI churn from any of them.
- **Subagent aborts (idle timeout / ceiling / kill):** The abort produces a
  `DelegationResult` with `ok: false` and an abort summary. Same path — arrives on
  default branch, wakes the continuation turn. The model sees the failure summary.
- **Subagent event arrives between two main-branch events:** The subagent event is
  pushed into `app.events` but doesn't invalidate the projection. The next main-branch
  event triggers a full rebuild that correctly filters by branch — the subagent event
  is invisible in the conversation. No stale cache.
- **Subagent-branch event with `DelegationResult` kind:** Cannot happen —
  `DelegationResult` is always created on the default branch in `spawn_subagent.rs`
  (line 152, `Event::new` defaults to `BranchId::default()`). The `is_subagent_branch`
  check would be `false` for it regardless.