# Concurrent Subagent Execution — Design

> **Status:** DESIGN (brainstormed 2026-07-25). Ready for `writing-plans`.

---

## 1. Goal

Allow the main chat loop to continue operating — accepting user input,
streaming turns, dispatching new subagents — while one or more subagents
execute work in the background. Today the main loop blocks while any
subagent is in flight (`in_flight_subagents.is_empty()` is a gate on
`spawn_turn`). This unlocks parallelism between the orchestrator and its
delegates.

---

## 2. Design decisions (from brainstorming)

1. **DelegationResult arrival mid-turn:** Queue it. The result waits until
   the current turn completes, then the orchestrator gets a continuation
   turn via `wake_after_delegation`. This is the **current behavior** —
   just unblocked from requiring the main turn to be idle.

2. **Queued message + delegation continuation:** Both fire when the turn
   completes. Queued message first (user-initiated), then the delegation
   continuation (system-initiated). The existing `pending_message` +
   `wake_after_delegation` flags already handle ordering; the change is
   removing the `in_flight_subagents.is_empty()` gate from the idle check.

3. **Concurrent subagent dispatch:** Limit to N concurrent subagents
   (configurable, default 3). The N is a **global pool**, not per-turn.
   Excess `dispatch_subagent` calls queue until a slot frees. This
   replaces the current single-in-flight guard.

4. **Subagent progress in the UI:** Keep it in the subagents drawer only.
   The conversation stays clean; the drawer tracks in-flight work with
   live status (running/done/failed). No inline chips in the conversation
   for subagent progress (the existing delegated chip stays for the
   dispatch → result summary, but it's not updated live).

5. **Cross-turn subagent dispatch:** The model can dispatch subagents while
   subagents from a previous turn are still running, subject to the global
   N concurrent limit.

6. **SQLite write isolation:** Each subagent gets its own SQLite connection
   to the same DB file. WAL mode (already enabled) allows concurrent
   readers + one writer. The subagent's events go directly to SQLite via
   its own connection, bypassing the main session actor's mpsc queue.
   The main session actor continues to handle main-branch appends; the
   subagent's branch events are isolated by branch ID.

---

## 3. Architecture

### 3.1 Current flow

```
User message → spawn_turn → model streams → dispatch_subagent
  → main turn YIELDS (streaming = false, busy = true)
  → subagent runs in tokio::spawn (separate branch)
  → DelegationResult arrives on AgentUpdate channel
  → main loop consumes it at TurnComplete
  → wake_after_delegation fires a continuation turn
```

The main loop is blocked between dispatch and result arrival. The user
can queue a message (`pending_message`) but it won't fire until the
subagent finishes AND the turn completes.

### 3.2 New flow

```
User message → spawn_turn → model streams → dispatch_subagent
  → main turn CONTINUES streaming (busy = false, subagent runs in background)
  → user can type, queue messages, start new turns
  → DelegationResult arrives on AgentUpdate channel
  → if main turn is streaming: wake_after_delegation = true (deferred)
  → if main turn is idle: continuation turn fires immediately
  → queued message fires first (pending_message), then continuation
```

### 3.3 The idle check

Current (main.rs:6547):
```rust
let idle = !app.streaming && app.in_flight_subagents.is_empty() && !app.yielded;
```

New:
```rust
let idle = !app.streaming && !app.yielded;
```

Subagents in flight no longer block the main loop. The `busy` flag
becomes:
```rust
app.shell.busy = app.streaming;
```

The subagents drawer independently shows in-flight work.

### 3.4 Concurrent subagent pool

Current: a single in-flight guard (`in_flight` HashMap, but
`dispatch_subagent` is sequential within a turn — the agent loop holds
until the DelegationResult).

New: a global pool with a configurable max:

```toml
[subagent]
max_concurrent = 3  # default
```

The `dispatch_subagent` tool, when the pool is full, returns a "queued"
response to the model. The model can continue working (other tool calls,
reasoning) and the subagent starts when a slot frees. When a subagent
finishes, its `DelegationResult` is delivered as before, and the next
queued subagent starts.

Implementation: the `in_flight` HashMap becomes a bounded pool. A
`VecDeque<QueuedSubagent>` holds overflow. When a `DelegationResult`
removes an ID from the pool, the next queued subagent is spawned.

### 3.5 SQLite write isolation

Each subagent opens its own `EventStore` connection to the same DB file:

```rust
let store = EventStore::open(db_path)?;
// WAL mode + busy_timeout (already configured) handle concurrent writes
```

The subagent's `run_subagent` function currently takes a `SessionHandle`
clone. It would instead take an `EventStore` (or a lightweight
`SubagentSession` wrapper that owns its own connection). Appends go
directly to SQLite, bypassing the main session actor.

The main session actor continues to own:
- Main-branch appends (conversation events)
- Session metadata (create, rename, delete, touch, list)
- Snapshot requests (loads from SQLite, which sees all branches)

Cross-branch reads (e.g., the main loop's `snapshot` loading all events)
already work — WAL mode guarantees a reader sees a consistent snapshot
even while a writer is appending.

**Risk:** the `EventStore::append` path currently assumes single-writer
access (no row-level locking). With two writers (main actor + subagent),
SQLite's WAL handles the page-level locking, but the `busy_timeout` (5s)
must be sufficient. If both writers contend on the same page (the events
table), one waits up to 5s. In practice, appends are fast (single INSERT
+ FTS update), so contention should be sub-millisecond.

---

## 4. Config

```toml
[subagent]
max_concurrent = 3          # max simultaneous subagents (global pool)
# existing:
idle_timeout_secs = 300     # per-subagent idle timeout
hard_timeout_secs = 900     # per-subagent absolute timeout
```

`max_concurrent = 1` restores the current sequential behavior (one
subagent at a time, main loop still unblocked).

---

## 5. UI changes

- **Subagents drawer:** already shows in-flight subagents. No change
  needed — it reads `app.in_flight_subagents` which is already maintained.
  When a subagent finishes, its row is removed (existing behavior).
- **Busy state:** `app.shell.busy` reflects only `app.streaming`, not
  subagents. The user sees "idle" when the main turn is done, even if
  subagents are running.
- **Queued subagents:** the drawer could show a "queued" count when
  subagents are waiting for a slot. Minor enhancement.
- **Conversation:** no change. The delegated chip appears when
  `dispatch_subagent` is called (existing) and is updated when the
  `DelegationResult` arrives (existing).

---

## 6. Event ordering

The `DelegationResult` event is on the **default branch** (main branch),
not the subagent's branch. It's appended by the main loop when the
subagent's result arrives on the `AgentUpdate` channel. This doesn't
change — the subagent sends its result via the channel, and the main loop
appends the `DelegationResult` event.

With concurrent subagents, multiple `DelegationResult` events may arrive
while a turn is streaming. Each one:
1. Removes the subagent ID from `in_flight`
2. If the pool has queued subagents, spawns the next one
3. Sets `wake_after_delegation = true` (if the main turn is streaming)
4. If the main turn is idle, fires a continuation turn immediately

Multiple `wake_after_delegation` flags collapse to one continuation turn
(the flag is a bool, not a counter). The continuation turn sees all
pending `DelegationResult` events in the log and can act on them.

---

## 7. Risks

1. **SQLite write contention:** two writers on the same DB. WAL mode +
   `busy_timeout = 5000` handles it, but high-frequency appends from
   both the main turn and 3 subagents could cause contention. Mitigation:
   measure; if contention is real, batch subagent appends (buffer in
   memory, flush periodically or on completion).

2. **Context budget with concurrent subagents:** each subagent consumes
   context on its own branch. The main turn's context is unaffected
   (branch isolation). But the economy view's token ledger sums across
   all branches — concurrent subagents inflate the total. Mitigation:
   the ledger already tracks per-branch; the UI could show per-branch
   breakdown.

3. **Subagent kill propagation:** the current Esc → cancel-subagent flow
   arms a kill state and the next Esc fires all in-flight. With N
   concurrent, the first Esc arms, the second fires ALL. This is already
   the behavior (`subagent_kill_armed`). No change needed.

4. **Race on `DelegationResult` + `TurnComplete`:** if a `DelegationResult`
   arrives in the same event-loop tick as `TurnComplete`, the ordering
   matters. The current code processes `AgentUpdate` events in order, so
   `DelegationResult` is processed before the `TurnComplete` handler
   checks `wake_after_delegation`. This is correct.

---

## 8. Out of scope

- **Parallel tool calls within a single sub-turn:** the model already
  can't call multiple tools in one response. This is a provider/protocol
  limitation, not a zoid limitation. Out of scope.
- **Distributed subagents:** subagents running on a different machine.
  Out of scope.
- **Subagent-to-subagent communication:** subagents dispatching their own
  subagents. Already possible (recursive `dispatch_subagent`), but the
  concurrent pool would need to account for nested dispatches. Defer.