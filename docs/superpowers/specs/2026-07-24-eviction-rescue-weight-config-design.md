# ACM Follow-up #1 — `[eviction]` Config Exposure of `RESCUE_WEIGHT` — Design

> **Status:** DESIGN (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** `docs/superpowers/specs/2026-07-24-acm-followups-roadmap.md` (item 1).
> **Builds on:** `docs/superpowers/specs/2026-07-23-acm-relevance-rescued-eviction-design.md`
> (Slice-4b, shipped — `DEFAULT_RESCUE_WEIGHT` is currently a `const`).

---

## 1. Goal & scope

Expose `DEFAULT_RESCUE_WEIGHT` (currently `const f32 = 12.0` in
`crates/zoid-core/src/eviction.rs:193`) as a runtime config key so it can be
tuned without a rebuild. The replay eval (Slice-4b Task 7) fixed 12.0 from one
dogfood corpus; different workflows may want a different rescue reach.

**In scope:**
- A new `[eviction]` config section with a single key: `rescue_weight`.
- `rescue_weight: Option<f32>` added to `EvictionPolicy`, flowing through
  `TurnConfig.eviction` to `preflight_gate` (the same path as the other
  eviction knobs).
- `None` / absent ⇒ fall back to `DEFAULT_RESCUE_WEIGHT` const (behavior
  unchanged).

**Out of scope:**
- Migrating existing eviction-adjacent fields (`band_headroom_pct`,
  `recent_n`, `compact_threshold_pct`) from `[economy]` to `[eviction]`.
  They stay in `[economy]`; this slice adds only `rescue_weight`.
- Config-screen UI for the knob (the config file is the surface).
- Config-parse-time range validation — the read-site `resolve_rescue_weight`
  clamping (§3.1) handles non-finite and out-of-range values without
  introducing a new validation pattern to the config layer.

---

## 2. Architecture

### 2.1 Config layer (`crates/zoid-core/src/config.rs`)

A new `EvictionConfig` struct:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EvictionConfig {
    /// Rescue weight in turn-index units ("maximal relevance is worth this
    /// many turns of newness"). None ⇒ DEFAULT_RESCUE_WEIGHT const.
    /// Range: ~4–32; see 4b design §5. 0 disables rescue (= pure recency).
    /// Negative, NaN, and +∞ are clamped at the read site (§3.1).
    pub rescue_weight: Option<f32>,
}
```

`#[derive(Default)]` suffices — `Option<f32>::default()` is `None`, which is
exactly the const-fallback semantics. No manual impl needed.

Add `pub eviction: EvictionConfig` to `Config` (alongside `economy`, `embed`,
etc.). **`Config` currently derives `Eq` (config.rs:34).** Adding
`eviction: EvictionConfig` (with `Option<f32>`) requires **dropping `Eq` from
`Config`**, retaining `PartialEq`. Verified: the only `Config`-level equality
assertion (`config.rs:653`, `assert_eq!(cfg, Config::default())`) requires
`PartialEq + Debug`, not `Eq`; no workspace consumer bounds `Config: Eq`. The
`Provenance` struct and `Source` enum (which do carry `Eq`) are unaffected —
they don't hold the new field.

The partial-config overlay gets a matching `PartialEviction` struct
(following the `PartialEmbed` / `PartialEconomy` pattern —
`#[derive(Debug, Default, Clone, Deserialize)]` with `Option<f32>` fields):

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEviction {
    pub rescue_weight: Option<f32>,
}
```

Add `pub eviction: PartialEviction` to `PartialConfig`, and a merge arm in
`merge()` (following the `p.embed.*` pattern at config.rs:583):

```rust
if let Some(v) = p.eviction.rescue_weight {
    cfg.eviction.rescue_weight = Some(v);
}
```

No `Provenance` field is added for `rescue_weight` — the `embed` fields don't
track provenance either (config.rs:583–591), and this is a low-stakes knob.
This keeps the change small.

The TOML surface:
```toml
[eviction]
rescue_weight = 16.0  # optional; absent ⇒ default 12.0
```

### 2.2 `EvictionPolicy` (`crates/zoid-core/src/eviction.rs`)

Add `rescue_weight: Option<f32>` to `EvictionPolicy`:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]  // Eq dropped — f32 is not Eq
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub recent_n: usize,
    pub max_output: Option<u64>,
    pub rescue_weight: Option<f32>,  // NEW
}
```

`EvictionPolicy::disabled()` sets `rescue_weight: None` (no rescue when
disabled — consistent with `enabled: false` being a total bypass).

The `Eq` derive is dropped. `f32` does not implement `Eq`, and the `Eq` impl
is not used in any assertion (verified: no `assert_eq` on `EvictionPolicy`
values in the test suite). `PartialEq` (float-aware) is retained for any
structural comparison needs. `Option<f32>` is `Copy`, so `EvictionPolicy`
retains `Copy` — the field is passed by value through `TurnConfig` unchanged.

### 2.3 Wire-in (`crates/zoid/src/main.rs` + `agent.rs`)

**`main.rs:6500`** — where `EvictionPolicy` is built from `app.economy`:
add `rescue_weight: app.config.eviction.rescue_weight` to the struct literal.

**`agent.rs:2794`** — in `preflight_gate`, where `GoalContext` is built:
replace the bare const with the resolved value:

```rust
let weight = resolve_rescue_weight(config.eviction.rescue_weight);
zoid_core::eviction::GoalContext { goal, vecs, weight }
```

The tracing line (`agent.rs:2807`) logs the resolved `weight` (not the const).

**`agent.rs:862–867`** — the context-length emergency retry call site stays
on `GoalContext::default()` (unchanged — rescue doesn't apply to the emergency
retry path).

### 2.4 What does NOT change

- `plan_evictions` signature — `GoalContext.weight` is already a field; the
  const just feeds it. No signature change.
- The relevance layer, `rank_normalize`, `turn_relevance` — all pure, all
  consume `ctx.weight` which now comes from config instead of a const.
- Property tests (4b Task 5) — still pass; they set `weight` explicitly on
  `GoalContext` and don't read the const or the policy.
- Subagent path — `EvictionPolicy::disabled()` ⇒ `rescue_weight: None` ⇒ no
  rescue (subagents don't evict).

---

## 3. Degradation & safety

| Condition | Behavior |
|---|---|
| `[eviction]` absent in config | `EvictionConfig::default()` ⇒ `None` ⇒ `DEFAULT_RESCUE_WEIGHT` (12.0) — identical to today |
| `rescue_weight = 0` | `GoalContext.weight = 0.0` ⇒ bump = 0 ⇒ pure recency (proven by `bounded_reach_weight_zero_is_pure_recency` test) |
| `rescue_weight` large finite (e.g. 100) | Clamped to `RESCUE_WEIGHT_MAX` (48.0) by `resolve_rescue_weight`. Band-preservation property test guarantees eviction still reaches `low_water` under pressure for any finite value ≤ the cap. |
| `rescue_weight = 1e308` (deserializes to `+∞`) | `resolve_rescue_weight` detects non-finite ⇒ `0.0` (pure recency). Prevents the band-starve pathology where `inf`-scored turns become permanently un-evictable. |
| `rescue_weight` negative or `-∞` | `resolve_rescue_weight` ⇒ `0.0` (pure recency). Preserves the `keep_score ≥ turn.index` rescue-only invariant. |

### 3.1 Non-finite and out-of-range value handling

Config type validation exists (serde rejects wrong-typed known keys —
`config.rs:689`); range validation does not. The existing `u8`/`usize` economy
fields already accept any in-type value without range checks. This slice
clamps at the read site rather than introducing range validation, consistent
with that posture.

**Why clamping must cover both sides, not just negatives:** TOML floats
deserialize into `f32` via serde. Values exceeding `f32::MAX` (e.g.
`1e308`, `3.4028236e38`) produce `f32::INFINITY` — a perfectly valid
deserialization, not a parse error. `f32::max(0.0)` only neutralizes the
negative side; `+∞` sails through unchanged. With `weight = inf`, the
relevance layer computes `bump[i] = inf * normalized[i]` for any on-goal
turn, making it permanently un-evictable — the exact "band starve" failure
the 4b design §5 warns about. The 4b property test
`band_preservation_rescue_never_shrinks_quota` proves band-safety for
*finite in-range* values (it uses `DEFAULT_RESCUE_WEIGHT = 12.0`); it does
not cover `inf`, which is out of range by construction.

**Resolution — a finite-and-bounded resolver** (pure function in
`eviction.rs`, tested independently):

```rust
/// Upper cap: 4× the default. Anything above this makes rescue so
/// over-protective that it's effectively a misconfiguration; clamping here
/// prevents the band-starve pathology while still allowing ample tuning range.
const RESCUE_WEIGHT_MAX: f32 = DEFAULT_RESCUE_WEIGHT * 4.0; // 48.0

/// Resolve the rescue weight, clamping to a safe positive range.
/// Negative / NaN / +∞ / -∞ all collapse to a safe value (0.0 for
/// under/invalid, RESCUE_WEIGHT_MAX for over), preserving the rescue-only
/// invariant and the band-preservation guarantee.
fn resolve_rescue_weight(raw: Option<f32>) -> f32 {
    let w = raw.unwrap_or(DEFAULT_RESCUE_WEIGHT);
    if w.is_finite() && w >= 0.0 {
        w.min(RESCUE_WEIGHT_MAX)
    } else {
        0.0
    }
}
```

This handles every pathological input:
- **Negative** (`-5.0`): `is_finite() && >= 0.0` is false ⇒ `0.0` (pure recency).
- **`+∞`** (`1e308` deserialized): `is_finite()` is false ⇒ `0.0`.
- **`-∞`** (`-1e400`): `is_finite()` is false ⇒ `0.0`.
- **NaN**: `is_finite()` is false ⇒ `0.0`. (TOML can't produce NaN, but the
  guard is defense-in-depth.)
- **Very large finite** (`100.0`): clamped to `RESCUE_WEIGHT_MAX` (48.0).
- **`0.0`**: passes through (disables rescue = pure recency, explicitly allowed).

No new validation infrastructure, no config-parse error paths. The
`EvictionConfig` doc comment notes that out-of-range values are clamped.

---

## 4. Testing

- **Config parse round-trip:** `[eviction]\nrescue_weight = 16.0` ⇒
  `config.eviction.rescue_weight == Some(16.0)`. Absent ⇒ `None`.
- **Negative-value clamping:** `rescue_weight = -5.0` parses to
  `Some(-5.0)` (no parse error), but `resolve_rescue_weight` clamps it to
  `0.0` ⇒ pure recency. Test the resolver directly: `resolve_rescue_weight(Some(-5.0)) == 0.0`.
- **Over-bound / +∞ clamping:** `rescue_weight = 1e308` (deserializes to
  `f32::INFINITY`) ⇒ `resolve_rescue_weight(Some(f32::INFINITY)) == 0.0`.
  Also test a large finite value: `resolve_rescue_weight(Some(100.0)) ==
  RESCUE_WEIGHT_MAX` (48.0). Add a band-preservation check: with the clamped
  weight, `plan_evictions` still drains to `low_water` under pressure
  (mirrors `band_preservation_rescue_never_shrinks_quota` with the clamped
  over-bound value, not `DEFAULT_RESCUE_WEIGHT`).
- **`EvictionPolicy` construction:** `main.rs` wiring passes
  `config.eviction.rescue_weight` into `EvictionPolicy.rescue_weight`.
  `disabled()` ⇒ `None`.
- **`preflight_gate` reads the value:** the existing
  `preflight_rescues_relevant_old_turn_over_newer_offgoal` integration test
  (4b Task 6) still passes (it uses `FakeEmbedder` + seeded vectors; the
  weight comes from the policy/const fallback — both yield 12.0 today).
  Add a variant test that sets `rescue_weight = Some(0.0)` via the policy
  and confirms the old on-goal turn is *not* rescued (pure recency).
- **Existing 4b property tests:** `bounded_reach_weight_zero_is_pure_recency`
  and `band_preservation_rescue_never_shrinks_quota` still pass — they set
  `GoalContext.weight` directly, independent of the config/policy source.

---

## 5. Cross-crate impact

- `EvictionPolicy` is defined in `zoid-core`, used in `zoid` (`agent.rs`,
  `main.rs`). Adding a field breaks all `EvictionPolicy { ... }` struct
  literals — **11 total**: 3 in `eviction.rs` test modules (lines 50, 609,
  970), 7 in `agent.rs` tests (lines 3220, 3348, 3408, 3629, 3897, 3960,
  4423), and 1 in `main.rs` (line 6500). All must add `rescue_weight: None`
  (tests) or `rescue_weight: app.config.eviction.rescue_weight` (main.rs).
  The `eviction.rs` test helper `fn policy(target, recent_n)` at line 608 is
  the best lever: updating it fixes lines 609 and 970 in one edit; line 50
  is standalone. `disabled()` gains `rescue_weight: None` and covers only its
  3 call sites (`agent.rs:265`, `subagent.rs:170`, `eviction.rs:45`) — the 7
  `agent.rs` test literals build enabled policies inline, not via `disabled()`.
- `Config` and its parsing are in `zoid-core`. The new `EvictionConfig`
  struct + `eviction` field + TOML parsing + partial-config overlay all live
  there.
- `cargo build --workspace && cargo test --workspace` after each task (per
  AGENTS.md cross-crate discipline).