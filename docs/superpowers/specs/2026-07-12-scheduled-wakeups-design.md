# Scheduled Wake-ups — Design

**Date:** 2026-07-12
**Status:** Design (approved shape; pending spec review)
**Umbrella:** Spec 3 of 3 under *subagent dispatch safety*
(`2026-07-12-subagent-dispatch-guardrails-design.md` is the parent index).
Independent of Spec 1 (guardrails) and Spec 2 (worktree fixes). This is the
largest of the three — a persistent subsystem, not a self-contained feature.

## Goal

Let the agent schedule its own future resumption: "I'm waiting on X — wake me in
N minutes to check it and continue." On fire, the agent resumes **the same
conversation** with full context plus a note it left itself. This is the
model-callable capability the umbrella brainstorm surfaced.

zoid today has **no re-invocation infrastructure**: a turn starts only from a
keypress (`Action::Submit` → `spawn_turn`, `main.rs:3539`/`5370`); the main
`select!` loop (`main.rs:2585`) watches only terminal events + an
outbound-notify `AgentUpdate` channel; nothing wakes it on a clock. So this spec
builds the missing pieces: a persisted schedule, a background watcher, and a
synthetic-turn injection path.

## Core principle — persistence via the event log; injection via the existing channel

Two existing zoid facts make this tractable:

- **Event-sourced durability.** Sessions rebuild from the SQLite event log on
  load (`app.events = EventLog::from_vec(loaded)`, `main.rs:3702`). Persisting
  wakes as `EventKind`s gives restart-survival and catch-up for free — no
  side-table.
- **The `ui` channel already wakes an idle loop.** Subagents push
  `AgentUpdate`s into `ui_rx`, waking the otherwise-blocked `select!`. The wake
  watcher does the same: it holds a clone of `ui_tx` and sends
  `AgentUpdate::WakeDue` when its timer elapses — so **no new `select!` branch is
  needed**, only a new task and a new `AgentUpdate` variant.

This is the one spec in the umbrella that changes the event schema — unavoidable,
because a wake must survive restart.

## Confirmed parameters

| Parameter | Value |
|-----------|-------|
| Schedule form | one-shot, **relative** input (`delay_secs`); stored as absolute `fire_at_ms` |
| Fire behavior | inject the note as a `UserMessage` into the **same session**, run a turn with full context |
| Missed wake (closed at due time) | **catch-up on reopen** — fires when the session next loads, stamped as late |
| Fire during an active turn | **queue** via the existing `pending_message` path; fires on `TurnComplete` |
| Min delay floor | **30 s** (reject shorter) |
| Max pending wakes | **16** (reject when at cap) |
| Master switch | `[wake] enabled` (default `true`) |
| Tool visibility | **main Chat agent only** (subagents have no persistent session to resume) |

## Architecture & data flow

```
schedule_wake tool (Emitting)          watcher task                 fire
─────────────────────────              ────────────                 ────
main loop appends                      loop {                       ui_rx recv WakeDue:
  EventKind::WakeScheduled               next = *next_due.borrow();    scan pending ≤ now
  { wake_id, fire_at_ms, note }          select! {                     for each:
insert into pending_wakes (BTreeMap)       _ = next_due.changed()        turn running?
set next_due = earliest fire_at              => continue,                  yes → queue (pending_message)
        │                                    _ = sleep_until(next)         no  → inject UserMessage
        ▼                                      => ui.send(WakeDue) }             (⏰ scheduled [+late])
  watcher re-arms                        }                                     append WakeFired
                                                                              drop from pending
on session load: rebuild pending_wakes from                                   spawn_turn
  (WakeScheduled − WakeFired − WakeCancelled);                          recompute next_due
  any fire_at ≤ now → fire immediately (catch-up)
```

## Components

### 1. Event kinds (persisted, new)

In `crates/zoid-core/src/event.rs` (`EventKind`, ~`:70-176`):

```rust
WakeScheduled { wake_id: String, fire_at_ms: i64, note: String },
WakeFired     { wake_id: String },
WakeCancelled { wake_id: String },
```

`wake_id` is a ULID. The **pending set** is a projection: every `WakeScheduled`
whose `wake_id` has no matching `WakeFired` or `WakeCancelled`. Rebuilt on load
from the event log; no separate persistence. These are bookkeeping events —
conversation rendering ignores them (the injected `UserMessage` is what shows).

### 2. Pending set + watcher

- **State (main loop):** `pending_wakes: BTreeMap<i64 /*fire_at_ms*/, WakeEntry { id, note }>`,
  rebuilt on load, mutated on schedule/cancel/fire. A
  `tokio::sync::watch::Sender<Option<i64>>` `next_due` holds the earliest
  `fire_at_ms` (or `None` when empty).
- **Watcher task** (new, spawned at startup beside the git poller,
  `main.rs:2216`): loops — read `next_due`; `select!` on `next_due.changed()`
  (re-arm) vs `tokio::time::sleep` until that instant; on elapse
  `ui.send(AgentUpdate::WakeDue).await`. Holds a `ui_tx` clone. Cheap and idle
  when nothing is scheduled (`None` → sleep on `changed()` only).

### 3. Firing / injection (main loop)

New `AgentUpdate::WakeDue` handled in the existing `ui_rx` arm; the same routine
runs once on session load for catch-up. For each pending wake with `fire_at_ms ≤ now`:

- **A turn is running** → enqueue the note through the existing `pending_message`
  mechanism (resumed after `TurnComplete`, `main.rs:2741`/`2846`/`2885`); leave
  it pending until actually injected.
- **Idle** → build the injected text — `⏰ scheduled: {note}` (plus
  ` (scheduled for {t}, fired late)` when overdue) — record it as an
  `EventKind::UserMessage`, append `EventKind::WakeFired { wake_id }`, remove
  from `pending_wakes`, and `spawn_turn` (mirroring the `pending_adjust`
  synthetic-turn precedent, `main.rs:2245-2248`/`4468-4474`). Recompute `next_due`.

`WakeFired` is appended only when the wake actually injects, so a crash between
`WakeDue` and injection leaves the wake pending → it re-fires on next load (at-
least-once, never lost).

### 4. Tools (main Chat agent only)

`crates/zoid-tools/src/wake_schedule.rs` + `wake_cancel.rs`, `Emitting`
(main-loop-executed, like the worktree tools), registered in `chat_tools`
(`invoke_skill.rs`), **excluded** from the base `registry()` subagents receive.

- `schedule_wake { delay_secs: u64, note: String }` → validates
  `enabled && delay_secs ≥ floor && pending < cap`; resolves
  `fire_at_ms = now + delay_secs*1000`; appends `WakeScheduled`; arms the
  watcher. Returns the `wake_id` and resolved fire time in the `ToolResult` (so
  the agent can cancel it later). Rejections return a plain error `ToolResult`.
- `cancel_wake { id: Option<String> }` → `Some(id)` appends `WakeCancelled` for
  that id; `None` cancels all pending; re-arms the watcher. Cancelling an
  unknown/already-fired id is a no-op success.

### 5. Runaway guard

`schedule_wake` enforces a **min-delay floor (30 s)** and **max-pending cap
(16)** so the agent cannot spin a tight self-wake loop (the exact runaway class
this umbrella exists to prevent). Both are constants in v1. The `[wake] enabled`
switch (below) is the hard off.

### 6. Config

`crates/zoid-core/src/config.rs`, `EconomyConfig` layered pattern
(`Partial*` + `Provenance` + 6-site merge):

```toml
[wake]
enabled = true   # false → schedule_wake refuses; the watcher never fires
```

`WakeConfig { enabled: bool }`. The floor/cap stay constants in v1 (promotable to
config later if needed).

## Error handling

- **At-least-once, never-lost:** `WakeFired` is written only at injection, so any
  crash before injection re-fires the wake on reload (catch-up). Injection and
  `WakeFired` are two separate appends (not atomic): a crash in the window between
  them re-fires and re-injects the note on reload. This is at-least-once by design
  — the alternative order (WakeFired first) would silently lose wakes. Exactly-once
  is not guaranteed; a duplicate injection is possible only across that crash window.
- **Disabled mid-flight:** if `[wake] enabled` is flipped to `false`,
  `schedule_wake` refuses new wakes; already-scheduled wakes still fire (they're
  persisted) — turning it off does not silently strand a promised wake. (A
  future `cancel_wake all` is the way to clear them.)
- **Session scoping:** a wake fires only when **its own** session is loaded (it
  resumes that conversation). Opening a different session does not fire it.
- **Cap/floor rejections** are ordinary error `ToolResult`s the agent sees and
  can adapt to (e.g. pick a longer delay).
- **Watcher liveness:** `next_due = None` when empty means the watcher parks on
  `changed()` — no busy-loop, no spurious wakeups.

## Testing

- **Projection:** rebuilding `pending_wakes` from a mixed
  `WakeScheduled`/`WakeFired`/`WakeCancelled` event stream yields exactly the
  un-fired, un-cancelled set (pure unit test — no timers).
- **Schedule tool:** validates floor (reject `< 30 s`), cap (reject at 16),
  `enabled=false` (reject); on success emits `WakeScheduled` with
  `fire_at = now + delay` and returns the id.
- **Cancel tool:** `Some(id)` and `None`(all) both emit the right
  `WakeCancelled`; unknown id is a no-op success.
- **Firing (injected clock):** a due wake with no active turn injects a
  `⏰ scheduled` `UserMessage`, appends `WakeFired`, and spawns a turn; with an
  active turn, it queues and fires after `TurnComplete`.
- **Catch-up:** loading a session whose wake `fire_at` is in the past fires it
  immediately with the late stamp.
- **Crash-safety:** a `WakeScheduled` with no `WakeFired`/`WakeCancelled`
  re-fires on the next load (at-least-once).

## Non-goals (YAGNI)

- Recurring / cron-like wakes (one-shot only in v1).
- Absolute wall-clock input (`delay_secs` only; internal storage is absolute).
- A rail drawer or `list_wakes` tool — control is `schedule_wake` +
  `cancel_wake` only.
- Cross-session or global wakes (each wake is bound to its session).
- Subagent-scheduled wakes (subagents have no persistent session).
- Reusing Spec 1's `WakeTimer` — that supervises a *running* task's idle/ceiling;
  this watcher schedules a *future* event. Different primitives.

## File-change map

| File | Change |
|------|--------|
| `crates/zoid-core/src/event.rs` | add `WakeScheduled` / `WakeFired` / `WakeCancelled` `EventKind`s |
| `crates/zoid-core/src/config.rs` | `[wake]` `WakeConfig { enabled }` + `PartialWake` + `Provenance` + 6-site merge |
| `crates/zoid/src/agent.rs` | new `AgentUpdate::WakeDue` variant |
| `crates/zoid/src/main.rs` | `pending_wakes` BTreeMap + `next_due` watch; rebuild on load; watcher task (holds `ui_tx`); `WakeDue` handler → queue-or-inject + `WakeFired` + `spawn_turn`; catch-up at load |
| `crates/zoid-tools/src/wake_schedule.rs` | **new** — `schedule_wake` tool (Emitting; floor/cap/enabled checks) |
| `crates/zoid-tools/src/wake_cancel.rs` | **new** — `cancel_wake` tool (Emitting) |
| `crates/zoid-tools/src/lib.rs` | export the new tools (kept out of `registry()`) |
| `crates/zoid/src/invoke_skill.rs` | register both in `chat_tools` |
