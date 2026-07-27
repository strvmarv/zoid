# Token-Budgeted Turn Protection — Design

> **Date:** 2026-07-27
> **Status:** Design — pending implementation
> **Depends on:** the `scale` parameter added to `plan_evictions` (2026-07-27
> calibration-mismatch fix); the existing band derivation in `band.rs`; the
> `group_turns` protection logic in `eviction.rs`.
> **Supersedes:** the `recent_n` count as the sole protection mechanism (the
> field is kept as a deprecated alias for back-compat).

---

## 1. Problem

`recent_n` is a fixed count of protected turns. It does not scale with model
capacity or adapt to turn size. The analysis test
(`crates/zoid/tests/recent_n_analysis.rs`) shows three failure modes:

| Scenario | Per-turn | `recent_n=4` floor | % of `low_water` | Problem |
|---|---|---|---|---|
| Light turns, 1M model | ~2k | 8k | 3.3% | Needlessly stingy — model loses cheap recent context |
| Medium turns, 1M model | ~14k | 56k | 23.3% | Significant but workable |
| Heavy turns, 1M model | ~74k | 298k | 124% | **Overflows the band** — planner can't reach `low_water` |
| Medium turns, 64k model | ~14k | 56k | 125% | **Overflows the band** on small models |

A "turn" in zoid is everything from one `UserMessage` to the next — all tool
calls, file reads, assistant text, subagent results. Turns vary by 40× in
practice (2k for a quick exchange vs. 80k for a multi-file subagent turn). A
fixed count cannot serve this range.

The existing degenerate handling (spec §3.6a/§6: "shrink the effective
`recent_n` toward a floor of 1 if needed") is specified but **not implemented**
in the protection logic — `group_turns` applies `is_recent` unconditionally with
no capacity check.

## 2. Solution

Replace the `recent_n` count with a **three-layer protection policy** computed
in `group_turns` (`eviction.rs`). Protection is determined per-turn by walking
backward from the newest turn, accumulating scaled token estimates:

### 2.1 The three layers

1. **Hard floor of 1** — the current (most recent) turn is always protected.
   The model never forgets the in-flight exchange.

2. **Minimum count** (`min_protected_turns`, default 3) — protect the last N
   turns regardless of their token size. This is the **quality backstop**: the
   model remembers the last few exchanges even when turns are enormous. The
   soft band (`high_water`/`low_water`) **never overrides** this — the request
   may exceed the soft target, and that is acceptable. The band is a soft
   target for cost/latency, not a hard limit on quality.

3. **Budget ceiling** (`protection_pct` of `low_water`, default 15%) — beyond
   the minimum count, protect *additional* recent turns until their cumulative
   scaled token estimate reaches the budget. This gives the model a longer
   recent-context window when turns are small (cheap to keep), at the cost of
   slightly larger requests. Pure quality bonus — the budget extension never
   reduces protection below the minimum count.

4. **Capacity backstop** — if the protected floor (from the minimum count
   alone, *not* the budget extension) would exceed
   `capacity − CAPACITY_SAFETY_MARGIN`, shrink `min_protected_turns` toward 1.
   This prevents a provider 400 error. The hard limit is `capacity`, not the
   soft band. For the truly degenerate case (system prompt + 1 turn >
   `capacity − SAFETY_MARGIN`), no amount of eviction helps — the protection
   still floors at 1, and the pre-flight gate's hard-ceiling compaction
   (`plan_compactions_for_overflow`) handles the rest or the request 400s
   honestly.

### 2.2 Precedence

```
capacity backstop  >  minimum count  >  budget ceiling
      (hard)              (hard)            (soft)
```

- **Capacity** can shrink the minimum count (quality loses to not-400ing).
- **Minimum count** always wins over the budget ceiling (the budget only
  *extends* beyond the minimum, never restricts it).
- **Budget ceiling** is the only soft layer — it can protect additional turns
  beyond the minimum but never fewer.

### 2.3 What stays the same

- `plan_evictions` structure — the `protected` flag on `TurnView` still gates
  candidate selection. The change is *how* `protected` is computed, not the
  planner's core eviction loop.
- The `scale` parameter (2026-07-27 fix) — the cumulative token estimate for
  protection uses the same scaled per-turn estimates as eviction's `reclaimed`,
  so the budget is measured in the same units as the band.
- Re-admit cooldown — uses `min_protected_turns` instead of `recent_n` for the
  recall cooldown window (`group_turns` line 600).
- The `EvictionScorer` trait, relevance rescue, and all other eviction logic —
  untouched.

## 3. The protection algorithm

Replace the current `is_recent` computation in `group_turns`:

**Current** (`eviction.rs:592`):
```rust
let is_recent = i + recent_n >= n;
```

**New** — compute protection in a backward pass after all turns are grouped
and their `token_estimate` (scaled) is known:

```rust
// After group_turns has built all TurnViews with their scaled token estimates:
//
// 1. Compute the budget: protection_pct × low_water.
// 2. Walk backward from the newest turn (index n-1).
// 3. Protect turns until:
//    a. The minimum count is met AND cumulative tokens reach the budget, OR
//    b. Cumulative tokens approach capacity − SAFETY_MARGIN (shrink min count).
// 4. Always protect at least turn n-1 (hard floor of 1).
//
// The capacity backstop only considers min_protected_turns, not the budget
// extension — if the minimum count alone overflows capacity, shrink it.
```

Concretely:

```rust
fn compute_protection(
    turns: &[TurnView],
    min_count: usize,
    budget: u64,           // protection_pct × low_water (scaled units)
    capacity_limit: u64,   // capacity − CAPACITY_SAFETY_MARGIN (scaled units)
    scale: f64,
) -> Vec<bool> {
    let n = turns.len();
    let mut protected = vec![false; n];
    if n == 0 { return protected; }

    // Hard floor: always protect the newest turn.
    protected[n - 1] = true;

    // Walk backward, protecting turns until we hit a limit.
    let mut count = 1; // newest already protected
    let mut cumulative = (turns[n - 1].token_estimate as f64 * scale) as u64;

    for i in (0..n.saturating_sub(1)).rev() {
        let turn_tokens = (turns[i].token_estimate as f64 * scale) as u64;

        // Capacity backstop: if adding this turn would overflow capacity,
        // stop — even before the minimum count is met. A 400 is worse than
        // protecting fewer turns. The hard floor of 1 (newest turn) is
        // already protected above and is never revoked.
        if cumulative + turn_tokens > capacity_limit {
            break;
        }

        // Minimum count: protect regardless of budget until min_count is met.
        // Budget ceiling: after min_count, protect only while under budget.
        if count < min_count || cumulative + turn_tokens <= budget {
            protected[i] = true;
            count += 1;
            cumulative += turn_tokens;
        } else {
            break;
        }
    }

    protected
}
```

Note: `token_estimate` in `TurnView` is currently raw chars/3 (from
`event_tokens`). The `scale` parameter (calibration_ratio × OVERCOUNT_BIAS)
converts it to the same units as the band. This is the same `scale` already
passed to `plan_evictions` — it must be threaded into `group_turns` or
applied in the backward pass.

### 3.1 Where this runs

`group_turns` is called from `plan_evictions` (line 634). The `scale` and
band are already available there. The change is:
1. Add `scale`, `min_count`, `budget`, `capacity_limit` params to
   `group_turns` (or compute protection in a separate pass after
   `group_turns` returns).
2. Replace the `is_recent` line with a call to `compute_protection`.
3. Keep the `is_evicted` and `in_readmit_cooldown` checks as before
   (they OR into `protected`).

### 3.2 Capacity backstop detail

The capacity backstop compares the protected floor (cumulative scaled tokens
of the protected turns) against `capacity − CAPACITY_SAFETY_MARGIN`. When it
fires, it stops protecting additional turns *even if the minimum count isn't
met* — but it always keeps the hard floor of 1. This means:

- On a 128k model with 3 heavy turns (74k each): cumulative = 222k + system 7k
  = 229k > 120k (capacity limit). The backstop fires after 1 turn (81k < 120k
  ✅). The minimum count of 3 is not met, but the alternative is a 400.
- On a 256k model with the same turns: 229k < 248k ✅. No shrink — all 3
  protected. The backstop doesn't fire.

The system prompt + tool spec overhead (7k typical) is counted against the
capacity limit because it's part of the request. The caller must include it
in `capacity_limit` or account for it separately.

## 4. Config changes

### 4.1 `EconomyConfig` (`config.rs`)

| Old | New | Default | Meaning |
|---|---|---|---|
| `recent_n: usize` | `min_protected_turns: usize` | 3 | Minimum turns always protected |
| (new) | `protection_pct: u8` | 15 | % of `low_water` for budget extension beyond minimum |

`recent_n` is kept as a deprecated alias: if present in config, it maps to
`min_protected_turns` and `protection_pct` defaults to 15. If both
`recent_n` and `min_protected_turns` are present, `min_protected_turns` wins.

### 4.2 `EvictionPolicy` (`eviction.rs`)

| Old | New |
|---|---|
| `recent_n: usize` | `min_protected_turns: usize` |
| (new) | `protection_pct: u8` |

The `EvictionPolicy` carries both values to the planner. `band()` is unchanged.

### 4.3 `PartialEconomy` / `Provenance` (`config.rs`)

- `PartialEconomy`: `recent_n: Option<usize>` → `min_protected_turns:
  Option<usize>` + `protection_pct: Option<u8>`.
- `Provenance`: `recent_n: Source` → `min_protected_turns: Source` +
  `protection_pct: Source`.
- The config-merge function (`apply_partial`) wires both new fields.
- Back-compat: a `[economy] recent_n = 4` in an existing config file is read
  as `min_protected_turns = 4` with `protection_pct` at default (15). A
  deprecation warning is logged.

### 4.4 Config view (`config_view.rs`)

The "recent turns" field row (line 221) becomes two rows:

```
"protected turns"  → min_protected_turns (Uint, default 3)
"protection %"    → protection_pct (Uint 0–100, default 15)
```

### 4.5 Settings command (`main.rs`)

The `"recent turns"` key alias (line 3960) becomes `"protected turns"` →
`economy.min_protected_turns`, plus a new `"protection pct"` →
`economy.protection_pct`.

## 5. Behavior across the model/turn-size range

At `min_protected_turns = 3`, `protection_pct = 15`:

| Model | `low_water` | Budget @15% | Light 2k/turn | Medium 14k/turn | Heavy 74k/turn |
|---|---|---|---|---|---|
| 64k | 44.6k | 6.7k | 3 turns + 0 bonus | 3 turns (42k, soft overflow) | shrink→1 (degenerate if 74k > 56k) |
| 128k | 92k | 13.8k | 3 + 4 bonus = 7 turns | 3 turns (42k) | shrink→1 (81k < 120k ✅) |
| 256k | 184k | 27.6k | 3 + 11 bonus = 14 turns | 3 turns (42k) | 3 turns (229k < 248k ✅) |
| 1M | 240k | 36k | 3 + 15 bonus = 18 turns | 3 + 1 bonus = 4 turns | 3 turns (229k << 992k ✅) |

"Soft overflow" = request exceeds `high_water`/`low_water` but fits under
`capacity`. The model works at higher cost/latency. No degradation — the model
remembers 3 full turns.

"Shrink→1" = capacity backstop fired. Only the current turn is protected.
Better than a 400. The model remembers the in-flight exchange; older context
is evicted/compacted.

"Degenerate" = even 1 turn + system prompt exceeds capacity. No protection
policy helps. The pre-flight gate's `plan_compactions_for_overflow` force-
compacts the largest tool results; if that still doesn't fit, the request
400s honestly. This is the spec's existing §6 behavior.

### 5.1 Why `protection_pct = 15` and not 20

`protection_pct` must be less than `band_headroom_pct` (default 20), so the
budget extension never eats the wave's drop distance. If the budget
extension consumed the entire headroom, the protected floor would equal
`low_water`, and the planner could never evict enough to reach it — every
eviction wave would stop at the protected boundary. At 15% vs 20% headroom,
the budget extension is capped at 75% of the wave's drop distance, leaving
25% for actual eviction. This is a safe margin; users who want more recent
context can raise it toward 19% at the cost of tighter eviction room.

## 6. Changes to existing code

### 6.1 `eviction.rs`

- `EvictionPolicy`: replace `recent_n: usize` with `min_protected_turns:
  usize` + `protection_pct: u8`.
- `group_turns`: add `scale: f64`, `min_count: usize`, `budget: u64`,
  `capacity_limit: u64` params. Replace the `is_recent` computation with
  the backward-pass `compute_protection` described in §3.
- `plan_evictions`: compute `budget` and `capacity_limit` from the band and
  policy, pass them to `group_turns`. The `scale` is already a param.
- `EvictionPolicy::disabled()`: zero out the new fields.
- The `readmit_mark` cooldown uses `min_count` instead of `recent_n`.
- All test helpers (`fn policy(...)`) and call sites updated.

### 6.2 `config.rs`

- `EconomyConfig`: replace `recent_n` with `min_protected_turns` +
  `protection_pct`. Update `Default`.
- `PartialEconomy`: add `min_protected_turns: Option<usize>` +
  `protection_pct: Option<u8>`. Keep `recent_n: Option<usize>` for back-compat
  deserialization (maps to `min_protected_turns`).
- `Provenance`: replace `recent_n` with `min_protected_turns` +
  `protection_pct`.
- `apply_partial`: wire the new fields; handle `recent_n` back-compat.

### 6.3 `main.rs`

- `EvictionPolicy` construction (line 7141): replace `recent_n` with
  `min_protected_turns` + `protection_pct`.
- Settings command aliases (line 3960): update keys.
- Config-view wiring (line 3875): update the env-var key.

### 6.4 `config_view.rs`

- Replace the "recent turns" field row with "protected turns" +
  "protection %".
- Update the test `Provenance` defaults (lines 303, 388).

### 6.5 `band.rs`

No changes. The `derive_band` function already produces `low_water`; the
protection budget is `low_water × protection_pct / 100`, computed in the
caller (`plan_evictions`).

## 7. Testing

### 7.1 Unit tests (`eviction.rs`)

- **`protects_min_count_regardless_of_size`**: turns larger than the budget
  are still protected up to `min_count`. The budget doesn't restrict the
  minimum.
- **`budget_extends_protection_for_small_turns`**: small turns beyond
  `min_count` are protected up to the budget.
- **`capacity_backstop_shrinks_min_count`**: when the protected floor exceeds
  `capacity − SAFETY_MARGIN`, the minimum count shrinks toward 1.
- **`hard_floor_protects_current_turn`**: the newest turn is always
  protected, even when it alone exceeds capacity.
- **`protection_uses_scale`**: the backward pass scales `token_estimate` by
  the `scale` factor, matching the band's units.
- **`readmit_cooldown_uses_min_count`**: the recall cooldown window uses
  `min_protected_turns`, not the old `recent_n`.

### 7.2 Existing tests

All existing `plan_evictions` tests pass `1.0` for scale and use raw token
counts that match the band — they are self-consistent. They need their
`recent_n` params updated to `min_protected_turns` + `protection_pct`, but the
behavior is identical when `protection_pct` is high enough not to bind (which
it is at the test scales).

### 7.3 Analysis test

The existing `crates/zoid/tests/recent_n_analysis.rs` is updated to compare
the old `recent_n=4` behavior with the new budgeted protection, showing the
overflow cases are fixed.

### 7.4 Steady-state test

The `holds_band_over_hundreds_of_turns` test in `eviction.rs` is updated to
use the new fields and still passes — the steady-state property (never
exceeds capacity, stays near the band when evictable content exists) holds
with budgeted protection.

## 8. Back-compat

- A config file with `[economy] recent_n = 4` is read as
  `min_protected_turns = 4, protection_pct = 15`. A deprecation warning is
  logged: `recent_n is deprecated, use min_protected_turns and protection_pct`.
- An env var `ZOID_RECENT_N` (if it exists) maps to `min_protected_turns`.
  New env vars: `ZOID_MIN_PROTECTED_TURNS`, `ZOID_PROTECTION_PCT`.
- The `:set recent turns <n>` command still works (alias for
  `min_protected_turns`). New: `:set protected turns <n>`,
  `:set protection pct <n>`.

## 9. Out of scope

- **Sub-turn eviction** (evicting individual events within a turn) — separate
  future work; the whole-turn granularity overshoot remains but is now bounded
  by the scale fix and the budgeted protection.
- **Generative compaction** (LLM-summarized stale messages) — vision §5,
  separate phase.
- **Per-kind policy** (vision §3.1) — the followups roadmap #5; this design
  is purely about the protection floor, not kind-specific eviction rules.
- **Changing `band_headroom_pct` default** — the 20% default stays; only
  `protection_pct` (15%) is new and intentionally below it.