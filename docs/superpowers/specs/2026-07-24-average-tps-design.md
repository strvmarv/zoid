# Average TPS in Session Widget — Design

> **Status:** DESIGN (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** UI polish — the right-rail session widget's TPS value.

---

## 1. Goal & scope

Change the TPS (tokens-per-second) value in the right-rail session widget
from a hybrid last-turn-tokens / average-duration formula to a rolling
average of per-turn TPS.

**Current behavior:** `last_output_tokens * 1000 / provider_total.avg()`
— divides the *last* turn's output tokens by the *rolling average* stream
duration. This is inconsistent: a single slow turn inflates the denominator
while the numerator reflects only the most recent turn, producing misleading
values (e.g., a fast turn after a slow one shows high TPS; a slow turn after
fast ones shows low TPS — neither reflects the actual rolling average rate).

**New behavior:** `provider_tps.avg()` — the rolling average of per-turn TPS
values, where each turn's TPS is `output_tokens * 1000 / stream_ms`. Each
turn contributes equally to the average, regardless of its duration.

**In scope:**
- A new `provider_tps: RollingStats` field on `ObsState`.
- Recording per-turn TPS at `TurnComplete` (where both `last_output_tokens`
  and `provider_total.last()` are available).
- Changing the TPS display computation from the hybrid formula to
  `provider_tps.avg()`.

**Out of scope:**
- Weighted average (weighting by output tokens or duration). Equal-weight
  per-turn average is simpler and sufficient for a UI widget.
- Changing the `RollingStats` window size (`ROLL_CAP`).
- The Overview page's `ttft_ms` / `total_ms` displays (those already use
  rolling averages and are unaffected).

---

## 2. Data model

### 2.1 `ObsState` gains `provider_tps: RollingStats` (obs.rs)

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

`RollingStats` is `#[derive(Debug, Default)]` — `Default` yields an empty
window, so `ObsState::default()` (which derives `Default`) produces an empty
`provider_tps` with `avg() == 0`. No manual initialization needed.

`provider_tps` is NOT recorded from the tracing layer — it's recorded from
the `TurnComplete` handler in main.rs, where both the output token count and
the stream duration are available. The tracing `record_provider` path only
has `ttft_ms` and `total_ms`, not the token count.

### 2.2 Recording at `TurnComplete` (main.rs:2984)

After the turn completes and the projection is refreshed, compute and
record the per-turn TPS:

```rust
AgentUpdate::TurnComplete => {
    // ... existing code ...

    // Record per-turn TPS for the rolling average (session widget).
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

Uses `provider_total.last()` (the stream duration of the just-finished turn),
not `provider_total.avg()` — we want the per-turn duration, not the rolling
average, so each TPS sample is the actual rate for that specific turn.

Guards: `stream_ms > 0` (avoid division by zero), `output_tokens > 0`
(implicit — if 0, TPS is 0, which is harmless to record but adds noise;
the `checked_div` returns 0 either way). The `obs_state.lock()` is scoped
and dropped before any `.await`.

### 2.3 Display computation (main.rs:2532)

Change:

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

The `stream_ms` local and `last_output_tokens` read are no longer needed at
this site — they're consumed at `TurnComplete` recording time instead. The
frame render tick just reads the rolling average.

---

## 3. Edge cases

- **First turn:** `provider_tps` is empty → `avg() == 0` → TPS shows 0.
  Same as today (the hybrid formula also returns 0 when `stream_ms == 0`).
- **Zero-output turn (e.g., context-length error):** `output_tokens == 0` →
  TPS sample is 0 → recorded. The rolling average dips. This is correct — a
  zero-output turn did produce 0 tokens/sec.
- **Zero-duration turn (impossible in practice, but defensive):**
  `stream_ms == 0` → the `stream_ms > 0` guard skips recording. No division
  by zero, no spurious infinite TPS sample.
- **Subagent turns:** Subagent turns emit their own `TurnComplete`. The
  `provider_total.last()` at that point is the subagent's stream duration,
  and `app.proj.last_output_tokens` is the subagent's output. This is
  correct — subagent turns are provider turns too, and their TPS should
  contribute to the rolling average. (If this is undesirable, gate the
  recording on `app.in_flight_subagents.is_empty()`, but that would miss
  legitimate subagent work.)
- **Rolling window eviction:** `RollingStats` evicts the oldest sample when
  the window is full (`ROLL_CAP` samples). The average naturally forgets old
  turns. Same behavior as `provider_total` today.

---

## 4. Cross-crate impact

- **`obs.rs` (zoid)** — `ObsState` gains `provider_tps: RollingStats`. No
  new method on `ObsState` — the recording happens at the call site
  (`s.provider_tps.record(tps)`), not through a `record_*` helper (the
  helper would need both `output_tokens` and `stream_ms`, but those come
  from different sources — the projection and the obs layer — so the
  call site composes them).
- **`main.rs` (zoid)** — `TurnComplete` handler gains the TPS recording
  block. The frame-render TPS computation (line 2532) changes to read
  `provider_tps.avg()`. The `stream_ms` local and `last_output_tokens`
  read at that site are removed.
- **`state.rs` (zoid-tui)** — `ShellState.tps: u64` is unchanged. The
  field type and rendering are the same; only the value source changes.
- **`render.rs` (zoid-tui)** — unchanged. The `format!("{}", state.tps)`
  display is the same.
- `cargo build --workspace && cargo test --workspace` after the change.

---

## 5. Testing

- **`ObsState` default:** `ObsState::default().provider_tps.avg() == 0`
  (empty window).
- **Recording + average:** create an `ObsState`, record TPS samples
  (100, 200, 300), assert `avg() == 200`.
- **Rolling eviction:** record `ROLL_CAP + 1` samples, assert the oldest
  is evicted (the average reflects only the last `ROLL_CAP` samples).
- **`TurnComplete` integration:** verify that after a turn with known
  output tokens and stream duration, `provider_tps.avg()` reflects the
  per-turn TPS. (This may be hard to test in isolation — the `TurnComplete`
  handler is in main.rs's event loop. A pragmatic approach: test the
  recording logic via a unit test that calls `s.provider_tps.record(tps)`
  directly, and verify the display reads `provider_tps.avg()`.)