# Average TPS in Session Widget — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Change the right-rail session widget's TPS from a hybrid
last-turn-tokens / average-duration formula to a rolling average of
per-turn TPS values.

**Architecture:** Add a `provider_tps: RollingStats` field to `ObsState`.
Record per-turn TPS at `TurnComplete` (where both `last_output_tokens`
and `provider_total.last()` are available). The frame-render TPS
computation reads `provider_tps.avg()` instead of the hybrid formula.

**Tech Stack:** Rust workspace (`zoid` binary). `RollingStats` is the
existing rolling-window statistics type in `obs.rs`.

**Spec:** `docs/superpowers/specs/2026-07-24-average-tps-design.md`

## Global Constraints

- **`RollingStats` is `#[derive(Debug, Default)]`** — adding a
  `RollingStats` field to `ObsState` (which also derives `Default`) requires
  no manual initialization. `Default::default()` yields an empty window
  with `avg() == 0`.
- **No lock held across `.await`** — the `obs_state.lock()` in the
  `TurnComplete` handler must be scoped and dropped before any `.await`.
  The existing code at line 2527 uses the same pattern.
- **`obs_state` is available** in the `run` function scope (passed as
  `&Arc<Mutex<ObsState>>` at line 2350). The `TurnComplete` handler is
  inside this function.
- **No co-author trailer** in commits (repo `AGENTS.md`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid/src/obs.rs` | `ObsState` gains `provider_tps: RollingStats` | Modify |
| `crates/zoid/src/main.rs` | Record per-turn TPS at `TurnComplete`; change display computation | Modify |

**Task order:** T1 (add field + recording + display change) — single task,
all changes are interdependent. One commit.

---

### Task 1: `provider_tps` field + `TurnComplete` recording + display change

**Files:**
- Modify: `crates/zoid/src/obs.rs`
- Modify: `crates/zoid/src/main.rs`

- [ ] **Step 1: Add `provider_tps: RollingStats` to `ObsState`**

In `crates/zoid/src/obs.rs`, at the `ObsState` struct (line 34), add
`provider_tps` after `provider_total` (line 39):

```rust
pub struct ObsState {
    pub turn: RollingStats,
    pub iterations: RollingStats,
    pub provider_ttft: RollingStats,
    pub provider_total: RollingStats,
    pub provider_tps: RollingStats,  // NEW — per-turn TPS rolling average
    pub frame: RollingStats,
    // ... rest unchanged
}
```

`ObsState` derives `Default` (line 34: `#[derive(Debug, Default)]`), and
`RollingStats` derives `Default`, so no manual init is needed.

- [ ] **Step 2: Record per-turn TPS at `TurnComplete`**

In `crates/zoid/src/main.rs`, in the `AgentUpdate::TurnComplete` handler
(line 2984), after the `app.shell.status_hint = None;` line (2993) and
before the `if app.in_flight_subagents.is_empty()` block (2995), add:

```rust
                        // Record per-turn TPS for the rolling average
                        // (session widget). Both values are available now:
                        // the turn is done, the Usage event is in the log,
                        // and provider_total.last() is this turn's stream ms.
                        {
                            let stream_ms = obs_state
                                .lock()
                                .ok()
                                .map(|s| s.provider_total.last())
                                .unwrap_or(0);
                            if stream_ms > 0 {
                                let output_tokens = app
                                    .proj
                                    .last_output_tokens
                                    .unwrap_or(0);
                                if let Ok(mut s) = obs_state.lock() {
                                    let tps = output_tokens
                                        .checked_mul(1000)
                                        .and_then(|t| t.checked_div(stream_ms))
                                        .unwrap_or(0);
                                    s.provider_tps.record(tps);
                                }
                            }
                        }
```

Note: the block is scoped with `{ }` so the `MutexGuard` from the first
`obs_state.lock()` drops before the second `obs_state.lock()`. No `.await`
inside the block — the lock is held only for synchronous reads/writes.

- [ ] **Step 3: Change the TPS display computation**

In `crates/zoid/src/main.rs`, at line 2527-2534, change:

```rust
        let stream_ms = obs_state
            .lock()
            .ok()
            .map(|s| s.provider_total.avg())
            .unwrap_or(0);
        app.shell.tps = (app.proj.last_output_tokens.unwrap_or(0) * 1000)
            .checked_div(stream_ms)
            .unwrap_or(0);
```

to:

```rust
        app.shell.tps = obs_state
            .lock()
            .ok()
            .map(|s| s.provider_tps.avg())
            .unwrap_or(0);
```

The `stream_ms` local and `last_output_tokens` read are no longer needed
at this site — they're consumed at `TurnComplete` recording time.

- [ ] **Step 4: Write tests**

In `crates/zoid/src/obs.rs`, add tests in the test module (at the end of
the file):

```rust
    #[test]
    fn provider_tps_default_is_zero() {
        let s = ObsState::default();
        assert_eq!(s.provider_tps.avg(), 0, "empty window → avg 0");
    }

    #[test]
    fn provider_tps_records_and_averages() {
        let mut s = ObsState::default();
        s.provider_tps.record(100);
        s.provider_tps.record(200);
        s.provider_tps.record(300);
        assert_eq!(s.provider_tps.avg(), 200, "(100+200+300)/3 = 200");
    }

    #[test]
    fn provider_tps_rolling_eviction() {
        let mut s = ObsState::default();
        // Fill the window with 100s.
        for _ in 0..ROLL_CAP {
            s.provider_tps.record(100);
        }
        // Push one more — the oldest 100 is evicted.
        s.provider_tps.record(200);
        // Window is now {100 × 63, 200 × 1} → avg = (63*100 + 200) / 64
        let expected = (63 * 100 + 200) / ROLL_CAP as u64;
        assert_eq!(s.provider_tps.avg(), expected, "oldest sample evicted");
    }
```

- [ ] **Step 5: Build and test**

Run: `cargo build --workspace`
Expected: success.

Run: `cargo test -p zoid -- provider_tps`
Expected: PASS (3 tests: default, records_and_averages, rolling_eviction).

Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success — full release gate. No regressions (the TPS display
value changes from a hybrid to a rolling average, but existing tests
don't assert specific TPS values).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/obs.rs crates/zoid/src/main.rs
git commit -m "feat(ui): average TPS in session widget — rolling per-turn TPS

Change the right-rail TPS from a hybrid last-tokens / avg-duration
formula to a rolling average of per-turn TPS values. ObsState gains
provider_tps: RollingStats. Record per-turn TPS at TurnComplete
(output_tokens * 1000 / provider_total.last()). Display reads
provider_tps.avg() instead of the hybrid formula. Each turn
contributes equally to the average regardless of duration."
```

---

## Self-Review

Run after the task: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
(AGENTS.md release gate). Confirm:
- `provider_tps` default is 0 (empty window).
- Recording samples and averaging works.
- Rolling eviction works (oldest sample drops at `ROLL_CAP`).
- The `TurnComplete` recording block has no `.await` inside the lock scope.
- The display reads `provider_tps.avg()` (not the hybrid formula).
- No regressions in existing tests.