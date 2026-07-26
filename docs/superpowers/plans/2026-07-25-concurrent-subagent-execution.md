# Concurrent Subagent Execution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow the main chat loop to continue operating while subagents
execute in the background. A global pool (default 3) limits concurrent
subagents; excess dispatches queue. Each subagent gets its own SQLite
connection. The `DelegationResult` wake model fires per-result, not
per-pool-empty.

**Architecture:** Five tasks, ordered by dependency:
- Task 1: Config (`max_concurrent`) — no runtime change
- Task 2: Wake logic — remove `is_empty()` from `should_wake_after_delegation`,
  `plan_delegation_wake`, and the `TurnComplete` handler
- Task 3: Idle/busy gates — unblock `Submit`, update `busy` flag
- Task 4: Concurrent pool + queue — replace the single-in-flight guard in
  `dispatch_subagent` with a bounded pool; queue overflow
- Task 5: Session takeover — fire `fire_subagent_kill` on takeover

**Tech Stack:** Rust (`zoid-core`, `zoid`, `zoid-tools` crates). No new deps.

**Spec:** `docs/superpowers/specs/2026-07-25-concurrent-subagent-execution-design.md`

## Global Constraints

- **No coverage reduction.** All existing tests must pass.
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-core/src/config.rs` | `SubagentConfig.max_concurrent` | Modify (Task 1) |
| `crates/zoid/src/main.rs` | Wake logic, idle/busy gates, takeover, pool queue | Modify (Tasks 2-5) |
| `crates/zoid/src/agent.rs` | `dispatch_subagent` pool check; `TurnConfig` carries pool | Modify (Task 4) |

---

### Task 1: Config — `max_concurrent`

**Goal:** Add `max_concurrent` to `SubagentConfig` and its `PartialConfig`
mirror. Default 3. Wire through TOML parsing and merge.

**Files:**
- Modify: `crates/zoid-core/src/config.rs`

- [ ] **Step 1: Add the field to `SubagentConfig`**

In `crates/zoid-core/src/config.rs`, add to `SubagentConfig` (line 145):

```rust
pub struct SubagentConfig {
    /// Idle (no-progress) timeout in seconds; 0 = disabled. Default 900.
    pub idle_timeout_secs: u64,
    /// Absolute wall-clock ceiling in seconds; 0 = disabled. Default 1800.
    pub hard_timeout_secs: u64,
    /// Max simultaneous subagents (global pool). Default 3. 1 restores
    /// sequential dispatch (but the main loop is still unblocked).
    pub max_concurrent: usize,
}
```

Update `Default` (line 152):
```rust
impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 900,
            hard_timeout_secs: 1800,
            max_concurrent: 3,
        }
    }
}
```

- [ ] **Step 2: Add to `PartialSubagent` and the merge**

Find `PartialSubagent` (search for `PartialSubagent` in config.rs). Add:
```rust
pub max_concurrent: Option<usize>,
```

In the merge function, add:
```rust
if let Some(v) = p.subagent.max_concurrent {
    cfg.subagent.max_concurrent = v;
    prov.subagent_max_concurrent = *src;
}
```

Add `subagent_max_concurrent: Source` to `Provenance` and its `Default` impl.

- [ ] **Step 3: Write tests**

```rust
#[test]
fn subagent_max_concurrent_defaults_to_3() {
    let c = Config::default();
    assert_eq!(c.subagent.max_concurrent, 3);
}

#[test]
fn subagent_max_concurrent_overrides_via_toml() {
    let (pc, _) = parse_toml("[subagent]\nmax_concurrent = 1").unwrap();
    let layers = vec![(Source::UserGlobal, pc)];
    let (cfg, _) = merge(&layers);
    assert_eq!(cfg.subagent.max_concurrent, 1);
}
```

- [ ] **Step 4: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(config): subagent max_concurrent (default 3)"
```

---

### Task 2: Wake logic — per-result continuation

**Goal:** Remove `in_flight_subagents.is_empty()` from `should_wake_after_delegation`,
`plan_delegation_wake`, and the `TurnComplete` handler so continuation
turns fire per-result, not per-pool-empty.

**Files:**
- Modify: `crates/zoid/src/main.rs`

- [ ] **Step 1: Write tests for the new `should_wake_after_delegation`**

The existing test `should_wake_after_delegation_truth_table` (line 8181)
asserts the 3-arg truth table. Update it to the 2-arg form (no
`in_flight_empty`):

```rust
#[test]
fn should_wake_after_delegation_truth_table() {
    // Not streaming, not yielded → wake
    assert!(should_wake_after_delegation(false, false));
    // Streaming → no wake (deferred to TurnComplete)
    assert!(!should_wake_after_delegation(true, false));
    // Yielded → no wake (session is dead)
    assert!(!should_wake_after_delegation(false, true));
    // Both → no wake
    assert!(!should_wake_after_delegation(true, true));
}
```

- [ ] **Step 2: Update `should_wake_after_delegation` (line 6350)**

```rust
fn should_wake_after_delegation(streaming: bool, yielded: bool) -> bool {
    !streaming && !yielded
}
```

- [ ] **Step 3: Update `plan_delegation_wake` (line 6474)**

Remove `in_flight_subagents.is_empty()` from both the `should_wake_after_delegation`
call and the else-branch:

```rust
fn plan_delegation_wake(app: &mut App) -> bool {
    let wake = should_wake_after_delegation(app.streaming, app.yielded);
    // ... (logging unchanged)
    if wake {
        true
    } else {
        if !app.streaming && !app.yielded {
            app.wake_after_delegation = true;
        }
        false
    }
}
```

- [ ] **Step 4: Remove the `is_empty()` gate from `TurnComplete` (line 3092)**

Current:
```rust
if app.in_flight_subagents.is_empty() {
    // queued message, deferred wake, drain_due_wakes
}
```

New: remove the `if` wrapper. The block runs unconditionally:

```rust
// No in_flight_subagents gate — post-turn logic runs even with
// background subagents still in flight.
let mut spawned = false;
if let Some(text) = app.pending_message.take() { ... }
if spawned {
    app.wake_after_delegation = false;
} else if take_deferred_delegation_wake(app) {
    spawn_turn(app);
}
// drain_due_wakes ...
```

- [ ] **Step 5: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "fix(agent): per-result delegation wake (drop is_empty gate)"
```

---

### Task 3: Idle/busy gates — unblock the main loop

**Goal:** Remove `in_flight_subagents.is_empty()` from the idle check
and the `Submit` action. Update `busy` to reflect only `streaming`. Keep
session/worktree management gated.

**Files:**
- Modify: `crates/zoid/src/main.rs`

- [ ] **Step 1: Update the idle check (line 6547)**

```rust
let idle = !app.streaming && !app.yielded;
```

- [ ] **Step 2: Update `busy` (line 2741)**

```rust
app.shell.busy = app.streaming;
```

Note: the motion-tick guard (line 3402) keeps
`!app.in_flight_subagents.is_empty()` so the drawer spinner animates
while subagents run. This is intentional — do NOT remove it.

- [ ] **Step 3: Unblock `Submit` (line 3959)**

Current:
```rust
if app.streaming || !app.in_flight_subagents.is_empty() {
    // queue message
}
```

New:
```rust
if app.streaming {
    // queue message (subagents running is no longer a blocker)
}
```

- [ ] **Step 4: Keep session/worktree management gated**

Lines 4113, 4176, 5881, 6303 — leave the `in_flight_subagents.is_empty()`
gate in place. Update the hint message to say:
```
"N subagents running — press Esc to kill them or wait for completion"
```

- [ ] **Step 5: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(agent): unblock main loop while subagents run"
```

---

### Task 4: Concurrent pool + queue

**Goal:** Replace the single-in-flight guard in `dispatch_subagent` with
a bounded pool. Excess dispatches queue. When a `DelegationResult` frees
a slot, the next queued subagent spawns.

**Files:**
- Modify: `crates/zoid/src/main.rs` (pool queue on `App`, drain on `DelegationResult`)
- Modify: `crates/zoid/src/agent.rs` (`dispatch_subagent` pool check)

- [ ] **Step 1: Add `QueuedSubagent` and the queue to `App`**

In `crates/zoid/src/main.rs`, add:

```rust
/// A subagent waiting for a pool slot to open.
struct QueuedSubagent {
    task: String,
    agent: String,
    resolved_profile: zoid_core::agent_profile::AgentProfile,
    resolved_name: String,
    cwd: PathBuf,
    branch: zoid_core::event::BranchId,
    tool_call_id: String,
    session_id: ulid::Ulid,
    want_worktree: bool,
}
```

Add to `App`:
```rust
queued_subagents: std::collections::VecDeque<QueuedSubagent>,
```

Initialize in the `App` constructor: `queued_subagents: VecDeque::new()`.

- [ ] **Step 2: Add `max_concurrent` to `TurnConfig`**

In `crates/zoid/src/agent.rs`, add to `TurnConfig` (line 147):

```rust
/// Max concurrent subagents (global pool). 0 = unlimited. Default 3.
pub max_concurrent: usize,
```

Default in the `TurnConfig` constructor (line 288): `max_concurrent: 3,`.

- [ ] **Step 3: Replace the single-in-flight guard in `dispatch_subagent`**

In `crates/zoid/src/agent.rs` (line 1524-1553), replace the
`if !set.lock().unwrap().is_empty()` block with a pool-size check:

```rust
Some(zoid_tools::ToolKind::Emitting) if tc.name == "dispatch_subagent" => {
    // Pool check: if at capacity, return a "queued" response (not an error).
    if let Some(set) = &config.in_flight {
        let n = set.lock().unwrap().len();
        if n >= config.max_concurrent && config.max_concurrent > 0 {
            emit(
                &session,
                &mut events,
                ui,
                &config.branch,
                EventKind::ToolResult {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    output: format!(
                        "subagent queued ({n} running, max {})",
                        config.max_concurrent
                    ),
                    is_error: false,
                },
                session_id,
                now,
            )
            .await?;
            // Signal the main loop to queue the subagent.
            let _ = ui.send(AgentUpdate::SubagentQueued {
                tool_call_id: tc.id.clone(),
                task: task.clone(),
                agent: resolved_agent_name.clone(),
            }).await;
            continue;
        }
    }
    // ... (rest of dispatch: resolve profile, create handle, spawn — unchanged)
```

**Note:** The `task` and `agent` variables are parsed AFTER the current
guard. For the queue path, they need to be parsed before the pool check.
Move the `task` extraction (lines 1555-1560) and the agent resolution
(lines 1574-1612) above the pool check.

- [ ] **Step 4: Add `SubagentQueued` to `AgentUpdate`**

In `crates/zoid/src/agent.rs`, add a new variant:

```rust
/// A dispatch_subagent call was queued because the pool is full.
SubagentQueued {
    tool_call_id: String,
    task: String,
    agent: String,
},
```

- [ ] **Step 5: Handle `SubagentQueued` in the main loop**

In `crates/zoid/src/main.rs`, in the `AgentUpdate` match:

```rust
AgentUpdate::SubagentQueued { tool_call_id, task, agent } => {
    // Resolve the profile and cwd the same way dispatch_subagent does.
    // For now, store the minimal info needed to spawn later.
    // The task/agent were already resolved in the agent loop; we need
    // to carry enough to spawn. In practice, the simplest approach:
    // store the raw task + agent name + the turn's cwd/branch.
    app.queued_subagents.push_back(QueuedSubagent {
        task,
        agent,
        resolved_profile: app.base_profile.clone(), // resolved later
        resolved_name: agent,
        cwd: /* current turn cwd */,
        branch: /* current branch */,
        tool_call_id,
        session_id: app.session_id,
        want_worktree: false, // queued subagents don't get worktrees (simplification)
    });
}
```

**Complexity note:** The profile resolution and worktree creation happen
in the agent loop, not the main loop. The queued subagent needs the
resolved profile. Two options:
- **Option A:** Carry the resolved `AgentProfile` through the
  `SubagentQueued` event. The agent loop resolves it before sending.
  Simplest — the profile is already resolved by the time the pool check
  fires.
- **Option B:** Re-resolve in the main loop. Needs the `AgentRegistry`
  in `App`, which is available (`app.agents`).

**Recommendation:** Option A. Add `resolved_profile: AgentProfile` and
`resolved_name: String` to `SubagentQueued`. The agent loop already has
both.

- [ ] **Step 6: Drain the queue on `DelegationResult`**

In `crates/zoid/src/main.rs`, in the `DelegationResult` handler (line 2979),
after removing the finished subagent from `in_flight`:

```rust
if let EventKind::DelegationResult { subagent_id, .. } = &ev.kind {
    app.in_flight_subagents.retain(|s| s.id != *subagent_id);
    app.in_flight.lock().unwrap().remove(subagent_id);
    // ... (existing kill_armed reset)

    // Drain: if a slot opened and the queue is non-empty, spawn the next.
    while app.in_flight.lock().unwrap().len() < app.config.subagent.max_concurrent
        && !app.queued_subagents.is_empty()
    {
        let qs = app.queued_subagents.pop_front().unwrap();
        // Spawn the subagent (same path as dispatch_subagent's spawn).
        spawn_queued_subagent(app, qs);
    }
}
```

`spawn_queued_subagent` is a helper in `main.rs` that mirrors the
`spawn_subagent` call in `agent.rs:1672` — creates the ULID, handle,
worktree (if any), and calls `spawn_subagent::spawn_subagent`.

- [ ] **Step 7: Write tests**

Test that `dispatch_subagent` returns a "queued" response when the pool
is full, and that the `DelegationResult` drains the queue.

This requires an integration test in `crates/zoid/tests/` that:
1. Seeds a turn with a `dispatch_subagent` tool call
2. Sets `max_concurrent = 1` and pre-fills `in_flight` with one handle
3. Asserts the tool result says "queued"
4. Appends a `DelegationResult` event
5. Asserts the queued subagent was spawned (new `SubagentStarted` event)

- [ ] **Step 8: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(agent): concurrent subagent pool with queue overflow"
```

---

### Task 5: Session takeover — kill subagents

**Goal:** When a session is taken over by another instance, fire
`fire_subagent_kill` to cancel all in-flight subagents before yielding.

**Files:**
- Modify: `crates/zoid/src/main.rs`

- [ ] **Step 1: Update `SessionTakenOver` handler (line 3140)**

```rust
AgentUpdate::SessionTakenOver => {
    if let Some(cancel) = &app.turn_cancel {
        cancel.cancel();
    }
    app.streaming = false;
    // Kill all in-flight subagents (concurrency: they may be running)
    fire_subagent_kill(&app.in_flight, None); // None = kill all
    app.in_flight_subagents.clear();
    app.in_flight.lock().unwrap().clear();
    app.queued_subagents.clear();
    app.yielded = true;
    app.shell.status_hint =
        Some("session taken over by another instance".into());
}
```

Note: `fire_subagent_kill` is in `agent.rs` and takes
`(&Arc<Mutex<HashMap<String, SubagentHandle>>>, target: Option<&str>)`.
`None` kills all. Import or call it via the agent module.

- [ ] **Step 2: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "fix(agent): kill subagents on session takeover"
```

---

## Self-Review

**Gilfoyle review (spec) issues addressed:**
- C1 (wake logic drops results): Task 2 — per-result wake, drops `is_empty()` from `should_wake_after_delegation` and `plan_delegation_wake`
- C2 (TurnComplete gate blocks post-turn logic): Task 2 Step 4 — removes the `if` wrapper
- H1 (4 user actions still blocked): Task 3 — `Submit` unblocked; session/worktree management stays gated (documented)
- H2 (takeover orphans subagents): Task 5 — `fire_subagent_kill` on `SessionTakenOver`
- H3 (in_flight vs in_flight_subagents): Task 4 — pool check on `in_flight` (handle registry), queue on `App.queued_subagents`
- M1-M3: documented in spec, no code action needed

**Risk areas:**
- Task 4 is the largest task — the `dispatch_subagent` refactor moves
  profile resolution above the pool check and adds a new `AgentUpdate`
  variant. Test thoroughly.
- The `SubagentQueued` event carries a full `AgentProfile` — it's a
  `Clone`-able struct, but it's large. Acceptable for a rare event.
- The `spawn_queued_subagent` helper in Task 4 Step 6 duplicates some
  logic from `agent.rs:1672`. DRY by extracting a shared `spawn_subagent_from_main`
  function if the duplication is significant.
- SQLite per-subagent connections (spec §6) are NOT in this plan — the
  subagent still uses the shared `SessionHandle` actor for appends. The
  actor's mpsc channel serializes writes. This is safe for correctness;
  the per-connection optimization is a follow-up if write contention
  becomes measurable.