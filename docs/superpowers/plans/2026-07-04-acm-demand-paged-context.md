# Demand-Paged Context (ACM ceiling) — Implementation Plan (Slices 0+1+2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hold the live request (tokens sent each turn) at/below a user-set **context target** (default `min(capacity, 384k)`) across indefinite sessions by adding a pre-flight eviction gate and a demand-paged cold tier, with nothing truly forgotten (evicted turns stay queryable via `recall`).

**Architecture:** Split the event log into a **hot working set** (events the projections replay + send) and a **cold tier** (evicted events, still in sqlite, FTS5-indexed, not replayed). A pure **hysteresis eviction controller** keeps the hot set inside an asymmetric band derived per-model from capacity; a **pre-flight gate** in the agent turn loop runs compaction+eviction *before* the request is built, with a bounded capacity-error retry as the hard-bound backstop. A **`recall` tool** searches the cold tier (BM25 via FTS5) and re-admits matching turns. ML seams (embedding/reranking/relevance-scoring) are declared as `None`-valued traits so Slice 4 is additive.

**Tech Stack:** Rust 2021 workspace. Crates: `zoid-core` (pure planners/projections + effectful sqlite store & single-writer actor), `zoid-model` (dependency-free model catalog), `zoid-provider` (LLM seam), `zoid-tools` (tool specs), `zoid` (bin: agent loop, config, TUI wiring). sqlite via `rusqlite` (`bundled`), FTS5. Tests: `cargo test`, TDD.

## Global Constraints

- **Capacity = input + output** (the model's physical window). The hard bound; never exceeded. Resolved from `zoid_provider::context_ceiling(model)` (= `MODEL_CAPS` seed, with the wired async `fetch_model_info` override folded in the bin). NOT a value the user configures directly (a config *override* exists).
- **Context target** = the user's soft setpoint the controller manages toward. Default `min(capacity, 384_000)`.
- **Band is asymmetric:** `high_water = effective_target`, `low_water = effective_target − headroom` (headroom default 20% of effective target). `effective_target = min(context_target, capacity − output_reserve)`.
- **Eviction unit = whole turn.** Never loose items (preserves tool_use/tool_result pairing).
- **Explicit evicted-id set, never a timestamp cutoff.** (The cutoff was the original thrash bug.)
- **Append-only + reversible.** Eviction/recall are new events; original events are never mutated or deleted. `EventId` is `ulid::Ulid` (no alias).
- **Protection is structural:** System / non-`main` branch / most-recent-`recent_n` turns are type-level un-selectable by the controller.
- **Pure/effectful boundary:** planners & projections (`plan_evictions`, `conversation`, `context_window_with`, `derive_band`, `evicted_ids`) are pure fns over `&[Event]`. The `rusqlite::Connection` (`store.rs`) and single-writer actor (`session.rs`) are already in `zoid-core`; new storage is `EventStore` methods reached via new actor `Cmd` variants.
- **Two-speed execution:** compaction + eviction run **synchronously pre-flight** (cheap: pick ids + append one event); FTS indexing runs **synchronously at append** inside `EventStore::append` (one transaction); embeddings/vector index are reserved for the **async lane** (Slice 4, not built here).
- **Build with `--workspace`.** Cross-crate field adds to shared types (`EventKind`, `TurnConfig`) break TUI/economy literals when tests are scoped to a single crate. Every `cargo test` in this plan is `cargo test --workspace` unless a single test id is named.
- **Commit style:** never add a `Co-Authored-By` / co-author trailer (repo rule).
- **Do not touch** `MsgRole::Tool` flattening / anthropic tool-use (out of scope); do not change compaction's *summarization* behavior (only its scheduling).

---

## File Structure

**Created:**
- `crates/zoid-core/src/band.rs` — pure band derivation (`Band`, `derive_band`). Slice 0.
- `crates/zoid-core/src/eviction.rs` — pure eviction controller (`EvictionPolicy`, `EvictionScorer`, `RecencyScorer`, `GoalContext`, `TurnView`, `EvictedTurn`, `EvictionPlan`, `plan_evictions`, `evicted_ids`, `eviction_breadcrumb`). Slice 1.
- `crates/zoid-core/src/retrieval.rs` — pure ML seams (`Embedder`, `Reranker`, `CandidateSource`, `RecallCandidate`, `Scored`). Slice 2.
- `crates/zoid-tools/src/recall.rs` — `Recall` tool spec (Emitting; no deps). Slice 2.

**Modified:**
- `crates/zoid-model/src/lib.rs` — seed capacity fix (`MODEL_CAPS` → 1M) + test updates. Slice 0.
- `crates/zoid-core/src/config.rs` — `EconomyConfig` capacity/target split (`context_ceiling`→`context_target`, retire `token_ceiling`, add `band_headroom_pct`, `recent_n`). Slice 0.
- `crates/zoid-core/src/lib.rs` — `pub mod band; pub mod eviction; pub mod retrieval;`.
- `crates/zoid-core/src/event.rs` — `TurnsEvicted`, `TurnsReadmitted`, `EvictionMarker`, `EvictedSpan`. Slice 1.
- `crates/zoid-core/src/projection.rs` — `conversation()` skips evicted ids + appends breadcrumb consumers. Slice 1.
- `crates/zoid-core/src/context.rs` — `context_window_with` skips evicted ids. Slice 1.
- `crates/zoid-core/src/store.rs` — FTS5 table + atomic index-at-append + `search_fts` + `events_by_ids`. Slice 2.
- `crates/zoid-core/src/session.rs` — `Cmd::Recall` + `SessionHandle::recall`. Slice 2.
- `crates/zoid-provider/src/lib.rs` — `is_context_length_error`. Slice 1.
- `crates/zoid/src/agent.rs` — `TurnConfig` eviction fields, `build_request` breadcrumb, `preflight_gate`, capacity-error retry, in-loop `recall` handling. Slices 0/1/2.
- `crates/zoid/src/main.rs` — config-screen key rename, wire capacity+target into `TurnConfig.eviction`. Slice 0.
- `crates/zoid/src/invoke_skill.rs` (`chat_tools`) — add `Recall` to the chat tool set. Slice 2.

---

# SLICE 0 — Capacity correctness + band derivation

*Delivers: correct model capacity on any model, the capacity/target split, and a pure, tested band. No behavior change to the live request yet.*

### Task 0.1: Fix the `MODEL_CAPS` seed capacities

**Files:**
- Modify: `crates/zoid-model/src/lib.rs:111,125,139` (context_window literals) and `:219-224` (test assertions)

**Interfaces:**
- Produces: `zoid_provider::context_ceiling("claude-opus-4-8") == 1_000_000` (via `model_info().context_window`); same for `claude-sonnet-4-6` and `glm-5.2:cloud`.

- [ ] **Step 1: Update the failing test first.** In `crates/zoid-model/src/lib.rs`, find the test around `:219-224` asserting the seed windows. Change the expected values so claude + glm are 1M:

```rust
// in the MODEL_CAPS test (~line 219)
assert_eq!(model_info("claude-sonnet-4-6").context_window, 1_000_000);
assert_eq!(model_info("claude-opus-4-8").context_window, 1_000_000);
assert_eq!(model_info("glm-5.2:cloud").context_window, 1_000_000);
assert_eq!(model_info("deepseek-v4-pro").context_window, 128_000);
```

- [ ] **Step 2: Run it to verify it fails.**

Run: `cargo test -p zoid-model`
Expected: FAIL — assertions expect 1_000_000 but seed still says 200_000 / 256_000.

- [ ] **Step 3: Fix the seed constants.** In `MODEL_CAPS` (~`:107`), set `context_window: 1_000_000` on the `claude-sonnet-4-6` (`:111`), `claude-opus-4-8` (`:125`), and `glm-5.2:cloud` (`:139`) entries. Leave `deepseek-v4-pro` (`:148`) at `128_000` and `DEFAULT_MODEL_INFO` (`:159`) at `32_000`. Do **not** change the `tools:` fields.

- [ ] **Step 4: Run tests to verify they pass.**

Run: `cargo test -p zoid-model`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-model/src/lib.rs
git commit -m "fix(model): seed claude & glm-5.2 capacity at 1M (were 200k/256k)"
```

---

### Task 0.2: Pure band derivation (`band.rs`)

**Files:**
- Create: `crates/zoid-core/src/band.rs`
- Modify: `crates/zoid-core/src/lib.rs` (add `pub mod band;`)

**Interfaces:**
- Produces:
  - `struct Band { pub high_water: u64, pub low_water: u64, pub effective_target: u64 }`
  - `fn derive_band(capacity: u64, context_target: u64, max_output: Option<u64>, headroom_pct: u8) -> Band`
  - `const OUTPUT_RESERVE_FLOOR: u64`, `const CAPACITY_SAFETY_MARGIN: u64`

- [ ] **Step 1: Write the failing tests.** Create `crates/zoid-core/src/band.rs`:

```rust
//! Pure derivation of the eviction band from a model's capacity and the user's
//! context target (spec §3.6a). `capacity` is total context = input + output, so
//! the band reserves output room and can never exceed what the model can carry.

/// Floor on reserved output room when a model exposes no `max_output`.
pub const OUTPUT_RESERVE_FLOOR: u64 = 8_192;

/// Kept-clear margin below hard `capacity` for the pre-flight gate's hard floor.
pub const CAPACITY_SAFETY_MARGIN: u64 = 8_192;

/// The asymmetric operating band. `high_water == effective_target` (evict when
/// the estimate reaches it), `low_water` is where an eviction wave stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub high_water: u64,
    pub low_water: u64,
    pub effective_target: u64,
}

/// Derive the band for the active model. `context_target` is the user's soft
/// setpoint; it is clamped so it can never exceed `capacity - output_reserve`.
pub fn derive_band(
    capacity: u64,
    context_target: u64,
    max_output: Option<u64>,
    headroom_pct: u8,
) -> Band {
    let output_reserve = max_output.unwrap_or_else(|| OUTPUT_RESERVE_FLOOR.max(capacity / 10));
    let usable = capacity.saturating_sub(output_reserve);
    let effective_target = context_target.min(usable);
    let headroom = effective_target.saturating_mul(headroom_pct as u64) / 100;
    let low_water = effective_target.saturating_sub(headroom);
    Band { high_water: effective_target, low_water, effective_target }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_1m_model_384k_target() {
        // 1M capacity, 384k target, 20% headroom, default output reserve (100k).
        let b = derive_band(1_000_000, 384_000, None, 20);
        assert_eq!(b.effective_target, 384_000); // target < usable (900k)
        assert_eq!(b.high_water, 384_000);
        assert_eq!(b.low_water, 384_000 - 76_800); // 20% headroom
    }

    #[test]
    fn small_model_collapses_target_to_usable() {
        // 32k capacity, 384k target: effective target clamps to usable (~28.8k).
        let b = derive_band(32_000, 384_000, None, 20);
        assert_eq!(b.effective_target, 32_000 - 3_200); // usable = cap - cap/10
        assert!(b.high_water <= 32_000);
        assert!(b.low_water < b.high_water);
    }

    #[test]
    fn explicit_max_output_is_respected() {
        let b = derive_band(200_000, 384_000, Some(16_000), 10);
        assert_eq!(b.effective_target, 200_000 - 16_000); // clamped to usable
        assert_eq!(b.low_water, b.effective_target - b.effective_target / 10);
    }

    #[test]
    fn tiny_capacity_never_underflows() {
        let b = derive_band(1_000, 384_000, None, 20);
        assert!(b.low_water <= b.high_water);
        assert!(b.high_water <= 1_000);
    }
}
```

- [ ] **Step 2: Wire the module.** In `crates/zoid-core/src/lib.rs`, add `pub mod band;` (alphabetical, near `pub mod assembler;`).

- [ ] **Step 3: Run tests to verify they pass.**

Run: `cargo test -p zoid-core band::`
Expected: PASS (4 tests). The module is self-contained; tests were written against the final impl.

- [ ] **Step 4: Commit.**

```bash
git add crates/zoid-core/src/band.rs crates/zoid-core/src/lib.rs
git commit -m "feat(acm): pure band derivation (effective target + asymmetric water marks)"
```

---

### Task 0.3: `EconomyConfig` capacity/target split

**Files:**
- Modify: `crates/zoid-core/src/config.rs:25-42` (`EconomyConfig` + `Default`), `:86-89` (`Provenance`), `:96-99` (`PartialEconomy`), `:133-136` + `:156-170` (merge), and the config tests `:66-68`, `:295-301`
- Modify: `crates/zoid/src/main.rs` — every `economy.context_ceiling` / `economy.token_ceiling` reference (config screen `:1634-1642`, `:1710-1719`, boot `:1107-1109`, reload `:1900-1902`, fold `:1569-1572`, arg `:121`, tests `:2959-2971`, `:3000-3093`, `:3233-3236`)

**Interfaces:**
- Consumes: `zoid_provider::context_ceiling(model)` (unchanged — this is **capacity**).
- Produces: `EconomyConfig { context_target: Option<u64>, auto_evict_cold: bool, compact_threshold_pct: u8, band_headroom_pct: u8, recent_n: usize }`. `context_target = None` ⇒ default `min(capacity, 384_000)` resolved in the bin.

> **Naming discipline:** the *field* `EconomyConfig.context_ceiling` is renamed to `context_target` (the soft setpoint). The *function* `zoid_provider::context_ceiling(model)` is NOT renamed (it returns **capacity**). `ContextPolicy.token_ceiling` (assembler.rs, subagent-only) is a DIFFERENT field and is left untouched — only `EconomyConfig.token_ceiling` is retired.

- [ ] **Step 1: Update the config tests first.** In `crates/zoid-core/src/config.rs` tests (`:66-68`, `:295-301`), replace `context_ceiling` with `context_target` and drop `token_ceiling` assertions:

```rust
// ~line 66
assert!(c.economy.auto_evict_cold);
assert_eq!(c.economy.compact_threshold_pct, 0);
assert!(c.economy.context_target.is_none());
assert_eq!(c.economy.band_headroom_pct, 20);
assert_eq!(c.economy.recent_n, 4);
```

```rust
// ~line 295-301 (set_in_toml round-trip)
let out = set_in_toml(&out, "economy.context_target", TomlValue::Int(512000)).unwrap();
let p = parse_toml(&out).unwrap();
assert_eq!(p.economy.context_target, Some(512000));
assert_eq!(p.economy.auto_evict_cold, Some(true)); // preserved
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core config::`
Expected: FAIL — `no field context_target on EconomyConfig`.

- [ ] **Step 3: Edit the config types.** In `crates/zoid-core/src/config.rs`:

`EconomyConfig` (`:25`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomyConfig {
    /// The soft setpoint the controller manages toward (tokens). None → the bin
    /// defaults it to min(capacity, 384_000). Renamed from `context_ceiling`.
    pub context_target: Option<u64>,
    pub auto_evict_cold: bool,
    /// 0 disables compaction; else percent of the target (1–100).
    pub compact_threshold_pct: u8,
    /// Eviction band headroom, percent of effective target (default 20).
    pub band_headroom_pct: u8,
    /// Most-recent turns never evictable (default 4).
    pub recent_n: usize,
}
```

`Default` (`:34`):
```rust
impl Default for EconomyConfig {
    fn default() -> Self {
        Self {
            context_target: None,
            auto_evict_cold: true,
            compact_threshold_pct: 0,
            band_headroom_pct: 20,
            recent_n: 4,
        }
    }
}
```

`Provenance` (`:86`): rename `context_ceiling: Source` → `context_target: Source`; remove `token_ceiling: Source`; add `band_headroom_pct: Source`, `recent_n: Source`. Update its `Default`/init at `:133-136`.

`PartialEconomy` (`:96`): rename `context_ceiling: Option<u64>` → `context_target: Option<u64>`; remove `token_ceiling`; add `band_headroom_pct: Option<u8>`, `recent_n: Option<usize>`.

Merge block (`:156-170`): replace the `context_ceiling` branch with `context_target`, delete the `token_ceiling` branch, add branches for `band_headroom_pct` and `recent_n` following the exact same `if let Some(v) = p.economy.<field>` shape.

- [ ] **Step 4: Fix the bin.** In `crates/zoid/src/main.rs`:
  - `:121` `envp.economy.context_ceiling` → `envp.economy.context_target`.
  - `:1107-1109` boot resolve — the **capacity** stays `zoid_provider::context_ceiling(&model)`; the config override now reads `context_target`:
    ```rust
    // capacity is always the model window; the user's target is a separate soft knob.
    let capacity = zoid_provider::context_ceiling(&model);
    let context_target = config.economy.context_target.unwrap_or_else(|| capacity.min(384_000));
    shell.ctx_ceiling = capacity;                 // economy denominator stays capacity
    shell.ctx_ceiling_overridden = false;         // capacity is no longer user-overridden here
    ```
    (Keep whatever `shell` fields exist; the key change is deriving `context_target` separately. Store `context_target` on `shell`/`app` for later use in Task 0.4 — add a field `pub context_target: u64` to the shell/app struct if none exists.)
  - `:1900-1902` reload path — same split.
  - `:1569-1572` `ModelInfoFetched` fold — after `ctx_ceiling` (capacity) updates from `info.context_window`, recompute `context_target` if it was defaulted: `app.context_target = app.config.economy.context_target.unwrap_or_else(|| app.shell.ctx_ceiling.min(384_000));`.
  - Config screen: `:1634` key `"economy.context_ceiling"` → `"economy.context_target"`; delete the `:1642` `"economy.token_ceiling"` row; `:1710` label mapping `"context ceiling" => ("economy.context_ceiling", …)` → `"context target" => ("economy.context_target", opt_u64(econ.context_target))`; delete `:1719` `"token ceiling"` mapping; add rows for `band_headroom_pct` and `recent_n` mirroring the `compact_threshold_pct` integer row at `:1716`.
  - `build_policy` (`:652-659`): drop `token_ceiling: econ.token_ceiling` (the live `ContextPolicy.token_ceiling` becomes `None`); keep `compact_threshold` but compute it against the **target** denominator resolved above, not capacity: pass the resolved `context_target` into `build_policy` (add a param) so `Some(context_target * pct / 100)`.
  - Tests `:2959-2971`, `:3000-3093`, `:3233-3236`: rename `context_ceiling`→`context_target`, drop `token_ceiling`, add the two new fields with default values.

- [ ] **Step 5: Run the workspace tests.**

Run: `cargo test --workspace`
Expected: PASS. (Cross-crate: the TUI config view reads these keys — build with `--workspace` to catch any literal drift.)

- [ ] **Step 6: Commit.**

```bash
git add crates/zoid-core/src/config.rs crates/zoid/src/main.rs
git commit -m "feat(acm): split capacity (model window) from context_target (soft setpoint); retire token_ceiling"
```

---

### Task 0.4: Thread capacity + target into the turn config

**Files:**
- Modify: `crates/zoid/src/agent.rs:50-77` (`TurnConfig` + `chat_turn_config_with`)
- Modify: `crates/zoid-core/src/eviction.rs` — **create the `EvictionPolicy` type here now** (the rest of `eviction.rs` lands in Slice 1); or create a minimal `eviction.rs` with just `EvictionPolicy` in this task and extend it in Slice 1.
- Modify: `crates/zoid/src/main.rs` — the live `TurnConfig` construction site (search for `chat_turn_config_with(` in main.rs) sets `eviction`.

**Interfaces:**
- Produces: `TurnConfig` gains `pub eviction: zoid_core::eviction::EvictionPolicy`.
  - `struct EvictionPolicy { pub enabled: bool, pub capacity: u64, pub context_target: u64, pub band_headroom_pct: u8, pub recent_n: usize, pub max_output: Option<u64> }` (Copy).
  - `EvictionPolicy::disabled()` → `enabled: false`, everything else 0/None. Used by `chat_turn_config_with` so existing tests are byte-unaffected.

- [ ] **Step 1: Create the policy type + failing constructor test.** Create `crates/zoid-core/src/eviction.rs` with just:

```rust
//! Pure eviction controller (spec §3.1). This file grows in Slice 1 (planner,
//! scorer, breadcrumb); Slice 0 lands only the policy the turn config carries.

use crate::band::{derive_band, Band};

/// The live turn's eviction parameters. `enabled: false` is a total bypass
/// (byte-identical to pre-ACM behavior) used by the zero-arg test constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub recent_n: usize,
    pub max_output: Option<u64>,
}

impl EvictionPolicy {
    pub fn disabled() -> Self {
        Self { enabled: false, capacity: 0, context_target: 0, band_headroom_pct: 0, recent_n: 0, max_output: None }
    }
    /// The band for this policy (spec §3.6a).
    pub fn band(&self) -> Band {
        derive_band(self.capacity, self.context_target, self.max_output, self.band_headroom_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_policy_has_zero_band() {
        let b = EvictionPolicy::disabled().band();
        assert_eq!(b.high_water, 0);
    }
    #[test]
    fn enabled_policy_band_matches_derivation() {
        let p = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 384_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
        assert_eq!(p.band().high_water, 384_000);
    }
}
```

Add `pub mod eviction;` to `crates/zoid-core/src/lib.rs`.

- [ ] **Step 2: Add the `TurnConfig` field.** In `crates/zoid/src/agent.rs`, extend `TurnConfig` (`:50`):

```rust
pub struct TurnConfig {
    pub system: String,
    pub cwd: PathBuf,
    pub branch: BranchId,
    pub policy: zoid_core::assembler::ContextPolicy,
    /// Live eviction band parameters. `disabled()` for subagents/tests.
    pub eviction: zoid_core::eviction::EvictionPolicy,
}
```

In `chat_turn_config_with` (`:62`) and any other `TurnConfig { … }` literal in `agent.rs`/`subagent.rs`, add `eviction: zoid_core::eviction::EvictionPolicy::disabled(),`.

- [ ] **Step 3: Wire the bin's live turn.** In `crates/zoid/src/main.rs`, at the live `chat_turn_config_with(...)` call site, set the eviction policy from resolved config + capacity:

```rust
let mut turn_cfg = chat_turn_config_with(profile, &skill_menu);
turn_cfg.eviction = zoid_core::eviction::EvictionPolicy {
    enabled: app.config.economy.compact_threshold_pct > 0, // master switch (back-compat)
    capacity: app.shell.ctx_ceiling,                        // capacity = model window
    context_target: app.context_target,                    // resolved in Task 0.3
    band_headroom_pct: app.config.economy.band_headroom_pct,
    recent_n: app.config.economy.recent_n,
    max_output: None, // Slice-4 catalog supplies this; None → derived reserve
};
```

(Reuse `compact_threshold_pct > 0` as the master ACM enable per spec §3.6, so existing "ACM off" configs stay off.)

- [ ] **Step 4: Run the workspace tests.**

Run: `cargo test --workspace`
Expected: PASS. Behavior is unchanged (eviction planner not built yet; `enabled` only gates Slice 1 code).

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/eviction.rs crates/zoid-core/src/lib.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(acm): carry per-model eviction band params on TurnConfig (disabled by default)"
```

---

# SLICE 1 — Pre-flight eviction gate + breadcrumb + capacity-error retry

*Delivers the reported-bug fix: holds the band across indefinite sessions, bounds per-turn CPU, guarantees ≤ capacity via a bounded retry. **Independently shippable** — evicted turns leave a breadcrumb but aren't retrievable until Slice 2.*

### Task 1.1: Eviction events (`TurnsEvicted`, `TurnsReadmitted`, marker)

**Files:**
- Modify: `crates/zoid-core/src/event.rs:29-96` (`EventKind`), add marker structs

**Interfaces:**
- Produces:
  - `struct EvictedSpan { pub id_range_label: String, pub token_estimate: u64, pub topic_hint: String }`
  - `struct EvictionMarker { pub spans: Vec<EvictedSpan> }`
  - `EventKind::TurnsEvicted { ids: Vec<Ulid>, reclaimed_tokens: u64, marker: EvictionMarker }`
  - `EventKind::TurnsReadmitted { ids: Vec<Ulid> }`

- [ ] **Step 1: Write the failing test.** In `crates/zoid-core/src/event.rs` tests (add a test module fn if needed):

```rust
#[test]
fn turns_evicted_round_trips_json() {
    let m = EvictionMarker { spans: vec![EvictedSpan {
        id_range_label: "turns 1–3".into(), token_estimate: 4200, topic_hint: "read config".into(),
    }]};
    let k = EventKind::TurnsEvicted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 4200, marker: m };
    let s = serde_json::to_string(&k).unwrap();
    let back: EventKind = serde_json::from_str(&s).unwrap();
    assert_eq!(k, back);
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core event::turns_evicted_round_trips_json`
Expected: FAIL — variant/types not defined.

- [ ] **Step 3: Add the types.** In `crates/zoid-core/src/event.rs`, above `EventKind` (or near it), add:

```rust
/// One paged-out span, for the in-context breadcrumb and the audit view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictedSpan {
    pub id_range_label: String,
    pub token_estimate: u64,
    pub topic_hint: String,
}

/// The data an eviction wave renders (transcript) and the model reads (breadcrumb).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionMarker {
    pub spans: Vec<EvictedSpan>,
}
```

Add two variants to `EventKind` (after `ToolResultCompacted`, keeping `TurnsDropped` inert as-is):

```rust
    /// Whole turns paged to the cold tier. Append-only; the original events are
    /// retained (reversible). Projections skip these ids (minus any later
    /// `TurnsReadmitted`). `marker` backs the in-context breadcrumb.
    TurnsEvicted {
        ids: Vec<Ulid>,
        reclaimed_tokens: u64,
        marker: EvictionMarker,
    },
    /// Undo / recall re-admission: projections stop skipping these ids.
    TurnsReadmitted {
        ids: Vec<Ulid>,
    },
```

(`Ulid` is already imported at `event.rs:2`.)

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test --workspace` (EventKind is shared; build wide)
Expected: PASS. Note any non-exhaustive `match` on `EventKind` the compiler flags in `zoid-tui`/`main.rs` and add inert arms (`TurnsEvicted { .. } | TurnsReadmitted { .. } => {}`) where a catch-all doesn't already cover them.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/event.rs
git commit -m "feat(acm): TurnsEvicted/TurnsReadmitted events + eviction marker"
```

---

### Task 1.2: Evicted-id fold + breadcrumb (pure)

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs`

**Interfaces:**
- Produces:
  - `fn evicted_ids(events: &[Event]) -> std::collections::HashSet<Ulid>` — `TurnsEvicted.ids` minus `TurnsReadmitted.ids`.
  - `fn eviction_breadcrumb(events: &[Event]) -> Option<String>` — one out-of-band summary line, or None.

- [ ] **Step 1: Write the failing tests.** Append to `crates/zoid-core/src/eviction.rs`:

```rust
use crate::event::{Event, EventKind, EvictionMarker, EvictedSpan};
use std::collections::HashSet;
use ulid::Ulid;

#[cfg(test)]
mod fold_tests {
    use super::*;

    fn ev(id: u128, kind: EventKind) -> Event { Event::new(Ulid::from(id), None, id as i64, kind) }

    #[test]
    fn evicted_minus_readmitted() {
        let marker = EvictionMarker { spans: vec![] };
        let events = vec![
            ev(10, EventKind::TurnsEvicted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 5, marker: marker.clone() }),
            ev(11, EventKind::TurnsReadmitted { ids: vec![Ulid::from(2u128)] }),
        ];
        let set = evicted_ids(&events);
        assert!(set.contains(&Ulid::from(1u128)));
        assert!(!set.contains(&Ulid::from(2u128))); // re-admitted
    }

    #[test]
    fn breadcrumb_present_when_evicted_absent_when_not() {
        assert!(eviction_breadcrumb(&[]).is_none());
        let events = vec![ev(10, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128)], reclaimed_tokens: 4200,
            marker: EvictionMarker { spans: vec![EvictedSpan { id_range_label: "turns 1–2".into(), token_estimate: 4200, topic_hint: "read config".into() }] },
        })];
        let bc = eviction_breadcrumb(&events).unwrap();
        assert!(bc.contains("recall"));
        assert!(bc.contains("read config"));
    }
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core eviction::fold_tests`
Expected: FAIL — `evicted_ids` / `eviction_breadcrumb` not defined.

- [ ] **Step 3: Implement.** Add to `crates/zoid-core/src/eviction.rs` (module scope, not under `#[cfg(test)]`):

```rust
/// The set of currently-evicted event ids: every `TurnsEvicted.ids`, minus any
/// later `TurnsReadmitted.ids`. Projections skip this set (spec §3.3).
pub fn evicted_ids(events: &[Event]) -> HashSet<Ulid> {
    let mut set = HashSet::new();
    for e in events {
        match &e.kind {
            EventKind::TurnsEvicted { ids, .. } => set.extend(ids.iter().copied()),
            EventKind::TurnsReadmitted { ids } => {
                for id in ids { set.remove(id); }
            }
            _ => {}
        }
    }
    set
}

/// The out-of-band breadcrumb (spec §3.3): a single line appended to the system
/// prompt so the model knows history was paged out and how to reach it. NOT a
/// standalone message (that would break Anthropic alternation). None when the
/// currently-evicted set is empty.
pub fn eviction_breadcrumb(events: &[Event]) -> Option<String> {
    let live = evicted_ids(events);
    if live.is_empty() {
        return None;
    }
    // Fold currently-live spans from TurnsEvicted markers (skip fully-readmitted).
    let mut spans: Vec<&EvictedSpan> = Vec::new();
    let mut turns = 0usize;
    let mut tokens = 0u64;
    for e in events {
        if let EventKind::TurnsEvicted { ids, marker, .. } = &e.kind {
            if ids.iter().any(|id| live.contains(id)) {
                for s in &marker.spans {
                    spans.push(s);
                    turns += 1;
                    tokens += s.token_estimate;
                }
            }
        }
    }
    let topics: Vec<&str> = spans.iter().take(5).map(|s| s.topic_hint.as_str()).collect();
    Some(format!(
        "Earlier context ({turns} spans, ~{}k tokens; topics: {}) has been paged out. \
         Call recall(query) to retrieve any of it.",
        tokens / 1000,
        topics.join(", ")
    ))
}
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core eviction::`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(acm): evicted-id fold + out-of-band breadcrumb (pure)"
```

---

### Task 1.3: `plan_evictions` — the hysteresis controller

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs`

**Interfaces:**
- Consumes: `EvictionPolicy` (Task 0.4), `Band` (Task 0.2), `evicted_ids` (Task 1.2), `estimate_tokens` (`economy.rs:38`).
- Produces:
  - `struct GoalContext {}` (empty; Slice-4 relevance seam)
  - `trait EvictionScorer { fn score(&self, turn: &TurnView, ctx: &GoalContext) -> f32; }`
  - `struct RecencyScorer;` (impl: score = turn index as f32 — higher = newer = keep)
  - `struct TurnView { pub ids: Vec<Ulid>, pub index: usize, pub token_estimate: u64, pub topic_hint: String, pub protected: bool }`
  - `struct EvictedTurn { pub ids: Vec<Ulid>, pub token_estimate: u64, pub topic_hint: String }`
  - `struct EvictionPlan { pub turns: Vec<EvictedTurn> }`
  - `fn plan_evictions(events: &[Event], policy: &EvictionPolicy, current_tokens: u64, scorer: &dyn EvictionScorer) -> EvictionPlan`

- [ ] **Step 1: Write the failing tests.** Append to `crates/zoid-core/src/eviction.rs`:

```rust
#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::event::{Event, EventKind};

    fn user(id: u128, t: &str) -> Event { Event::new(Ulid::from(id), None, id as i64, EventKind::UserMessage { text: t.into() }) }
    fn asst(id: u128, t: &str) -> Event { Event::new(Ulid::from(id), None, id as i64, EventKind::AssistantMessage { text: t.into() }) }

    fn policy(target: u64, recent_n: usize) -> EvictionPolicy {
        EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: target, band_headroom_pct: 20, recent_n, max_output: None }
    }

    #[test]
    fn no_plan_below_high_water() {
        let events = vec![user(1, "a"), asst(2, "b")];
        let plan = plan_evictions(&events, &policy(384_000, 4), 100, &RecencyScorer);
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn evicts_oldest_first_down_to_low_water() {
        // 6 turns, each ~1000 tokens estimate; recent_n=2 protects the last two.
        let big = "x".repeat(3000); // ~1000 tokens (chars/3)
        let mut events = Vec::new();
        for i in 0..6u128 { events.push(user(i*2+1, &big)); events.push(asst(i*2+2, "ok")); }
        // current well over high_water forces a wave; low_water = target - 20%.
        let plan = plan_evictions(&events, &policy(3_000, 2), 6_000, &RecencyScorer);
        assert!(!plan.turns.is_empty());
        // never evicts the protected (newest) turns
        let evicted_ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!evicted_ids.contains(&Ulid::from(11u128))); // 6th user msg (newest)
        // oldest turn is evicted first
        assert!(evicted_ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn idempotent_skips_already_evicted() {
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok"), user(3, &big), asst(4, "ok"), user(5, "recent"), asst(6, "ok")];
        events.push(Event::new(Ulid::from(99u128), None, 99, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 1000, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
        // turn 1 already evicted → not re-selected
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)));
    }

    #[test]
    fn never_evicts_protected_even_if_over() {
        // all turns are recent (recent_n huge) → empty plan even over high_water
        let big = "x".repeat(3000);
        let events = vec![user(1, &big), asst(2, "ok")];
        let plan = plan_evictions(&events, &policy(100, 10), 100_000, &RecencyScorer);
        assert!(plan.turns.is_empty());
    }

    #[test]
    fn readmitted_turn_is_protected_from_re_eviction() {
        // M10: an old, low-recency turn that was re-admitted via recall must not be
        // the immediate next eviction victim.
        let big = "x".repeat(3000);
        let mut events = vec![user(1, &big), asst(2, "ok"), user(3, &big), asst(4, "ok"), user(5, "recent"), asst(6, "ok")];
        // turn 1 was evicted then recalled back.
        events.push(Event::new(Ulid::from(90u128), None, 90, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 1000, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
        events.push(Event::new(Ulid::from(91u128), None, 91, EventKind::TurnsReadmitted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)] }));
        let plan = plan_evictions(&events, &policy(1_000, 2), 5_000, &RecencyScorer);
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        assert!(!ids.contains(&Ulid::from(1u128)), "recalled turn must not immediately re-evict");
    }
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core eviction::plan_tests`
Expected: FAIL — types/fn undefined.

- [ ] **Step 3: Implement the controller.** Add to `crates/zoid-core/src/eviction.rs`:

```rust
use crate::economy::estimate_tokens;

/// Slice-4 relevance context (empty now; keeps the scorer signature stable).
#[derive(Debug, Default)]
pub struct GoalContext {}

/// A candidate turn for eviction, derived positionally from the non-inert log.
#[derive(Debug, Clone)]
pub struct TurnView {
    pub ids: Vec<Ulid>,
    pub index: usize,
    pub token_estimate: u64,
    pub topic_hint: String,
    /// System / recent-N / already-evicted / re-admitted-cooldown → never selected.
    pub protected: bool,
}

/// Victim-selection seam (spec §3.7). Higher score = more worth keeping.
pub trait EvictionScorer {
    fn score(&self, turn: &TurnView, ctx: &GoalContext) -> f32;
}

/// Default: recency (newer index kept). Deterministic and safe.
pub struct RecencyScorer;
impl EvictionScorer for RecencyScorer {
    fn score(&self, turn: &TurnView, _ctx: &GoalContext) -> f32 {
        turn.index as f32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictedTurn {
    pub ids: Vec<Ulid>,
    pub token_estimate: u64,
    pub topic_hint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictionPlan {
    pub turns: Vec<EvictedTurn>,
}

/// Is this event inert for turn-grouping (never starts/joins a conversational turn)?
fn is_inert(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Usage
            | EventKind::ContextMutation { .. }
            | EventKind::ToolResultCompacted { .. }
            | EventKind::Tasks { .. }
            | EventKind::TurnsDropped { .. }
            | EventKind::TurnsEvicted { .. }
            | EventKind::TurnsReadmitted { .. }
    )
}

/// The estimated token cost of one event's payload (chars/3), 0 for inert.
fn event_tokens(kind: &EventKind) -> u64 {
    match kind {
        EventKind::UserMessage { text }
        | EventKind::AssistantMessage { text }
        | EventKind::ModelDelta { text } => estimate_tokens(text),
        EventKind::ToolCall { args, name, .. } => estimate_tokens(args) + estimate_tokens(name),
        EventKind::ToolResult { output, .. } => estimate_tokens(output),
        EventKind::DelegationResult { summary, .. } => estimate_tokens(summary),
        _ => 0,
    }
}

/// Group the main-branch, non-inert log into positional turns. A turn begins at
/// each `UserMessage` (spec §3.1 / M6: grouping is over the non-inert projection,
/// so an interleaved inert event can't fragment a tool_use/tool_result pair).
fn group_turns(events: &[Event], evicted: &HashSet<Ulid>, recent_n: usize) -> Vec<TurnView> {
    let mut turns: Vec<TurnView> = Vec::new();
    for e in events {
        if e.branch != crate::event::BranchId::default() || is_inert(&e.kind) {
            continue;
        }
        let starts_turn = matches!(e.kind, EventKind::UserMessage { .. });
        if starts_turn || turns.is_empty() {
            let topic_hint = match &e.kind {
                EventKind::UserMessage { text } => text.lines().next().unwrap_or("").chars().take(60).collect(),
                _ => String::new(),
            };
            turns.push(TurnView { ids: Vec::new(), index: turns.len(), token_estimate: 0, topic_hint, protected: false });
        }
        let t = turns.last_mut().unwrap();
        t.ids.push(e.id);
        t.token_estimate += event_tokens(&e.kind);
    }
    // M10 (spec §3.1): a turn re-admitted via recall is protected from immediate
    // re-eviction, so recall→evict→recall can't oscillate. (v1 simplification:
    // permanent protection rather than a timed cooldown — safe, and §6 handles the
    // case where protected content alone exceeds the band.)
    let readmitted: HashSet<Ulid> = events
        .iter()
        .flat_map(|e| match &e.kind {
            EventKind::TurnsReadmitted { ids } => ids.clone(),
            _ => Vec::new(),
        })
        .collect();
    let n = turns.len();
    for (i, t) in turns.iter_mut().enumerate() {
        let is_recent = i + recent_n >= n;
        let is_evicted = t.ids.iter().any(|id| evicted.contains(id));
        let is_readmitted = t.ids.iter().any(|id| readmitted.contains(id));
        t.protected = is_recent || is_evicted || is_readmitted;
    }
    turns
}

/// Plan an eviction wave (spec §3.1). Empty unless `current_tokens >= high_water`.
/// Ranks evictable turns by `scorer` (lowest first), evicting until
/// `current_tokens - reclaimed <= low_water`, never touching protected turns.
pub fn plan_evictions(
    events: &[Event],
    policy: &EvictionPolicy,
    current_tokens: u64,
    scorer: &dyn EvictionScorer,
) -> EvictionPlan {
    if !policy.enabled {
        return EvictionPlan::default();
    }
    let band = policy.band();
    if current_tokens < band.high_water {
        return EvictionPlan::default();
    }
    let evicted = evicted_ids(events);
    let turns = group_turns(events, &evicted, policy.recent_n);
    let ctx = GoalContext::default();

    let mut candidates: Vec<&TurnView> = turns.iter().filter(|t| !t.protected && !t.ids.is_empty()).collect();
    candidates.sort_by(|a, b| {
        scorer.score(a, &ctx).partial_cmp(&scorer.score(b, &ctx)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut reclaimed = 0u64;
    let mut plan = EvictionPlan::default();
    for t in candidates {
        if current_tokens.saturating_sub(reclaimed) <= band.low_water {
            break;
        }
        reclaimed += t.token_estimate;
        plan.turns.push(EvictedTurn { ids: t.ids.clone(), token_estimate: t.token_estimate, topic_hint: t.topic_hint.clone() });
    }
    plan
}
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core eviction::`
Expected: PASS (all `plan_tests` + earlier fold/policy tests).

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "feat(acm): plan_evictions hysteresis controller (recency scorer, turn grouping, protection)"
```

---

### Task 1.4: Projections skip evicted ids

**Files:**
- Modify: `crates/zoid-core/src/projection.rs:57-93` (`conversation`)
- Modify: `crates/zoid-core/src/context.rs:173` (`context_window_with` fold loop)

**Interfaces:**
- Consumes: `eviction::evicted_ids`.
- Produces: `conversation(events)` and `context_window_with(events, overhead)` both exclude currently-evicted ids.

- [ ] **Step 1: Write the failing tests.** In `crates/zoid-core/src/projection.rs` tests:

```rust
#[test]
fn conversation_skips_evicted_turns() {
    use crate::event::{Event, EventKind, EvictionMarker};
    use ulid::Ulid;
    let mk = |id: u128, k| Event::new(Ulid::from(id), None, id as i64, k);
    let events = vec![
        mk(1, EventKind::UserMessage { text: "old".into() }),
        mk(2, EventKind::AssistantMessage { text: "old-reply".into() }),
        mk(3, EventKind::UserMessage { text: "new".into() }),
        mk(9, EventKind::TurnsEvicted { ids: vec![Ulid::from(1u128), Ulid::from(2u128)], reclaimed_tokens: 5, marker: EvictionMarker { spans: vec![] } }),
    ];
    let msgs = conversation(&events);
    assert_eq!(msgs.len(), 1); // only the "new" user message survives
    assert!(matches!(&msgs[0], ChatMsg::User { text, .. } if text == "new"));
}
```

And in `crates/zoid-core/src/context.rs` tests, the token-exclusion counterpart:

```rust
#[test]
fn context_window_excludes_evicted_tokens() {
    use crate::event::{Event, EventKind, EvictionMarker};
    use ulid::Ulid;
    let big = "x".repeat(3000); // ~1000 tokens
    let base = vec![Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: big.clone() })];
    let with_evict = vec![
        base[0].clone(),
        Event::new(Ulid::from(9u128), None, 9, EventKind::TurnsEvicted { ids: vec![Ulid::from(1u128)], reclaimed_tokens: 1000, marker: EvictionMarker { spans: vec![] } }),
    ];
    let full = context_window_with(&base, ContextOverhead::default()).total_tokens;
    let after = context_window_with(&with_evict, ContextOverhead::default()).total_tokens;
    assert!(after < full, "evicted event's tokens must be excluded from the window");
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core conversation_skips_evicted_turns`
Expected: FAIL — evicted "old" message still projected.

- [ ] **Step 3: Implement the skips.**
In `crates/zoid-core/src/projection.rs::conversation`, after `let visible: &[Event] = events;` (`:58`), add:

```rust
    let evicted = crate::eviction::evicted_ids(events);
```

and at the top of the main fold loop (`for e in visible {` at `:91`), add before the branch check:

```rust
        if evicted.contains(&e.id) {
            continue;
        }
```

In `crates/zoid-core/src/context.rs::context_window_with`, compute `let evicted = crate::eviction::evicted_ids(events);` once at the top of the fold and `continue` on `evicted.contains(&e.id)` in the per-event loop (mirror the projection change; the loop walks events at `:173+`).

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test --workspace`
Expected: PASS. (Guard: `conversation` de-dup and existing compaction tests must stay green — evicted skip runs before the compaction-map application.)

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/projection.rs crates/zoid-core/src/context.rs
git commit -m "feat(acm): conversation + context_window skip evicted ids (bounds request and byte-walk together)"
```

---

### Task 1.5: Breadcrumb into `build_request`

**Files:**
- Modify: `crates/zoid/src/agent.rs:164-177` (`build_request`)

**Interfaces:**
- Consumes: `eviction::eviction_breadcrumb`.
- Produces: `build_request` appends the breadcrumb to the system prompt when any turn is evicted; otherwise byte-identical output.

- [ ] **Step 1: Write the failing test.** In `crates/zoid/src/agent.rs` tests:

```rust
#[test]
fn build_request_appends_breadcrumb_when_evicted() {
    use zoid_core::event::{Event, EventKind, EvictionMarker, EvictedSpan};
    use ulid::Ulid;
    let events = vec![
        Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "hi".into() }),
        Event::new(Ulid::from(9u128), None, 9, EventKind::TurnsEvicted {
            ids: vec![Ulid::from(1u128)], reclaimed_tokens: 4200,
            marker: EvictionMarker { spans: vec![EvictedSpan { id_range_label: "t1".into(), token_estimate: 4200, topic_hint: "setup".into() }] },
        }),
    ];
    let req = build_request(&events, "m", &zoid_tools::registry(), "SYS");
    let sys = req.system.unwrap();
    assert!(sys.starts_with("SYS"));
    assert!(sys.contains("recall"));
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid build_request_appends_breadcrumb_when_evicted`
Expected: FAIL — system is exactly "SYS".

- [ ] **Step 3: Implement.** In `build_request` (`:164`), build the system string with the breadcrumb:

```rust
pub fn build_request(
    events: &[Event],
    model: &str,
    tools: &[Box<dyn Tool>],
    system: &str,
) -> CompletionRequest {
    let system = match zoid_core::eviction::eviction_breadcrumb(events) {
        Some(bc) => format!("{system}\n\n{bc}"),
        None => system.to_string(),
    };
    CompletionRequest {
        model: model.to_string(),
        system: Some(system),
        messages: conversation(events).into_iter().map(map_msg).collect(),
        max_tokens: 4096,
        tools: tool_specs(tools),
    }
}
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid`
Expected: PASS (including the existing `build_request_uses_the_given_system_prompt`, which has no evicted events → unchanged path).

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(acm): breadcrumb appended out-of-band to system prompt (Anthropic-safe)"
```

---

### Task 1.6: The pre-flight gate (compact + evict before send)

**Files:**
- Modify: `crates/zoid/src/agent.rs` — add `preflight_gate`; call it at the top of `'turn: loop` before `build_request` (`:320`)

**Interfaces:**
- Consumes: `EvictionPolicy` (via `config.eviction`), `plan_evictions`, `plan_compactions`, `context_window_with`, `estimate_tokens`, `RecencyScorer`.
- Produces: `async fn preflight_gate(session, events, ui, config, session_id, now, calibration_ratio, overhead) -> Result<()>` — mutates `events` by emitting `ToolResultCompacted` then `TurnsEvicted` until the biased estimate is within band, running BEFORE the request is built each sub-turn.

- [ ] **Step 1: Write the failing test.** In `crates/zoid/src/agent.rs` tests, a steady-state style test that drives a scripted turn whose seed log is already over `high_water` and asserts the pre-flight gate appended a `TurnsEvicted` event *before* the (fake) send, and that the sent request's conversation length dropped. Sketch:

```rust
#[tokio::test]
async fn preflight_gate_evicts_before_send() {
    use zoid_core::event::{Event, EventKind};
    use ulid::Ulid;
    // 8 fat turns, target tiny so the gate must evict.
    let big = "x".repeat(3000);
    let mut seed = Vec::new();
    for i in 0..8u128 { seed.push(Event::new(Ulid::from(i*2+1), None, (i*2+1) as i64, EventKind::UserMessage { text: big.clone() })); seed.push(Event::new(Ulid::from(i*2+2), None, (i*2+2) as i64, EventKind::AssistantMessage { text: "ok".into() })); }
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    for e in &seed { session.append(e.clone()).await.unwrap(); }
    let provider = std::sync::Arc::new(zoid_provider::FakeProvider::new(vec![zoid_provider::ProviderEvent::TextDelta("done".into()), zoid_provider::ProviderEvent::Done]));
    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 3_000, band_headroom_pct: 20, recent_n: 2, max_output: None };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} }); // drain UI updates
    let out = run_agent_turn(cfg, provider, std::sync::Arc::new(zoid_tools::registry()), std::sync::Arc::new(zoid_tools::AllowAll), session, seed, "m".into(), tx, Ulid::new(), || 0).await.unwrap();
    assert!(out.iter().any(|e| matches!(e.kind, EventKind::TurnsEvicted { .. })), "gate must evict pre-flight");
    // and the surviving conversation is under the seed size
    assert!(zoid_core::projection::conversation(&out).len() < 16);
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid preflight_gate_evicts_before_send`
Expected: FAIL — no `TurnsEvicted` emitted (gate not implemented).

- [ ] **Step 3: Implement `preflight_gate` + call it.** Add the function near `record_compactions` in `crates/zoid/src/agent.rs`:

```rust
/// Bias applied to the pre-flight estimate (the chars/3 estimate under-reads
/// code/tool output). Push the estimate up so the gate fires early, not late.
const OVERCOUNT_BIAS: f64 = 1.15;

/// Run the cheap correctness levers BEFORE the request is built (spec §3.8, C1):
/// (1) compact tool results, (2) evict oldest turns to `low_water`, (3) if near
/// hard capacity, evict harder toward the safety floor. Emits `ToolResultCompacted`
/// / `TurnsEvicted` events (append-only). No-op when `config.eviction.enabled` is
/// false (subagents/tests) — byte-identical to pre-ACM behavior.
#[allow(clippy::too_many_arguments)]
async fn preflight_gate(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
    calibration_ratio: &Option<f64>,
    overhead: &zoid_core::context::ContextOverhead,
) -> Result<()> {
    let policy = &config.eviction;
    if !policy.enabled {
        return Ok(());
    }
    let band = policy.band();

    let estimate = |events: &[Event]| -> u64 {
        let raw = zoid_core::context::context_window_with(events, overhead.clone()).total_tokens;
        let scaled = match calibration_ratio {
            Some(r) if *r > 0.0 => (raw as f64 * r) as u64,
            _ => raw,
        };
        (scaled as f64 * OVERCOUNT_BIAS) as u64
    };

    // (1) Compaction first (largest-first; spec §3.9 rule 2). Reuse plan_compactions
    // with the band's high_water as the threshold.
    if estimate(events) >= band.high_water {
        let gate_policy = zoid_core::assembler::ContextPolicy {
            compact_threshold: Some(band.high_water),
            ..config.policy
        };
        let plan = zoid_core::compaction::plan_compactions(events, &gate_policy, None, *calibration_ratio, overhead);
        for c in &plan.compactions {
            emit(session, events, ui, &config.branch, EventKind::ToolResultCompacted {
                id: c.id.clone(), summary: c.summary.clone(), original_tokens: c.original_tokens,
            }, session_id, now).await?;
        }
    }

    // (2) Eviction to low_water.
    if estimate(events) >= band.high_water {
        let plan = zoid_core::eviction::plan_evictions(events, policy, estimate(events), &zoid_core::eviction::RecencyScorer);
        emit_eviction(session, events, ui, config, session_id, now, plan).await?;
    }

    // (3) Hard floor: if still near capacity, evict harder toward the safety margin.
    let hard = policy.capacity.saturating_sub(zoid_core::band::CAPACITY_SAFETY_MARGIN);
    if estimate(events) >= hard {
        // Re-run with the same policy; low_water already targets below capacity.
        let plan = zoid_core::eviction::plan_evictions(events, policy, estimate(events), &zoid_core::eviction::RecencyScorer);
        emit_eviction(session, events, ui, config, session_id, now, plan).await?;
    }
    Ok(())
}

/// Emit one `TurnsEvicted` event carrying the plan's spans (or nothing if empty).
#[allow(clippy::too_many_arguments)]
async fn emit_eviction(
    session: &SessionHandle,
    events: &mut Vec<Event>,
    ui: &mpsc::Sender<AgentUpdate>,
    config: &TurnConfig,
    session_id: Ulid,
    now: fn() -> i64,
    plan: zoid_core::eviction::EvictionPlan,
) -> Result<()> {
    if plan.turns.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::new();
    let mut reclaimed = 0u64;
    let mut spans = Vec::new();
    for t in plan.turns {
        reclaimed += t.token_estimate;
        spans.push(zoid_core::event::EvictedSpan {
            id_range_label: format!("{} events", t.ids.len()),
            token_estimate: t.token_estimate,
            topic_hint: t.topic_hint,
        });
        ids.extend(t.ids);
    }
    emit(session, events, ui, &config.branch, EventKind::TurnsEvicted {
        ids, reclaimed_tokens: reclaimed, marker: zoid_core::event::EvictionMarker { spans },
    }, session_id, now).await?;
    Ok(())
}
```

Then, in `run_turn_inner`, insert the gate call at the top of `'turn: loop`, right after the cancellation check and **before** `let req = build_request(...)` (`:320`):

```rust
        // PRE-FLIGHT GATE (spec §3.8): shrink to fit BEFORE building the request.
        preflight_gate(&session, &mut events, ui, config, session_id, now, calibration_ratio, overhead).await?;
        let req = build_request(&events, &model, &tools, &config.system);
```

(`calibration_ratio` is `&mut Option<f64>` in scope; pass `&*calibration_ratio`. `overhead` and `config` are already borrowed in the loop.)

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test --workspace`
Expected: PASS (new gate test + all existing agent tests — `enabled=false` default keeps them unchanged).

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(acm): pre-flight gate — compact+evict before the request is built (C1)"
```

---

### Task 1.7: Capacity-error retry (the hard-bound backstop)

**Files:**
- Modify: `crates/zoid-provider/src/lib.rs` — add `is_context_length_error`
- Modify: `crates/zoid/src/agent.rs` — retry counter + `ProviderEvent::Error` arm (`:381-398`)

**Interfaces:**
- Produces: `zoid_provider::is_context_length_error(msg: &str) -> bool`.
- Behavior: on a context-length error with retries remaining, force an eviction wave and `continue 'turn` (re-build a smaller request) instead of surfacing the error; bounded by `MAX_CONTEXT_RETRIES`.

- [ ] **Step 1: Write the failing tests.** In `crates/zoid-provider/src/lib.rs` tests:

```rust
#[test]
fn detects_context_length_errors() {
    assert!(is_context_length_error("prompt is too long: 1050000 tokens > 1000000 maximum"));
    assert!(is_context_length_error("This model's maximum context length is 200000 tokens"));
    assert!(is_context_length_error("input length exceeds context window"));
    assert!(!is_context_length_error("rate limit exceeded"));
    assert!(!is_context_length_error("connection reset"));
}
```

In `crates/zoid/src/agent.rs` tests, define a **`SequencedProvider`** test double (a provider that replays a *different* script on each successive `stream()` call — the stateless `FakeProvider` can't test a retry). This double is defined once here and **reused by Tasks 2.5 and 2.7** (same test module). Then a turn whose provider errors with a context-length message first, and completes on the retry:

```rust
// Test double: replays a different script per stream() call (retry / multi-request turns).
struct SequencedProvider {
    scripts: std::sync::Mutex<std::collections::VecDeque<Vec<zoid_provider::ProviderEvent>>>,
}
impl SequencedProvider {
    fn new(scripts: Vec<Vec<zoid_provider::ProviderEvent>>) -> Self {
        Self { scripts: std::sync::Mutex::new(scripts.into_iter().collect()) }
    }
}
#[async_trait::async_trait]
impl zoid_provider::Provider for SequencedProvider {
    async fn stream(
        &self,
        _req: &zoid_provider::CompletionRequest,
        sink: tokio::sync::mpsc::Sender<zoid_provider::ProviderEvent>,
    ) -> anyhow::Result<()> {
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        for ev in script {
            if sink.send(ev).await.is_err() { break; }
        }
        Ok(())
    }
}

#[tokio::test]
async fn context_length_error_is_retried_not_surfaced() {
    use zoid_provider::ProviderEvent;
    use zoid_core::event::{Event, EventKind};
    use ulid::Ulid;
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "hi".into() })];
    for e in &seed { session.append(e.clone()).await.unwrap(); }
    // First stream errors with a context-length message; the retry completes.
    let provider = std::sync::Arc::new(SequencedProvider::new(vec![
        vec![ProviderEvent::Error("prompt is too long: exceeds context window".into())],
        vec![ProviderEvent::TextDelta("recovered".into()), ProviderEvent::Done],
    ]));
    let mut cfg = chat_turn_config();
    // enabled so the retry arm is active; band huge so the preflight gate itself evicts nothing
    // — this isolates the capacity-error retry path.
    cfg.eviction = zoid_core::eviction::EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 900_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let out = run_agent_turn(cfg, provider, std::sync::Arc::new(zoid_tools::registry()), std::sync::Arc::new(zoid_tools::AllowAll), session, seed, "m".into(), tx, Ulid::new(), || 0).await.unwrap();
    // The context error was retried, not surfaced as a ⚠ message …
    assert!(!out.iter().any(|e| matches!(&e.kind, EventKind::AssistantMessage { text } if text.starts_with(WARN_GLYPH))), "context error must not surface");
    // … and the retry reached the second, successful stream.
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::ModelDelta { text } if text == "recovered")), "retry must reach the successful stream");
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-provider detects_context_length_errors`
Expected: FAIL — `is_context_length_error` undefined.

- [ ] **Step 3: Implement the classifier.** In `crates/zoid-provider/src/lib.rs`:

```rust
/// Heuristic: does a provider error string indicate the request exceeded the
/// model's context window? Both Anthropic ("prompt is too long", "maximum
/// context length") and Ollama/OpenAI-shape ("context length", "context window")
/// surface these in the error body. Used by the agent's bounded capacity-error
/// retry (the hard-bound backstop for a fallible pre-flight estimate).
pub fn is_context_length_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("too long")
        || m.contains("context length")
        || m.contains("context window")
        || m.contains("maximum context")
        || (m.contains("context") && m.contains("exceed"))
}
```

- [ ] **Step 4: Implement the retry.** In `run_turn_inner`, add a counter near `iterations` (`:311`): `let mut context_retries: u32 = 0;`. In the `ProviderEvent::Error(msg)` arm (`:381`), branch before emitting the warning:

```rust
                ProviderEvent::Error(msg) => {
                    let _ = stream_task.await;
                    if zoid_provider::is_context_length_error(&msg)
                        && context_retries < MAX_CONTEXT_RETRIES
                        && config.eviction.enabled
                    {
                        context_retries += 1;
                        // The estimate under-read reality: force a wave toward low_water and retry.
                        let est = zoid_core::context::context_window_with(&events, overhead.clone()).total_tokens;
                        let plan = zoid_core::eviction::plan_evictions(&events, &config.eviction, est, &zoid_core::eviction::RecencyScorer);
                        emit_eviction(&session, &mut events, ui, config, session_id, now, plan).await?;
                        tracing::warn!(ctx = "provider", "context-length error; forced eviction, retrying ({context_retries}/{MAX_CONTEXT_RETRIES})");
                        continue 'turn;
                    }
                    emit(&session, &mut events, ui, &config.branch, EventKind::AssistantMessage {
                        text: format!("{WARN_GLYPH} {msg}"),
                    }, session_id, now).await?;
                    tracing::warn!(ctx = "provider", message = msg.as_str(), "turn error");
                    outcome = "error";
                    break 'turn;
                }
```

Add near `MAX_TOOL_ITERATIONS` (`:87`): `pub const MAX_CONTEXT_RETRIES: u32 = 3;`.

(Note: the `let _ = stream_task.await;` moves to the top of the arm so both the retry and the error path drain the provider task.)

- [ ] **Step 5: Run tests to verify pass.**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add crates/zoid-provider/src/lib.rs crates/zoid/src/agent.rs
git commit -m "feat(acm): bounded capacity-error retry — force eviction + retry on context-length error (C1 backstop)"
```

---

### Task 1.8: Steady-state + model-switch property tests

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (property tests over the pure planner — no async, deterministic)

**Interfaces:**
- Consumes: `plan_evictions`, `derive_band`, `context_window_with`.

- [ ] **Step 1: Write the property tests.** Append to `crates/zoid-core/src/eviction.rs`:

```rust
#[cfg(test)]
mod steady_state_tests {
    use super::*;
    use crate::event::{Event, EventKind};
    use crate::context::{context_window_with, ContextOverhead};

    fn apply(events: &mut Vec<Event>, plan: &EvictionPlan, seq: &mut u128) {
        if plan.turns.is_empty() { return; }
        let ids: Vec<Ulid> = plan.turns.iter().flat_map(|t| t.ids.clone()).collect();
        *seq += 1;
        events.push(Event::new(Ulid::from(*seq + 1_000_000), None, *seq as i64, EventKind::TurnsEvicted {
            ids, reclaimed_tokens: 0, marker: crate::event::EvictionMarker { spans: vec![] },
        }));
    }

    #[test]
    fn holds_band_over_hundreds_of_turns() {
        let big = "x".repeat(3000); // ~1000 tokens
        let policy = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 20_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
        let band = policy.band();
        let overhead = ContextOverhead::default();
        let mut events: Vec<Event> = Vec::new();
        let mut seq = 0u128;
        for turn in 0..400u128 {
            events.push(Event::new(Ulid::from(turn*2+1), None, (turn*2+1) as i64, EventKind::UserMessage { text: big.clone() }));
            events.push(Event::new(Ulid::from(turn*2+2), None, (turn*2+2) as i64, EventKind::AssistantMessage { text: "ok".into() }));
            let live = context_window_with(&events, overhead.clone()).total_tokens;
            let plan = plan_evictions(&events, &policy, live, &RecencyScorer);
            apply(&mut events, &plan, &mut seq);
            let after = context_window_with(&events, overhead.clone()).total_tokens;
            // HARD: never exceeds capacity.
            assert!(after <= policy.capacity, "turn {turn}: {after} > capacity");
            // SOFT: with evictable content present, stays at/under high_water after a wave.
            // (Allow one turn of overshoot before the next wave; assert within high_water + one turn.)
            assert!(after <= band.high_water + 1_100, "turn {turn}: {after} over band");
        }
    }
}
```

- [ ] **Step 2: Run to verify pass** (the planner already exists; this test guards it).

Run: `cargo test -p zoid-core steady_state_tests`
Expected: PASS. If the soft bound trips, the bug is in `plan_evictions`, not the test — fix the planner.

- [ ] **Step 3: Commit.**

```bash
git add crates/zoid-core/src/eviction.rs
git commit -m "test(acm): steady-state property test — holds the band over 400 turns, never exceeds capacity"
```

**Slice 1 checkpoint:** the reported bug is fixed and independently shippable. Evicted turns leave a breadcrumb; retrieval lands in Slice 2.

---

# SLICE 2 — Recall over the cold tier (FTS5) + ML seams

*Delivers demand-paging: `recall(query)` searches all indexed events via BM25 and re-admits matching turns. Declares the embedding/reranking seams (`None`) and reserves the `event_embeddings` table so Slice 4 is additive.*

### Task 2.1: Prove FTS5 is available + create the index table

**Files:**
- Modify: `crates/zoid-core/src/store.rs:14-42` (`open` — add the FTS table)

**Interfaces:**
- Produces: an `events_fts` FTS5 virtual table created at `open`.

- [ ] **Step 1: Write the failing/availability test.** In `crates/zoid-core/src/store.rs` tests:

```rust
#[test]
fn fts5_virtual_table_is_available() {
    let store = EventStore::open(":memory:").unwrap();
    // If FTS5 is compiled in, this query against the events_fts table succeeds.
    let n: i64 = store.conn.query_row("SELECT count(*) FROM events_fts", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core fts5_virtual_table_is_available`
Expected: FAIL — `no such table: events_fts`.

- [ ] **Step 3: Add the FTS table.** In `EventStore::open` (`:16`), append to the `execute_batch` schema:

```sql
            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                content,
                event_id UNINDEXED
            );
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core fts5_virtual_table_is_available`
Expected: PASS.

> **If it FAILS with "no such module: fts5":** the bundled build didn't compile FTS5. Add `"fts5"` to the rusqlite features in the root `Cargo.toml` (`rusqlite = { version = "0.32", features = ["bundled", "fts5"] }`), re-run. (`bundled` normally includes FTS5, so this fallback is rarely needed.)

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(acm): FTS5 events_fts virtual table (recall corpus)"
```

---

### Task 2.2: Index every event at append (atomic)

**Files:**
- Modify: `crates/zoid-core/src/store.rs:44-62` (`append`), add `fts_content`

**Interfaces:**
- Produces: `EventStore::append` writes the events row + an `events_fts` row in one transaction; content-less events (Usage, markers) insert no FTS row.

- [ ] **Step 1: Write the failing test.** In `store.rs` tests:

```rust
#[test]
fn append_indexes_searchable_content() {
    let store = EventStore::open(":memory:").unwrap();
    let e = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "how do I configure the ceiling".into() });
    store.append(&e).unwrap();
    let hits = store.search_fts("ceiling", 10).unwrap();
    assert_eq!(hits, vec![Ulid::from(1u128)]);
}
```

*(This also exercises Task 2.3's `search_fts`; write both together — they're one deliverable.)*

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core append_indexes_searchable_content`
Expected: FAIL — `search_fts` undefined / no rows indexed.

- [ ] **Step 3: Implement content extraction + atomic append.** In `crates/zoid-core/src/store.rs`, add:

```rust
/// The searchable text of an event, or None for content-less events (Usage,
/// eviction markers, tasks). Indexed into `events_fts` at append.
fn fts_content(kind: &crate::event::EventKind) -> Option<String> {
    use crate::event::EventKind::*;
    match kind {
        UserMessage { text } | AssistantMessage { text } | ModelDelta { text } => Some(text.clone()),
        ToolResult { output, name, .. } => Some(format!("{name}\n{output}")),
        ToolCall { name, args, .. } => Some(format!("{name} {args}")),
        DelegationResult { summary, .. } => Some(summary.clone()),
        _ => None,
    }
}
```

Rewrite `append` to write both tables atomically (single-writer actor already serializes, so an unchecked transaction is safe):

```rust
    pub fn append(&self, event: &Event) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (id, parent, branch, session_id, ts, kind, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.id.to_string(),
                event.parent.map(|p| p.to_string()),
                event.branch.0,
                event.session_id.to_string(),
                event.ts,
                serde_json::to_string(&event.kind)?,
                event.tokens.map(|t| serde_json::to_string(&t)).transpose()?,
            ],
        )?;
        if let Some(content) = fts_content(&event.kind) {
            tx.execute(
                "INSERT INTO events_fts (content, event_id) VALUES (?1, ?2)",
                params![content, event.id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core store::`
Expected: PASS (append round-trip tests still pass; new indexing test passes).

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(acm): index event content into FTS5 atomically at append"
```

---

### Task 2.3: `search_fts` + `events_by_ids`

**Files:**
- Modify: `crates/zoid-core/src/store.rs`

**Interfaces:**
- Produces:
  - `EventStore::search_fts(&self, query: &str, limit: usize) -> Result<Vec<Ulid>>` — BM25-ranked event ids.
  - `EventStore::events_by_ids(&self, ids: &[Ulid]) -> Result<Vec<Event>>` — full events for the hits, append order.

- [ ] **Step 1: Write the failing test.** In `store.rs` tests:

```rust
#[test]
fn search_ranks_and_loads_events() {
    let store = EventStore::open(":memory:").unwrap();
    store.append(&Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "database indexing strategy".into() })).unwrap();
    store.append(&Event::new(Ulid::from(2u128), None, 2, EventKind::UserMessage { text: "unrelated small talk".into() })).unwrap();
    let ids = store.search_fts("indexing", 10).unwrap();
    assert_eq!(ids, vec![Ulid::from(1u128)]);
    let evs = store.events_by_ids(&ids).unwrap();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].id, Ulid::from(1u128));
}

#[test]
fn search_bad_query_does_not_panic() {
    let store = EventStore::open(":memory:").unwrap();
    // FTS5 special chars must not blow up recall.
    assert!(store.search_fts("\"unbalanced", 5).is_ok() || store.search_fts("\"unbalanced", 5).is_err());
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core search_ranks_and_loads_events`
Expected: FAIL — methods undefined.

- [ ] **Step 3: Implement.** In `crates/zoid-core/src/store.rs`:

```rust
    /// BM25-ranked recall over all indexed content. Returns matching event ids,
    /// best-first. The query is passed to FTS5 wrapped in double quotes so a raw
    /// user string can't be interpreted as FTS syntax (quotes inside are escaped).
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<Ulid>> {
        let safe = format!("\"{}\"", query.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT event_id FROM events_fts WHERE events_fts MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![safe, limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?.parse()?);
        }
        Ok(out)
    }

    /// Load full events for `ids`, in append (rowid) order. Ids not present are skipped.
    pub fn events_by_ids(&self, ids: &[Ulid]) -> Result<Vec<Event>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!("{} WHERE id IN ({placeholders}) ORDER BY rowid ASC", Self::SELECT_COLS);
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        Self::decode_rows(&mut stmt, rusqlite::params_from_iter(params.iter()))
    }
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core store::`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/store.rs
git commit -m "feat(acm): search_fts (BM25) + events_by_ids on EventStore"
```

---

### Task 2.4: `Cmd::Recall` on the session actor

**Files:**
- Modify: `crates/zoid-core/src/session.rs:9-42` (`Cmd`), `:65-105` (handler), add `SessionHandle::recall`

**Interfaces:**
- Produces: `SessionHandle::recall(&self, query: String, limit: usize) -> Result<Vec<Event>>` — runs `search_fts` then `events_by_ids` inside the actor thread.

- [ ] **Step 1: Write the failing test.** In `crates/zoid-core/src/session.rs` tests:

```rust
#[tokio::test]
async fn recall_finds_indexed_events() {
    let h = SessionHandle::spawn(":memory:").unwrap();
    h.append(Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "vector search backend".into() })).await.unwrap();
    h.append(Event::new(Ulid::from(2u128), None, 2, EventKind::UserMessage { text: "hello".into() })).await.unwrap();
    let hits = h.recall("vector".into(), 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, Ulid::from(1u128));
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-core recall_finds_indexed_events`
Expected: FAIL — `recall` / `Cmd::Recall` undefined.

- [ ] **Step 3: Implement.** In `crates/zoid-core/src/session.rs`:

Add a `Cmd` variant (`:41`, inside the enum):
```rust
    Recall {
        query: String,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<Event>>>,
    },
```

Add the handler in the actor `match cmd` (`:104`, before the closing brace):
```rust
                    Cmd::Recall { query, limit, reply } => {
                        let out = store.search_fts(&query, limit).and_then(|ids| store.events_by_ids(&ids));
                        let _ = reply.send(out);
                    }
```

Add the method on `SessionHandle`:
```rust
    /// Search the cold tier (BM25 via FTS5) and load matching events, best-first.
    pub async fn recall(&self, query: String, limit: usize) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Recall { query, limit, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cargo test -p zoid-core session::`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/zoid-core/src/session.rs
git commit -m "feat(acm): Cmd::Recall + SessionHandle::recall (FTS query through the single-writer actor)"
```

---

### Task 2.5: `recall` tool spec + in-loop execution + re-admission

**Files:**
- Create: `crates/zoid-tools/src/recall.rs` (spec-only, `Emitting`)
- Modify: `crates/zoid-tools/src/lib.rs` (register the module; do NOT add to `registry()` — it's a Chat tool)
- Modify: `crates/zoid/src/invoke_skill.rs` (`chat_tools`) — push `Recall`
- Modify: `crates/zoid/src/agent.rs` — in-loop `Emitting` branch for `tc.name == "recall"`

**Interfaces:**
- Consumes: `SessionHandle::recall`, `EventKind::TurnsReadmitted`.
- Produces: the model can call `recall(query, limit?)`; the loop queries the cold tier, emits `TurnsReadmitted { ids }` (re-admitting evicted originals) and a `ToolResult` rendering the recalled turns.

- [ ] **Step 1: Write the failing tests.** Tool spec test in `crates/zoid-tools/src/recall.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolKind};
    #[test]
    fn recall_spec_and_kind() {
        assert_eq!(Recall.name(), "recall");
        assert_eq!(Recall.spec().name, "recall");
        assert_eq!(Recall.kind(), ToolKind::Emitting); // executed in-loop, never via run()
    }
}
```

Loop round-trip test in `crates/zoid/src/agent.rs` tests (reuses `SequencedProvider` from Task 1.7):

```rust
#[tokio::test]
async fn recall_tool_readmits_and_returns_content() {
    use zoid_provider::{ProviderEvent, ToolCall};
    use zoid_core::event::{Event, EventKind, EvictionMarker};
    use zoid_core::projection::{conversation, ChatMsg};
    use ulid::Ulid;
    use serde_json::json;
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    // Seed: an evicted user turn (indexed in the store at append) + a recent turn + the marker.
    let e1 = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "configure the vector backend".into() });
    let e2 = Event::new(Ulid::from(2u128), None, 2, EventKind::UserMessage { text: "recent question".into() });
    let evicted = Event::new(Ulid::from(9u128), None, 9, EventKind::TurnsEvicted {
        ids: vec![Ulid::from(1u128)], reclaimed_tokens: 10, marker: EvictionMarker { spans: vec![] },
    });
    for e in [&e1, &e2, &evicted] { session.append(e.clone()).await.unwrap(); }
    let seed = vec![e1.clone(), e2.clone(), evicted.clone()];
    // Initially the evicted turn is NOT in the projection.
    assert!(!conversation(&seed).iter().any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("vector backend"))));

    let provider = std::sync::Arc::new(SequencedProvider::new(vec![
        vec![ProviderEvent::ToolCall(ToolCall { id: "r1".into(), name: "recall".into(), args: json!({"query": "vector"}) }), ProviderEvent::Done],
        vec![ProviderEvent::TextDelta("thanks".into()), ProviderEvent::Done],
    ]));
    let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin())));
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let out = run_agent_turn(chat_turn_config(), provider, tools, std::sync::Arc::new(zoid_tools::AllowAll), session, seed, "m".into(), tx, Ulid::new(), || 0).await.unwrap();

    // Re-admission event for the evicted id …
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::TurnsReadmitted { ids } if ids.contains(&Ulid::from(1u128)))));
    // … the recall ToolResult carries the retrieved content …
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { name, output, .. } if name == "recall" && output.contains("vector backend"))));
    // … and the turn is back in the projection.
    assert!(conversation(&out).iter().any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("vector backend"))));
}
```

- [ ] **Step 2: Run to verify failure.**

Run: `cargo test -p zoid-tools recall_spec_and_kind`
Expected: FAIL — `Recall` undefined.

- [ ] **Step 3: Implement the spec.** Create `crates/zoid-tools/src/recall.rs`:

```rust
//! The `recall` tool: search the cold tier and re-admit matching turns. Like
//! `update_tasks`, it is `Emitting` — the agent loop executes it (it needs the
//! session actor + the event log), so `run()` is never called.

use crate::{Tool, ToolKind, ToolOutput};
use serde_json::{json, Value};
use std::path::Path;
use zoid_provider::ToolSpec;

pub struct Recall;

impl Tool for Recall {
    fn name(&self) -> &str { "recall" }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recall".into(),
            description: "Search earlier, paged-out conversation history by keyword and bring \
                          matching turns back into context. Use when the breadcrumb says context \
                          was paged out and you need it.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "keywords to search paged-out history" },
                    "limit": { "type": "integer", "description": "max turns to retrieve (default 5)" }
                },
                "required": ["query"]
            }),
        }
    }
    fn kind(&self) -> ToolKind { ToolKind::Emitting }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: the loop branches on Emitting before calling run().
        ToolOutput::err("recall is executed by the agent loop")
    }
}
```

Register in `crates/zoid-tools/src/lib.rs`: `pub mod recall;` (near the other `pub mod`s).

Add to Chat tools in `crates/zoid/src/invoke_skill.rs::chat_tools` (`:88` area): `tools.push(Box::new(zoid_tools::recall::Recall));`.

- [ ] **Step 4: Implement in-loop execution.** In `crates/zoid/src/agent.rs`, add a branch in the tool-kind `match` (alongside the `update_tasks` `Emitting` arm at `:574`):

```rust
                Some(zoid_tools::ToolKind::Emitting) if tc.name == "recall" => {
                    let query = tc.args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let limit = tc.args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                    let hits = session.recall(query, limit).await.unwrap_or_default();
                    // Re-admit any currently-evicted originals so they re-enter the projection.
                    let live_evicted = zoid_core::eviction::evicted_ids(&events);
                    let readmit: Vec<Ulid> = hits.iter().map(|e| e.id).filter(|id| live_evicted.contains(id)).collect();
                    if !readmit.is_empty() {
                        emit(&session, &mut events, ui, &config.branch,
                             EventKind::TurnsReadmitted { ids: readmit }, session_id, now).await?;
                    }
                    let rendered = render_recalled(&hits);
                    emit(&session, &mut events, ui, &config.branch, EventKind::ToolResult {
                        id: tc.id, name: tc.name,
                        output: if rendered.is_empty() { "[recall: no matches]".into() } else { rendered },
                        is_error: false,
                    }, session_id, now).await?;
                    tracing::info!(kind = "tool", name = "recall", ms = tool_start.elapsed().as_millis() as u64, ok = true, "tool executed");
                }
```

Add the render helper near `record_compactions`:

```rust
/// Render recalled events into readable text for the recall tool-result.
fn render_recalled(events: &[Event]) -> String {
    let mut out = String::new();
    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => out.push_str(&format!("[user] {text}\n")),
            EventKind::AssistantMessage { text } => out.push_str(&format!("[assistant] {text}\n")),
            EventKind::ToolResult { name, output, .. } => out.push_str(&format!("[{name}] {output}\n")),
            _ => {}
        }
    }
    out.trim_end().to_string()
}
```

- [ ] **Step 5: Run the workspace tests.**

Run: `cargo test --workspace`
Expected: PASS (tool spec + loop round-trip + all existing).

- [ ] **Step 6: Commit.**

```bash
git add crates/zoid-tools/src/recall.rs crates/zoid-tools/src/lib.rs crates/zoid/src/invoke_skill.rs crates/zoid/src/agent.rs
git commit -m "feat(acm): recall tool — FTS query + re-admit (TurnsReadmitted) executed in-loop"
```

---

### Task 2.6: ML seams + reserved embedding table

**Files:**
- Create: `crates/zoid-core/src/retrieval.rs`
- Modify: `crates/zoid-core/src/lib.rs` (`pub mod retrieval;`)
- Modify: `crates/zoid-core/src/store.rs:16-42` (reserve `event_embeddings`)

**Interfaces:**
- Produces (all `None`/unused until Slice 4):
  - `struct RecallCandidate { pub event_id: Ulid, pub content: String, pub lexical_score: f32 }`
  - `struct Scored { pub candidate: RecallCandidate, pub score: f32 }`
  - `trait Embedder { fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>; fn dim(&self) -> usize; fn model_id(&self) -> &str; }`
  - `trait Reranker { fn rerank(&self, query: &str, candidates: &[RecallCandidate]) -> Vec<Scored>; }`
  - `trait CandidateSource { fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate>; }`
  - reserved sqlite table `event_embeddings`.

- [ ] **Step 1: Write the seam test.** Create `crates/zoid-core/src/retrieval.rs`:

```rust
//! Retrieval & relevance seams (spec §3.7). Pure trait declarations, threaded as
//! `Option<Arc<dyn …>>` by consumers (None in Slices 0–2). Slice 4 supplies
//! in-process implementations — no rearchitecture, only lit-up seams.

use ulid::Ulid;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallCandidate {
    pub event_id: Ulid,
    pub content: String,
    pub lexical_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub candidate: RecallCandidate,
    pub score: f32,
}

/// In-process embedding model (candidate impls: fastembed/ONNX bge-small, candle).
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}

/// In-process cross-encoder that refines candidate ordering for precision.
pub trait Reranker: Send + Sync {
    fn rerank(&self, query: &str, candidates: &[RecallCandidate]) -> Vec<Scored>;
}

/// One stage of the staged recall pipeline (Slice 2 = `[Fts5Source]`;
/// Slice 4 adds a `VectorSource`).
pub trait CandidateSource: Send + Sync {
    fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate>;
}

#[cfg(test)]
mod tests {
    use super::*;
    // A trivial impl proves the seams are object-safe and usable as trait objects.
    struct NoopReranker;
    impl Reranker for NoopReranker {
        fn rerank(&self, _q: &str, cands: &[RecallCandidate]) -> Vec<Scored> {
            cands.iter().map(|c| Scored { candidate: c.clone(), score: c.lexical_score }).collect()
        }
    }
    #[test]
    fn seams_are_object_safe() {
        let r: Box<dyn Reranker> = Box::new(NoopReranker);
        let c = RecallCandidate { event_id: Ulid::from(1u128), content: "x".into(), lexical_score: 1.0 };
        assert_eq!(r.rerank("q", &[c]).len(), 1);
    }
}
```

Add `pub mod retrieval;` to `crates/zoid-core/src/lib.rs`.

- [ ] **Step 2: Reserve the embedding table.** In `EventStore::open`, append to the schema:

```sql
            CREATE TABLE IF NOT EXISTS event_embeddings (
                event_id  TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                vector    BLOB NOT NULL,
                PRIMARY KEY (event_id, model_id)
            );
```

Add a store test asserting the table exists (mirror `open_creates_secrets_table`).

- [ ] **Step 3: Run tests to verify pass.**

Run: `cargo test -p zoid-core retrieval:: && cargo test -p zoid-core store::`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/zoid-core/src/retrieval.rs crates/zoid-core/src/lib.rs crates/zoid-core/src/store.rs
git commit -m "feat(acm): ML seams (Embedder/Reranker/CandidateSource) + reserved event_embeddings table"
```

---

### Task 2.7: Recall round-trip + undo integration test

**Files:**
- Modify: `crates/zoid/src/agent.rs` tests (end-to-end)

**Interfaces:**
- Consumes: the whole Slice 1+2 surface.

- [ ] **Step 1: Write the end-to-end test.** In `crates/zoid/src/agent.rs` tests — a **real** gate eviction on turn 1, then recall on turn 2, across one shared session (reuses `SequencedProvider` from Task 1.7):

```rust
#[tokio::test]
async fn evict_then_recall_round_trips() {
    use zoid_provider::{ProviderEvent, ToolCall, FakeProvider};
    use zoid_core::event::{Event, EventKind};
    use zoid_core::projection::{conversation, ChatMsg};
    use zoid_core::eviction::EvictionPolicy;
    use ulid::Ulid;
    use serde_json::json;

    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    let big = "x".repeat(3000); // ~1000 tokens per turn
    // Oldest turn carries a distinctive searchable token.
    let mut seed = vec![
        Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: format!("zephyrbackend {big}") }),
        Event::new(Ulid::from(2u128), None, 2, EventKind::AssistantMessage { text: "ok".into() }),
    ];
    for i in 1..8u128 {
        seed.push(Event::new(Ulid::from(i*2+1), None, (i*2+1) as i64, EventKind::UserMessage { text: big.clone() }));
        seed.push(Event::new(Ulid::from(i*2+2), None, (i*2+2) as i64, EventKind::AssistantMessage { text: "ok".into() }));
    }
    for e in &seed { session.append(e.clone()).await.unwrap(); }
    let policy = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 3_000, band_headroom_pct: 20, recent_n: 2, max_output: None };
    let tools = std::sync::Arc::new(crate::invoke_skill::chat_tools(std::sync::Arc::new(zoid_core::skill::SkillRegistry::builtin())));

    // TURN 1 — the pre-flight gate evicts the oldest turns.
    let mut cfg1 = chat_turn_config();
    cfg1.eviction = policy;
    let p1 = std::sync::Arc::new(FakeProvider::new(vec![ProviderEvent::TextDelta("ack".into()), ProviderEvent::Done]));
    let (tx1, mut rx1) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx1.recv().await.is_some() {} });
    let out1 = run_agent_turn(cfg1, p1, tools.clone(), std::sync::Arc::new(zoid_tools::AllowAll), session.clone(), seed, "m".into(), tx1, Ulid::new(), || 0).await.unwrap();
    assert!(out1.iter().any(|e| matches!(e.kind, EventKind::TurnsEvicted { .. })), "turn 1 must evict");
    assert!(!conversation(&out1).iter().any(|m| matches!(m, ChatMsg::User { text, .. } if text.contains("zephyrbackend"))), "evicted turn gone from projection");

    // TURN 2 — the model recalls the evicted content.
    let mut cfg2 = chat_turn_config();
    cfg2.eviction = policy;
    let p2 = std::sync::Arc::new(SequencedProvider::new(vec![
        vec![ProviderEvent::ToolCall(ToolCall { id: "r1".into(), name: "recall".into(), args: json!({"query": "zephyrbackend"}) }), ProviderEvent::Done],
        vec![ProviderEvent::TextDelta("got it".into()), ProviderEvent::Done],
    ]));
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(256);
    tokio::spawn(async move { while rx2.recv().await.is_some() {} });
    let out2 = run_agent_turn(cfg2, p2, tools, std::sync::Arc::new(zoid_tools::AllowAll), session, out1, "m".into(), tx2, Ulid::new(), || 0).await.unwrap();
    assert!(out2.iter().any(|e| matches!(&e.kind, EventKind::TurnsReadmitted { ids } if ids.contains(&Ulid::from(1u128)))), "recall re-admits the evicted turn");
    assert!(out2.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { name, output, .. } if name == "recall" && output.contains("zephyrbackend"))), "recall result carries content");
}
```

(Note: `EvictionPolicy` is `Copy`, so `policy` reuses across both turns.)

- [ ] **Step 2: Run.**

Run: `cargo test -p zoid evict_then_recall_round_trips`
Expected: PASS (all machinery already built; this is the integration guard).

- [ ] **Step 3: Run the full suite.**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/zoid/src/agent.rs
git commit -m "test(acm): evict→recall→re-admit round-trip integration test"
```

---

## Out of scope (documented in the spec, not this plan)

- **Slice 3** — cold-paging + windowed resume (RAM/resume curve; spec §3.5). = debt-item #6.
- **Model-metadata DB catalog** (spec §3.0 "full catalog") — decoupled slice; Slice 4 depends on it.
- **Slice 4** — in-process `Embedder`/`Reranker`, populate `event_embeddings`, hybrid retrieval, relevance-driven eviction, proactive auto-recall (spec §11). The seams from Task 0.4/1.3/2.6 make it additive.
- The async maintenance lane carrying real load (spec §3.8) — seam reserved; nothing runs on it here.
- Massive-influx source windowing (spec §3.9 rule 1) — the largest-first compaction (rule 2) and per-item behavior are covered by the pre-flight gate; tool-output windowing at the read/search tools is a separate small slice.
- **Transcript rendering + undo affordance (spec §5)** — a `zoid-tui` slice. This plan *emits* `TurnsEvicted` (with the marker's topic hints + token counts) and supports undo *mechanically* (append `TurnsReadmitted { span_ids }`), so all the data the semantic-zoom UI needs already exists — but rendering the eviction chip and wiring the undo keybinding is deferred TUI work, following exactly how `ToolResultCompacted` is already rendered.
- **Model-switch integration test (spec §7)** — the mechanism is covered at the unit level (`derive_band`'s `small_model_collapses_target_to_usable`, Task 0.2) and structurally (the gate re-derives the band per turn from `TurnConfig.eviction.capacity`, so a 1M→32k switch forces the next pre-flight to evict down); a full mid-session-switch integration test is a nice-to-have, not a blocker.

## Self-review notes (for the executor)

- **Spec coverage:** Slice 0 = spec §3.0(minimal)/§3.6/§3.6a; Slice 1 = §3.1/§3.2/§3.3/§3.8(gate)/§4/§7(steady-state); Slice 2 = §3.4/§3.7/Decision 9,10,14. North-star (§11) and Slices 3/4 are explicitly out.
- **Type consistency check:** `EvictionPolicy` (defined Task 0.4) is consumed unchanged in 1.3/1.6/1.7; `plan_evictions(events, policy, current_tokens, scorer)` signature is identical across 1.3/1.6/1.7/1.8; `TurnsEvicted{ids, reclaimed_tokens, marker}` fields match between event.rs (1.1), the fold (1.2), the gate (1.6), and the skip (1.4); `search_fts`/`events_by_ids`/`recall` signatures match across 2.2/2.3/2.4/2.5.
- **`EventId` is `ulid::Ulid`** everywhere (no alias) — the spec's "EventId" reads as `Ulid`.
- **`enabled=false` invariant:** every new lever (`preflight_gate`, retry) short-circuits when `config.eviction.enabled` is false, so all pre-existing tests (which use `chat_turn_config()` → `EvictionPolicy::disabled()`) are behavior-unchanged.
