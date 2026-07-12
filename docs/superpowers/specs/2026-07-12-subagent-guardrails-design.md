# Subagent Dispatch Guardrails — Design

**Date:** 2026-07-12
**Status:** Design (approved shape; pending spec review)
**Umbrella:** Spec 1 of 3 under *subagent dispatch safety*
(`2026-07-12-subagent-dispatch-guardrails-design.md` is the parent index).
Siblings — **Spec 2** worktree tooling fixes (WT-1/WT-2), **Spec 3** scheduled
wake-ups — are separate specs. This one is self-contained.

## Goal

Bound a dispatched subagent's runtime and make it stoppable. Today a subagent
can hang indefinitely on a single `await` — the 25-iteration cap
(`SUBAGENT_MAX_ITERATIONS`, `subagent.rs:29`) bounds *loops*, not a *stalled
await that never advances an iteration* — and once spawned it cannot be stopped
by any means: no timer, no tool, no keystroke reaches it. Add three abort
triggers on one shared cancellation primitive:

1. **Wall-clock timeout** — idle (no-progress) *and* an absolute ceiling.
2. **Orchestrator kill tool** — the main agent can cancel a subagent it dispatched.
3. **User kill via Esc** — the existing hard-stop keystroke also kills subagents.

The no-output/read-write heuristic and the budget-hint prompt injection from the
parent notes are **deferred** (see Non-goals).

## Core principle — reuse the cancellation machinery that already exists

`run_agent_turn_cancellable` (`agent.rs:488`) already does the hard part: on
`hard.cancelled()` it drains every un-executed tool call with a balanced
`[skipped: turn aborted]` result, force-kills a running local shell via
`config.kill.kill()` (`agent.rs:2009-2019`), and still fires `TurnComplete` on
every exit path. Subagents route through `run_agent_turn` (`subagent.rs:191`), a
thin convenience wrapper that hands that machinery **two `CancellationToken`s
that never fire** (`agent.rs:476-477`).

So the entire feature is: **give each subagent real tokens, hold them in a
registry, and fire them.** No new cancellation, drain, or force-kill logic — only
plumbing the tokens outward and a supervisor to trip the timeout one.

## Confirmed parameters

| Parameter | Value |
|-----------|-------|
| Idle (no-progress) timeout | **120 s** default, `0` = off, configurable |
| Absolute ceiling | **900 s** (15 min) default, `0` = off, configurable |
| Iteration cap | **25**, unchanged (complements the time caps) |
| Kill tool | `cancel_subagent { id: Option<String> }` — id kills one, omitted kills all |
| Kill tool visibility | **main Chat agent only** — subagents cannot kill subagents |
| On abort | failure `DelegationResult` (`ok: false`) + **worktree discarded** |
| Esc | unified two-press escalation; the force press also kills all in-flight subagents |

## Architecture & data flow

```
dispatch                         supervision                     abort → report
────────                         ───────────                     ──────────────
spawn_subagent:                  WakeTimer task (per subagent):   any firer sets
  make {cancel, hard,              loop { select! {                 abort_reason,
        progress, abort_reason}     _ = done.cancelled() => stop     then hard.cancel()
  register in App.subagents         _ = tick => if idle>N || run>C          │
  run_subagent(.., cancel, hard,        { set reason; hard.cancel() } }}     ▼
              progress) ───────►  run_agent_turn_cancellable drains + kills shell
  on completion: done.cancel()          │ returns
                                        ▼
                                 spawn_subagent: hard.is_cancelled()?
                                   → failure DelegationResult(ok:false, reason)
                                     + drop worktree (discard partial work)
                                   → session.append + ui.send(Appended)
                                        │
                                        ▼
                                 main.rs consumer clears App.subagents[id]
                                   + drawer row (existing retain, main.rs:2663)
```

The main-loop `select!` (`main.rs:2585`), the kill tool, and `escalate_cancel`
(`main.rs:4369`) all fire tokens **through the same registry** — one home for
"what is running and how to stop it."

## Components

### 1. `WakeTimer` — reusable supervisor primitive (new)

`crates/zoid/src/wake_timer.rs`. Subagent-agnostic: it watches a heartbeat and a
start time, and on breach records a caller-supplied reason value and fires a
token. It is **generic over the reason type** (`R`) so it carries values without
knowing what they mean — which lets the subagent registry share one reason slot
across all three firers (timeout, kill tool, Esc).

```rust
impl WakeTimer {
    /// Spawn a supervisor. Fires `fire` when `now - progress > idle` (no-progress)
    /// or `now - start > ceiling` (absolute); writes the matching reason value
    /// into `reason` (first-writer-wins) just before firing. Stops when `done`
    /// is cancelled. `None` disables that arm; both `None` → no task spawned.
    pub fn spawn<R: Send + 'static>(
        idle: Option<Duration>,
        ceiling: Option<Duration>,
        progress: Arc<AtomicI64>,           // last-progress epoch ms; bumped per iteration
        now: fn() -> i64,
        idle_reason: R,                     // written on the idle breach
        ceiling_reason: R,                  // written on the ceiling breach
        reason: Arc<Mutex<Option<R>>>,      // shared with the caller's registry
        fire: CancellationToken,
        done: CancellationToken,
    ) -> tokio::task::JoinHandle<()>;
}
```

It knows nothing of subagents, so it is unit-testable with an injected `now`, a
hand-driven `progress`, and any toy reason type — no real sleeps, no subagent
scaffolding.

### 2. Subagent registry (App state)

Upgrade the existing in-flight set — `app.in_flight: Arc<Mutex<HashSet<String>>>`
(`main.rs:1576-1582`) — to `Arc<Mutex<HashMap<String, SubagentHandle>>>`:

```rust
pub struct SubagentHandle {
    pub cancel: CancellationToken,                 // graceful (reserved; parity with main turn)
    pub hard: CancellationToken,                   // force-kill this subagent
    pub progress: Arc<AtomicI64>,                  // heartbeat the WakeTimer reads
    pub abort_reason: Arc<Mutex<Option<AbortReason>>>, // set by whichever firer trips first
}

pub enum AbortReason { IdleTimeout, Ceiling, Killed }
```

`abort_reason` is the single shared slot: `spawn_subagent` hands it to the
`WakeTimer` as its `reason` (with `idle_reason = IdleTimeout`,
`ceiling_reason = Ceiling`); the kill tool and Esc write `Killed` into the same
slot before firing `hard`. First writer wins; `spawn_subagent` reads it after the
run returns to label the failure `DelegationResult`. The drawer's
`app.in_flight_subagents: Vec<SubagentInfo>` stays as-is (display only).

### 3. Real tokens + heartbeat into `run_subagent`

`run_subagent` (`subagent.rs:111`) gains `cancel`, `hard`, and `progress`
parameters and calls `run_agent_turn_cancellable` **directly** with the real
`cancel`/`hard` (instead of `run_agent_turn`'s throwaways). The shared loop
(`run_agent_turn_cancellable`, `agent.rs:488`) gains **one additive** parameter
`progress: Option<&AtomicI64>`, bumped to `now()` at the top of each iteration;
the main turn passes `None`. A hung `await` mid-iteration never bumps → idle
fires. `run_agent_turn` (`agent.rs:448`) keeps its signature and passes `None` +
throwaway tokens, so existing callers/tests are unaffected.

### 4. Timeout supervision

In `spawn_subagent` (`spawn_subagent.rs:54`): build the `SubagentHandle`, insert
it into the registry, create a `done` token, and — only if a timeout is
configured — `WakeTimer::spawn(idle, ceiling, progress, now, AbortReason::IdleTimeout,
AbortReason::Ceiling, abort_reason.clone(), hard.clone(), done.clone())`. Run the
subagent; on completion `done.cancel()` so the supervisor exits. The `JoinHandle` no longer needs to be discarded blindly — the registry +
`done` own the lifecycle.

### 5. Orchestrator kill tool (new)

`crates/zoid-tools/src/subagent_kill.rs` — `cancel_subagent { id: Option<String> }`.
`Some(id)` sets that handle's `abort_reason = Killed` and fires its `hard`;
`None` does so for every in-flight subagent. Registered in `chat_tools`
(`invoke_skill.rs:99-104`, beside `dispatch_subagent`/`subagent_diff`) — **not**
in the base `registry()` (`lib.rs:114`) that subagents receive, so a subagent
cannot cancel its siblings. Because it needs registry access, it follows the
same `Emitting`/agent-loop-executed pattern as the worktree tools (the tool stub
signals; the main loop performs the cancel against `app.subagents`).

### 6. Esc — unified escalation

Extend `escalate_cancel` (`main.rs:4369-4378`): the force branch, after firing
the main turn's `hard`, iterates the registry and fires every subagent's `hard`
(reason `Killed`). Add the no-active-turn case: when subagents are in-flight but
`turn_cancel` is `None`, the first Esc arms `"kill N subagents? Esc again to
confirm"` (a `subagent_kill_armed` flag mirroring the graceful→hard pattern) and
the second fires them all. One consistent two-press model over whatever is live.

### 7. `[subagent]` config

`crates/zoid-core/src/config.rs`, following the `EconomyConfig` layered pattern
(`config.rs:78-107`, `PartialEconomy` at `:264`, merge at `:409`):

```toml
[subagent]
idle_timeout_secs = 120   # 0 = disabled
hard_timeout_secs = 900   # 0 = disabled
```

`SubagentConfig { idle_timeout_secs: u64, hard_timeout_secs: u64 }` +
`PartialSubagent` (`derive(Debug, Default, Clone, Deserialize)`, `serde(default)`)
+ `Provenance` fields, added to `Config` and its `Default` impl, wired through the
6-site merge. The kill tool and Esc are always available (no config).

## Error handling

- **Abort always emits a `DelegationResult`.** The registry entry is cleared only
  when the main loop consumes that event (`main.rs:2663-2666`), so an aborted
  subagent can never leak a drawer row. This directly addresses the parent doc's
  "delivery gap" (failure #2): rather than a separate hunt (the drop was not
  independently reproducible — both sends are `let _ =` but the event is not lost
  unless the channel closes), the guardrail path *guarantees* a terminal event.
- **Idempotent.** `CancellationToken` fires are idempotent; a timeout that races a
  user kill, or a double Esc, is safe. `abort_reason` is first-writer-wins.
- **No orphan tasks.** The `WakeTimer` self-terminates via `done` on normal
  completion; nothing is left spinning.
- **Timeouts disabled** (`idle=0 && ceiling=0`) → no `WakeTimer` is spawned, but
  the handle is still registered so the kill tool and Esc keep working.
- **Worktree on abort:** the `WorktreeGuard` is dropped (full cleanup — dir
  pruned, branch deleted), discarding partial work, exactly as the current error
  path does (`spawn_subagent.rs:98-100`).

## Testing

- **`WakeTimer`** (injected `now`, hand-driven `progress`): fires on idle breach;
  fires on ceiling breach; does **not** fire before either; `done` stops it
  cleanly. No real sleeps.
- **Abort path:** firing `hard` yields a failure `DelegationResult`
  (`ok: false`, summary carries the reason), the worktree is dropped, and the
  registry entry is cleared.
- **Kill tool:** `Some(id)` fires exactly that handle; `None` fires all.
- **Esc:** the force branch fires every registered subagent `hard`; the
  no-active-turn armed-confirm path fires on the second press only.
- **Integration:** dispatch a subagent running a sleeping test tool; assert the
  idle timeout kills it and a `DelegationResult` arrives.
- **No persistence tests** — guardrails are transient runtime state; no
  `EventKind`, DB, or schema change (the emitted `DelegationResult` already
  exists).

## Non-goals (YAGNI)

- No-output / read-write-ratio heuristic (deferred — false-positive risk on
  legitimate read-only investigations).
- Budget-hint prompt injection (deferred).
- Per-dispatch timeout override on `dispatch_subagent` (global config only in v1).
- Scheduled wake-ups / agent self-scheduling (**Spec 3** — a distinct subsystem;
  its scheduler is *not* `WakeTimer`).
- Worktree WT-1/WT-2 fixes (**Spec 2**).
- Output-token-cap truncation (external `zoid-releases` bug report).
- Persisting kill history.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid/src/wake_timer.rs` | **new** — `WakeTimer` + `WakeReason` |
| `crates/zoid/src/subagent.rs` | `run_subagent` gains `cancel`/`hard`/`progress`; calls `run_agent_turn_cancellable` directly |
| `crates/zoid/src/agent.rs` | additive `progress: Option<&AtomicI64>` on `run_agent_turn_cancellable`, bumped per iteration; `run_agent_turn` passes `None` + throwaway tokens |
| `crates/zoid/src/spawn_subagent.rs` | accept tokens; build + register `SubagentHandle`; spawn `WakeTimer`; on `hard.is_cancelled()` emit failure `DelegationResult` + drop worktree |
| `crates/zoid/src/main.rs` | `in_flight` → `subagents` registry map; `DelegationResult` consumer clears it; `escalate_cancel` fires subagent tokens + no-turn armed path; thread `[subagent]` timeouts into dispatch |
| `crates/zoid-tools/src/subagent_kill.rs` | **new** — `cancel_subagent` tool (Emitting; main-loop executed) |
| `crates/zoid-tools/src/lib.rs` | export the new tool (kept out of `registry()`) |
| `crates/zoid/src/invoke_skill.rs` | register `cancel_subagent` in `chat_tools` |
| `crates/zoid-core/src/config.rs` | `[subagent]` `SubagentConfig` + `PartialSubagent` + `Provenance` + 6-site merge |
