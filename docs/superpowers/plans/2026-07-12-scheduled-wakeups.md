# Scheduled Wake-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the main Chat agent schedule its own future resumption — "wake me in N seconds to check X" — surviving restarts, and on fire resume the *same* session with full context plus the note it left itself.

**Architecture:** Persist wakes as three new `EventKind`s (event-sourced durability — the pending set is a projection, no side table). A background watcher task holds a `watch` cell of the earliest `fire_at_ms` and sends `AgentUpdate::WakeDue` when it elapses (waking the idle main `select!` exactly like the git poller does). The main loop drains *due* wakes: idle → inject a synthetic `UserMessage` + record `WakeFired` + `spawn_turn`; busy → leave pending, drained at `TurnComplete`. Two Emitting tools (`schedule_wake`/`cancel_wake`), main-Chat-only, mutate the schedule via the main loop over a `oneshot` reply (the `WorktreeRequested` idiom).

**Tech Stack:** Rust, tokio (`watch`, `mpsc`, `tokio::time`), serde, ULID, SQLite event log.

## Global Constraints

- Schedule form: one-shot, **relative** `delay_secs: u64` input; stored as absolute `fire_at_ms: i64`.
- Fire behavior: inject the note as an `EventKind::UserMessage` into the **same session**; run a full-context turn.
- Missed wake (session closed at due time): **catch-up on reopen** — fires when the session next loads, stamped late.
- Fire during an active turn: the wake stays in `pending_wakes` and is drained at **`TurnComplete`** (NOT via `pending_message` — that slot holds user keystrokes and must not be clobbered).
- Min delay floor: **30 s** (reject `delay_secs < 30`).
- Max pending wakes: **16** (reject a new schedule when 16 are already pending).
- Master switch: `[wake] enabled` (default **`true`**); `false` → `schedule_wake` refuses and the watcher never fires.
- Tool visibility: **main Chat agent only** — `schedule_wake`/`cancel_wake` are pushed in `chat_tools`, kept OUT of the base `zoid_tools::registry()` that subagents receive.
- `wake_id` is a ULID string. `WakeFired` is appended **only at injection** (at-least-once, never lost: a crash before injection re-fires on reload).
- Injected text: `⏰ scheduled: {note}`; append ` (fired late)` when `now_ms - fire_at_ms > 5000`.
- Floor (30 s), cap (16), and late-threshold (5000 ms) are `const`s in v1; only `enabled` is config.
- New `EventKind`s are bookkeeping — inert to rendering/FTS/eviction (only the injected `UserMessage` shows in the conversation).
- No "Co-Authored-By" / co-author trailer in any commit (hard rule).

---

## File Structure

- `crates/zoid-core/src/event.rs` — 3 new `EventKind` variants (`WakeScheduled`/`WakeFired`/`WakeCancelled`).
- `crates/zoid-core/src/projection.rs` — inert arms in the one exhaustive `match &e.kind` (`:174`).
- `crates/zoid-core/src/config.rs` — `[wake]` `WakeConfig { enabled }` via the `EconomyConfig` layered pattern (6 sites).
- `crates/zoid-tui/src/config_view.rs`, `crates/zoid-tui/tests/shell_snapshot.rs`, `crates/zoid/src/main.rs` (test) — repair the exhaustive `Provenance { … }` literals (E0063).
- `crates/zoid/src/agent.rs` — 3 new `AgentUpdate` variants + 2 Emitting dispatch arms.
- `crates/zoid/src/main.rs` — `pending_wakes` state + `next_wake_tx` watch + watcher task + rebuild-on-load + `WakeDue`/`ScheduleWake`/`CancelWake` handlers + `TurnComplete` drain + pure helpers (projection, due-selection, injection text, validation).
- `crates/zoid-tools/src/wake.rs` — **new** — `ScheduleWake` + `CancelWake` Emitting tools.
- `crates/zoid-tools/src/lib.rs`, `crates/zoid/src/invoke_skill.rs` — export + register the two tools in `chat_tools` only.

---

## Task 1: Event kinds + pending-wake projection

**Files:**
- Modify: `crates/zoid-core/src/event.rs:179` (add variants before the enum's closing `}` at `:180`)
- Modify: `crates/zoid-core/src/projection.rs:174` (add inert arms to the exhaustive match; inert cluster ~`:270-286`)
- Modify: `crates/zoid/src/main.rs` (add pure `rebuild_pending_wakes` + inline test in `mod tests`)

**Interfaces:**
- Produces: `EventKind::WakeScheduled { wake_id: String, fire_at_ms: i64, note: String }`, `EventKind::WakeFired { wake_id: String }`, `EventKind::WakeCancelled { wake_id: String }`.
- Produces: `fn rebuild_pending_wakes(events: &[zoid_core::event::Event]) -> std::collections::BTreeMap<(i64, String), String>` in `main.rs` — maps `(fire_at_ms, wake_id) → note` for every scheduled-but-not-fired-not-cancelled wake. Later tasks read `.keys().next().map(|(t, _)| *t)` for the earliest `fire_at`.

- [ ] **Step 1: Write the failing projection test.** Add to the inline `mod tests` in `crates/zoid/src/main.rs`:

```rust
    #[test]
    fn rebuild_pending_wakes_projects_unfired_uncancelled() {
        use zoid_core::event::{Event, EventKind};
        let mk = |kind| Event::new(Ulid::new(), None, 0, kind);
        let evs = vec![
            mk(EventKind::WakeScheduled { wake_id: "a".into(), fire_at_ms: 300, note: "later".into() }),
            mk(EventKind::WakeScheduled { wake_id: "b".into(), fire_at_ms: 100, note: "soon".into() }),
            mk(EventKind::WakeScheduled { wake_id: "c".into(), fire_at_ms: 200, note: "gone".into() }),
            mk(EventKind::WakeFired { wake_id: "b".into() }),      // b fired → excluded
            mk(EventKind::WakeCancelled { wake_id: "c".into() }),  // c cancelled → excluded
        ];
        let pending = rebuild_pending_wakes(&evs);
        // Only `a` survives; BTreeMap orders by (fire_at, id).
        assert_eq!(pending.len(), 1, "only the un-fired, un-cancelled wake survives");
        assert_eq!(pending.get(&(300, "a".to_string())).map(String::as_str), Some("later"));
        // Earliest fire_at of the pending set.
        assert_eq!(pending.keys().next().map(|(t, _)| *t), Some(300));
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --bin zoid rebuild_pending_wakes`
Expected: compile error — `EventKind::WakeScheduled` and `rebuild_pending_wakes` undefined.

- [ ] **Step 3: Add the three `EventKind` variants.** In `crates/zoid-core/src/event.rs`, immediately before the closing `}` of `pub enum EventKind` (after the `QuestionAnswered { … }` variant at `:176`):

```rust
    /// A one-shot wake scheduled by the agent (`schedule_wake` tool). Persisted
    /// so it survives restart; the pending set is the projection of every
    /// WakeScheduled with no matching WakeFired/WakeCancelled. Bookkeeping only —
    /// inert to conversation rendering; the injected UserMessage is what shows.
    WakeScheduled {
        wake_id: String,
        fire_at_ms: i64,
        note: String,
    },
    /// A scheduled wake actually fired (injected its note + spawned a turn).
    /// Written ONLY at injection, so a crash before injection re-fires on reload.
    WakeFired { wake_id: String },
    /// A scheduled wake was cancelled before firing (`cancel_wake` tool).
    WakeCancelled { wake_id: String },
```

- [ ] **Step 4: Add inert arms to the exhaustive projection match.** In `crates/zoid-core/src/projection.rs`, in the `match &e.kind {` at `:174`, extend the inert-metadata arm cluster (near `:270-286`, where `Usage`/`ContextMutation`/etc. are handled as no-ops) so the three new variants are grouped there. Find the existing inert arm (a group like `EventKind::Usage { .. } | EventKind::… => { /* metadata: no conversation effect */ }`) and add the three variants to that `|` list:

```rust
            EventKind::WakeScheduled { .. }
            | EventKind::WakeFired { .. }
            | EventKind::WakeCancelled { .. } => { /* bookkeeping: no conversation row */ }
```

(If the inert cluster is a single combined arm, append these three with `|`. The rule: they must produce NO `ChatMsg`. Confirm no `_ =>` exists — this match is exhaustive by design.)

- [ ] **Step 5: Add the pure projection helper.** In `crates/zoid/src/main.rs`, near the other pure helpers above `fn spawn_turn` (e.g. beside `fn should_wake_after_delegation`):

```rust
/// Project the pending-wake set from the event log: every `WakeScheduled`
/// whose `wake_id` has no later `WakeFired`/`WakeCancelled`. Keyed by
/// `(fire_at_ms, wake_id)` so the map is ordered by fire time (and same-ms
/// schedules don't collide); the value is the note. Pure — rebuilt on load and
/// unit-tested without timers. Takes an iterator (not `&[Event]`) because the
/// live `EventLog` stores `Vec<Arc<Event>>` and exposes only `iter()` — there is
/// no contiguous `&[Event]` slice to borrow (C2).
fn rebuild_pending_wakes<'a>(
    events: impl IntoIterator<Item = &'a zoid_core::event::Event>,
) -> std::collections::BTreeMap<(i64, String), String> {
    use zoid_core::event::EventKind;
    // Fold to the latest state per wake_id, then materialize the survivors.
    let mut by_id: std::collections::HashMap<String, (i64, String)> =
        std::collections::HashMap::new();
    for e in events {
        match &e.kind {
            EventKind::WakeScheduled { wake_id, fire_at_ms, note } => {
                by_id.insert(wake_id.clone(), (*fire_at_ms, note.clone()));
            }
            EventKind::WakeFired { wake_id } | EventKind::WakeCancelled { wake_id } => {
                by_id.remove(wake_id);
            }
            _ => {}
        }
    }
    by_id
        .into_iter()
        .map(|(id, (fire_at, note))| ((fire_at, id), note))
        .collect()
}
```

- [ ] **Step 6: Run the projection test + full workspace.**

Run: `cargo test -p zoid --bin zoid rebuild_pending_wakes`
Expected: PASS.

Run: `cargo test`
Expected: PASS across the workspace (the projection.rs arm makes zoid-core compile; no other match breaks).

- [ ] **Step 7: Commit.**

```bash
git add crates/zoid-core/src/event.rs crates/zoid-core/src/projection.rs crates/zoid/src/main.rs
git commit -m "feat(wake): WakeScheduled/WakeFired/WakeCancelled events + pending-wake projection"
```

---

## Task 2: `[wake]` config section

**Files:**
- Modify: `crates/zoid-core/src/config.rs` — 6 sites (mirror the `subagent` section, template at `:131`)
- Modify (E0063 repairs): `crates/zoid-tui/src/config_view.rs:282`, `:363`; `crates/zoid/src/main.rs:6417` (test literal); `crates/zoid-tui/tests/shell_snapshot.rs:923`, `:963`, `:1008`, `:1065`, `:1141`, `:1187`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `zoid_core::config::WakeConfig { pub enabled: bool }` (default `true`); `Config.wake: WakeConfig`; `Provenance.wake_enabled: Source`. Task 3 reads `app.economy`-adjacent config for the master switch; Task 5's `schedule_wake` validation reads `enabled`.

- [ ] **Step 1: Write the failing merge test.** Add to `crates/zoid-core/src/config.rs`'s test module (where `subagent` merge is tested):

```rust
    #[test]
    fn wake_enabled_defaults_true_and_merges() {
        // Default: enabled = true, provenance Default.
        let (cfg, prov) = merge(&[]);
        assert!(cfg.wake.enabled, "wake.enabled defaults to true");
        assert_eq!(prov.wake_enabled, Source::Default);

        // A layer that turns it off is applied and recorded. Note the tuple is
        // (Source, PartialConfig) — the order `merge` iterates (config.rs:423).
        let mut partial = PartialConfig::default();
        partial.wake.enabled = Some(false);
        let (cfg, prov) = merge(&[(Source::File, partial)]);
        assert!(!cfg.wake.enabled, "an explicit false is merged");
        assert_eq!(prov.wake_enabled, Source::File);
    }
```

> The merge entry point is `pub fn merge(layers: &[(Source, PartialConfig)]) -> (Config, Provenance)` (config.rs:402). Read the neighboring `subagent` merge test first and mirror its exact construction/return-binding shape.

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid-core wake_enabled_defaults_true_and_merges`
Expected: compile error — `cfg.wake`, `prov.wake_enabled`, `partial.wake` undefined.

- [ ] **Step 3: Add `WakeConfig` (struct + Default).** In `crates/zoid-core/src/config.rs`, next to `SubagentConfig` (`:131`):

```rust
/// `[wake]` — the master switch for agent-scheduled wake-ups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeConfig {
    /// `false` → `schedule_wake` refuses and the watcher never fires. Default true.
    pub enabled: bool,
}
impl Default for WakeConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}
```

- [ ] **Step 4: Wire the 5 remaining in-file sites.** In `crates/zoid-core/src/config.rs`:

1. `Config` struct — after `subagent: SubagentConfig,` (`:41`): `pub wake: WakeConfig,`
2. `Config::default()` — after `subagent: SubagentConfig::default(),` (`:162`): `wake: WakeConfig::default(),`
3. `Provenance` struct — after `subagent_hard_timeout_secs: Source,` (`:300`): `pub wake_enabled: Source,`
4. `PartialWake` + `PartialConfig` field — near `:348`: 

```rust
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PartialWake {
    pub enabled: Option<bool>,
}
```
   and in `PartialConfig` after `subagent: PartialSubagent,` (`:385`): `#[serde(default)] pub wake: PartialWake,`
5. `merge()` — the `Provenance { … }` initializer (near `:404-421`): add `wake_enabled: Source::Default,`; and the apply block after the subagent block (near `:464-471`):

```rust
        if let Some(v) = p.wake.enabled {
            cfg.wake.enabled = v;
            prov.wake_enabled = *src;
        }
```

- [ ] **Step 5: Repair the exhaustive `Provenance { … }` literals (E0063).** Each of these constructs `Provenance` with no `..` spread and must gain `wake_enabled: Source::Default,` (use the same `Source` the neighboring fields use in that literal — in tests/snapshots that is `Source::Default`):

- `crates/zoid-tui/src/config_view.rs:282`
- `crates/zoid-tui/src/config_view.rs:363`
- `crates/zoid/src/main.rs:6417`
- `crates/zoid-tui/tests/shell_snapshot.rs:923`, `:963`, `:1008`, `:1065`, `:1141`, `:1187`

> If `config_view.rs` renders a provenance row per field, adding the field is display-only; no new row is required for v1 (the `[wake]` switch has no config-view surface). Just make the literal compile.

- [ ] **Step 6: Run the merge test + FULL workspace.**

Run: `cargo test -p zoid-core wake_enabled_defaults_true_and_merges`
Expected: PASS.

Run: `cargo test`
Expected: PASS across ALL crates. A Provenance field addition compiles per-crate but breaks other crates' literals — run the whole workspace with `set -o pipefail` and confirm a real exit 0 (do not trust a piped `tail`).

- [ ] **Step 7: Commit.**

```bash
git add crates/zoid-core/src/config.rs crates/zoid-tui/src/config_view.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid/src/main.rs
git commit -m "feat(wake): [wake] enabled config (WakeConfig, layered merge + provenance)"
```

---

## Task 3: Pending-wake state, watcher task, rebuild-on-load

**Files:**
- Modify: `crates/zoid/src/agent.rs:373` (add `AgentUpdate::WakeDue`)
- Modify: `crates/zoid/src/main.rs` — `App` fields (`pending_wakes`, `next_wake_tx`) + both constructors (`:2123` real, `:6474` test); watcher task beside the git poller (`:2244`); rebuild-on-load at startup (`:1878-1879`) and resume (`:3786`); pure `next_wake_due(&pending)` + `recompute` helper

**Interfaces:**
- Consumes: `rebuild_pending_wakes` (Task 1); `WakeConfig.enabled` (Task 2).
- Produces: `App.pending_wakes: BTreeMap<(i64, String), String>`; `App.next_wake_tx: tokio::sync::watch::Sender<Option<i64>>` (earliest `fire_at_ms`, `None` when empty); `AgentUpdate::WakeDue`; `fn earliest_fire_at(pending: &BTreeMap<(i64, String), String>) -> Option<i64>`. Task 4 handles `WakeDue`; Task 5 mutates `pending_wakes` + re-arms `next_wake_tx`.

- [ ] **Step 1: Write the failing test** (earliest-fire-at + rebuild-populates-state). Add to `mod tests` in `crates/zoid/src/main.rs`:

```rust
    #[test]
    fn earliest_fire_at_is_the_min_key() {
        let mut pending = std::collections::BTreeMap::new();
        assert_eq!(earliest_fire_at(&pending), None, "empty → None (watcher parks)");
        pending.insert((500i64, "a".to_string()), "n".to_string());
        pending.insert((100i64, "b".to_string()), "n".to_string());
        assert_eq!(earliest_fire_at(&pending), Some(100), "earliest fire_at wins");
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --bin zoid earliest_fire_at_is_the_min_key`
Expected: compile error — `earliest_fire_at` undefined.

- [ ] **Step 3: Add `earliest_fire_at` + `AgentUpdate::WakeDue`.** In `crates/zoid/src/main.rs` near `rebuild_pending_wakes`:

```rust
/// The earliest `fire_at_ms` in the pending set (the watcher's next deadline),
/// or `None` when nothing is scheduled (the watcher parks on `changed()`).
fn earliest_fire_at(pending: &std::collections::BTreeMap<(i64, String), String>) -> Option<i64> {
    pending.keys().next().map(|(t, _)| *t)
}
```

In `crates/zoid/src/agent.rs`, before the closing `}` of `pub enum AgentUpdate` (`:373`):

```rust
    /// The wake watcher's timer elapsed; the main loop should drain any wakes
    /// whose `fire_at_ms <= now` (inject if idle, else defer to TurnComplete).
    WakeDue,
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p zoid --bin zoid earliest_fire_at_is_the_min_key`
Expected: PASS.

- [ ] **Step 5: Add the `App` fields + constructor inits.** In `crates/zoid/src/main.rs`, in `struct App` (near the `pending_message`/`wake_after_delegation` fields, `~:1648-1655`):

```rust
    /// Pending scheduled wakes, `(fire_at_ms, wake_id) → note`, ordered by fire
    /// time. Rebuilt from the event log on load; mutated by schedule/cancel/fire.
    pending_wakes: std::collections::BTreeMap<(i64, String), String>,
    /// The watcher's next deadline (earliest `fire_at_ms`, `None` = park). Sending
    /// a new value re-arms the watcher immediately (schedule/cancel/fire).
    next_wake_tx: tokio::sync::watch::Sender<Option<i64>>,
```

In BOTH constructors (real `~:2123` after `wake_after_delegation: false,`; test `~:6474` likewise):

```rust
        pending_wakes: std::collections::BTreeMap::new(),
        next_wake_tx: tokio::sync::watch::channel(None).0,
```

> The real constructor rebuilds the set right after the event log loads (Step 7); the test constructor can leave it empty.

- [ ] **Step 6: Spawn the watcher task beside the git poller.** In `crates/zoid/src/main.rs`, near the git-poller spawn (`:2244`), add (unconditionally — it is cheap and idle when nothing is scheduled):

```rust
    // Wake watcher: parks on the next deadline; sends WakeDue when it elapses.
    // Re-armed immediately whenever `next_wake_tx` changes (schedule/cancel/fire).
    {
        let ui = app.ui_tx.clone();
        let mut next_rx = app.next_wake_tx.subscribe();
        tokio::spawn(async move {
            loop {
                let next = *next_rx.borrow_and_update();
                let sleep = async {
                    match next {
                        Some(fire_at_ms) => {
                            let now = now_ms();
                            let delay = (fire_at_ms - now).max(0) as u64;
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                        // Nothing scheduled: park until the cell changes.
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = sleep => {
                        if ui.send(AgentUpdate::WakeDue).await.is_err() {
                            break; // main loop gone
                        }
                        // Re-borrow next loop; the handler will re-arm via next_wake_tx.
                    }
                    changed = next_rx.changed() => {
                        if changed.is_err() {
                            break; // sender dropped — app exiting
                        }
                    }
                }
            }
        });
    }
```

> `now_ms()` is the existing wall-clock helper used by `derive_session_name`. The watcher never busy-spins: with `None` it awaits `changed()` only; with `Some` it sleeps to the deadline then sends one `WakeDue`. After firing, the handler (Task 4) removes the wake and re-arms `next_wake_tx`, so a re-fire of the same instant can't loop.

- [ ] **Step 7: Rebuild the pending set on load + arm the watcher.** `EventLog` stores `Vec<Arc<Event>>` and exposes only `iter() -> impl Iterator<Item = &Event>` (no `as_slice()`, no slice `Deref`), so `rebuild_pending_wakes` is called via `.iter()` (C2). Arming `next_wake_tx` after the rebuild is REQUIRED at BOTH sites — the constructor seeds the channel with `None`, so a future-dated wake loaded from the log would otherwise never be armed and never fire (I1).

Startup: after the App is fully built AND the watcher is spawned (Step 6), before entering the main `select!` loop:

```rust
    app.pending_wakes = rebuild_pending_wakes(app.events.iter());
    // Arm the watcher for the earliest loaded wake (future ones; due ones are
    // handled by the Task-4 catch-up drain). Race-free: the watcher already
    // called `subscribe()` synchronously before it was spawned (Step 6).
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
```

Resume (`handle_action`, after the log swap at `:3786`):

```rust
        app.pending_wakes = rebuild_pending_wakes(app.events.iter());
        let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
```

> `app.events.iter()` yields `&Event`; `rebuild_pending_wakes` takes `impl IntoIterator<Item = &Event>`, so this compiles directly. The Task-1 unit test passing `&evs` (a `&Vec<Event>`) and the Task-4/5 tests passing `&log` (a `&Vec<Event>` from `session.snapshot().await`) also satisfy the bound unchanged.

- [ ] **Step 8: Run the crate + workspace.**

Run: `cargo test -p zoid`
Expected: PASS.

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 9: Commit.**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(wake): pending-wake state + watch-driven watcher task + rebuild-on-load"
```

---

## Task 4: Firing / injection (WakeDue handler + catch-up + TurnComplete drain)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `AgentUpdate::WakeDue` arm in the UI `select!` (near the last arm `:3060`); a `drain_due_wakes` routine; catch-up call at load (Task 3's `:1878`/`:3786` hooks); `TurnComplete` drain (`:2764`, in the idle block); pure `wake_injection_text` + `due_wake_ids` helpers

**Interfaces:**
- Consumes: `pending_wakes`, `next_wake_tx`, `earliest_fire_at`, `AgentUpdate::WakeDue`, `record`, `spawn_turn`.
- Produces: `fn wake_injection_text(note: &str, fire_at_ms: i64, now_ms: i64) -> String`; the injection behavior (record `UserMessage` + `WakeFired`, remove from `pending_wakes`, re-arm, `spawn_turn` when idle).

- [ ] **Step 1: Write the failing injection-text test.** Add to `mod tests` in `crates/zoid/src/main.rs`:

```rust
    #[test]
    fn wake_injection_text_stamps_late_only_when_overdue() {
        // On-time (within 5s): no late stamp.
        assert_eq!(
            wake_injection_text("check CI", 10_000, 10_200),
            "⏰ scheduled: check CI"
        );
        // Overdue by > 5s (catch-up on reopen): late stamp appended.
        assert_eq!(
            wake_injection_text("check CI", 10_000, 20_000),
            "⏰ scheduled: check CI (fired late)"
        );
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --bin zoid wake_injection_text_stamps_late_only_when_overdue`
Expected: compile error — `wake_injection_text` undefined.

- [ ] **Step 3: Add the pure injection-text helper.** In `crates/zoid/src/main.rs` near the other wake helpers:

```rust
/// The synthetic UserMessage text a fired wake injects. Appends a late stamp
/// only when the fire is more than 5 s overdue (i.e. a catch-up on reopen, not
/// a normal on-time timer elapse).
const WAKE_LATE_STAMP_MS: i64 = 5_000;
fn wake_injection_text(note: &str, fire_at_ms: i64, now_ms: i64) -> String {
    if now_ms - fire_at_ms > WAKE_LATE_STAMP_MS {
        format!("⏰ scheduled: {note} (fired late)")
    } else {
        format!("⏰ scheduled: {note}")
    }
}
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p zoid --bin zoid wake_injection_text_stamps_late_only_when_overdue`
Expected: PASS.

- [ ] **Step 5: Add the `drain_due_wakes` routine.** In `crates/zoid/src/main.rs`, near `spawn_turn`. It injects at most one turn's worth: it records ALL due wakes' `UserMessage` + `WakeFired` (so nothing is lost), removes them from `pending_wakes`, re-arms the watcher, and — only if idle and not yielded — spawns one turn to process them. Returns whether a turn was spawned:

```rust
/// Drain every wake whose `fire_at_ms <= now`. When the orchestrator is idle and
/// not yielded: record each as a synthetic UserMessage + a WakeFired marker
/// (at-least-once — WakeFired is written ONLY here), drop them from the pending
/// set, re-arm the watcher, and spawn ONE continuation turn to process them.
/// When BUSY: touch nothing except parking the watcher (send None) so it stops
/// re-firing on the now-past deadline; the wakes stay pending and the
/// `TurnComplete` drain fires them once the turn ends (in correct log order).
/// Returns whether a turn was spawned.
async fn drain_due_wakes(app: &mut App) -> anyhow::Result<bool> {
    let now = now_ms();
    // Due keys (fire_at <= now), smallest-first. Exclusive upper bound at
    // `now + 1` so a wake due at exactly `now` is included and `now+…` excluded.
    let due: Vec<(i64, String)> = app
        .pending_wakes
        .range(..(now + 1, String::new()))
        .map(|((t, id), _)| (*t, id.clone()))
        .collect();
    if due.is_empty() {
        return Ok(false);
    }
    let idle = !app.streaming && app.in_flight_subagents.is_empty() && !app.yielded;
    if !idle {
        // Busy: leave the wakes pending and park the watcher so it does not spin
        // on the past deadline. TurnComplete's drain fires them when idle.
        let _ = app.next_wake_tx.send(None);
        return Ok(false);
    }
    // Idle: fire them. Only now do we mutate the pending set + record events.
    for (fire_at, id) in &due {
        let note = app.pending_wakes.remove(&(*fire_at, id.clone())).unwrap_or_default();
        let text = wake_injection_text(&note, *fire_at, now);
        app.record(EventKind::UserMessage { text }).await?;
        app.record(EventKind::WakeFired { wake_id: id.clone() }).await?;
    }
    // Re-arm the watcher to the next remaining deadline (or park).
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    app.streaming = true;
    spawn_turn(app);
    Ok(true)
}
```

> **Busy branch is side-effect-free (C1):** it does NOT record or remove — otherwise a wake arriving mid-turn would delete itself and its `UserMessage`/`WakeFired` would land mid-stream (ahead of the in-flight turn's own output) while `TurnComplete`'s drain, finding an empty set, never spawns. Parking the watcher (`send(None)`) stops it busy-spinning on the past deadline; the next re-arm happens when `TurnComplete`'s drain fires the wake while idle.
> **Due bound (I2):** `..(now + 1, String::new())` is exclusive of `(now+1, "")`, so it includes every key with `fire_at <= now` (a real wake at `now` has key `(now, "01ABC…")` which is `< (now+1, "")`) and excludes `fire_at > now`. This matches the design's "fire_at ≤ now".

- [ ] **Step 6: Handle `AgentUpdate::WakeDue` in the UI `select!`.** In `crates/zoid/src/main.rs`, add a new arm to the `match update { … }` (after the `WorktreeRequested` arm at `:3060`, before the match's closing brace):

```rust
                    AgentUpdate::WakeDue => {
                        let _ = drain_due_wakes(app).await?;
                    }
```

- [ ] **Step 7: Catch-up on load.** At the two load sites (startup after the Task 3 rebuild near `:1883`; resume after the Task 3 rebuild at `:3786`), fire any already-overdue wakes. At startup the run loop hasn't begun; the simplest correct hook is to call `drain_due_wakes(&mut app).await?` once immediately BEFORE entering the main `select!` loop (after `App` is fully built and the watcher spawned). At resume, call it right after the rebuild:

Startup (just before the `loop { tokio::select! { … } }` at `:2628`):

```rust
    // Catch-up: fire any wakes whose fire_at already passed while closed.
    let _ = drain_due_wakes(&mut app).await?;
```

Resume (`handle_action`, after `app.next_wake_tx.send(...)` from Task 3 Step 7):

```rust
        let _ = drain_due_wakes(app).await?;
```

- [ ] **Step 8: Drain at `TurnComplete` (busy → fires after the turn).** In the `TurnComplete` arm (`:2764`), inside the `if app.in_flight_subagents.is_empty()` idle block, AFTER the `pending_message` / `take_deferred_delegation_wake` handling (near `:2808-2811`), add a final drain so a wake that arrived mid-turn now fires:

```rust
                            // A wake may have come due while this turn ran; now
                            // that we're idle, drain it (spawns its own turn).
                            if !app.streaming {
                                let _ = drain_due_wakes(app).await?;
                            }
```

> Guarded by `!app.streaming` so it does not run when the pending_message/delegation paths just spawned a turn (those already carry context forward; the wake will drain after *that* turn completes). This composes with the existing idle-consumption without clobbering `pending_message`.

- [ ] **Step 9: Write a state-transition test for idle injection.** Add to `mod tests` (uses `test_app`, no real timer):

```rust
    #[tokio::test]
    async fn due_wake_injects_usermessage_and_fires_when_idle() {
        let mut app = test_app().await;
        app.streaming = false;
        let past = now_ms() - 10_000; // already due, > 5s late
        app.pending_wakes.insert((past, "w1".to_string()), "check the build".to_string());
        let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));

        let spawned = drain_due_wakes(&mut app).await.unwrap();

        assert!(spawned, "an idle orchestrator fires the due wake");
        assert!(app.streaming, "firing marks the turn streaming");
        assert!(app.pending_wakes.is_empty(), "the fired wake leaves the pending set");
        let log = app.session.snapshot().await.unwrap();
        assert!(
            log.iter().any(|e| matches!(&e.kind,
                EventKind::UserMessage { text } if text == "⏰ scheduled: check the build (fired late)")),
            "the late-stamped note is injected as a UserMessage"
        );
        assert!(
            log.iter().any(|e| matches!(&e.kind, EventKind::WakeFired { wake_id } if wake_id == "w1")),
            "a WakeFired marker is recorded at injection (at-least-once)"
        );
    }

    #[tokio::test]
    async fn future_wake_does_not_fire() {
        let mut app = test_app().await;
        app.streaming = false;
        let future = now_ms() + 60_000;
        app.pending_wakes.insert((future, "w2".to_string()), "later".to_string());
        let spawned = drain_due_wakes(&mut app).await.unwrap();
        assert!(!spawned, "a not-yet-due wake must not fire");
        assert_eq!(app.pending_wakes.len(), 1, "it stays pending");
    }
```

- [ ] **Step 10: Run crate + workspace.**

Run: `cargo test -p zoid`
Expected: PASS (both new tests + the helper test green).

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 11: Commit.**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(wake): fire due wakes — inject UserMessage + WakeFired, catch-up on load, drain at TurnComplete"
```

---

## Task 5: `schedule_wake` + `cancel_wake` tools

**Files:**
- Create: `crates/zoid-tools/src/wake.rs` — `ScheduleWake` + `CancelWake` Emitting tools
- Modify: `crates/zoid-tools/src/lib.rs` — `pub mod wake;` (keep OUT of `registry()`)
- Modify: `crates/zoid/src/invoke_skill.rs:108` — push both into `chat_tools`
- Modify: `crates/zoid/src/agent.rs` — 2 Emitting dispatch arms (near `:1643`/`:1700`) + 2 `AgentUpdate` request variants (`:373`)
- Modify: `crates/zoid/src/main.rs` — `ScheduleWake`/`CancelWake` handlers (beside `handle_worktree_request` `:5432`, called from new `select!` arms); pure `validate_schedule`; floor/cap consts

**Interfaces:**
- Consumes: `WakeConfig.enabled` (Task 2); `pending_wakes`, `next_wake_tx`, `earliest_fire_at`, `record` (Tasks 3-4).
- Produces: tools `schedule_wake { delay_secs: u64, note: String }` and `cancel_wake { id: Option<String> }`; `AgentUpdate::ScheduleWake { delay_secs, note, reply }` and `AgentUpdate::CancelWake { id, reply }` (`reply: oneshot::Sender<Result<String, String>>`); `fn validate_schedule(enabled: bool, pending_count: usize, delay_secs: u64) -> Result<(), String>`.

- [ ] **Step 1: Write the failing validation test.** Add to `mod tests` in `crates/zoid/src/main.rs`:

```rust
    #[test]
    fn validate_schedule_enforces_switch_floor_and_cap() {
        assert!(validate_schedule(false, 0, 60).is_err(), "disabled → reject");
        assert!(validate_schedule(true, 0, 29).is_err(), "below 30s floor → reject");
        assert!(validate_schedule(true, 16, 60).is_err(), "at 16 pending cap → reject");
        assert!(validate_schedule(true, 15, 30).is_ok(), "enabled, 30s, under cap → ok");
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --bin zoid validate_schedule_enforces_switch_floor_and_cap`
Expected: compile error — `validate_schedule` undefined.

- [ ] **Step 3: Add the pure validation helper + consts.** In `crates/zoid/src/main.rs` near the wake helpers:

```rust
/// Runaway guards for agent-scheduled wakes (constants in v1).
const WAKE_MIN_DELAY_SECS: u64 = 30;
const WAKE_MAX_PENDING: usize = 16;
/// Validate a `schedule_wake` request against the master switch, the 30 s floor,
/// and the 16-pending cap. Returns a user-facing error string on rejection.
fn validate_schedule(enabled: bool, pending_count: usize, delay_secs: u64) -> Result<(), String> {
    if !enabled {
        return Err("scheduled wake-ups are disabled ([wake] enabled = false)".into());
    }
    if delay_secs < WAKE_MIN_DELAY_SECS {
        return Err(format!("delay must be at least {WAKE_MIN_DELAY_SECS}s"));
    }
    if pending_count >= WAKE_MAX_PENDING {
        return Err(format!("too many pending wakes (max {WAKE_MAX_PENDING})"));
    }
    Ok(())
}
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -p zoid --bin zoid validate_schedule_enforces_switch_floor_and_cap`
Expected: PASS.

- [ ] **Step 5: Create the two Emitting tools.** Create `crates/zoid-tools/src/wake.rs` (mirror `worktree_enter.rs`):

```rust
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `schedule_wake { delay_secs, note }` — Emitting: the main loop validates,
/// persists a WakeScheduled, and arms the watcher. Main Chat agent only.
pub struct ScheduleWake;
impl Tool for ScheduleWake {
    fn name(&self) -> &str { "schedule_wake" }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_wake".into(),
            description: "Schedule a one-shot reminder to resume THIS conversation after \
                          delay_secs seconds. On fire you are re-invoked with `note` as a \
                          message. Minimum 30s. Use when waiting on something to check later."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "delay_secs": { "type": "integer", "minimum": 30,
                        "description": "Seconds from now to wake (>= 30)." },
                    "note": { "type": "string",
                        "description": "What to remind yourself to do on wake." }
                },
                "required": ["delay_secs", "note"]
            }),
        }
    }
    fn kind(&self) -> ToolKind { ToolKind::Emitting }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("schedule_wake is executed by the agent loop")
    }
}

/// `cancel_wake { id? }` — Emitting: cancels one pending wake by id, or all when
/// `id` is omitted. Main Chat agent only.
pub struct CancelWake;
impl Tool for CancelWake {
    fn name(&self) -> &str { "cancel_wake" }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "cancel_wake".into(),
            description: "Cancel a scheduled wake by its id (from schedule_wake), or all \
                          pending wakes when id is omitted."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string",
                        "description": "The wake id to cancel; omit to cancel all pending wakes." }
                }
            }),
        }
    }
    fn kind(&self) -> ToolKind { ToolKind::Emitting }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        ToolOutput::err("cancel_wake is executed by the agent loop")
    }
}
```

Add `pub mod wake;` to `crates/zoid-tools/src/lib.rs` (near `pub mod worktree_enter;`). Do NOT add either tool to `registry()`/`registry_with_kill()`.

- [ ] **Step 6: Add the request `AgentUpdate` variants + Emitting dispatch arms.** In `crates/zoid/src/agent.rs`, before `AgentUpdate`'s closing `}` (`:373`):

```rust
    /// `schedule_wake` request → main loop validates + persists; replies Ok(wake_id) or Err(msg).
    ScheduleWake {
        delay_secs: u64,
        note: String,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    /// `cancel_wake` request → main loop cancels one/all; replies Ok(summary) or Err(msg).
    CancelWake {
        id: Option<String>,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
```

In the Emitting dispatch region of the tool loop (beside the `enter_worktree`/`exit_worktree` arms, `~:1559`/`:1643`), add two arms that send the request, await the reply, and `emit` a `ToolResult` (mirror the `enter_worktree` arm exactly — oneshot, `ui.send(...).await`, `rx.await`, `emit(... ToolResult { output, is_error })`):

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "schedule_wake" => {
                    let delay_secs = tc.args.get("delay_secs").and_then(|v| v.as_u64()).unwrap_or(0);
                    let note = tc.args.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui.send(AgentUpdate::ScheduleWake { delay_secs, note, reply: tx }).await;
                    let (output, is_error) = match rx.await {
                        Ok(Ok(id)) => (format!("scheduled (id {id})"), false),
                        Ok(Err(e)) => (e, true),
                        Err(_) => ("schedule_wake failed (no reply)".into(), true),
                    };
                    emit(&session, &mut events, ui, &config.branch,
                        EventKind::ToolResult { id: tc.id, name: tc.name, output, is_error },
                        session_id, now).await?;
                    continue;
                }
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "cancel_wake" => {
                    let id = tc.args.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = ui.send(AgentUpdate::CancelWake { id, reply: tx }).await;
                    let (output, is_error) = match rx.await {
                        Ok(Ok(msg)) => (msg, false),
                        Ok(Err(e)) => (e, true),
                        Err(_) => ("cancel_wake failed (no reply)".into(), true),
                    };
                    emit(&session, &mut events, ui, &config.branch,
                        EventKind::ToolResult { id: tc.id, name: tc.name, output, is_error },
                        session_id, now).await?;
                    continue;
                }
```

> Match the neighboring `enter_worktree` arm EXACTLY — read it first (agent.rs:1559-1611) and copy both the `emit(...)` argument order (arg names/order may differ slightly from the sketch above) AND its control-flow ending: the trailing `continue;` shown above is a harmless no-op because this dispatch `match` is the last statement in the loop body, so drop it if the neighbor arms omit it, or keep it if they use it — do whatever the neighbor does (M2).

- [ ] **Step 7: Handle the requests in the main loop.** In `crates/zoid/src/main.rs`, add `ScheduleWake`/`CancelWake` arms to the UI `match update` (beside the `WakeDue` arm from Task 4) that call handlers beside `handle_worktree_request` (`:5432`):

```rust
                    AgentUpdate::ScheduleWake { delay_secs, note, reply } => {
                        let _ = reply.send(handle_schedule_wake(app, delay_secs, note).await);
                    }
                    AgentUpdate::CancelWake { id, reply } => {
                        let _ = reply.send(handle_cancel_wake(app, id).await);
                    }
```

Handlers (near `handle_worktree_request`):

```rust
/// Validate + persist a scheduled wake, insert it into the pending set, and
/// re-arm the watcher. Returns the new wake id (or a user-facing error).
async fn handle_schedule_wake(app: &mut App, delay_secs: u64, note: String) -> Result<String, String> {
    validate_schedule(app.config.wake.enabled, app.pending_wakes.len(), delay_secs)?;
    let wake_id = Ulid::new().to_string();
    let fire_at_ms = now_ms() + (delay_secs as i64) * 1000;
    app.record(EventKind::WakeScheduled { wake_id: wake_id.clone(), fire_at_ms, note: note.clone() })
        .await
        .map_err(|e| format!("failed to persist wake: {e}"))?;
    app.pending_wakes.insert((fire_at_ms, wake_id.clone()), note);
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    Ok(wake_id)
}

/// Cancel one pending wake by id, or all when `id` is None. Records a
/// WakeCancelled per removed wake and re-arms the watcher. Cancelling an
/// unknown/already-fired id is a no-op success.
async fn handle_cancel_wake(app: &mut App, id: Option<String>) -> Result<String, String> {
    let targets: Vec<(i64, String)> = match &id {
        Some(want) => app.pending_wakes.keys()
            .filter(|(_, wid)| wid == want).cloned().collect(),
        None => app.pending_wakes.keys().cloned().collect(),
    };
    for (fire_at, wid) in &targets {
        app.pending_wakes.remove(&(*fire_at, wid.clone()));
        app.record(EventKind::WakeCancelled { wake_id: wid.clone() })
            .await
            .map_err(|e| format!("failed to persist cancel: {e}"))?;
    }
    let _ = app.next_wake_tx.send(earliest_fire_at(&app.pending_wakes));
    Ok(match id {
        Some(_) => format!("cancelled {} wake(s)", targets.len()),
        None => format!("cancelled all {} pending wake(s)", targets.len()),
    })
}
```

> `app.config.wake.enabled` — use the actual field path for the loaded `Config` on `App` (grep how `app` reads `subagent`/economy config; adapt if it is `app.economy`-adjacent or a separate `app.config`). The point is the master switch from Task 2.

- [ ] **Step 8: Register both tools in `chat_tools` only.** In `crates/zoid/src/invoke_skill.rs`, after the `ExitWorktree` push (`:108`):

```rust
    tools.push(Box::new(zoid_tools::wake::ScheduleWake));
    tools.push(Box::new(zoid_tools::wake::CancelWake));
```

- [ ] **Step 9: Assert subagent exclusion.** Extend the existing exclusion test. In `crates/zoid-tools/src/lib.rs` `registry_excludes_chat_only_tools` (`:366`), assert neither `schedule_wake` nor `cancel_wake` is in `registry()`. In `crates/zoid/src/invoke_skill.rs` `chat_tools_includes_dispatch_and_diff` (`:187`), assert both ARE in `chat_tools`:

```rust
    // in registry_excludes_chat_only_tools (zoid-tools/src/lib.rs):
    assert!(!names.contains(&"schedule_wake"), "schedule_wake is chat-only");
    assert!(!names.contains(&"cancel_wake"), "cancel_wake is chat-only");
```
```rust
    // in chat_tools_includes_dispatch_and_diff (invoke_skill.rs):
    assert!(names.contains(&"schedule_wake"), "chat_tools includes schedule_wake");
    assert!(names.contains(&"cancel_wake"), "chat_tools includes cancel_wake");
```

- [ ] **Step 10: Write a handler state test.** Add to `mod tests` in `main.rs`:

```rust
    #[tokio::test]
    async fn schedule_then_cancel_roundtrips_the_pending_set() {
        let mut app = test_app().await;
        // Assumes wake.enabled defaults true in the test config.
        let id = handle_schedule_wake(&mut app, 60, "check CI".into()).await.unwrap();
        assert_eq!(app.pending_wakes.len(), 1, "schedule inserts one pending wake");
        assert_eq!(earliest_fire_at(&app.pending_wakes), app.pending_wakes.keys().next().map(|(t,_)| *t));

        let msg = handle_cancel_wake(&mut app, Some(id)).await.unwrap();
        assert!(app.pending_wakes.is_empty(), "cancel removes it");
        assert!(msg.contains("cancelled 1"));

        // A projection rebuild over the recorded events agrees (Scheduled then Cancelled → empty).
        let log = app.session.snapshot().await.unwrap();
        assert!(rebuild_pending_wakes(&log).is_empty(), "event-log projection matches live state");
    }
```

> If `test_app`'s `Config` does not default `wake.enabled = true`, set `app.config.wake.enabled = true` at the top of the test (match the real field path).

- [ ] **Step 11: Run crate + FULL workspace.**

Run: `cargo test -p zoid`
Expected: PASS.

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 12: Commit.**

```bash
git add crates/zoid-tools/src/wake.rs crates/zoid-tools/src/lib.rs crates/zoid/src/invoke_skill.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(wake): schedule_wake + cancel_wake tools (Emitting, main-Chat-only) with floor/cap/enabled guards"
```

---

## Final Verification

- [ ] Full workspace suite: `cargo test` — PASS across all crates (run with `set -o pipefail`; confirm a real exit 0).
- [ ] Clean build: `cargo build -p zoid` — clean.
- [ ] Subagent exclusion audit: `grep -n "schedule_wake\|cancel_wake" crates/zoid-tools/src/lib.rs` — confirm neither appears in `registry()`/`registry_with_kill()` (only `pub mod wake;`).
- [ ] Watcher liveness sanity: confirm the watcher `select!`s on `sleep` vs `next_rx.changed()` and that `None` parks on `changed()` only (no busy-loop), and both drop-paths (`ui.send` err, `changed` err) break the loop.

---

## Self-Review Notes

- **Spec coverage:** event kinds + projection (Task 1) → persistence/catch-up/at-least-once. `[wake] enabled` (Task 2). Watcher + state + rebuild-on-load (Task 3) → the "ui channel wakes the idle loop" mechanism, no new `select!` primitive beyond one arm. Firing/injection + catch-up + TurnComplete drain (Task 4) → same-session synthetic `UserMessage`, late stamp, at-least-once `WakeFired`. Tools + guards (Task 5) → `schedule_wake`/`cancel_wake`, 30 s floor, 16 cap, master switch, main-Chat-only.
- **Deliberate deviation from the design doc (flag for plan review):** the design says fire-during-active-turn queues "via the existing `pending_message` path." This plan does NOT reuse `pending_message` — that single slot also holds the user's queued keystrokes, and a wake firing while the user has text queued would clobber it (or vice-versa). Instead `pending_wakes` stays the source of truth and due wakes are drained at both `WakeDue` arrival and `TurnComplete` (when idle). Same observable behavior (busy → fires after the turn), no collision, and it handles multiple simultaneously-due wakes (which a single `pending_message` slot cannot).
- **Deliberate simplification:** the design's late stamp `(scheduled for {t}, fired late)` drops the absolute `{t}` (would need tz-formatting a timestamp); v1 stamps just ` (fired late)`. Cheap to extend later if wanted.
- **Composition with the shipped wake-on-DelegationResult patch:** distinct mechanism. The `TurnComplete` drain (Task 4 Step 8) runs AFTER the `pending_message`/`take_deferred_delegation_wake` handling and is guarded by `!app.streaming`, so it never double-spawns.
- **Type consistency:** `pending_wakes: BTreeMap<(i64, String), String>` and `next_wake_tx: watch::Sender<Option<i64>>` are defined once (Task 3) and read identically in Tasks 4-5. `AgentUpdate::WakeDue`/`ScheduleWake`/`CancelWake` shapes match sender (agent.rs) and receiver (main.rs). `validate_schedule`/`wake_injection_text`/`earliest_fire_at`/`rebuild_pending_wakes` defined once, used consistently.
- **KNOWN GAP (for the whole-branch review):** no test drives the live watcher `sleep`/`changed()` timer end-to-end (per the Spec 2 ruling, testing tokio's own `watch`/timer semantics is flaky and low-value). The projection, due-selection, injection text, validation, and handler state-transitions are all unit-tested; the watcher wiring is type-checked. Catch-up-on-load is covered by `drain_due_wakes` tests with a past `fire_at`.
