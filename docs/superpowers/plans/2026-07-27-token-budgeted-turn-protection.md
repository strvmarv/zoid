# Token-Budgeted Turn Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fixed `recent_n` turn-count protection with a three-layer policy (hard floor of 1, minimum count, budget ceiling, capacity backstop) that scales protection with model capacity and adapts to turn size.

**Architecture:** A new `compute_protection` backward pass in `eviction.rs` replaces the single `is_recent` line in `group_turns`. Config gains `min_protected_turns` + `protection_pct` fields (with `recent_n` kept as a deprecated alias). The `scale` parameter (already committed) lets the backward pass measure protection in the same units as the eviction band.

**Tech Stack:** Rust 2021, `zoid-core` (eviction + config), `zoid-tui` (config view), `zoid` (main.rs wiring + agent tests).

## Global Constraints

- `protection_pct` default = 15 (must be < `band_headroom_pct` default 20).
- `min_protected_turns` default = 3.
- `CAPACITY_SAFETY_MARGIN = 8192` (from `band.rs:9` — do not change).
- `recent_n` must remain deserializable for back-compat (maps to `min_protected_turns`, `protection_pct` defaults to 15).
- `scale <= 0.0` is treated as `1.0` (the safe default — established by the prerequisite commit).
- All existing `plan_evictions` tests pass `1.0` for scale and use raw token counts matching the band; behavior is identical when `protection_pct` is high enough not to bind (it is at test scales).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/zoid-core/src/eviction.rs` | `EvictionPolicy` fields, `compute_protection`, `group_turns` protection pass, `plan_evictions` wiring, unit tests |
| `crates/zoid-core/src/config.rs` | `EconomyConfig`, `PartialEconomy`, `Provenance` field renames + back-compat, `apply_partial` wiring, `Default` impls, tests |
| `crates/zoid/src/main.rs` | `EvictionPolicy` construction, settings aliases, config-view env key, `Provenance::default` test literal, `EconomyConfig` test literal |
| `crates/zoid-tui/src/config_view.rs` | Field rows for "protected turns" + "protection %", `Provenance` test default literals |
| `crates/zoid/src/agent.rs` | 8 `EvictionPolicy { ... }` test constructor literals updated |
| `crates/zoid/tests/context_smoke.rs` | 3 `EvictionPolicy { ... }` literals (`:137, :306, :415`) |
| `crates/zoid-tui/tests/shell_snapshot.rs` | 6 `Provenance { ... }` literals (`:935, :976, :1023, :1082, :1160, :1208`) |

---

## Task 1: `EvictionPolicy` fields + `compute_protection` (TDD core)

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (struct at `:10-18`, `disabled()` at `:21-31`, `group_turns` at `:531`, `plan_evictions` at `:617-634`)
- Test: `crates/zoid-core/src/eviction.rs` (`mod plan_tests` + new `mod protection_tests`)

**Interfaces:**
- Consumes: `scale: f64` (already a `plan_evictions` param), `Band { high_water, low_water }` from `band.rs`, `CAPACITY_SAFETY_MARGIN: u64 = 8192` from `band.rs:9`, `TurnView { token_estimate: u64, ... }` at `:443`
- Produces: `EvictionPolicy { min_protected_turns: usize, protection_pct: u8 }` (replacing `recent_n: usize`), `fn compute_protection(turns, min_count, budget, capacity_limit, scale) -> Vec<bool>`

- [ ] **Step 1: Write the failing tests for `compute_protection`**

Add a new `mod protection_tests` after the existing `mod plan_tests` in `eviction.rs`. These tests use the `user`/`asst` helpers from the existing test module. First check the helper signatures:

```rust
// crates/zoid-core/src/eviction.rs — add at the end of the test region.
// The existing test helpers `user(id, text)` and `asst(id, text)` build Events.
// TurnView is constructed inside group_turns; for compute_protection tests we
// build TurnViews directly.

#[cfg(test)]
mod protection_tests {
    use super::*;

    fn tv(tokens: u64) -> TurnView {
        TurnView {
            ids: vec![],
            index: 0,
            token_estimate: tokens,
            topic_hint: String::new(),
            protected: false,
        }
    }

    /// Hard floor: the newest turn (index n-1) is always protected, even when
    /// it alone exceeds capacity.
    #[test]
    fn hard_floor_protects_current_turn() {
        let turns = vec![tv(100_000)];
        let p = compute_protection(&turns, 3, 1_000, 500, 1.0);
        assert!(p[0], "newest (only) turn always protected");
    }

    /// Minimum count: turns larger than the budget are still protected up to
    /// min_count. The budget does not restrict the minimum.
    #[test]
    fn protects_min_count_regardless_of_size() {
        // 5 turns, each 10k tokens. min_count=3, budget=1 (tiny).
        // All 3 newest must be protected despite budget=1.
        let turns: Vec<TurnView> = (0..5).map(|_| tv(10_000)).collect();
        let p = compute_protection(&turns, 3, 1, 1_000_000, 1.0);
        // turns 2,3,4 protected (newest 3). 0,1 not.
        assert!(p[2] && p[3] && p[4], "min_count turns protected");
        assert!(!p[0] && !p[1], "beyond min_count not protected");
    }

    /// Budget ceiling: small turns beyond min_count protected up to budget.
    #[test]
    fn budget_extends_protection_for_small_turns() {
        // 10 turns, each 100 tokens. min_count=3, budget=500.
        // 3 min (cumulative 300) + 2 bonus (cumulative 400, 500 ≤ budget) = 5
        // protected; 6th would make cumulative 600 > 500 → stop.
        let turns: Vec<TurnView> = (0..10).map(|_| tv(100)).collect();
        let p = compute_protection(&turns, 3, 500, 1_000_000, 1.0);
        let count = p.iter().filter(|&&x| x).count();
        assert_eq!(count, 5, "3 min + 2 bonus = 5");
        // turn 4 (6th from end) is the first beyond budget → not protected
        assert!(!p[0] && !p[4], "turn 4 (6th from end) beyond budget not protected");
    }

    /// Capacity backstop: when the protected floor exceeds capacity, shrink
    /// min_count toward 1. Never revoke the hard floor of 1.
    #[test]
    fn capacity_backstop_shrinks_min_count() {
        // 3 turns, each 74k tokens. min_count=3, capacity_limit=120k.
        // turn 2 (newest): 74k < 120k → protected.
        // turn 1: 74k + 74k = 148k > 120k → stop. min_count not met but 1 protected.
        let turns: Vec<TurnView> = (0..3).map(|_| tv(74_000)).collect();
        let p = compute_protection(&turns, 3, 1_000_000, 120_000, 1.0);
        assert!(p[2], "hard floor protected");
        assert!(!p[0] && !p[1], "capacity backstop shrank min_count to 1");
    }

    /// Scale: the backward pass scales token_estimate by the scale factor,
    /// matching the band's units.
    #[test]
    fn protection_uses_scale() {
        // 10 turns, each 100 raw tokens. scale=2.0 → 200 scaled per turn.
        // min_count=3, budget=800 scaled. 3 min (cumulative 600) + 1 bonus
        // (cumulative 800 ≤ budget 800) = 4; 5th would make cumulative 1000
        // > 800 → stop.
        let turns: Vec<TurnView> = (0..10).map(|_| tv(100)).collect();
        let p = compute_protection(&turns, 3, 800, 1_000_000, 2.0);
        let count = p.iter().filter(|&&x| x).count();
        assert_eq!(count, 4, "scale=2.0: 3 min + 1 bonus (cumulative 800 ≤ budget 800) = 4");
    }

    /// Empty turns → empty protection vector (no panic).
    #[test]
    fn empty_turns_no_panic() {
        let turns: Vec<TurnView> = vec![];
        let p = compute_protection(&turns, 3, 1_000, 500, 1.0);
        assert!(p.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib protection_tests`
Expected: FAIL — `compute_protection` not defined (compile error: cannot find function).

- [ ] **Step 3: Add the `EvictionPolicy` field rename + `compute_protection`**

Replace the `EvictionPolicy` struct (`eviction.rs:10-18`):

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub min_protected_turns: usize,
    pub protection_pct: u8,
    pub max_output: Option<u64>,
    pub rescue_weight: Option<f32>,
}
```

Replace `disabled()` (`eviction.rs:21-31`):

```rust
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            capacity: 0,
            context_target: 0,
            band_headroom_pct: 0,
            min_protected_turns: 0,
            protection_pct: 0,
            max_output: None,
            rescue_weight: None,
        }
    }
```

Add `compute_protection` just above `group_turns` (before `eviction.rs:531`):

```rust
/// Three-layer turn protection (spec §2.1/§3). Walks backward from the newest
/// turn, protecting until: (a) min_count met AND cumulative scaled tokens
/// reach the budget, OR (b) cumulative tokens approach capacity − SAFETY_MARGIN
/// (shrink min_count toward 1). Always protects at least the newest turn.
///
/// `budget` and `capacity_limit` are in *scaled* units (raw × scale).
/// `token_estimate` per turn is raw chars/3; `scale` converts it to match.
fn compute_protection(
    turns: &[TurnView],
    min_count: usize,
    budget: u64,
    capacity_limit: u64,
    scale: f64,
) -> Vec<bool> {
    let n = turns.len();
    let mut protected = vec![false; n];
    if n == 0 {
        return protected;
    }
    let s = if scale > 0.0 { scale } else { 1.0 };

    // Hard floor: always protect the newest turn.
    protected[n - 1] = true;

    let mut count = 1; // newest already protected
    let mut cumulative = (turns[n - 1].token_estimate as f64 * s) as u64;

    for i in (0..n.saturating_sub(1)).rev() {
        let turn_tokens = (turns[i].token_estimate as f64 * s) as u64;

        // Capacity backstop: stop if adding this turn would overflow capacity.
        // A 400 is worse than protecting fewer turns. The hard floor of 1 is
        // already protected and never revoked.
        if cumulative.saturating_add(turn_tokens) > capacity_limit {
            break;
        }

        // Minimum count: protect regardless of budget until min_count is met.
        // Budget ceiling: after min_count, protect only while under budget.
        if count < min_count || cumulative.saturating_add(turn_tokens) <= budget {
            protected[i] = true;
            count += 1;
            cumulative = cumulative.saturating_add(turn_tokens);
        } else {
            break;
        }
    }

    protected
}
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cargo test -p zoid-core --lib protection_tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(eviction): add compute_protection + rename EvictionPolicy fields

Three-layer turn protection (hard floor, min count, budget ceiling,
capacity backstop). Replaces recent_n with min_protected_turns +
protection_pct on EvictionPolicy. compute_protection is not yet wired
into group_turns (next task)."
```

---

## Task 2: Wire `compute_protection` into `group_turns` + `plan_evictions`

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (`group_turns` at `:531`, `plan_evictions` at `:617-634`, test helper `policy()` at `:738`, all test call sites)
- Test: `crates/zoid-core/src/eviction.rs` (existing `plan_tests` + `steady_state_tests`)

**Interfaces:**
- Consumes: `compute_protection(turns, min_count, budget, capacity_limit, scale) -> Vec<bool>` from Task 1, `EvictionPolicy.min_protected_turns` + `protection_pct` from Task 1
- Produces: `group_turns` now applies budgeted protection; `plan_evictions` computes `budget` and `capacity_limit` from the band

- [ ] **Step 1: Update the `policy()` test helper to the new fields**

Replace the test helper at `eviction.rs:738`:

```rust
    fn policy(target: u64, min_protected_turns: usize) -> EvictionPolicy {
        EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: target,
            band_headroom_pct: 20,
            min_protected_turns,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        }
    }
```

- [ ] **Step 2: Update all `plan_tests` call sites**

Every `policy(N, K)` call where `K` was `recent_n` now passes `min_protected_turns`. The `protection_pct: 15` default is high enough not to bind at test scales (the tests use small turn counts and large budgets relative to turn sizes), so behavior is identical. Update each call in `mod plan_tests` and `mod steady_state_tests`:

- `policy(384_000, 4)` → stays `policy(384_000, 4)` (the second arg is now `min_protected_turns`, same value — only the field name changed on the struct, which the helper already handles).
- The `policy(...)` calls that passed `recent_n` values of `2`, `4`, `10` keep those same numeric values as `min_protected_turns`.

Run: `cargo test -p zoid-core --lib plan_tests`
Expected: COMPILE ERRORS at the `group_turns` call in `plan_evictions` (still references `policy.recent_n`).

- [ ] **Step 3: Update `group_turns` signature + protection pass**

Replace the `group_turns` signature (`eviction.rs:531`):

```rust
fn group_turns(
    events: &[&Event],
    evicted: &HashSet<Ulid>,
    min_protected_turns: usize,
    budget: u64,
    capacity_limit: u64,
    scale: f64,
) -> Vec<TurnView> {
```

Replace the protection loop (`eviction.rs:590-603`). The `is_evicted` and `in_readmit_cooldown` checks stay; `is_recent` is replaced by the `compute_protection` result:

```rust
    let n = turns.len();
    // Three-layer protection (spec §3): hard floor, min count, budget ceiling,
    // capacity backstop. Computed in a backward pass over scaled token estimates.
    let protection = compute_protection(
        &turns,
        min_protected_turns,
        budget,
        capacity_limit,
        scale,
    );
    for (i, t) in turns.iter_mut().enumerate() {
        let is_protected = protection[i];
        let is_evicted = t.ids.iter().any(|id| evicted.contains(id));
        // Within the re-admit cooldown: protected only for `min_protected_turns`
        // turns after the re-admission, so recall→evict→recall can't oscillate
        // but recalled content can never form a permanent unevictable floor
        // (final-review M10).
        let in_readmit_cooldown = t
            .ids
            .iter()
            .any(|id| readmit_mark.get(id).is_some_and(|mark| n - mark < min_protected_turns));
        t.protected = is_protected || is_evicted || in_readmit_cooldown;
    }
    turns
```

- [ ] **Step 4: Update `plan_evictions` to compute `budget` + `capacity_limit` and pass them**

Replace the `group_turns` call in `plan_evictions` (`eviction.rs:634`):

```rust
    let band = policy.band();
    // Budget for protection extension beyond min_count: protection_pct of
    // low_water. Must be < band_headroom_pct (default 20) so the extension
    // never eats the wave's drop distance (spec §5.1). Clamp at runtime as a
    // defensive guard — a misconfigured protection_pct ≥ band_headroom_pct
    // would make the protected floor equal low_water and stall every wave.
    let pct = (policy.protection_pct as u64).min(policy.band_headroom_pct as u64);
    let budget = band.low_water.saturating_mul(pct) / 100;
    // capacity_limit = capacity − CAPACITY_SAFETY_MARGIN. The safety margin
    // (8192) also covers the typical ~7k system-prompt + tool-spec overhead;
    // the caller does not add system overhead separately (spec §3.2).
    let capacity_limit = policy.capacity.saturating_sub(
        crate::band::CAPACITY_SAFETY_MARGIN,
    );
    let turns = group_turns(
        &events,
        &evicted,
        policy.min_protected_turns,
        budget,
        capacity_limit,
        scale,
    );
```

Note: the `let band = policy.band();` line already exists at `eviction.rs:628`. Remove the duplicate if the compiler warns (keep the one inside `plan_evictions` before the `group_turns` call; the existing `if current_tokens < band.high_water` check at `:629` uses it). The `budget` and `capacity_limit` lines go after the `high_water` check and before the `group_turns` call.

- [ ] **Step 5: Update the `enabled_policy_band_matches_derivation` test**

At `eviction.rs:53-63`, the `EvictionPolicy { ... }` literal must use the new fields:

```rust
        let p = EvictionPolicy {
            enabled: true,
            capacity: 1_000_000,
            context_target: 384_000,
            band_headroom_pct: 20,
            min_protected_turns: 4,
            protection_pct: 15,
            max_output: None,
            rescue_weight: None,
        };
```

- [ ] **Step 6: Run all eviction tests**

Run: `cargo test -p zoid-core --lib`
Expected: PASS (all 29+ existing tests + 6 new protection_tests). The existing tests use `policy(...)` which now sets `protection_pct: 15` — at the test scales (small turn counts, large capacity) the budget does not bind, so `min_protected_turns` behaves like the old `recent_n`.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(eviction): wire compute_protection into group_turns

group_turns now applies the three-layer protection (hard floor, min
count, budget ceiling, capacity backstop) via compute_protection,
replacing the fixed is_recent count. plan_evictions computes budget
(protection_pct × low_water) and capacity_limit (capacity −
CAPACITY_SAFETY_MARGIN) and threads them through. The readmit cooldown
now uses min_protected_turns."
```

---

## Task 3: Config fields — `EconomyConfig` + `PartialEconomy` + `Provenance`

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (`EconomyConfig` at `:82-102`, `Default` at `:104-116`, `Provenance` at `:439-462`, `PartialEconomy` at `:466-474`, `apply_partial` at `:647-650`, `Provenance::default` at `:598`, tests at `:243, :422, :855`)
- Test: `crates/zoid-core/src/config.rs`

**Interfaces:**
- Consumes: `EvictionPolicy.min_protected_turns` + `protection_pct` from Task 1
- Produces: `EconomyConfig.min_protected_turns: usize` + `protection_pct: u8`, `PartialEconomy.min_protected_turns` + `protection_pct` (+ back-compat `recent_n`), `Provenance.min_protected_turns` + `protection_pct`

- [ ] **Step 1: Write the failing back-compat test**

Add to the existing test module in `config.rs`:

```rust
    #[test]
    fn recent_n_alias_maps_to_min_protected_turns() {
        // [economy] recent_n = 7 is read as min_protected_turns = 7,
        // protection_pct at default 15.
        let (p, _) = parse_toml("[economy]\nrecent_n = 7").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert_eq!(cfg.economy.min_protected_turns, 7);
        assert_eq!(cfg.economy.protection_pct, 15);
    }

    #[test]
    fn min_protected_turns_and_protection_pct_direct() {
        let (p, _) = parse_toml("[economy]\nmin_protected_turns = 5\nprotection_pct = 12").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert_eq!(cfg.economy.min_protected_turns, 5);
        assert_eq!(cfg.economy.protection_pct, 12);
    }

    #[test]
    fn min_protected_turns_wins_over_recent_n() {
        // Both present → min_protected_turns wins, recent_n ignored.
        let (p, _) = parse_toml("[economy]\nrecent_n = 7\nmin_protected_turns = 5").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert_eq!(cfg.economy.min_protected_turns, 5);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib recent_n_alias_maps_to_min_protected_turns min_protected_turns_and_protection_pct_direct min_protected_turns_wins_over_recent_n`
Expected: FAIL — `min_protected_turns` / `protection_pct` fields don't exist on `EconomyConfig` (compile error).

- [ ] **Step 3: Rename `EconomyConfig` fields**

In `config.rs`, replace the `recent_n` field (`:93-94`) with two fields:

```rust
    /// Minimum turns always protected regardless of size (default 3).
    pub min_protected_turns: usize,
    /// % of low_water for protection budget extension beyond minimum (default 15).
    /// Must be < band_headroom_pct (default 20).
    pub protection_pct: u8,
```

Update `Default` (`:104-116`) — replace `recent_n: 4,` with:

```rust
            min_protected_turns: 3,
            protection_pct: 15,
```

- [ ] **Step 4: Update `PartialEconomy` for back-compat**

In `PartialEconomy` (`:466-474`), keep `recent_n` and add the two new fields:

```rust
    pub recent_n: Option<usize>,
    pub min_protected_turns: Option<usize>,
    pub protection_pct: Option<u8>,
```

- [ ] **Step 5: Update `Provenance`**

In `Provenance` (`:439-462`), replace `pub recent_n: Source,` (`:447`) with:

```rust
    pub min_protected_turns: Source,
    pub protection_pct: Source,
```

Update `Provenance::default` (`:598`) — replace `recent_n: Source::Default,` with:

```rust
        min_protected_turns: Source::Default,
        protection_pct: Source::Default,
```

- [ ] **Step 6: Update `apply_partial`**

Replace the `recent_n` wiring (`:647-650`) with back-compat logic:

```rust
        // Back-compat: recent_n maps to min_protected_turns (protection_pct stays
        // at default 15). If both recent_n and min_protected_turns are present,
        // min_protected_turns wins (applied second).
        if let Some(v) = p.economy.recent_n {
            cfg.economy.min_protected_turns = v;
            prov.min_protected_turns = *src;
        }
        if let Some(v) = p.economy.min_protected_turns {
            cfg.economy.min_protected_turns = v;
            prov.min_protected_turns = *src;
        }
        if let Some(v) = p.economy.protection_pct {
            cfg.economy.protection_pct = v;
            prov.protection_pct = *src;
        }
```

- [ ] **Step 7: Update existing config tests**

At `config.rs:243`, replace `assert_eq!(c.economy.recent_n, 4);` with:

```rust
        assert_eq!(c.economy.min_protected_turns, 3);
        assert_eq!(c.economy.protection_pct, 15);
```

At `config.rs:422`, the test `ui_defaults_when_section_absent` parses `[economy]\nrecent_n = 3` — this now exercises the back-compat path. Add an assertion after the merge:

```rust
        assert_eq!(cfg.economy.min_protected_turns, 3, "recent_n=3 aliases to min_protected_turns");
```

At `config.rs:855`, the `wrong_typed_known_key_is_still_err` test uses `recent_n = "four"` — keep it (back-compat: `recent_n` is still a known key, still type-checked). No change needed, but verify it still compiles.

- [ ] **Step 8: Run config tests**

Run: `cargo test -p zoid-core --lib`
Expected: PASS (all config + eviction tests).

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): min_protected_turns + protection_pct with recent_n back-compat

EconomyConfig/PartialEconomy/Provenance gain min_protected_turns +
protection_pct. recent_n kept as a deprecated alias mapping to
min_protected_turns (protection_pct defaults to 15). If both present,
min_protected_turns wins."
```

---

## Task 4: `main.rs` wiring — `EvictionPolicy` construction, settings, env key, test literals

**Files:**
- Modify: `crates/zoid/src/main.rs` (`EvictionPolicy` construction at `:7177-7185`, settings key at `:3891`, settings alias at `:3980`, `EconomyConfig` test literal at `:7733`, `Provenance` test literal at `:8096`)
- Test: `crates/zoid/src/main.rs` (inline tests)

**Interfaces:**
- Consumes: `EconomyConfig.min_protected_turns` + `protection_pct` from Task 3, `EvictionPolicy.min_protected_turns` + `protection_pct` from Task 1
- Produces: live `EvictionPolicy` with the new fields; settings aliases `:set protected turns` / `:set protection pct`

- [ ] **Step 1: Update `EvictionPolicy` construction**

At `main.rs:7177-7185`, replace `recent_n: app.economy.recent_n,` with:

```rust
        min_protected_turns: app.economy.min_protected_turns,
        protection_pct: app.economy.protection_pct,
```

- [ ] **Step 2: Update settings key (env var path)**

At `main.rs:3888-3894`, replace the `"recent turns"` FieldTarget:

```rust
        "protected turns" => FieldTarget::Toml {
            key: "economy.min_protected_turns",
            ty: TomlTy::UintPlain,
        },
        "protection pct" => FieldTarget::Toml {
            key: "economy.protection_pct",
            ty: TomlTy::U8Pct,
        },
```

- [ ] **Step 3: Update settings alias (the `:set` string→value map)**

At `main.rs:3980`, replace the `"recent turns"` alias line:

```rust
        "protected turns" => ("economy.min_protected_turns", TomlValue::Int(econ.min_protected_turns as i64)),
        "protection pct" => ("economy.protection_pct", TomlValue::Int(econ.protection_pct as i64)),
```

- [ ] **Step 4: Update the `EconomyConfig` test literal**

At `main.rs:7733` (in `policy_from_config_maps_pct_to_absolute` test), replace `recent_n: 4,` with:

```rust
            min_protected_turns: 3,
            protection_pct: 15,
```

- [ ] **Step 5: Update the `Provenance` test literal**

At `main.rs:8096`, replace `recent_n: Source::Default,` with:

```rust
                    min_protected_turns: Source::Default,
                    protection_pct: Source::Default,
```

- [ ] **Step 6: Build + run main.rs tests**

Run: `cargo test -p zoid --lib`
Expected: PASS. (If there are compile errors from other `recent_n` references in `main.rs`, search for them: `grep -n "recent_n" crates/zoid/src/main.rs` and update each to the new field names.)

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(main): wire min_protected_turns + protection_pct into runtime

EvictionPolicy construction, settings aliases (:set protected turns /
:set protection pct), env var keys, and test literals updated."
```

---

## Task 5: `config_view.rs` — field rows + `Provenance` test literals

**Files:**
- Modify: `crates/zoid-tui/src/config_view.rs` (field row at `:229-235`, `Provenance` test defaults at `:311, :401`)
- Test: `crates/zoid-tui/src/config_view.rs` (inline tests)

**Interfaces:**
- Consumes: `EconomyConfig.min_protected_turns` + `protection_pct` + `Provenance.min_protected_turns` + `protection_pct` from Task 3
- Produces: two config-view rows ("protected turns", "protection %")

- [ ] **Step 1: Replace the "recent turns" field row**

At `config_view.rs:229-235`, replace the single `FieldRow`:

```rust
            FieldRow {
                label: "protected turns",
                value: cfg.economy.min_protected_turns.to_string(),
                kind: FieldKind::Uint,
                source: prov.min_protected_turns,
                env_shadowed: prov.min_protected_turns == Source::Env,
                secret_key: None,
            },
            FieldRow {
                label: "protection %",
                value: cfg.economy.protection_pct.to_string(),
                kind: FieldKind::Uint,
                source: prov.protection_pct,
                env_shadowed: prov.protection_pct == Source::Env,
                secret_key: None,
            },
```

- [ ] **Step 2: Update `Provenance` test default literals**

At `config_view.rs:311`, replace `recent_n: Source::Default,` with:

```rust
            min_protected_turns: Source::Default,
            protection_pct: Source::Default,
```

At `config_view.rs:401`, same replacement:

```rust
            min_protected_turns: Source::Default,
            protection_pct: Source::Default,
```

- [ ] **Step 3: Build + run config_view tests**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS. (If compile errors from other `recent_n` refs in the file, search: `grep -n "recent_n" crates/zoid-tui/src/config_view.rs` and update.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/config_view.rs
git commit -m "feat(config-view): protected turns + protection % rows

Replace 'recent turns' row with 'protected turns' + 'protection %'.
Update Provenance test default literals."
```

---

## Task 6: `agent.rs` test constructor literals

**Files:**
- Modify: `crates/zoid/src/agent.rs` (8 `EvictionPolicy { ... }` test literals at `:3492, :3621, :3702, :3759, :3981, :4250, :4314, :4780`)
- Test: `crates/zoid/src/agent.rs` (inline tests)

**Interfaces:**
- Consumes: `EvictionPolicy.min_protected_turns` + `protection_pct` from Task 1
- Produces: compiling agent tests

- [ ] **Step 1: Find all `recent_n` literals in agent.rs test constructors**

Run: `grep -n "recent_n:" crates/zoid/src/agent.rs`
Each is a `recent_n: N,` line inside an `EvictionPolicy { ... }` literal. There are 8 of them (5× `recent_n: 2,` + 3× `recent_n: 4,`). Note: `:3563` is a *comment* (`// ... recent_n=2 → 13,15 protected`) and will remain in the grep output — it's harmless.

- [ ] **Step 2: Replace each `recent_n: N,` with the two new fields**

For each occurrence, replace:

```rust
            recent_n: N,
```

with:

```rust
            min_protected_turns: N,
            protection_pct: 15,
```

Use `replace_all` carefully — the values differ (`recent_n: 2,` vs `recent_n: 4,`). Do two passes:

Pass 1 — all `recent_n: 2,`:
```
old: "            recent_n: 2,\n"
new: "            min_protected_turns: 2,\n            protection_pct: 15,\n"
replace_all: true
```

Pass 2 — all `recent_n: 4,`:
```
old: "            recent_n: 4,\n"
new: "            min_protected_turns: 4,\n            protection_pct: 15,\n"
replace_all: true
```

If any other numeric values appear, handle them individually.

- [ ] **Step 3: Build + run agent tests**

Run: `cargo test -p zoid --lib`
Expected: PASS. (Verify no remaining `recent_n:` struct literals: `grep -n "recent_n:" crates/zoid/src/agent.rs` should return nothing. The `:3563` comment contains `recent_n=2` — harmless, expected to remain.)

- [ ] **Step 4: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "test(agent): update EvictionPolicy literals to new fields

All 8 test constructor literals updated from recent_n to
min_protected_turns + protection_pct (default 15)."
```

---

## Task 7: Update `recent_n_analysis.rs` + final full-workspace verification

**Files:**
- Modify: `crates/zoid/tests/recent_n_analysis.rs` (analysis test comparing old vs new protection)
- Modify: `crates/zoid/tests/context_smoke.rs` (3 `EvictionPolicy { ... }` literals at `:137, :306, :415`)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (6 `Provenance { ... }` literals at `:935, :976, :1023, :1082, :1160, :1208`)
- Modify: `crates/zoid-core/src/eviction.rs:1195` (`holds_band_over_hundreds_of_turns` steady-state test `EvictionPolicy { ... }` literal)
- Test: whole workspace

**Interfaces:**
- Consumes: `EvictionPolicy.min_protected_turns` + `protection_pct` from Task 1, `compute_protection` from Task 1
- Produces: updated analysis test showing overflow cases are fixed

- [ ] **Step 1: Update the analysis test**

The existing `recent_n_analysis.rs` (untracked, just committed in the prerequisite) references `recent_n` in `EvictionPolicy` construction. Update each `EvictionPolicy { ... }` literal in the file:

- Replace `recent_n: N,` with `min_protected_turns: N,` + `protection_pct: 15,` (same pattern as Task 6).

Run: `grep -n "recent_n" crates/zoid/tests/recent_n_analysis.rs` to find all occurrences. Update each.

- [ ] **Step 1b: Update `context_smoke.rs` literals**

Replace each `recent_n: N,` with `min_protected_turns: N,` + `protection_pct: 15,` at lines `:137, :306, :415`. Use the same `replace_all` two-pass approach as Task 6 (`recent_n: 2,` → `min_protected_turns: 2, protection_pct: 15,`; `recent_n: 4,` → `min_protected_turns: 4, protection_pct: 15,`).

- [ ] **Step 1c: Update `shell_snapshot.rs` Provenance literals**

Replace each `recent_n: Source::Default,` with `min_protected_turns: Source::Default,` + `protection_pct: Source::Default,` at lines `:935, :976, :1023, :1082, :1160, :1208`. Use `replace_all: true` since all 6 are identical.

- [ ] **Step 1d: Update `eviction.rs:1195` steady-state test literal**

In the `holds_band_over_hundreds_of_turns` test (`steady_state_tests` module), the `EvictionPolicy { ... }` literal at `:1195` has `recent_n: 4,`. Replace with `min_protected_turns: 4,` + `protection_pct: 15,`.

- [ ] **Step 2: Run the analysis test (ignored, requires --ignored)**

Run: `cargo test -p zoid --test recent_n_analysis -- --ignored --nocapture 2>&1 | tail -30`
Expected: PASS (the test runs the analysis and prints the table; it should show the overflow cases are now handled by the capacity backstop).

- [ ] **Step 3: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS (all crates). If any test fails due to a missed `recent_n` reference, search the whole workspace: `grep -rn "recent_n" crates/ --include="*.rs"` and update any remaining occurrences (excluding the back-compat `PartialEconomy.recent_n` field in `config.rs`, which stays).

- [ ] **Step 4: Verify no stray `recent_n` refs outside back-compat**

Run: `grep -rn "recent_n" crates/ --include="*.rs"`
Expected: Only `config.rs` lines for `PartialEconomy.recent_n` (the back-compat alias field) + `config.rs` test `recent_n = "four"` + `config.rs` test `recent_n = 3` (back-compat deserialization tests) + the new `recent_n_alias_maps_to_min_protected_turns` test. No `recent_n:` struct literals, no `policy.recent_n`, no `.recent_n` field access outside `apply_partial`'s back-compat block.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/tests/recent_n_analysis.rs crates/zoid/tests/context_smoke.rs crates/zoid-tui/tests/shell_snapshot.rs crates/zoid-core/src/eviction.rs
git commit -m "test: update analysis + smoke + snapshot tests for budgeted protection

recent_n_analysis, context_smoke, shell_snapshot, and the steady-state
eviction test now use min_protected_turns + protection_pct. Analysis shows
overflow cases handled by the capacity backstop instead of overflowing
the band."
```

---

## Self-Review

### Spec coverage

- §1 Problem → addressed by Tasks 1-2 (the `compute_protection` + `group_turns` change).
- §2.1 Three layers (hard floor, min count, budget ceiling) → Task 1 `compute_protection`.
- §2.1 layer 4 (capacity backstop) → Task 1 `compute_protection` (the `cumulative + turn_tokens > capacity_limit` break).
- §2.2 Precedence → Task 1 (capacity break fires before min_count check; min_count check is `count < min_count ||` which overrides budget).
- §2.3 What stays the same → Task 2 (protected flag still gates candidates; scale reused; readmit cooldown uses min_count).
- §3 Algorithm → Task 1 (`compute_protection`) + Task 2 (wiring into `group_turns`).
- §3.1 Where this runs → Task 2 (group_turns called from plan_evictions; scale/band available there).
- §3.2 Capacity backstop detail → Task 1 (capacity_limit param) + Task 2 (capacity_limit = capacity − CAPACITY_SAFETY_MARGIN).
- §4.1 EconomyConfig → Task 3.
- §4.2 EvictionPolicy → Task 1.
- §4.3 PartialEconomy / Provenance / apply_partial → Task 3.
- §4.4 Config view → Task 5.
- §4.5 Settings command → Task 4.
- §5 Behavior table → verified by Task 1 unit tests + Task 7 analysis test.
- §5.1 protection_pct = 15 rationale → Global Constraints (documented in Task 3 field comment).
- §6.1 eviction.rs changes → Tasks 1-2.
- §6.2 config.rs changes → Task 3.
- §6.3 main.rs changes → Task 4.
- §6.4 config_view.rs changes → Task 5.
- §6.5 band.rs no changes → confirmed (no task needed).
- §7.1 Unit tests (6 named) → Task 1 (all 6 written).
- §7.2 Existing tests updated → Tasks 2, 6, 7.
- §7.3 Analysis test → Task 7.
- §7.4 Steady-state test → Task 2 (updated, still passes).
- §8 Back-compat → Task 3 (`recent_n` alias in PartialEconomy + apply_partial).
- §9 Out of scope → no tasks (correct).

### Placeholder scan
No placeholders. All steps have exact code, exact commands, exact line numbers.

### Type consistency
- `EvictionPolicy.min_protected_turns: usize` — defined Task 1, used Tasks 2/4/6.
- `EvictionPolicy.protection_pct: u8` — defined Task 1, used Tasks 2/4/6.
- `EconomyConfig.min_protected_turns: usize` — defined Task 3, used Tasks 4/5.
- `EconomyConfig.protection_pct: u8` — defined Task 3, used Tasks 4/5.
- `Provenance.min_protected_turns: Source` — defined Task 3, used Tasks 4/5.
- `Provenance.protection_pct: Source` — defined Task 3, used Tasks 4/5.
- `PartialEconomy.min_protected_turns: Option<usize>` — defined Task 3, used Task 3.
- `PartialEconomy.protection_pct: Option<u8>` — defined Task 3, used Task 3.
- `compute_protection(turns, min_count, budget, capacity_limit, scale) -> Vec<bool>` — defined Task 1, called Task 2.
- `group_turns(..., min_protected_turns, budget, capacity_limit, scale)` — defined Task 2, called Task 2.

All consistent.

### Gilfoyle review (2026-07-27)

Plan reviewed by the gilfoyle agent profile against the spec + live codebase. Findings applied:

- **C2 (fixed):** `budget_extends_protection_for_small_turns` comment math was garbled ("+4 = 200"). Fixed to trace cumulative accumulation correctly.
- **C3 (fixed):** Plan missed `recent_n` refs in `context_smoke.rs` (3), `shell_snapshot.rs` (6), and `eviction.rs:1195` (steady-state). Added Steps 1b/1c/1d to Task 7.
- **H1 (fixed):** Plan said "9" `agent.rs` literals; there are 8. Fixed count + noted `:3563` comment remains in grep.
- **H3 (fixed):** `protection_uses_scale` comment conflated per-turn and cumulative budget. Fixed.
- **M2 (fixed):** Added runtime clamp `pct = min(protection_pct, band_headroom_pct)` in `plan_evictions` to enforce spec §5.1.
- **M4 (fixed):** Dropped vestigial `protection_pct` param from `group_turns` signature.
- **L1 (fixed):** Loop range "590-602" → "590-603".
- **L8 (fixed):** `zoid_core::band::CAPACITY_SAFETY_MARGIN` → `crate::band::CAPACITY_SAFETY_MARGIN` (in-crate path).
- **C1 (noted, not changed):** The spec's prose §2.1 layer 4 says the capacity backstop should only consider "the minimum count alone, not the budget extension," but the spec's own §3 reference implementation (lines 125-167) applies the break uniformly. The plan faithfully implements the spec's reference code. This is a spec-internal inconsistency (prose vs. pseudocode), not a plan defect. The uniform-break behavior is more conservative and defensible. Flag the spec prose for amendment.
- **H2 (noted):** The `capacity_limit` wiring relies on `CAPACITY_SAFETY_MARGIN` (8192) to cover the typical ~7k system-prompt overhead. Documented as a conscious decision in Task 2 Step 4.
- **M1 (noted):** Cross-layer back-compat precedence (recent_n in one layer, min_protected_turns in another) is spec-ambiguous. The plan's within-layer test covers the documented behavior. A cross-layer test is a nice-to-have if the spec is amended to clarify.