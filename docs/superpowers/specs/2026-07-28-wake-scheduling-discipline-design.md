# Wake Scheduling Discipline — Design

**Date:** 2026-07-28
**Status:** Design (approved)
**Umbrella:** Prompt + runtime hardening for `schedule_wake` tooling.

## Problem

The LLM over-schedules `schedule_wake` calls — dispatching 2-3 wakes for the
same event in a single turn, and re-scheduling after a wake fires without
canceling prior ones. This accumulates duplicate wakes that each fire
independently, re-invoking the model multiple times for the same event and
producing duplicate/triplicate responses.

Four root causes, all confirmed in the current code:

1. **Tool description has no discipline guidance.** `wake.rs:16-18` says only
   "Schedule a one-shot reminder... Use when waiting on something to check
   later." Nothing about scheduling one per event or avoiding duplicates.

2. **Tool result gives no nudge.** `agent.rs:1949` returns `scheduled (id X)` —
   bare confirmation with no reminder to not schedule more for the same event.

3. **SYSTEM_PROMPT has zero wake guidance.** The `wrap_reassertion` mechanism
   never reinforces wake discipline.

4. **Runtime allows unlimited duplicates.** `WAKE_MAX_PENDING = 16` is a global
   cap but doesn't prevent 16 wakes for the *same* event. Per-note deduplication
   doesn't exist — the `handle_schedule_wake` function (`main.rs:6851`) blindly
   inserts every wake into `pending_wakes`.

## Goal

Prevent wake over-scheduling through three mutually-reinforcing changes:

1. **Prompt hardening** (same pattern as the subagent no-poll hardening shipped
   in 0.7.2): tool description, tool result, and SYSTEM_PROMPT.
2. **Runtime per-note deduplication**: reject a new wake if a pending wake with
   the same `note` already exists. Return a clear error explaining the
   duplicate, so the model sees feedback rather than silent success.

## Non-goals

- No change to `cancel_wake` — it already works correctly.
- No change to the wake firing mechanism or `rebuild_pending_wakes`.
- No change to `WAKE_MAX_PENDING` (16) — the per-note dedup is the targeted fix;
   the global cap remains as a backstop.
- No config flag — the dedup is unconditional.

## Design — three changes

### 1. Restructure the `schedule_wake` tool description (salience)

`crates/zoid-tools/src/wake.rs:16-18` — add discipline guidance:

**Current:**

> Schedule a one-shot reminder to resume THIS conversation after delay_secs
> seconds. On fire you are re-invoked with `note` as a message. Minimum 30s.
> Use when waiting on something to check later.

**New:**

> Schedule a one-shot reminder to resume THIS conversation after delay_secs
> seconds. On fire you are re-invoked with `note` as a message. Minimum 30s.
> Use when waiting on something to check later. Schedule exactly ONE wake per
> event — do not schedule multiple wakes for the same thing. If a wake is
> already pending, cancel it before scheduling a new one. Duplicate wakes for
> the same note are rejected.

### 2. Inject a nudge into the `schedule_wake` tool result (critical moment)

`agent.rs:1949` — append a discipline reminder to the success output:

**Current:**

```rust
Ok(Ok(id)) => (format!("scheduled (id {id})"), false),
```

**New:**

```rust
Ok(Ok(id)) => (format!(
    "scheduled (id {id}) — do not schedule additional wakes for the same \
     event. This wake will re-invoke you; cancel it with cancel_wake if you \
     no longer need it."
), false),
```

### 3. Add wake discipline to `SYSTEM_PROMPT` (periodic reinforcement)

`agent.rs:36-46` — append one sentence after the subagent discipline sentence:

> ...never poll for status or call list_subagents to check on a subagent you
> dispatched. When waiting on something, schedule exactly one wake — never
> schedule duplicate wakes for the same event, and cancel a pending wake before
> scheduling a replacement.

### 4. Runtime per-note deduplication (structural guardrail)

`main.rs:6851-6868` (`handle_schedule_wake`) — before inserting a new wake,
check if a pending wake with the same `note` already exists. If so, return an
error:

```rust
async fn handle_schedule_wake(
    app: &mut App,
    delay_secs: u64,
    note: String,
) -> Result<String, String> {
    validate_schedule(app.config.wake.enabled, app.pending_wakes.len(), delay_secs)?;

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

    let wake_id = Ulid::new().to_string();
    // ... rest unchanged
}
```

The error message is itself a nudge — it tells the model what to do instead
("cancel it first" / "wait for it to fire").

## Testing

- **Tool description test** (`wake.rs`): assert the description contains
  "exactly ONE wake per event" and "Duplicate wakes for the same note are
  rejected".
- **Tool result test** (`agent.rs`): if a test exists for the wake tool result,
  extend it to assert the nudge is present. If not, add a unit test asserting
  `SYSTEM_PROMPT` contains wake discipline language.
- **SYSTEM_PROMPT test** (`agent.rs`): extend `system_prompt_reinforces_no_poll`
  (or add a new test) to assert wake discipline ("exactly one wake", "duplicate
  wakes").
- **Per-note dedup test** (`main.rs`): add a test that scheduling a wake with a
  note matching an existing pending wake's note returns an error, and that
  scheduling with a different note succeeds.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid-tools/src/wake.rs` | Restructure tool description (change 1). Add description assertion tests. |
| `crates/zoid/src/agent.rs` | (a) Append wake discipline to `SYSTEM_PROMPT` (change 3). (b) Inject nudge into tool result (change 2). (c) Update/extend tests. |
| `crates/zoid/src/main.rs` | Add per-note dedup check in `handle_schedule_wake` (change 4). Add dedup test. |