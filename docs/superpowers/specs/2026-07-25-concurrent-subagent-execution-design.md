# Concurrent Subagent Execution — Design (Revised)

> **Status:** DESIGN (brainstormed + gilfoyle-reviewed, 2026-07-25). Ready for `writing-plans`.
>
> **Revision:** Addresses gilfoyle review: C1 (wake logic drops results), C2
> (TurnComplete gate blocks all post-turn logic), H1 (4 user actions still
> blocked), H2 (takeover orphans subagents), H3 (in_flight vs
> in_flight_subagents conflation), M1-M3 (under-specification).

---

## 1. Goal

Allow the main chat loop to continue operating — accepting user input,
streaming turns, dispatching new subagents — while one or more subagents
execute work in the background. Today the main loop blocks while any
subagent is in flight. The `in_flight_subagents.is_empty()` gate appears
in **six** locations across `main.rs`, all of which must be addressed.

---

## 2. Design decisions (from brainstorming)

1. **DelegationResult arrival mid-turn:** Queue it. The result waits until
   the current turn completes, then the orchestrator gets a continuation
   turn. The wake must fire per-result, not per-pool-empty.
2. **Queued message + delegation continuation:** Both fire when the turn
   completes. Queued message first, then continuation.
3. **Concurrent subagent dispatch:** Limit to N concurrent subagents
   (configurable, default 3). Global pool, not per-turn. Excess calls queue.
4. **Subagent progress in the UI:** Subagents drawer only. Conversation
   stays clean.
5. **Cross-turn subagent dispatch:** Allowed, subject to the global pool.
6. **SQLite write isolation:** Each subagent gets its own `EventStore`
   connection to the same DB file (WAL mode + `busy_timeout`). Subagent-
   branch events go directly to SQLite, bypassing the main session actor.
   The `DelegationResult` event (on the default branch) is still appended
   by the main loop via the session actor.

---

## 3. The six `is_empty()` gates

Every `in_flight_subagents.is_empty()` gate must be addressed. Leaving
any unchanged silently drops results or blocks the user.

### 3.1 The idle check (main.rs:6547)

**Current:** `let idle = !app.streaming && app.in_flight_subagents.is_empty() && !app.yielded;`

**New:** `let idle = !app.streaming && !app.yielded;`

Subagents in flight no longer block the main loop.

### 3.2 The busy flag (main.rs:2741)

**Current:** `app.shell.busy = app.streaming || !app.in_flight_subagents.is_empty();`

**New:** `app.shell.busy = app.streaming;`

The subagents drawer independently shows in-flight work. The motion-tick
guard (main.rs:3402) keeps `!app.in_flight_subagents.is_empty()` so the
spinner animates while subagents run — this is intentional (the drawer's
spinner should spin even when the status bar shows "idle").

### 3.3 The wake logic — `should_wake_after_delegation` (main.rs:6350)

**Current:**
```rust
fn should_wake_after_delegation(streaming, in_flight_empty, yielded) -> bool {
    !streaming && in_flight_empty && !yielded
}
```

**Problem:** With N>1, subagent A finishes while B runs. `in_flight_empty
= false` → wake returns `false` → A's result is never surfaced as a
continuation turn.

**New:** Drop the `in_flight_empty` term. A continuation turn should fire
whenever a `DelegationResult` arrives and the main turn isn't streaming,
regardless of whether other subagents are still running:

```rust
fn should_wake_after_delegation(streaming, yielded) -> bool {
    !streaming && !yielded
}
```

### 3.4 The wake arming — `plan_delegation_wake` (main.rs:6474-6500)

**Current:** The else-branch (6495) only arms `wake_after_delegation` if
`app.in_flight_subagents.is_empty()`.

**New:** Remove the `is_empty()` condition. Arm the wake whenever the main
turn isn't streaming and hasn't yielded:

```rust
if !app.streaming && !app.yielded {
    app.wake_after_delegation = true;
}
```

### 3.5 The `TurnComplete` handler (main.rs:3092)

**Current:** ALL post-turn logic (queued message, deferred wake,
`drain_due_wakes`) is gated inside `if app.in_flight_subagents.is_empty()`.

**New:** Remove the gate. The post-turn logic runs on every `TurnComplete`:

```rust
AgentUpdate::TurnComplete => {
    // ... (existing streaming=false, cancel token cleanup)
    // No in_flight_subagents gate — run unconditionally:
    if let Some(text) = app.pending_message.take() { ... }
    if app.wake_after_delegation { ... }
    drain_due_wakes(...)
}
```

### 3.6 User action gates (main.rs:3959, 4113, 4176, 5881, 6303)

**Current:** Five user actions gate on `!app.in_flight_subagents.is_empty()`:
- `Submit` (3959) — queue message
- `:new` / `NewSession` (5881) — start new session
- `:session resume` / `SessionPick` (4113) — switch session
- `:session delete` / `SessionDelete` (4176) — delete session
- `:worktree` / `handle_worktree_request` (6303) — enter worktree

**Decision:**

- **`Submit` (3959):** Remove the subagent gate. The user can submit while
  subagents run. The `streaming` gate remains — if a turn is streaming, the
  message queues. If the main loop is idle but subagents are running, the
  message spawns a turn immediately.

- **`SessionPick`, `NewSession`, `SessionDelete`, `handle_worktree_request`:**
  These change the session/cwd context. With subagents writing to the
  current session's DB on their own branches, switching sessions mid-
  subagent is safe (branch isolation), but the subagents would continue
  writing to the OLD session's DB. **Keep these gated** — session/worktree
  management is blocked while subagents are in flight. The user sees a
  hint: "N subagents running — wait or Esc to kill". This is a deliberate
  UX cliff: killing subagents to switch sessions is a reasonable tradeoff.

---

## 4. Session takeover (H2)

**Current:** `SessionTakenOver` (main.rs:3140) clears `in_flight_subagents`
(UI only) and sets `yielded = true`, but does NOT kill the running
subagent tasks. Today this is safe because the main loop is blocked —
takeover can't arrive while a subagent runs.

**New:** `SessionTakenOver` must fire `fire_subagent_kill` to cancel all
in-flight subagents before yielding:

```rust
AgentUpdate::SessionTakenOver => {
    if let Some(cancel) = &app.turn_cancel { cancel.cancel(); }
    app.streaming = false;
    // Kill all in-flight subagents (concurrency: they may be running)
    fire_subagent_kill(&app.in_flight);
    app.in_flight_subagents.clear();
    app.in_flight.lock().unwrap().clear();
    app.yielded = true;
    ...
}
```

`fire_subagent_kill` already exists and fires the `CancelToken` in each
`SubagentHandle`. The subagent's `run_agent_turn_cancellable` checks the
token each iteration and exits cleanly.

---

## 5. Concurrent subagent pool

### 5.1 The two registries (H3)

There are TWO distinct structures — the spec must not conflate them:

- **`app.in_flight`** (`Arc<Mutex<HashMap<String, SubagentHandle>>>`):
  the live `SubagentHandle` registry. Used by `fire_subagent_kill` and
  the timeout supervisor. Each handle carries a `CancelToken`.
- **`app.in_flight_subagents`** (`Vec<SubagentInfo>`): the UI drawer's
  display list (`id`, `task`, `agent`).

Both must stay in sync. The pool limit applies to the count in
`app.in_flight` (the real registry). The UI list mirrors it.

### 5.2 Pool structure

Add a `queued_subagents: VecDeque<QueuedSubagent>` to `App`. When the
pool is full, `dispatch_subagent` pushes to the queue instead of
spawning. When a `DelegationResult` removes an ID from the pool, the
next queued subagent is spawned.

```rust
struct QueuedSubagent {
    task: String,
    agent: String,   // profile name
    // Carried from the dispatch_subagent tool call:
    branch: BranchId,
    cwd: PathBuf,
    // ... other run_subagent params
}
```

### 5.3 The `dispatch_subagent` tool response

**Current:** The tool returns a hard error if any subagent is in flight
(agent.rs:1525-1553).

**New:** When the pool is full, the tool returns a "queued" response
(not an error):

```json
{"output": "subagent queued (N running, position M in queue)"}
```

The model can continue working (other tool calls, reasoning). When a
slot frees, the subagent starts and the `SubagentStarted` event is
appended as usual. The `DelegationResult` arrives later.

When the pool has room, the tool returns the normal "dispatched"
response.

### 5.4 Config

```toml
[subagent]
max_concurrent = 3  # default; 1 restores sequential behavior (main loop still unblocked)
```

`max_concurrent = 1` means: one subagent at a time, but the main loop is
free for the user. Excess dispatches queue.

---

## 6. SQLite write isolation

### 6.1 The invariant

Subagent-branch events are isolated by `BranchId`. Each subagent writes
to its own branch. The only shared-branch event is `DelegationResult`,
which is on the default branch and is still appended by the **main loop**
via the session actor (not by the subagent's direct connection).

### 6.2 The per-subagent connection

`run_subagent` currently takes a `SessionHandle` clone. It would instead
take an `EventStore` (or a `SubagentSession` wrapper) that owns its own
`Connection` to the same DB file. WAL mode + `busy_timeout = 5000`
handles concurrent writers.

### 6.3 Signature churn (M2)

`run_agent_turn_cancellable` and every `emit`/`emit_with_tokens` call
take `session: &SessionHandle`. To bypass the actor, either:

- **Option A:** A `SubagentSession` struct that implements the same
  `append()` / `snapshot()` async interface but owns its own
  `EventStore`. Threaded through ~20 call sites as a replacement for
  `SessionHandle`.

- **Option B:** Make `run_agent_turn_cancellable` generic over the
  session type. More invasive but cleaner.

**Recommendation:** Option A. The subagent turn loop calls `append` and
`snapshot` (for recall). A `SubagentSession` wrapper that owns an
`EventStore` and implements the same interface is the smallest diff.

### 6.4 Write ordering (M1)

The main session actor serializes main-branch appends (total order).
Direct subagent connections break that total order for subagent-branch
events — but subagent-branch events are isolated by `BranchId` and
replayed independently. SQLite WAL serializes writers via the exclusive
write lock; commit order across connections is nondeterministic but
safe because branches don't interleave. This invariant must be stated
in the implementation plan.

---

## 7. Economy token ledger (M3)

`token_ledger` sums ALL events regardless of branch. Concurrent
subagents inflate the economy view's total live (not just on reload).
The projection cache (`app.proj.ledger_total`) is computed from
`app.events.iter()` which includes subagent-branch events.

**Mitigation:** The economy drawer already shows the total. With
concurrent subagents, the total includes their spend — which is
correct (they ARE spending tokens). The per-branch breakdown is a
future enhancement. No code change needed for correctness; the total
is honest.

---

## 8. Event ordering

`DelegationResult` events are on the **default branch** (verified:
`spawn_subagent.rs:153-164` creates them via `Event::new(...)` without
`.with_branch()`, so `BranchId::default()`). They are appended by the
main loop when the subagent's result arrives on the `AgentUpdate`
channel. This doesn't change.

With concurrent subagents, multiple `DelegationResult` events may arrive
while a turn is streaming. Each one:
1. Removes the subagent ID from `in_flight` and `in_flight_subagents`
2. If the pool has queued subagents, spawns the next one
3. If the main turn is not streaming and not yielded: arms
   `wake_after_delegation = true`
4. If the main turn is streaming: `wake_after_delegation` stays armed
   (set by `plan_delegation_wake` at TurnComplete)

At `TurnComplete` (now unconditionally, not gated on `is_empty()`):
1. Queued message fires first (`pending_message`)
2. Then `wake_after_delegation` fires a continuation turn
3. Then `drain_due_wakes` fires scheduled wakes

The continuation turn sees all pending `DelegationResult` events in the
log and can act on them. Multiple results collapse to one continuation
turn (the flag is a bool).

---

## 9. Risks

1. **SQLite write contention:** two+ writers on the same DB. WAL +
   `busy_timeout` handles it. Mitigation: measure; batch if needed.
2. **Economy ledger inflation:** concurrent subagents inflate the total.
   This is correct (they are spending). Per-branch breakdown is future.
3. **Session takeover orphans:** fixed by firing `fire_subagent_kill` in
   the `SessionTakenOver` handler (§4).
4. **Race on `DelegationResult` + `TurnComplete`:** the main loop
   processes `AgentUpdate` events in order. `DelegationResult` is
   processed before `TurnComplete` checks `wake_after_delegation`.
   Correct.

---

## 10. Out of scope

- Parallel tool calls within a single sub-turn (provider/protocol limit)
- Distributed subagents (different machines)
- Subagent-to-subagent communication (recursive dispatch with pool accounting)
- Per-branch economy breakdown in the UI