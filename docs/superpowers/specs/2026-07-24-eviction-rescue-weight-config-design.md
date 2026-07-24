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
- Validation/bounds enforcement beyond the existing property-test guarantees
  (any in-range value is safe per 4b §5; a wildly out-of-range value just
  makes rescue ineffective or too aggressive — no panic, no corruption).

---

## 2. Architecture

### 2.1 Config layer (`crates/zoid-core/src/config.rs`)

A new `EvictionConfig` struct:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvictionConfig {
    /// Rescue weight in turn-index units ("maximal relevance is worth this
    /// many turns of newness"). None ⇒ DEFAULT_RESCUE_WEIGHT const.
    /// Range: ~4–32; see 4b design §5. 0 disables rescue (= pure recency).
    pub rescue_weight: Option<f32>,
}

impl Default for EvictionConfig {
    fn default() -> Self {
        Self { rescue_weight: None }  // ⇒ const fallback
    }
}
```

Add `pub eviction: EvictionConfig` to `Config` (alongside `economy`, `embed`,
etc.). The partial-config overlay gets a matching `PartialEviction` struct
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
structural comparison needs.

### 2.3 Wire-in (`crates/zoid/src/main.rs` + `agent.rs`)

**`main.rs:6500`** — where `EvictionPolicy` is built from `app.economy`:
add `rescue_weight: app.config.eviction.rescue_weight` to the struct literal.

**`agent.rs:2794`** — in `preflight_gate`, where `GoalContext` is built:
replace the bare const with the resolved value:

```rust
let weight = config
    .eviction
    .rescue_weight
    .unwrap_or(zoid_core::eviction::DEFAULT_RESCUE_WEIGHT)
    .max(0.0);  // clamp negative ⇒ 0.0 (pure recency)
zoid_core::eviction::GoalContext { goal, vecs, weight }
```

The tracing line (`agent.rs:2807`) logs the resolved `weight` (not the const).

**`agent.rs:833`** — the context-length emergency retry call site stays on
`GoalContext::default()` (unchanged — rescue doesn't apply to the emergency
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
| `rescue_weight` very high (e.g. 100) | Rescue reach exceeds candidate window — an ancient on-goal turn survives. Band-preservation property test guarantees eviction still reaches `low_water` under pressure. Not a crash, not corruption — just an over-protective policy. |
| `rescue_weight` negative | `weight < 0` would make the bump negative — *anti-rescue* (relevance penalizes). The `keep_score ≥ turn.index` rescue-only invariant would break. **Reject negative values at config parse time** with a validation error. |

### 3.1 Negative-value handling

There is no config validation infrastructure today — values are merged as-is.
Introducing parse-time rejection for one field would be a new pattern. Instead,
**clamp at the read site**: in `preflight_gate`, resolve the weight as:

```rust
let weight = config
    .eviction
    .rescue_weight
    .unwrap_or(zoid_core::eviction::DEFAULT_RESCUE_WEIGHT)
    .max(0.0);  // negative ⇒ 0.0 (pure recency) — preserves rescue-only invariant
```

This is the simplest mechanical guarantee: a negative value is silently
treated as "no rescue" (0.0), which is safe and never breaks the
`keep_score ≥ turn.index` invariant. No new validation infrastructure, no
config-parse error paths. The `EvictionConfig` doc comment notes that
negative values are clamped to 0.0.

`0.0` is explicitly allowed (disables rescue = pure recency). No upper bound
is enforced — the property tests prove any positive value is band-safe. An
unreasonable value is self-limiting (rescue becomes ineffective or
over-protective, not dangerous).

---

## 4. Testing

- **Config parse round-trip:** `[eviction]\nrescue_weight = 16.0` ⇒
  `config.eviction.rescue_weight == Some(16.0)`. Absent ⇒ `None`.
- **Negative-value clamping:** `rescue_weight = -5.0` parses to
  `Some(-5.0)` (no parse error), but `preflight_gate` clamps it to `0.0`
  ⇒ pure recency. Test at the read-site level (the `GoalContext.weight`
  is `0.0` when the policy says `-5.0`).
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
  literals — there are ~8 in `agent.rs` tests + 1 in `main.rs`. All must add
  `rescue_weight: None` (tests) or `rescue_weight: app.config.eviction.rescue_weight`
  (main.rs). The `disabled()` constructor is the single source for test
  defaults.
- `Config` and its parsing are in `zoid-core`. The new `EvictionConfig`
  struct + `eviction` field + TOML parsing + partial-config overlay all live
  there.
- `cargo build --workspace && cargo test --workspace` after each task (per
  AGENTS.md cross-crate discipline).