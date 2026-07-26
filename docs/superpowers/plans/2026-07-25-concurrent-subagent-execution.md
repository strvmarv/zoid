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

Find `PartialSubagent` (config.rs, search for `PartialSubagent`). Add:
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

Add `subagent_max_concurrent: Source` to `Provenance`.

**C1 fix — all construction sites:** `Provenance` has no `Default`
derive and is constructed by name at **three** sites:
1. `config.rs:512` (the `merge` function)
2. `main.rs:7539` (the `test_app()` helper)
3. `crates/zoid-tui/tests/shell_snapshot.rs:933` (snapshot test)

Deriving `Default` on `Provenance` (it's `Copy + all-fields-are-Source`,
trivial) removes the footgun permanently. Add `#[derive(Default)]` to
`Provenance` and `Source` (if not already). Then update the `merge` site
to use `..Provenance::default()` for the new field, and the other two
sites don't need changes (they already spread `..Default::default()` or
will pick up the derived default).

If deriving `Default` is too invasive (e.g., `Source` doesn't derive
`Default`), instead enumerate all three sites and add
`subagent_max_concurrent: Source::Default` to each.

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
// Preserve the !app.streaming guard around drain_due_wakes — a
// queued message or deferred wake may have just set streaming=true
// and called spawn_turn. Draining while streaming would double-spawn.
if !app.streaming {
    drain_due_wakes(app).await?;
}
```

**Note:** `take_deferred_delegation_wake` (main.rs:6506) is
`wake = app.wake_after_delegation && !app.yielded` — it has no
`is_empty()` term and needs **no change**. The per-result wake
works because `plan_delegation_wake` (Step 3) now arms
`wake_after_delegation` regardless of pool state. Do NOT modify
`take_deferred_delegation_wake`.

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

- [ ] **Step 5: Update the `submit_while_delegating_queues_message` test**

The test at main.rs:8150 asserts that `Submit` while subagents are
running queues the message (`!app.streaming`, `pending_message == Some(...)`).
After Step 3, `Submit` with `streaming=false` spawns a turn immediately
(subagents running is no longer a blocker). Update the test to reflect
the new behavior: `Submit` while subagents run and no turn is streaming
spawns a turn (`app.streaming == true`, `pending_message == None`).

The test for the *actual* queue path (`Submit` while `streaming=true`
AND subagents running) should still assert `pending_message == Some(...)`.

- [ ] **Step 6: Run the gate + commit**

```bash

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
    want_worktree: bool,
    tool_call_id: String,
    session_id: ulid::Ulid,
}
```

Add to `App`:
```rust
queued_subagents: std::collections::VecDeque<QueuedSubagent>,
```

Initialize in the `App` constructor: `queued_subagents: VecDeque::new()`.

- [ ] **Step 2: Add `max_concurrent` to `TurnConfig` and wire it in `spawn_turn`**

In `crates/zoid/src/agent.rs`, add to `TurnConfig` (line 147):

```rust
/// Max concurrent subagents (global pool). 0 = unlimited. Default 3.
pub max_concurrent: usize,
```

Default in the `TurnConfig` constructor (the `chat_turn_config_with`
function, ~line 270): `max_concurrent: 3,`.

**C2 fix — wire in `spawn_turn`:** In `crates/zoid/src/main.rs`,
`spawn_turn` (~line 6589) sets `turn_config.subagent_idle` and
`turn_config.subagent_ceiling` from `app.config.subagent.*`. Add
alongside them:

```rust
turn_config.max_concurrent = app.config.subagent.max_concurrent;
```

Without this, the pool check always sees the hardcoded default (3),
ignoring the user's config.

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

In `crates/zoid/src/agent.rs`, add a new variant. **C3 fix:** The variant
must carry the resolved profile (Option A) — not just `task`/`agent` —
so the main loop can spawn without re-resolving. `AgentProfile` is
`Clone + Send` (already moved into `tokio::spawn` in `spawn_subagent`).

```rust
/// A dispatch_subagent call was queued because the pool is full.
/// Carries everything needed to spawn when a slot opens.
SubagentQueued {
    tool_call_id: String,
    task: String,
    agent: String,
    resolved_profile: zoid_core::agent_profile::AgentProfile,
    resolved_name: String,
    want_worktree: bool,
    cwd: PathBuf,
},
```

- [ ] **Step 5: Handle `SubagentQueued` in the main loop**

In `crates/zoid/src/main.rs`, in the `AgentUpdate` match:

```rust
AgentUpdate::SubagentQueued {
    tool_call_id, task, agent, resolved_profile, resolved_name,
    want_worktree, cwd,
} => {
    app.queued_subagents.push_back(QueuedSubagent {
        task,
        agent,
        resolved_profile,
        resolved_name,
        cwd,
        branch: zoid_core::event::BranchId::default(),
        tool_call_id,
        session_id: app.session_id,
        want_worktree,
    });
}
```

**M2 fix:** `want_worktree` is carried from the agent loop (parsed
from the tool args before the pool check). A queued subagent with
`worktree=true` gets its worktree when spawned — no behavior cliff.

- [ ] **Step 6: Drain the queue on `DelegationResult`**

In `crates/zoid/src/main.rs`, in the `DelegationResult` handler (line 2979),
after removing the finished subagent from `in_flight`:

```rust
if let EventKind::DelegationResult { subagent_id, .. } = &ev.kind {
    app.in_flight_subagents.retain(|s| s.id != *subagent_id);
    app.in_flight.lock().unwrap().remove(subagent_id);
    // ... (existing kill_armed reset)

    // Drain: if a slot opened and the queue is non-empty, spawn the next.
    // M5 fix: treat max_concurrent = 0 as unlimited (never drain from
    // queue — but with 0 the pool check never queues either, so the
    // queue is always empty in that mode).
    let max = app.config.subagent.max_concurrent;
    while max == 0 || app.in_flight.lock().unwrap().len() < max
        && !app.queued_subagents.is_empty()
    {
        let qs = app.queued_subagents.pop_front().unwrap();
        spawn_queued_subagent(app, qs);
    }
}
```

**H4 fix — `spawn_queued_subagent` must register the handle:**

The helper must replicate the full spawn path from `agent.rs:1653-1693`:
create the ULID, create the `SubagentHandle` (cancel + hard + progress +
abort_reason), **insert it into `app.in_flight`** (the handle registry —
NOT just the UI list), send `SubagentStarted`, create the worktree (if
`want_worktree`), and call `crate::spawn_subagent::spawn_subagent`.

```rust
fn spawn_queued_subagent(app: &mut App, qs: QueuedSubagent) {
    let sub_ulid = ulid::Ulid::new();
    let sub_id = format!("sub-{sub_ulid}");

    let sub_cancel = tokio_util::sync::CancellationToken::new();
    let sub_hard = tokio_util::sync::CancellationToken::new();
    let sub_progress = std::sync::Arc::new(
        std::sync::atomic::AtomicI64::new(now_ms())
    );
    let sub_abort_reason = std::sync::Arc::new(
        std::sync::Mutex::new(None)
    );

    // Register the handle BEFORE spawning (H4 — without this,
    // fire_subagent_kill and the timeout supervisor can't reach it).
    app.in_flight.lock().unwrap().insert(
        sub_id.clone(),
        zoid::agent::SubagentHandle {
            cancel: sub_cancel.clone(),
            hard: sub_hard.clone(),
            progress: sub_progress.clone(),
            abort_reason: sub_abort_reason.clone(),
            task: qs.task.clone(),
            agent: qs.resolved_name.clone(),
        },
    );

    // Notify the UI (existing handler at main.rs:3307 pushes to
    // in_flight_subagents + shell.subagent_rows — no change needed).
    let _ = app.ui_tx.send(AgentUpdate::SubagentStarted {
        id: sub_id.clone(),
        task: qs.task.clone(),
        agent: qs.resolved_name.clone(),
    }).await;

    // Worktree (if requested — carried from the original dispatch call).
    let wt = if qs.want_worktree && std::path::Path::new(".git").exists() {
        crate::worktree::create_worktree(
            std::path::Path::new("."),
            &format!("sub-{sub_ulid}"),
        ).ok()
    } else {
        None
    };
    let cwd = wt.as_ref()
        .map(|w| std::fs::canonicalize(w.path())
            .unwrap_or_else(|_| w.path().to_path_buf()))
        .unwrap_or_else(|| qs.cwd.clone());

    // Spawn — pull params from `app` (M1 fix: explicit param sources).
    crate::spawn_subagent::spawn_subagent(
        qs.task,
        qs.resolved_profile,
        app.events.snapshot(),        // seed: current event log
        app.provider.clone(),          // provider
        cwd,
        app.model.clone(),             // model
        // thinking mode — resolve the same way spawn_turn does:
        zoid_provider::ThinkingMode::Off, // simplified; resolve from config
        app.session.clone(),           // session (shared actor)
        app.session_id,                // session_id
        app.ui_tx.clone(),             // ui channel
        now_ms as fn() -> i64,         // clock
        sub_id.clone(),
        wt,
        app.config.approval.clone(),   // approval config
        sub_cancel,
        sub_hard,
        sub_progress,
        sub_abort_reason,
        // idle/ceiling from config (same as spawn_turn):
        (app.config.subagent.idle_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(
                app.config.subagent.idle_timeout_secs)),
        (app.config.subagent.hard_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(
                app.config.subagent.hard_timeout_secs)),
    );
}
```

**Note:** The existing `SubagentStarted` handler (main.rs:3307) needs
**no change** — it pushes to `in_flight_subagents` and
`shell.subagent_rows`, which is correct for both direct and queued
spawns. The handle registration into `app.in_flight` happens in the
helper (not the handler), matching the agent-loop pattern.

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
    // Kill all in-flight subagents (concurrency: they may be running).
    // fire_subagent_kill is pub in agent.rs, signature:
    //   (reg: &Arc<Mutex<HashMap<String, SubagentHandle>>>, target: Option<&str>) -> usize
    // target=None kills all. Already called from main.rs:4015 with the
    // same signature, so the import path is established.
    zoid::agent::fire_subagent_kill(&app.in_flight, None);
    app.in_flight_subagents.clear();
    app.in_flight.lock().unwrap().clear();
    app.queued_subagents.clear();
    app.yielded = true;
    app.shell.status_hint =
        Some("session taken over by another instance".into());
}
```

- [ ] **Step 2: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "fix(agent): kill subagents on session takeover"
```

---

## Self-Review

**Gilfoyle review (plan) issues addressed:**
- C1 (Provenance breaks 3 sites): Step 2 derives `Default` on `Provenance` or enumerates all 3 sites
- C2 (spawn_turn never sets max_concurrent): Task 4 Step 2 adds `turn_config.max_concurrent = app.config.subagent.max_concurrent`
- C3 (SubagentQueued variant contradicts Step 5): Task 4 Step 4 carries full `resolved_profile: AgentProfile` (Option A, definitive)
- H1 (take_deferred_delegation_wake unchanged): Task 2 Step 4 explicitly notes no change needed
- H2 (TurnComplete drain guard): Task 2 Step 4 preserves `if !app.streaming` around `drain_due_wakes`
- H3 (submit_while_delegating test breaks): Task 3 Step 5 updates the test
- H4 (spawn_queued_subagent must register handle): Task 4 Step 6 enumerates handle creation + `in_flight.insert` explicitly
- H5 (fire_subagent_kill signature): Task 5 Step 1 uses `zoid::agent::fire_subagent_kill(&app.in_flight, None)` (2-arg, correct)
- M1 (app param sources): Task 4 Step 6 lists each `app.*` source for `spawn_subagent` params
- M2 (want_worktree cliff): Task 4 carries `want_worktree` through `SubagentQueued` and `QueuedSubagent`
- M3 (SubagentStarted handler): Task 4 Step 6 confirms no change needed
- M5 (max_concurrent=0 drain): Task 4 Step 6 treats 0 as unlimited in the drain guard

**SQLite (M4):** Per-subagent connections deferred. The shared session
actor serializes appends — correct for correctness, but 3 concurrent
subagents each calling `session.append().await` serialize on the actor's
single thread, adding latency proportional to concurrent event rate.
Per-connection optimization is the follow-up if write contention becomes
measurable. This is a deliberate phasing decision, not a bug.

**Risk areas:**
- Task 4 is the largest task — the `dispatch_subagent` refactor moves
  profile resolution above the pool check and adds a new `AgentUpdate`
  variant. Test thoroughly.
- The `SubagentQueued` event carries a full `AgentProfile` — it's
  `Clone + Send` (confirmed), large but acceptable for a rare event.
- The `spawn_queued_subagent` helper in Task 4 Step 6 duplicates the
  handle-creation + spawn logic from `agent.rs:1653-1693`. DRY by
  extracting a shared function if the duplication is significant.
- SQLite per-subagent connections are NOT in this plan — the subagent
  still uses the shared `SessionHandle` actor for appends. The actor's
  mpsc channel serializes writes. This is safe for correctness; the
  per-connection optimization is a follow-up if write contention
  becomes measurable.