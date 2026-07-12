# Subagent Dispatch Guardrails Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound a dispatched subagent's runtime (idle + absolute ceiling) and make it stoppable by a timer, an orchestrator kill tool, and the user's Esc key — all firing the cancellation machinery `run_agent_turn_cancellable` already implements.

**Architecture:** Give each subagent real `cancel`/`hard` `CancellationToken`s plus a heartbeat `AtomicI64`, hold them in an upgraded in-flight registry (`HashSet<String>` → `HashMap<String, SubagentHandle>`), and fire the `hard` token from three sources: a per-subagent `WakeTimer` supervisor (idle/ceiling breach), a `cancel_subagent` Emitting tool, and the Esc escalation path. On any abort the subagent's run is forced to a failure `DelegationResult` and its worktree is discarded. No new persistence.

> **Deliberate deviation from the spec:** the spec's file-change map threads the heartbeat as an *additive parameter* `progress: Option<&AtomicI64>` on `run_agent_turn_cancellable`. This plan instead carries it as a `TurnConfig.progress` field (read at the heartbeat bump). The behavior is identical; the field approach is cleaner because `TurnConfig` already threads every other per-turn handle and avoids widening the already-long `run_agent_turn_cancellable` signature. Flagged here so a plan↔spec diff during review doesn't read it as an accidental gap.

**Tech Stack:** Rust, tokio + `tokio_util::sync::CancellationToken`, `std::sync::atomic::AtomicI64`, existing `TurnConfig`/`run_agent_turn_cancellable` turn machinery, layered TOML config (`zoid-core`).

## Global Constraints

- Idle timeout default 120s, hard ceiling default 900s, both `0 = off`, from `[subagent]` config.
- Iteration cap stays 25 (`SUBAGENT_MAX_ITERATIONS`, `subagent.rs:29`) — do NOT change it.
- `cancel_subagent { id: Option<String> }`: id kills one, omitted kills all; main-Chat-agent-only (`chat_tools`, NOT `registry()`).
- On abort: emit failure `DelegationResult` (`ok:false`, `summary`=reason) + discard worktree (drop guard).
- No new `EventKind` / DB / schema change (guardrails are transient; `DelegationResult` already exists).
- Never add Co-Authored-By or any co-author trailer to commits.
- Each task ends with `cargo test -p <touched crates>` green; commit per task.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/zoid/src/wake_timer.rs` | **new** — `WakeTimer` supervisor primitive (generic over reason `R`). |
| `crates/zoid-core/src/config.rs` | `[subagent]` `SubagentConfig` + `PartialSubagent` + `Provenance` + merge. |
| `crates/zoid/src/agent.rs` | `SubagentHandle` + `AbortReason` types; `TurnConfig` gains `progress`/`subagent_idle`/`subagent_ceiling`; heartbeat bump; dispatch builds+registers a handle and calls the new `spawn_subagent`; `cancel_subagent` Emitting branch + `fire_subagent_kill` helper. |
| `crates/zoid/src/subagent.rs` | `run_subagent` gains `cancel`/`hard`/`progress`; calls `run_agent_turn_cancellable` directly. |
| `crates/zoid/src/spawn_subagent.rs` | accept tokens + timeouts; spawn `WakeTimer`; on `hard.is_cancelled()` force failure + drop worktree. |
| `crates/zoid/src/main.rs` | `in_flight` map upgrade; thread `[subagent]` timeouts into the turn; Esc escalation fires subagent tokens + no-turn armed-confirm path. |
| `crates/zoid-tools/src/subagent_kill.rs` | **new** — `cancel_subagent` Emitting tool stub. |
| `crates/zoid-tools/src/lib.rs` | `pub mod subagent_kill;` (kept OUT of `registry()`). |
| `crates/zoid/src/invoke_skill.rs` | register `cancel_subagent` in `chat_tools`. |

---

### Task 1: `WakeTimer` supervisor primitive

**Files:**
- Create: `crates/zoid/src/wake_timer.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod wake_timer;`)

**Interfaces:**
- Produces: `zoid::wake_timer::WakeTimer::spawn<R: Send + 'static>(idle: Option<Duration>, ceiling: Option<Duration>, progress: Arc<AtomicI64>, now: fn() -> i64, idle_reason: R, ceiling_reason: R, reason: Arc<Mutex<Option<R>>>, fire: CancellationToken, done: CancellationToken) -> tokio::task::JoinHandle<()>`. Fires `fire` and records the matching reason (first-writer-wins) when `now() - progress > idle` OR `now() - start > ceiling`; stops when `done` fires; both `None` → an already-finished task.

- [ ] **Step 1: Register the module.** Add to `crates/zoid/src/lib.rs` next to the other `pub mod` lines (e.g. right after `pub mod subagent;` — run `grep -n "pub mod subagent" crates/zoid/src/lib.rs` to find it):

```rust
pub mod wake_timer;
```

- [ ] **Step 2: Write the failing tests.** Create `crates/zoid/src/wake_timer.rs` with ONLY the test module first (the `WakeTimer` type does not exist yet, so this fails to compile — that is the failing state):

```rust
//! A reusable timeout supervisor. It watches a heartbeat (`progress`, an epoch-ms
//! `AtomicI64` the caller bumps as work advances) and a start time, and on breach
//! records a caller-supplied reason value (first-writer-wins) then fires a
//! `CancellationToken`. Generic over the reason type `R` so it can carry any
//! value without knowing its meaning. Stops cleanly when `done` fires.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub struct WakeTimer;

impl WakeTimer {
    /// Spawn a supervisor. Fires `fire` when `now() - progress > idle` (no-progress)
    /// or `now() - start > ceiling` (absolute); writes the matching reason value
    /// into `reason` (first-writer-wins) just before firing. Stops when `done`
    /// is cancelled. `None` disables that arm; both `None` → no work is done.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn<R: Send + 'static>(
        idle: Option<Duration>,
        ceiling: Option<Duration>,
        progress: Arc<AtomicI64>,
        now: fn() -> i64,
        idle_reason: R,
        ceiling_reason: R,
        reason: Arc<Mutex<Option<R>>>,
        fire: CancellationToken,
        done: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        // Both arms disabled: nothing to supervise. Return a finished task so the
        // handle is uniform for callers (the kill tool + Esc still work via the
        // registry even when no timer runs).
        if idle.is_none() && ceiling.is_none() {
            return tokio::spawn(async {});
        }
        let start = now();
        let idle_ms = idle.map(|d| d.as_millis() as i64);
        let ceiling_ms = ceiling.map(|d| d.as_millis() as i64);
        tokio::spawn(async move {
            let mut idle_reason = Some(idle_reason);
            let mut ceiling_reason = Some(ceiling_reason);
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    biased;
                    _ = done.cancelled() => return,
                    _ = ticker.tick() => {
                        let t = now();
                        let last = progress.load(Ordering::Relaxed);
                        let idle_breach = idle_ms.map_or(false, |i| i > 0 && t - last > i);
                        let ceiling_breach = ceiling_ms.map_or(false, |c| c > 0 && t - start > c);
                        if idle_breach || ceiling_breach {
                            // Ceiling wins the label when both trip on the same tick.
                            let r = if ceiling_breach {
                                ceiling_reason.take()
                            } else {
                                idle_reason.take()
                            };
                            if let Some(r) = r {
                                let mut slot = reason.lock().unwrap();
                                if slot.is_none() {
                                    *slot = Some(r);
                                }
                            }
                            fire.cancel();
                            return;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestReason {
        Idle,
        Ceiling,
    }

    // Each test owns a distinct process-global clock + fn pointer so parallel
    // test runs never race a shared static. `now: fn() -> i64` cannot close over
    // per-test state, so a dedicated static per test is the race-free way to
    // inject a controllable clock.
    static CLOCK_IDLE: AtomicI64 = AtomicI64::new(0);
    fn now_idle() -> i64 {
        CLOCK_IDLE.load(Ordering::Relaxed)
    }

    static CLOCK_CEIL: AtomicI64 = AtomicI64::new(0);
    fn now_ceil() -> i64 {
        CLOCK_CEIL.load(Ordering::Relaxed)
    }

    static CLOCK_NOBREACH: AtomicI64 = AtomicI64::new(0);
    fn now_nobreach() -> i64 {
        CLOCK_NOBREACH.load(Ordering::Relaxed)
    }

    static CLOCK_DONE: AtomicI64 = AtomicI64::new(0);
    fn now_done() -> i64 {
        CLOCK_DONE.load(Ordering::Relaxed)
    }

    #[tokio::test(start_paused = true)]
    async fn fires_on_idle_breach() {
        CLOCK_IDLE.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0)); // never bumped → idle grows
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_idle,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Advance the injected wall clock past the 1s idle window (progress stays 0).
        CLOCK_IDLE.store(2000, Ordering::Relaxed);
        // Drive the ticker so it samples the clock and breaches.
        tokio::time::advance(Duration::from_millis(300)).await;
        fire.cancelled().await; // resolves once the timer fires
        assert!(fire.is_cancelled(), "idle breach must fire the token");
        assert_eq!(*reason.lock().unwrap(), Some(TestReason::Idle));
        let _ = h.await;
    }

    #[tokio::test(start_paused = true)]
    async fn fires_on_ceiling_breach() {
        CLOCK_CEIL.store(0, Ordering::Relaxed); // start captured as 0 at spawn
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            None,
            Some(Duration::from_secs(1)),
            progress.clone(),
            now_ceil,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Keep progress fresh so idle can never be the cause; only the ceiling trips.
        CLOCK_CEIL.store(2000, Ordering::Relaxed);
        progress.store(2000, Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(300)).await;
        fire.cancelled().await;
        assert!(fire.is_cancelled(), "ceiling breach must fire the token");
        assert_eq!(*reason.lock().unwrap(), Some(TestReason::Ceiling));
        let _ = h.await;
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_fire_before_breach() {
        CLOCK_NOBREACH.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let _h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_nobreach,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done,
        );
        // Only 500ms of "idle" — under the 1s window.
        CLOCK_NOBREACH.store(500, Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(300)).await;
        assert!(!fire.is_cancelled(), "must not fire before the window elapses");
        assert!(reason.lock().unwrap().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn done_stops_the_supervisor() {
        CLOCK_DONE.store(0, Ordering::Relaxed);
        let progress = Arc::new(AtomicI64::new(0));
        let reason = Arc::new(Mutex::new(None));
        let fire = CancellationToken::new();
        let done = CancellationToken::new();
        let h = WakeTimer::spawn(
            Some(Duration::from_secs(1)),
            None,
            progress,
            now_done,
            TestReason::Idle,
            TestReason::Ceiling,
            reason.clone(),
            fire.clone(),
            done.clone(),
        );
        done.cancel(); // normal completion signalled before any breach
        tokio::time::advance(Duration::from_millis(300)).await;
        h.await.expect("supervisor task should exit on done");
        assert!(!fire.is_cancelled(), "done must stop the timer without firing");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail.**

The Step 2 code block ships the `impl` and the tests together, so to see a genuine RED state: temporarily comment out the body of `impl WakeTimer { ... }` (leave `pub struct WakeTimer;` and the `impl WakeTimer {}` shell so the module still parses), then run:

Run: `cargo test -p zoid --lib wake_timer`
Expected: compile error — `no function or associated item named 'spawn' found for struct 'WakeTimer'`. This is the real failing gate (the tests reference a `spawn` that doesn't exist yet).

Then restore the `impl` body before Step 4.

- [ ] **Step 4: Run tests to verify they pass.**

Run: `cargo test -p zoid --lib wake_timer`
Expected: PASS — `fires_on_idle_breach`, `fires_on_ceiling_breach`, `does_not_fire_before_breach`, `done_stops_the_supervisor` all green.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid/src/wake_timer.rs crates/zoid/src/lib.rs
git commit -m "feat(subagent): add WakeTimer timeout supervisor primitive"
```

---

### Task 2: `[subagent]` config section

**Files:**
- Modify: `crates/zoid-core/src/config.rs`
  - `Config` struct (`:27-41`), `Config::default` (`:129-146`)
  - `Provenance` struct (`:243-260`), merge Provenance init (`:355-371`), merge loop (`:372-…`)
  - `PartialConfig` struct (`:322-337`)

**Interfaces:**
- Produces: `zoid_core::config::SubagentConfig { pub idle_timeout_secs: u64, pub hard_timeout_secs: u64 }` (defaults 120 / 900); reachable as `config.subagent`. `PartialSubagent { idle_timeout_secs: Option<u64>, hard_timeout_secs: Option<u64> }` under `[subagent]`. `Provenance.subagent_idle_timeout_secs` / `.subagent_hard_timeout_secs`.

- [ ] **Step 1: Write the failing tests.** Add to the `#[cfg(test)] mod tests` in `crates/zoid-core/src/config.rs` (after `defaults_are_sane`, around `:178`):

```rust
    #[test]
    fn subagent_defaults_are_120_and_900() {
        let c = Config::default();
        assert_eq!(c.subagent.idle_timeout_secs, 120);
        assert_eq!(c.subagent.hard_timeout_secs, 900);
    }

    #[test]
    fn subagent_section_parses_and_merges() {
        let (p, _warn) =
            parse_toml("[subagent]\nidle_timeout_secs = 30\nhard_timeout_secs = 0").unwrap();
        assert_eq!(p.subagent.idle_timeout_secs, Some(30));
        assert_eq!(p.subagent.hard_timeout_secs, Some(0));
        let (cfg, prov) = merge(&[(Source::UserGlobal, p)]);
        assert_eq!(cfg.subagent.idle_timeout_secs, 30);
        assert_eq!(cfg.subagent.hard_timeout_secs, 0); // 0 = off, still a valid value
        assert_eq!(prov.subagent_idle_timeout_secs, Source::UserGlobal);
        assert_eq!(prov.subagent_hard_timeout_secs, Source::UserGlobal);
    }
```

- [ ] **Step 2: Run tests to verify they fail.**

Run: `cargo test -p zoid-core subagent`
Expected: compile error — `Config` has no field `subagent`, `PartialConfig` has no `subagent`, `Provenance` has no `subagent_*` fields.

- [ ] **Step 3: Add the `SubagentConfig` type + default.** Insert after the `EmbedConfig` default block (after `:127`) in `crates/zoid-core/src/config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubagentConfig {
    /// Idle (no-progress) timeout in seconds; 0 = disabled. Default 120.
    pub idle_timeout_secs: u64,
    /// Absolute wall-clock ceiling in seconds; 0 = disabled. Default 900.
    pub hard_timeout_secs: u64,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 120,
            hard_timeout_secs: 900,
        }
    }
}
```

- [ ] **Step 4: Add the field to `Config`.** In the `Config` struct (`:27-41`) add after `pub embed: EmbedConfig,`:

```rust
    pub subagent: SubagentConfig,
```

In `Config::default` (`:129-146`) add after `embed: EmbedConfig::default(),`:

```rust
            subagent: SubagentConfig::default(),
```

- [ ] **Step 5: Add the `PartialSubagent` + `PartialConfig` wiring.** After `PartialEmbed` (`:305`) add:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialSubagent {
    pub idle_timeout_secs: Option<u64>,
    pub hard_timeout_secs: Option<u64>,
}
```

In `PartialConfig` (`:322-337`) add after `pub embed: PartialEmbed,`:

```rust
    pub subagent: PartialSubagent,
```

- [ ] **Step 6: Add the `Provenance` fields + merge.** In `Provenance` (`:243-260`) add after `pub ui_edit_diff_inline: Source,`:

```rust
    pub subagent_idle_timeout_secs: Source,
    pub subagent_hard_timeout_secs: Source,
```

In the `merge` Provenance initializer (`:355-371`) add after `ui_edit_diff_inline: Source::Default,`:

```rust
        subagent_idle_timeout_secs: Source::Default,
        subagent_hard_timeout_secs: Source::Default,
```

In the merge loop, after the economy block (after `:412`, i.e. after the `reassert_interval_tokens` apply) add:

```rust
        if let Some(v) = p.subagent.idle_timeout_secs {
            cfg.subagent.idle_timeout_secs = v;
            prov.subagent_idle_timeout_secs = *src;
        }
        if let Some(v) = p.subagent.hard_timeout_secs {
            cfg.subagent.hard_timeout_secs = v;
            prov.subagent_hard_timeout_secs = *src;
        }
```

- [ ] **Step 7: Run tests to verify they pass.**

Run: `cargo test -p zoid-core subagent`
Expected: PASS — `subagent_defaults_are_120_and_900`, `subagent_section_parses_and_merges`.

- [ ] **Step 8: Confirm nothing else broke.**

Run: `cargo test -p zoid-core`
Expected: PASS (whole crate green).

- [ ] **Step 9: Commit.**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [subagent] idle/hard timeout config"
```

---

### Task 3: Registry types + `TurnConfig` fields + heartbeat + `HashSet`→`HashMap`

**Files:**
- Modify: `crates/zoid/src/agent.rs`
  - add `AbortReason` + `SubagentHandle` (near `TurnConfig`, after `:71`)
  - `TurnConfig` struct (`:76-116`): add `progress`, `subagent_idle`, `subagent_ceiling`; change `in_flight` type
  - `TurnConfig` `Debug` impl (`:122-141`)
  - `chat_turn_config_with` (`:146-171`)
  - heartbeat bump after `:896`
  - dispatch site (`:1405-1423`): build + register `SubagentHandle`
- Modify: `crates/zoid/src/subagent.rs` inline `TurnConfig` (`:160-175`)
- Modify: `crates/zoid/src/main.rs`
  - `App.in_flight` field (`:1582`), both inits (`:2081-2083`, `:6239-6241`)

**Interfaces:**
- Consumes: nothing new from earlier tasks.
- Produces: `zoid::agent::AbortReason { IdleTimeout, Ceiling, Killed }` (`Debug, Clone, Copy, PartialEq, Eq`) with `fn label(&self) -> &'static str`; `zoid::agent::SubagentHandle { cancel, hard: CancellationToken, progress: Arc<AtomicI64>, abort_reason: Arc<Mutex<Option<AbortReason>>> }` (`Clone`). `TurnConfig.in_flight: Option<Arc<Mutex<HashMap<String, SubagentHandle>>>>`, `TurnConfig.progress: Option<Arc<AtomicI64>>`, `TurnConfig.subagent_idle`/`subagent_ceiling: Option<Duration>`.

- [ ] **Step 1: Write the failing test.** Add a `#[cfg(test)]` block near the bottom of `crates/zoid/src/agent.rs` (or extend an existing test module) — this pins the new types:

```rust
#[cfg(test)]
mod guardrail_types_tests {
    use super::{AbortReason, SubagentHandle};
    use std::sync::atomic::AtomicI64;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn abort_reason_labels_are_stable() {
        assert_eq!(AbortReason::IdleTimeout.label(), "idle timeout");
        assert_eq!(AbortReason::Ceiling.label(), "hard timeout");
        assert_eq!(AbortReason::Killed.label(), "killed");
    }

    #[test]
    fn subagent_handle_is_constructible_and_clonable() {
        let h = SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
        };
        let h2 = h.clone();
        h.hard.cancel();
        assert!(h2.hard.is_cancelled(), "clone shares the same token");
    }
}
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --lib guardrail_types_tests`
Expected: compile error — `AbortReason` / `SubagentHandle` undefined.

- [ ] **Step 3: Add the types.** In `crates/zoid/src/agent.rs`, right before the `TurnConfig` doc comment (before `:73`), add:

```rust
/// Why a subagent was aborted. Shared across all three firers (timeout supervisor,
/// kill tool, Esc) via a single first-writer-wins slot in `SubagentHandle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// No-progress (idle) timeout tripped.
    IdleTimeout,
    /// Absolute wall-clock ceiling tripped.
    Ceiling,
    /// Cancelled by the orchestrator kill tool or the user's Esc.
    Killed,
}

impl AbortReason {
    /// Short human label for the failure summary.
    pub fn label(&self) -> &'static str {
        match self {
            AbortReason::IdleTimeout => "idle timeout",
            AbortReason::Ceiling => "hard timeout",
            AbortReason::Killed => "killed",
        }
    }
}

/// Live handle to one in-flight subagent: how to stop it and how it reports why.
/// Held in the registry (`App.in_flight` / `TurnConfig.in_flight`) so the timeout
/// supervisor, the kill tool, and Esc can all reach the same tokens.
#[derive(Clone)]
pub struct SubagentHandle {
    /// Graceful cancel (reserved; parity with the main turn).
    pub cancel: CancellationToken,
    /// Force-kill this subagent (drains + kills its shell via the turn loop).
    pub hard: CancellationToken,
    /// Heartbeat: last-progress epoch ms, bumped per iteration by the turn loop.
    pub progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// First-writer-wins abort reason, set by whichever firer trips first.
    pub abort_reason: std::sync::Arc<std::sync::Mutex<Option<AbortReason>>>,
}
```

> `CancellationToken` is already imported in `agent.rs` (used by `run_agent_turn_cancellable`). Confirm with `grep -n "use tokio_util::sync::CancellationToken" crates/zoid/src/agent.rs`; if absent, add it to the imports.

- [ ] **Step 4: Change `TurnConfig`.** In `crates/zoid/src/agent.rs`, change the `in_flight` field (`:110-112`) from:

```rust
    /// Shared in-flight subagent ID set for the sequential-dispatch guard.
    /// None when dispatch_subagent is disabled or for subagent turns.
    pub in_flight: Option<std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>>,
```

to:

```rust
    /// Shared in-flight subagent registry (id → live handle) for the
    /// sequential-dispatch guard and the guardrail firers. None when
    /// dispatch_subagent is disabled or for subagent turns.
    pub in_flight:
        Option<std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SubagentHandle>>>>,
```

Then, immediately after the `reassert_interval` field (after `:115`, still inside the struct), add:

```rust
    /// Heartbeat slot the turn loop bumps each iteration so a subagent's
    /// `WakeTimer` can detect a stalled `await`. `None` for the main chat turn.
    pub progress: Option<std::sync::Arc<std::sync::atomic::AtomicI64>>,
    /// Idle-timeout for a subagent dispatched from THIS turn (chat only). `None`
    /// = disabled. Consumed at the dispatch site; ignored by subagent turns.
    pub subagent_idle: Option<std::time::Duration>,
    /// Absolute-ceiling for a subagent dispatched from THIS turn. `None` = off.
    pub subagent_ceiling: Option<std::time::Duration>,
```

- [ ] **Step 5: Update the `Debug` impl.** In `crates/zoid/src/agent.rs` `Debug for TurnConfig` (`:122-141`), after `.field("reassert_interval", &self.reassert_interval)` (before `.finish()`) add:

```rust
            .field("progress", &self.progress.is_some())
            .field("subagent_idle", &self.subagent_idle)
            .field("subagent_ceiling", &self.subagent_ceiling)
```

- [ ] **Step 6: Update `chat_turn_config_with`.** In the `TurnConfig { … }` literal (`:155-170`), after `reassert_interval: 0,` add:

```rust
        progress: None,
        subagent_idle: None,
        subagent_ceiling: None,
```

- [ ] **Step 7: Update the subagent inline `TurnConfig`.** In `crates/zoid/src/subagent.rs` (`:160-175`), after `reassert_interval: 0,` add:

```rust
        progress: None,
        subagent_idle: None,
        subagent_ceiling: None,
```

- [ ] **Step 8: Add the heartbeat bump.** In `crates/zoid/src/agent.rs`, immediately after `iterations += 1;` (`:896`) add:

```rust
        if let Some(p) = &config.progress {
            p.store(now(), std::sync::atomic::Ordering::Relaxed);
        }
```

- [ ] **Step 9: Build + register a handle at the dispatch site.** In `crates/zoid/src/agent.rs`, replace the in-flight insert block (`:1402-1407`):

```rust
                    // Track the in-flight subagent BEFORE spawning, so a
                    // fast-completing subagent can't emit DelegationResult
                    // (which removes the ID) before we insert it.
                    if let Some(set) = &config.in_flight {
                        set.lock().unwrap().insert(sub_id.clone());
                    }
```

with (creates the tokens now — not fired until Task 4 wires them into `spawn_subagent`):

```rust
                    // Create the guardrail tokens + heartbeat for this subagent and
                    // register a handle BEFORE spawning, so a fast-completing
                    // subagent can't emit DelegationResult (which removes the ID)
                    // before we insert it. Tokens are created here but not fired
                    // until the WakeTimer + firers are wired (Task 4).
                    let sub_cancel = CancellationToken::new();
                    let sub_hard = CancellationToken::new();
                    let sub_progress =
                        std::sync::Arc::new(std::sync::atomic::AtomicI64::new(now()));
                    let sub_abort_reason = std::sync::Arc::new(std::sync::Mutex::new(None));
                    if let Some(reg) = &config.in_flight {
                        reg.lock().unwrap().insert(
                            sub_id.clone(),
                            SubagentHandle {
                                cancel: sub_cancel.clone(),
                                hard: sub_hard.clone(),
                                progress: sub_progress.clone(),
                                abort_reason: sub_abort_reason.clone(),
                            },
                        );
                    }
```

> The `spawn_subagent(...)` call directly below (`:1409-1423`) stays UNCHANGED in this task — the new locals (`sub_cancel`, `sub_hard`, `sub_progress`, `sub_abort_reason`) are wired into it in Task 4. To avoid an unused-variable warning in this intermediate state, prefix them with `_` here and remove the underscores in Task 4 — OR fold Task 3 + Task 4 into one commit if your reviewer prefers no transient warnings. This plan keeps them separate; add `#[allow(unused_variables)]` on the enclosing match arm if the warning blocks a `-D warnings` build.

- [ ] **Step 10: Upgrade `App.in_flight` in `main.rs`.** Change the field (`:1582`):

```rust
    in_flight: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
```

to:

```rust
    in_flight:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, zoid::agent::SubagentHandle>>>,
```

Change both inits (`:2081-2083` and `:6239-6241`) from:

```rust
        in_flight: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::new(),
        )),
```

to:

```rust
        in_flight: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
```

> The DelegationResult consumer (`:2666`, `app.in_flight.lock().unwrap().remove(subagent_id);`) and the sequential-dispatch guard (`agent.rs:1322-1323`, `!set.lock().unwrap().is_empty()`) both work unchanged on a `HashMap`. The clone at `main.rs:5425` (`Some(app.in_flight.clone())`) also compiles unchanged now that the types match.

- [ ] **Step 11: Run the type test + full crate build.**

Run: `cargo test -p zoid --lib guardrail_types_tests`
Expected: PASS.

Run: `cargo test -p zoid`
Expected: PASS — existing subagent/turn tests still green (heartbeat is `None` everywhere; registry is a map but no handle is fired yet).

- [ ] **Step 12: Commit.**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/subagent.rs crates/zoid/src/main.rs
git commit -m "feat(subagent): registry handle types + heartbeat field; in_flight HashSet->HashMap"
```

---

### Task 4: Real tokens + `WakeTimer` into dispatch → `spawn_subagent` → `run_subagent`

**Files:**
- Modify: `crates/zoid/src/subagent.rs` (`run_subagent` signature `:110-125`; call `:191-204`; test `:439-487`)
- Modify: `crates/zoid/src/spawn_subagent.rs` (whole `spawn_subagent`)
- Modify: `crates/zoid/src/agent.rs` (dispatch call `:1409-1423`)
- Modify: `crates/zoid/src/main.rs` (`spawn_turn`, after `:5425`)

**Interfaces:**
- Consumes: `SubagentHandle`, `AbortReason` (Task 3); `WakeTimer::spawn` (Task 1); `config.subagent` (Task 2).
- Produces: `run_subagent(.., cancel: CancellationToken, hard: CancellationToken, progress: Arc<AtomicI64>)`; `spawn_subagent(.., cancel, hard, progress, abort_reason: Arc<Mutex<Option<AbortReason>>>, idle: Option<Duration>, ceiling: Option<Duration>)`.

- [ ] **Step 1: Write the failing test (abort → failure DelegationResult).** In `crates/zoid/src/spawn_subagent.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn abort_summary_uses_reason_label() {
        // A killed/timed-out subagent must surface its reason in the summary.
        let s = super::abort_summary(Some(crate::agent::AbortReason::IdleTimeout));
        assert!(s.contains("idle timeout"), "summary carries the reason: {s}");
        let s2 = super::abort_summary(None);
        assert!(!s2.is_empty(), "a reasonless abort still has a summary");
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid --lib abort_summary`
Expected: compile error — `abort_summary` undefined.

- [ ] **Step 3: Change `run_subagent`.** In `crates/zoid/src/subagent.rs`, update the imports at `:21`:

```rust
use crate::agent::{run_agent_turn_cancellable, tool_specs, AgentUpdate, TurnConfig, WARN_GLYPH};
```

(replace `run_agent_turn` with `run_agent_turn_cancellable`). Add to the `use` block near the top:

```rust
use std::sync::atomic::AtomicI64;
use tokio_util::sync::CancellationToken;
```

Extend the `run_subagent` parameter list (after `approval: zoid_core::config::ApprovalConfig,` at `:124`):

```rust
    cancel: CancellationToken,
    hard: CancellationToken,
    progress: Arc<AtomicI64>,
```

In the inline `TurnConfig { … }` (`:160-175`), change `progress: None,` (added in Task 3) to:

```rust
        progress: Some(progress.clone()),
```

Replace the `run_agent_turn(...)` call (`:191-204`) with:

```rust
    let produced = run_agent_turn_cancellable(
        config,
        provider,
        tools,
        gate,
        session,
        crate::eventlog::EventLog::from_vec(vec![seed]),
        model,
        ui,
        session_id,
        companion_hub,
        now,
        cancel,
        hard,
    )
    .await?;
```

- [ ] **Step 4: Update the existing `run_subagent` test.** In `crates/zoid/src/subagent.rs`, the `subagent_runs_constructed_task_and_returns_summary` test (`:460-476`) calls `run_subagent(...)` with 13 args. Add the three new args before `.await`:

```rust
        let res = run_subagent(
            "refactor parse()",
            &crate::eventlog::EventLog::new(),
            &AgentProfile::builtin(),
            provider,
            std::path::PathBuf::from("."),
            "glm".into(),
            zoid_provider::ThinkingMode::Off,
            session.clone(),
            Ulid::new(),
            tx,
            || 0,
            "sub-test".into(),
            zoid_core::config::ApprovalConfig::default(),
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        )
        .await
        .unwrap();
```

- [ ] **Step 4b: Update the three integration-test call sites.** `run_subagent` also has THREE callers in the `tests/` directory (separate compilation targets built by `cargo test -p zoid`). Each passes 13 args and will fail to compile (`E0061`) unless updated with the same three trailing args. In each, insert the three args after the `zoid_core::config::ApprovalConfig::default(),` line and before the closing `)`:

```rust
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
```

The three sites (match the indentation at each — `subagent_integration.rs:92` is inside a block so indented one level deeper):
- `crates/zoid/tests/subagent_integration.rs:92` (`let res = run_subagent(` …)
- `crates/zoid/tests/delegation_integration.rs:43` (`let res = run_subagent(` …)
- `crates/zoid/tests/delegation_integration.rs:115` (`let _res = run_subagent(` …)

> These three sites do NOT construct `App`, so Task 3's `HashSet→HashMap` change did not touch them — this Task-4 signature change is the only reason they need updating. Without this step, Step 9 (`cargo test -p zoid`) and the Final Verification both fail to compile.

- [ ] **Step 5: Rewrite `spawn_subagent`.** Replace the whole `spawn_subagent` fn in `crates/zoid/src/spawn_subagent.rs` (`:38-119`) with:

```rust
/// Build a summary line for an aborted (killed / timed-out) subagent.
pub(crate) fn abort_summary(reason: Option<crate::agent::AbortReason>) -> String {
    match reason {
        Some(r) => format!("aborted ({})", r.label()),
        None => "aborted".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_subagent(
    task: String,
    seed: crate::eventlog::EventLog,
    provider: Arc<dyn Provider>,
    cwd: PathBuf,
    model: String,
    thinking: zoid_provider::ThinkingMode,
    session: SessionHandle,
    session_id: Ulid,
    ui: mpsc::Sender<AgentUpdate>,
    now: fn() -> i64,
    sub_id: String,
    wt: Option<crate::worktree::WorktreeGuard>,
    approval: zoid_core::config::ApprovalConfig,
    cancel: tokio_util::sync::CancellationToken,
    hard: tokio_util::sync::CancellationToken,
    progress: std::sync::Arc<std::sync::atomic::AtomicI64>,
    abort_reason: std::sync::Arc<std::sync::Mutex<Option<crate::agent::AbortReason>>>,
    idle: Option<std::time::Duration>,
    ceiling: Option<std::time::Duration>,
) {
    tokio::spawn(async move {
        // Supervisor: trips `hard` (with a reason) on idle/ceiling breach. `done`
        // stops it on normal completion so nothing is left spinning.
        let done = tokio_util::sync::CancellationToken::new();
        let _timer = crate::wake_timer::WakeTimer::spawn(
            idle,
            ceiling,
            progress.clone(),
            now,
            crate::agent::AbortReason::IdleTimeout,
            crate::agent::AbortReason::Ceiling,
            abort_reason.clone(),
            hard.clone(),
            done.clone(),
        );

        let res = crate::subagent::run_subagent(
            &task,
            &seed,
            &AgentProfile::builtin(),
            provider,
            cwd,
            model,
            thinking,
            session.clone(),
            session_id,
            ui.clone(),
            now,
            sub_id.clone(),
            approval,
            cancel,
            hard.clone(),
            progress,
        )
        .await;

        // Stop the supervisor now that the run has returned.
        done.cancel();

        // If a firer tripped `hard`, force the failure branch regardless of what
        // the drained turn returned: label it with the abort reason and discard
        // the worktree (partial work is not kept).
        let res = if hard.is_cancelled() {
            let reason = *abort_reason.lock().unwrap();
            Err(anyhow::anyhow!(abort_summary(reason)))
        } else {
            res
        };

        // Commit the subagent's working-tree changes on the success path, then
        // retain the branch for subagent_diff retrieval. On error (incl. abort),
        // drop the guard (full cleanup discards partial work).
        match &res {
            Ok(_) => {
                if let Some(wt) = &wt {
                    let _ = std::process::Command::new("git")
                        .args(["-C"])
                        .arg(wt.path())
                        .args(["add", "-A"])
                        .output();
                    let _ = std::process::Command::new("git")
                        .args(["-C"])
                        .arg(wt.path())
                        .args(["commit", "-m", &format!("subagent {sub_id}")])
                        .output();
                }
                if let Some(wt) = wt {
                    let _ = wt.into_kept_branch();
                }
            }
            Err(_) => {
                drop(wt);
            }
        }

        let (subagent_id, branch, summary, ok) = delegation_fields(res, &sub_id);
        let ev = Event::new(
            Ulid::new(),
            None,
            now(),
            EventKind::DelegationResult {
                subagent_id,
                branch,
                summary,
                ok,
            },
        )
        .with_session(session_id);
        let _ = session.append(ev.clone()).await;
        let _ = ui.send(AgentUpdate::Appended(Box::new(ev))).await;
    });
}
```

- [ ] **Step 6: Wire the dispatch call in `agent.rs`.** In `crates/zoid/src/agent.rs`, replace the `spawn_subagent(...)` call (`:1409-1423`) with (removing any leading underscores added in Task 3 Step 9):

```rust
                    crate::spawn_subagent::spawn_subagent(
                        task,
                        events.snapshot(),
                        provider.clone(),
                        cwd,
                        model.clone(),
                        config.thinking.clone(),
                        session.clone(),
                        session_id,
                        ui.clone(),
                        now,
                        sub_id.clone(),
                        wt,
                        config.approval.clone(),
                        sub_cancel,
                        sub_hard,
                        sub_progress,
                        sub_abort_reason,
                        config.subagent_idle,
                        config.subagent_ceiling,
                    );
```

- [ ] **Step 7: Thread the config timeouts into the chat turn.** In `crates/zoid/src/main.rs` `spawn_turn`, immediately after `turn_config.in_flight = Some(app.in_flight.clone());` (`:5425`) add:

```rust
    // Subagent guardrail timeouts (0 = disabled → None). Only the chat turn
    // dispatches subagents, so only it carries these.
    turn_config.subagent_idle = (app.config.subagent.idle_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(app.config.subagent.idle_timeout_secs));
    turn_config.subagent_ceiling = (app.config.subagent.hard_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(app.config.subagent.hard_timeout_secs));
```

- [ ] **Step 8: Run to verify the new test passes.**

Run: `cargo test -p zoid --lib abort_summary`
Expected: PASS.

- [ ] **Step 9: Run the touched crates.**

Run: `cargo test -p zoid`
Expected: PASS — `subagent_runs_constructed_task_and_returns_summary` (now 16 args) and all guardrail tests green.

- [ ] **Step 10: Commit.**

```bash
git add crates/zoid/src/subagent.rs crates/zoid/src/spawn_subagent.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(subagent): wire real tokens + WakeTimer; abort forces failure DelegationResult"
```

---

### Task 5: `cancel_subagent` Emitting tool + kill handler

**Files:**
- Create: `crates/zoid-tools/src/subagent_kill.rs`
- Modify: `crates/zoid-tools/src/lib.rs` (add `pub mod subagent_kill;`; NOT in `registry()`/`registry_with_kill()`)
- Modify: `crates/zoid/src/invoke_skill.rs` (`chat_tools`, after `:100`)
- Modify: `crates/zoid/src/agent.rs` (add `fire_subagent_kill` helper + a `cancel_subagent` Emitting match arm after the `exit_worktree` arm, ~`:1502-1525`)

**Interfaces:**
- Consumes: `SubagentHandle`, `AbortReason` (Task 3).
- Produces: `zoid_tools::subagent_kill::CancelSubagent` (Emitting tool `cancel_subagent { id: Option<String> }`); `zoid::agent::fire_subagent_kill(reg: &Arc<Mutex<HashMap<String, SubagentHandle>>>, target: Option<&str>) -> usize` (sets `Killed` first-writer-wins + fires `hard`; returns count).

- [ ] **Step 1: Write the failing tool test.** Create `crates/zoid-tools/src/subagent_kill.rs`:

```rust
use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

/// `cancel_subagent { id? }` — an Emitting tool the main Chat agent uses to abort
/// a dispatched subagent. `Some(id)` kills one; omitted kills all in-flight.
/// The agent loop performs the cancel against its shared subagent registry; this
/// stub only advertises the tool (its `run` is never called).
pub struct CancelSubagent;

impl Tool for CancelSubagent {
    fn name(&self) -> &str {
        "cancel_subagent"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "cancel_subagent".into(),
            description: "Cancel a dispatched subagent. Pass `id` to cancel one \
                          specific subagent, or omit it to cancel all in-flight \
                          subagents. Aborted subagents report a failure result and \
                          their worktree is discarded."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The subagent id (e.g. sub-01H…) to cancel. Omit to cancel all."
                    }
                },
                "required": []
            }),
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Emitting
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the agent loop branches on Emitting before calling run().
        ToolOutput::err("cancel_subagent is executed by the agent loop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_kind() {
        assert_eq!(CancelSubagent.name(), "cancel_subagent");
        assert_eq!(CancelSubagent.kind(), ToolKind::Emitting);
    }

    #[test]
    fn id_is_optional() {
        let spec = CancelSubagent.spec();
        assert_eq!(spec.name, "cancel_subagent");
        let required = spec.parameters["required"].as_array().unwrap();
        assert!(required.is_empty(), "id must be optional (omit = kill all)");
    }

    #[test]
    fn not_in_base_registry() {
        // Subagents must NOT be able to cancel their siblings.
        assert!(
            !crate::registry().iter().any(|t| t.name() == "cancel_subagent"),
            "cancel_subagent must be chat-only, never in the subagent registry"
        );
    }
}
```

- [ ] **Step 2: Register the module (NOT in `registry()`).** In `crates/zoid-tools/src/lib.rs`, add near the other `pub mod` declarations (e.g. beside `pub mod worktree_enter;` — `grep -n "pub mod worktree_enter" crates/zoid-tools/src/lib.rs`):

```rust
pub mod subagent_kill;
```

Do NOT add `CancelSubagent` to `registry()` (`:114-129`) or `registry_with_kill()` (`:134-149`).

- [ ] **Step 3: Run to verify the tool test passes / registry test guards.**

Run: `cargo test -p zoid-tools subagent_kill`
Expected: PASS — `name_and_kind`, `id_is_optional`, `not_in_base_registry`.

- [ ] **Step 4: Write the failing handler-helper test.** In `crates/zoid/src/agent.rs`, extend `guardrail_types_tests` (from Task 3) with:

```rust
    #[test]
    fn fire_kill_targets_one_or_all() {
        use super::fire_subagent_kill;
        use std::collections::HashMap;

        let mk = || SubagentHandle {
            cancel: CancellationToken::new(),
            hard: CancellationToken::new(),
            progress: Arc::new(AtomicI64::new(0)),
            abort_reason: Arc::new(Mutex::new(None)),
        };
        let a = mk();
        let b = mk();
        let mut map = HashMap::new();
        map.insert("sub-a".to_string(), a.clone());
        map.insert("sub-b".to_string(), b.clone());
        let reg = Arc::new(Mutex::new(map));

        // Target one.
        let n = fire_subagent_kill(&reg, Some("sub-a"));
        assert_eq!(n, 1);
        assert!(a.hard.is_cancelled());
        assert_eq!(*a.abort_reason.lock().unwrap(), Some(super::AbortReason::Killed));
        assert!(!b.hard.is_cancelled(), "untargeted subagent untouched");

        // Target all.
        let n = fire_subagent_kill(&reg, None);
        assert_eq!(n, 2, "None fires every registered subagent");
        assert!(b.hard.is_cancelled());
    }
```

- [ ] **Step 5: Run to verify it fails.**

Run: `cargo test -p zoid --lib fire_kill_targets_one_or_all`
Expected: compile error — `fire_subagent_kill` undefined.

- [ ] **Step 6: Add the `fire_subagent_kill` helper.** In `crates/zoid/src/agent.rs`, right after the `SubagentHandle` struct (added in Task 3), add:

```rust
/// Fire the `hard` token (and record `Killed`, first-writer-wins) for one
/// subagent by id, or for ALL in-flight subagents when `target` is `None`.
/// Returns how many handles were fired. Shared by the `cancel_subagent` tool
/// handler and the Esc escalation path.
pub fn fire_subagent_kill(
    reg: &std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SubagentHandle>>>,
    target: Option<&str>,
) -> usize {
    let reg = reg.lock().unwrap();
    let handles: Vec<&SubagentHandle> = match target {
        Some(id) => reg.get(id).into_iter().collect(),
        None => reg.values().collect(),
    };
    let mut fired = 0usize;
    for h in handles {
        {
            let mut slot = h.abort_reason.lock().unwrap();
            if slot.is_none() {
                *slot = Some(AbortReason::Killed);
            }
        }
        h.hard.cancel();
        fired += 1;
    }
    fired
}
```

- [ ] **Step 7: Add the Emitting match arm.** In `crates/zoid/src/agent.rs`, after the `exit_worktree` Emitting arm ends (after `:1525`, i.e. after its closing `}`), add a new arm:

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "cancel_subagent" => {
                    let target = tc
                        .args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let fired = if let Some(reg) = &config.in_flight {
                        fire_subagent_kill(reg, target.as_deref())
                    } else {
                        0
                    };
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: format!("{{\"cancelled\": {fired}}}"),
                            is_error: false,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                }
```

> The aborted subagent's registry entry is NOT removed here — it is cleared when the forced-failure `DelegationResult` is consumed (`main.rs:2664-2667`), guaranteeing the drawer row clears exactly once.

- [ ] **Step 8: Register the tool in `chat_tools`.** In `crates/zoid/src/invoke_skill.rs`, after `tools.push(Box::new(zoid_tools::subagent_diff::SubagentDiff));` (`:100`) add:

```rust
    // Orchestrator kill switch: cancel a dispatched subagent by id, or all.
    // Chat-only (needs the shared registry); never in the subagent registry so
    // a subagent can't cancel its siblings.
    tools.push(Box::new(zoid_tools::subagent_kill::CancelSubagent));
```

- [ ] **Step 9: Run the handler + tool tests.**

Run: `cargo test -p zoid --lib fire_kill_targets_one_or_all`
Expected: PASS.

Run: `cargo test -p zoid -p zoid-tools`
Expected: PASS.

- [ ] **Step 10: Commit.**

```bash
git add crates/zoid-tools/src/subagent_kill.rs crates/zoid-tools/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/invoke_skill.rs
git commit -m "feat(subagent): cancel_subagent kill tool + fire_subagent_kill handler"
```

---

### Task 6: Esc — unified escalation + no-turn armed confirm

**Files:**
- Modify: `crates/zoid/src/main.rs`
  - `App` struct: add `subagent_kill_armed: bool` (near `:1595`, after `turn_hard`)
  - both `App` constructors: init `subagent_kill_armed: false` (near `:2086` and `:6244`)
  - `escalate_cancel` (`:4369-4380`) + its unit-test callers (`:5579`, `:5584`)
  - `Action::CancelTurn` handler (`:3591-3598`)
  - DelegationResult consumer (`:2664-2668`): disarm when the registry empties

**Interfaces:**
- Consumes: `zoid::agent::fire_subagent_kill` (Task 5), `App.in_flight` map (Task 3).
- Produces: no new public API; behavior only.

- [ ] **Step 1: Write the failing test (escalation fires subagents).** In `crates/zoid/src/main.rs`, near the existing `escalate_cancel` tests (`:5579-5584`), add:

```rust
    #[test]
    fn escalate_force_fires_registered_subagents() {
        use std::collections::HashMap;
        let graceful = tokio_util::sync::CancellationToken::new();
        let hard = tokio_util::sync::CancellationToken::new();
        let sub = zoid::agent::SubagentHandle {
            cancel: tokio_util::sync::CancellationToken::new(),
            hard: tokio_util::sync::CancellationToken::new(),
            progress: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
            abort_reason: std::sync::Arc::new(std::sync::Mutex::new(None)),
        };
        let mut map = HashMap::new();
        map.insert("sub-x".to_string(), sub.clone());
        let reg = std::sync::Arc::new(std::sync::Mutex::new(map));

        // First press: graceful only — subagents untouched.
        let _ = escalate_cancel(&graceful, &hard, &reg);
        assert!(!sub.hard.is_cancelled(), "first Esc must not kill subagents");

        // Second press: force — every registered subagent's hard fires with Killed.
        let hint = escalate_cancel(&graceful, &hard, &reg);
        assert_eq!(hint, "force-stopping…");
        assert!(sub.hard.is_cancelled(), "force Esc kills in-flight subagents");
        assert_eq!(
            *sub.abort_reason.lock().unwrap(),
            Some(zoid::agent::AbortReason::Killed)
        );
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -p zoid escalate_force_fires_registered_subagents`
Expected: compile error — `escalate_cancel` takes 2 args, not 3.

- [ ] **Step 3: Extend `escalate_cancel`.** In `crates/zoid/src/main.rs`, replace `escalate_cancel` (`:4369-4380`) with:

```rust
fn escalate_cancel(
    graceful: &tokio_util::sync::CancellationToken,
    hard: &tokio_util::sync::CancellationToken,
    subagents: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, zoid::agent::SubagentHandle>>,
    >,
) -> &'static str {
    if graceful.is_cancelled() {
        hard.cancel();
        // The force press also kills every in-flight subagent (reason Killed).
        zoid::agent::fire_subagent_kill(subagents, None);
        "force-stopping…"
    } else {
        graceful.cancel();
        "cancelling… (Esc again to force)"
    }
}
```

- [ ] **Step 4: Fix the existing `escalate_cancel` unit-test callers.** In `crates/zoid/src/main.rs` (`:5575-5585` region), the two existing calls `escalate_cancel(&graceful, &hard)` must pass an empty registry. Update that test to build one and pass it:

```rust
        let reg = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::<String, zoid::agent::SubagentHandle>::new(),
        ));
        assert_eq!(
            escalate_cancel(&graceful, &hard, &reg),
            "cancelling… (Esc again to force)"
        );
        assert_eq!(escalate_cancel(&graceful, &hard, &reg), "force-stopping…");
```

- [ ] **Step 5: Add the `subagent_kill_armed` field + inits.** In the `App` struct after `turn_hard` (`:1595`) add:

```rust
    /// Armed state for the no-active-turn subagent kill confirm: first Esc arms,
    /// second Esc fires all in-flight subagents. Reset when the registry empties
    /// or a turn's tokens take over the escalation.
    subagent_kill_armed: bool,
```

In BOTH `App` constructors, after `turn_hard: None,` (`:2086` and `:6244`) add:

```rust
        subagent_kill_armed: false,
```

- [ ] **Step 6a: Add the pure armed-confirm decision helper.** In `crates/zoid/src/main.rs`, next to `escalate_cancel` (after its closing `}`, ~`:4381`), add a pure function so the no-active-turn two-press flow is testable without a full `App`:

```rust
/// Pure decision for the no-active-turn subagent-kill escalation. Given the
/// current armed flag and how many subagents are in flight, returns
/// `(next_armed, should_fire, status_hint)`. First press arms (no fire); second
/// press fires all and disarms. Kept pure so the transition is unit-testable;
/// the caller performs the actual `fire_subagent_kill` when `should_fire`.
fn subagent_kill_decision(armed: bool, pending: usize) -> (bool, bool, String) {
    if armed {
        (false, true, format!("killing {pending} subagent(s)…"))
    } else {
        (true, false, format!("kill {pending} subagent(s)? Esc again to confirm"))
    }
}
```

- [ ] **Step 6b: Rewrite the `Action::CancelTurn` handler.** In `crates/zoid/src/main.rs`, replace the `Action::CancelTurn` arm (`:3591-3598`) with:

```rust
        Action::CancelTurn => {
            // First Esc: graceful (finish current step, drain, end). Second Esc
            // while already cancelling: hard-stop — force-kill the running tool AND
            // every in-flight subagent. The resulting TurnComplete clears both tokens.
            if let (Some(g), Some(h)) = (&app.turn_cancel, &app.turn_hard) {
                app.shell.status_hint = Some(escalate_cancel(g, h, &app.in_flight).into());
            } else {
                // No active main turn, but subagents may be running: two-press confirm.
                let pending = app.in_flight.lock().unwrap().len();
                if pending > 0 {
                    let (next_armed, fire, hint) =
                        subagent_kill_decision(app.subagent_kill_armed, pending);
                    if fire {
                        zoid::agent::fire_subagent_kill(&app.in_flight, None);
                    }
                    app.subagent_kill_armed = next_armed;
                    app.shell.status_hint = Some(hint);
                }
            }
        }
```

- [ ] **Step 7: Disarm when the registry empties.** In the DelegationResult consumer (`:2664-2668`), after `app.in_flight.lock().unwrap().remove(subagent_id);` add:

```rust
                            if app.in_flight.lock().unwrap().is_empty() {
                                app.subagent_kill_armed = false;
                            }
```

- [ ] **Step 8: Write the failing armed-confirm test.** This tests the REAL decision helper `subagent_kill_decision` (the pure core of the no-active-turn branch from Step 6b) — not a re-implementation. Add to `crates/zoid/src/main.rs` tests (near the `escalate_cancel` tests, `:5579`):

```rust
    #[test]
    fn subagent_kill_decision_arms_then_fires() {
        // First press (disarmed): arms, does NOT fire, prompts for confirm.
        let (next, fire, hint) = super::subagent_kill_decision(false, 3);
        assert!(next, "first press arms");
        assert!(!fire, "first press must not fire");
        assert!(hint.contains("Esc again"), "first press asks to confirm: {hint}");

        // Second press (armed): fires, disarms, reports the kill.
        let (next, fire, hint) = super::subagent_kill_decision(true, 3);
        assert!(!next, "second press disarms");
        assert!(fire, "second press fires");
        assert!(hint.contains("killing"), "second press reports the kill: {hint}");
    }
```

> The actual firing (`fire_subagent_kill`) is already covered by `fire_kill_targets_one_or_all` (Task 5); this test pins the two-press *decision*, which is the only logic unique to the no-active-turn Esc path. (`super::` because the helper is a free fn in `main.rs`; adjust the path if your test module nests differently.)

- [ ] **Step 9: Run to verify all Task 6 tests pass.**

Run: `cargo test -p zoid escalate_force_fires_registered_subagents subagent_kill_decision_arms_then_fires`
Expected: PASS.

- [ ] **Step 10: Run the whole crate.**

Run: `cargo test -p zoid`
Expected: PASS — updated `escalate_cancel` tests + all guardrail tests green.

- [ ] **Step 11: Commit.**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(subagent): Esc escalation kills subagents + no-turn armed confirm"
```

---

## Final Verification

- [ ] Run the full workspace test suite:

Run: `cargo test -p zoid -p zoid-core -p zoid-tools`
Expected: PASS across all touched crates.

- [ ] Sanity build (catches unused-variable / warnings if you build with `-D warnings`):

Run: `cargo build -p zoid`
Expected: clean build.

---

## Self-Review Notes (author checklist — already applied)

- **Spec coverage:** WakeTimer (Task 1); `[subagent]` config (Task 2); registry `SubagentHandle`/`AbortReason` + heartbeat + HashSet→HashMap (Task 3); real tokens + supervisor + abort→failure `DelegationResult` + worktree drop (Task 4); `cancel_subagent` chat-only Emitting tool + handler (Task 5); Esc unified escalation + no-turn armed confirm (Task 6). Iteration cap 25 untouched. No new `EventKind`/DB.
- **Timeouts disabled path:** `idle=0 && ceiling=0` → `spawn_turn` sets both to `None` → `WakeTimer::spawn` returns a finished task, but the handle is still registered so the kill tool + Esc keep working (Task 4 Step 7 + Task 1 early return).
- **First-writer-wins reason:** enforced identically in `WakeTimer` (Task 1) and `fire_subagent_kill` (Task 5).
- **No placeholders:** every code step is compile-ready; no TODO/TBD.
- **Armed-confirm is tested for real:** the no-active-turn two-press flow is extracted into the pure `subagent_kill_decision` helper (Task 6 Step 6a) and unit-tested (Step 8) — the test exercises the shipped decision, not a copy.
- **KNOWN GAP (for the final whole-branch review):** the spec's end-to-end Testing item ("dispatch a subagent running a sleeping tool → idle timeout kills it → a failure `DelegationResult` arrives") is covered only at the *unit* level here (`WakeTimer` breach tests + `abort_summary` + the forced-failure branch in Task 4). A true full-loop integration test needs a `FakeProvider` that never emits `Done` plus `idle=Some(short)`, asserting the emitted `DelegationResult { ok:false }`. Deferred rather than under-specified: the existing suite exercises `spawn_subagent`/`run_turn_inner` only via the single `run_subagent` happy-path async test, so the harness shape must be confirmed against that test before writing it. Add it as an extra Task 4 step if the reviewer wants the integration guarantee.
