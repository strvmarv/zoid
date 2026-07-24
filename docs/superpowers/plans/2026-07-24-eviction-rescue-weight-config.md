# `[eviction]` Config Exposure of `RESCUE_WEIGHT` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose `DEFAULT_RESCUE_WEIGHT` (currently `const f32 = 12.0`) as a runtime `[eviction] rescue_weight` config key so it can be tuned without a rebuild.

**Architecture:** A new `EvictionConfig` struct + `[eviction]` TOML section in `zoid-core/src/config.rs` (mirroring the `EmbedConfig` / `PartialEmbed` pattern). A `rescue_weight: Option<f32>` field added to `EvictionPolicy` (flowing through `TurnConfig.eviction` to `preflight_gate`, the same path as the other eviction knobs). A pure `resolve_rescue_weight()` function in `eviction.rs` that clamps non-finite and out-of-range values at the read site. `None` / absent ⇒ const fallback (behavior unchanged).

**Tech Stack:** Rust workspace (`zoid-core` pure; `zoid` binary). Config: TOML via `serde` + `toml` crate, layered `PartialConfig` merge. `f32` weights. `Option<f32>` for nullable config.

**Spec:** `docs/superpowers/specs/2026-07-24-eviction-rescue-weight-config-design.md`

## Global Constraints

- **`Eq` derives dropped where `Option<f32>` is introduced.** `EvictionPolicy` (eviction.rs:8) and `Config` (config.rs:34) both lose `Eq`, retain `PartialEq`. Verified: no `assert_eq` on either type requires `Eq`; the one `Config`-level `assert_eq` (config.rs:653) needs only `PartialEq + Debug`.
- **`Option<f32>` is `Copy`** — both `EvictionPolicy` and `EvictionConfig` retain `Copy`.
- **Cross-crate discipline.** `EvictionPolicy` is defined in `zoid-core`, used in `zoid` (`agent.rs`, `main.rs`). `Config` is in `zoid-core`. Every task builds `cargo build --workspace` and `cargo test --workspace`, not just `-p zoid-core`.
- **No co-author trailer** in commits (repo `AGENTS.md`).
- **12 `EvictionPolicy { ... }` struct literals** must be updated when the field is added: 3 in `eviction.rs` (lines 50, 609, 970), 8 in `agent.rs` (lines 3220, 3348, 3408, 3629, 3897, 3960, 4423, +1), 1 in `main.rs` (line 6500). All test literals get `rescue_weight: None`; `main.rs` gets `rescue_weight: app.config.eviction.rescue_weight`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/zoid-core/src/eviction.rs` | `EvictionPolicy` gains `rescue_weight: Option<f32>`; `RESCUE_WEIGHT_MAX` const; `resolve_rescue_weight()` pure fn; update `disabled()` + 3 test literals | Modify |
| `crates/zoid-core/src/config.rs` | `EvictionConfig` struct; `PartialEviction` struct; `eviction` field on `Config` + `PartialConfig`; merge arm; drop `Eq` from `Config` | Modify |
| `crates/zoid/src/main.rs` | Pass `app.config.eviction.rescue_weight` into `EvictionPolicy` literal at line 6500 | Modify |
| `crates/zoid/src/agent.rs` | `preflight_gate` calls `resolve_rescue_weight()` instead of bare const; update 7 test `EvictionPolicy` literals | Modify |

**Task order:** T1 (resolver + `EvictionPolicy` field) → T2 (config layer) → T3 (wire-in `main.rs` + `agent.rs`). T1 must come first because it breaks all struct literals (the compile sweep). T2 adds the config surface. T3 connects them. Recommended linear order T1→T2→T3.

---

### Task 1: `resolve_rescue_weight` + `rescue_weight` field on `EvictionPolicy`

**Files:**
- Modify: `crates/zoid-core/src/eviction.rs` (line 8 `EvictionPolicy` derive + fields; line 19 `disabled()`; new `RESCUE_WEIGHT_MAX` const near line 193; new `resolve_rescue_weight` fn; 3 test struct literals at lines 50, 609, 970)
- Modify: `crates/zoid/src/agent.rs` (7 test `EvictionPolicy` literals at lines 3220, 3348, 3408, 3629, 3897, 3960, 4423)
- Modify: `crates/zoid/src/main.rs` (1 `EvictionPolicy` literal at line 6500)

**Interfaces:**
- Consumes: `DEFAULT_RESCUE_WEIGHT` (existing const at eviction.rs:193).
- Produces:
  - `pub const RESCUE_WEIGHT_MAX: f32` (48.0) — upper clamp bound.
  - `pub fn resolve_rescue_weight(raw: Option<f32>) -> f32` — pure clamping resolver.
  - `EvictionPolicy.rescue_weight: Option<f32>` — new public field.

- [ ] **Step 1: Write the failing tests for `resolve_rescue_weight`**

Add these to the `mod tests` block at the top of `eviction.rs` (after the `enabled_policy_band_matches_derivation` test, before the closing `}` of `mod tests` at line 60):

```rust
#[test]
fn resolve_rescue_weight_none_uses_default() {
    assert_eq!(resolve_rescue_weight(None), DEFAULT_RESCUE_WEIGHT);
}

#[test]
fn resolve_rescue_weight_some_finite_passes_through_capped() {
    assert_eq!(resolve_rescue_weight(Some(8.0)), 8.0);
    assert_eq!(resolve_rescue_weight(Some(0.0)), 0.0);
    assert_eq!(resolve_rescue_weight(Some(RESCUE_WEIGHT_MAX)), RESCUE_WEIGHT_MAX);
}

#[test]
fn resolve_rescue_weight_large_finite_clamped_to_max() {
    assert_eq!(resolve_rescue_weight(Some(100.0)), RESCUE_WEIGHT_MAX);
}

#[test]
fn resolve_rescue_weight_negative_clamped_to_zero() {
    assert_eq!(resolve_rescue_weight(Some(-5.0)), 0.0);
}

#[test]
fn resolve_rescue_weight_non_finite_clamped_to_zero() {
    assert_eq!(resolve_rescue_weight(Some(f32::INFINITY)), 0.0);
    assert_eq!(resolve_rescue_weight(Some(f32::NEG_INFINITY)), 0.0);
    assert_eq!(resolve_rescue_weight(Some(f32::NAN)), 0.0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core -- resolve_rescue_weight`
Expected: FAIL — `cannot find function resolve_rescue_weight` / `cannot find const RESCUE_WEIGHT_MAX`.

- [ ] **Step 3: Add `RESCUE_WEIGHT_MAX` const + `resolve_rescue_weight` function**

Add immediately after `DEFAULT_RESCUE_WEIGHT` (eviction.rs:193):

```rust
/// Upper cap for `resolve_rescue_weight`: 4× the default. Anything above this
/// makes rescue so over-protective that it's effectively a misconfiguration;
/// clamping here prevents the band-starve pathology while still allowing ample
/// tuning range.
pub const RESCUE_WEIGHT_MAX: f32 = DEFAULT_RESCUE_WEIGHT * 4.0; // 48.0

/// Resolve the rescue weight, clamping to a safe positive range.
/// Negative / NaN / +∞ / -∞ all collapse to 0.0 (pure recency), preserving
/// the rescue-only invariant and the band-preservation guarantee. Large finite
/// values are capped at `RESCUE_WEIGHT_MAX`. `None` ⇒ `DEFAULT_RESCUE_WEIGHT`.
pub fn resolve_rescue_weight(raw: Option<f32>) -> f32 {
    let w = raw.unwrap_or(DEFAULT_RESCUE_WEIGHT);
    if w.is_finite() && w >= 0.0 {
        w.min(RESCUE_WEIGHT_MAX)
    } else {
        0.0
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-core -- resolve_rescue_weight`
Expected: PASS (all 5 tests).

- [ ] **Step 5: Add `rescue_weight` field to `EvictionPolicy` and update `disabled()`**

Change the derive and struct at eviction.rs:8:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub recent_n: usize,
    pub max_output: Option<u64>,
    pub rescue_weight: Option<f32>,
}
```

Update `disabled()` (eviction.rs:19):

```rust
pub fn disabled() -> Self {
    Self {
        enabled: false,
        capacity: 0,
        context_target: 0,
        band_headroom_pct: 0,
        recent_n: 0,
        max_output: None,
        rescue_weight: None,
    }
}
```

- [ ] **Step 6: Update all 12 `EvictionPolicy { ... }` struct literals (compile sweep)**

The new field breaks every struct literal. Run this to find them all:

```bash
grep -rn "EvictionPolicy\s*{" crates/ | grep -v "fn \|disabled\|struct \|impl "
```

Add `rescue_weight: None` to each **test** literal and `rescue_weight: app.config.eviction.rescue_weight` to the `main.rs` literal. The 11 sites are:

**`crates/zoid-core/src/eviction.rs`** (3 sites):
- Line 50 (`enabled_policy_band_matches_derivation`) — add `rescue_weight: None,` after `max_output: None,`
- Line 609 (`fn policy(target, recent_n)` helper) — add `rescue_weight: None,` after `max_output: None,`
- Line 970 (`holds_band_over_hundreds_of_turns`) — add `rescue_weight: None,` after `max_output: None,`

**`crates/zoid/src/agent.rs`** (7 sites — all in `#[cfg(test)]` modules):
- Lines 3220, 3348, 3408, 3629, 3897, 3960, 4423 — each has `max_output: None,` as the last field; add `rescue_weight: None,` after it.

**`crates/zoid/src/main.rs`** (1 site):
- Line 6500 — add `rescue_weight: app.config.eviction.rescue_weight,` after `max_output: None,`

> **Note:** The `main.rs` literal references `app.config.eviction.rescue_weight`, which doesn't exist yet (Task 2 adds `EvictionConfig` to `Config`). This will NOT compile until Task 2 is done. That is intentional — the alternative (passing `None` now and fixing it in Task 3) creates a silent no-op that's easy to forget. Accept the workspace build failure here; T2 makes it compile.

- [ ] **Step 7: Build the workspace (expect a compile failure — Task 2 fixes it)**

Run: `cargo build --workspace 2>&1 | head -5`
Expected: FAIL — `no field 'eviction' on type 'Config'` (or similar) at `main.rs:6500`. This is the expected state — T2 adds the `eviction` field to `Config`.

If the ONLY error is the `main.rs` `app.config.eviction` reference, proceed to Task 2. If there are other errors (e.g., a missed struct literal), fix those first — re-run the grep to find any site you missed.

- [ ] **Step 8: Commit (all changes — the workspace doesn't fully build yet, but the commit is coherent)**

```bash
git add crates/zoid-core/src/eviction.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(zoid-core): resolve_rescue_weight + rescue_weight field on EvictionPolicy

Add RESCUE_WEIGHT_MAX (48.0) const and resolve_rescue_weight() pure
fn that clamps non-finite and out-of-range values. Add rescue_weight:
Option<f32> to EvictionPolicy (drop Eq — f32 is not Eq). Update all
12 struct literals (11 tests + 1 main.rs). main.rs references
app.config.eviction.rescue_weight — wired in next task. Workspace
build fails only on that reference until T2 lands."
```

---

### Task 2: `EvictionConfig` + `[eviction]` config section

**Files:**
- Modify: `crates/zoid-core/src/config.rs` (new `EvictionConfig` + `PartialEviction` structs; `eviction` field on `Config` + `PartialConfig`; merge arm; drop `Eq` from `Config` derive; tests)

**Interfaces:**
- Consumes: nothing from T1 (config layer is independent of `EvictionPolicy`).
- Produces:
  - `pub struct EvictionConfig { pub rescue_weight: Option<f32> }` (derives `Debug, Clone, Copy, PartialEq, Default`).
  - `pub struct PartialEviction { pub rescue_weight: Option<f32> }` (derives `Debug, Default, Clone, Deserialize`).
  - `Config.eviction: EvictionConfig` and `PartialConfig.eviction: PartialEviction`.
  - Merge arm in `merge()` that copies `p.eviction.rescue_weight` into `cfg.eviction.rescue_weight`.

- [ ] **Step 1: Add `EvictionConfig` struct**

Add after `EmbedConfig` and its `Default` impl (config.rs:127), before `SubagentConfig`:

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

- [ ] **Step 2: Add `eviction` field to `Config` and drop `Eq` from its derive**

Change the `Config` derive at config.rs:34 from:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
```
to:
```rust
#[derive(Debug, Clone, PartialEq)]
```

Add `pub eviction: EvictionConfig,` to the `Config` struct (after `pub embed: EmbedConfig,` at line 47).

- [ ] **Step 3: Add `PartialEviction` struct**

Add after `PartialEmbed` (config.rs:383), before `PartialSubagent`:

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEviction {
    pub rescue_weight: Option<f32>,
}
```

- [ ] **Step 4: Add `eviction` field to `PartialConfig`**

Add `pub eviction: PartialEviction,` to `PartialConfig` (after `pub embed: PartialEmbed,` at line 426).

- [ ] **Step 5: Add merge arm in `merge()`**

In the `merge` function, after the `p.embed.auto_download` block (config.rs:589–591), before the closing `}` of the `for (src, p) in layers` loop:

```rust
if let Some(v) = p.eviction.rescue_weight {
    cfg.eviction.rescue_weight = Some(v);
}
```

- [ ] **Step 6: Write the failing tests**

Add to the `mod config_tests` module (near the `embed_defaults_and_parse` test at line 264):

```rust
#[test]
fn eviction_defaults_to_none() {
    let c = Config::default();
    assert!(c.eviction.rescue_weight.is_none());
}

#[test]
fn eviction_section_parses_and_merges() {
    let (p, _warn) = parse_toml("[eviction]\nrescue_weight = 16.0").unwrap();
    assert_eq!(p.eviction.rescue_weight, Some(16.0));
    let (cfg, _prov) = merge(&[(Source::UserGlobal, p)]);
    assert_eq!(cfg.eviction.rescue_weight, Some(16.0));
}

#[test]
fn eviction_absent_section_is_none() {
    let (p, _warn) = parse_toml("model = \"a\"").unwrap();
    assert!(p.eviction.rescue_weight.is_none());
    let (cfg, _prov) = merge(&[(Source::UserGlobal, p)]);
    assert!(cfg.eviction.rescue_weight.is_none());
}

#[test]
fn eviction_overrides_across_layers() {
    let (user, _) = parse_toml("[eviction]\nrescue_weight = 8.0").unwrap();
    let (proj, _) = parse_toml("[eviction]\nrescue_weight = 16.0").unwrap();
    let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
    assert_eq!(cfg.eviction.rescue_weight, Some(16.0)); // project wins
}

#[test]
fn eviction_inf_parses_without_error() {
    // TOML deserializes 1e308 into f32::INFINITY — not a parse error.
    // Clamping happens at the read site (resolve_rescue_weight), not here.
    let (p, _warn) = parse_toml("[eviction]\nrescue_weight = 1e308").unwrap();
    assert_eq!(p.eviction.rescue_weight, Some(f32::INFINITY));
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p zoid-core -- eviction`
Expected: PASS (all 5 tests).

- [ ] **Step 8: Build the workspace (main.rs should now compile)**

Run: `cargo build --workspace`
Expected: success — `Config` now has the `eviction` field that `main.rs:6500` references.

- [ ] **Step 9: Run the full workspace test suite**

Run: `cargo test --workspace --no-fail-fast`
Expected: success — no regressions. The `empty_layer_changes_nothing` test (config.rs:652) still passes because `Config: PartialEq` is retained (only `Eq` was dropped).

- [ ] **Step 10: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat(config): add [eviction] rescue_weight config section

EvictionConfig + PartialEviction (mirrors EmbedConfig/PartialEmbed).
Drop Eq from Config (f32 in EvictionConfig is not Eq; PartialEq
retained — no assert_eq requires Eq). Merge arm copies the overlay.
Tests: parse, merge, layer override, +∞ deserialization, defaults."
```

---

### Task 3: Wire `resolve_rescue_weight` into `preflight_gate`

**Files:**
- Modify: `crates/zoid/src/agent.rs` (`preflight_gate` at line 2794 — replace bare const with `resolve_rescue_weight`; update tracing at line 2807)

**Interfaces:**
- Consumes: `resolve_rescue_weight` (T1), `config.eviction.rescue_weight` (T2).
- Produces: `GoalContext.weight` sourced from config instead of a bare const.

- [ ] **Step 1: Replace the bare const in `preflight_gate`**

At agent.rs:2794, replace:

```rust
                    zoid_core::eviction::GoalContext {
                        goal,
                        vecs,
                        weight: zoid_core::eviction::DEFAULT_RESCUE_WEIGHT,
                    }
```

with:

```rust
                    zoid_core::eviction::GoalContext {
                        goal,
                        vecs,
                        weight: zoid_core::eviction::resolve_rescue_weight(
                            config.eviction.rescue_weight,
                        ),
                    }
```

- [ ] **Step 2: Update the tracing line**

At agent.rs:2807, replace:

```rust
            weight = zoid_core::eviction::DEFAULT_RESCUE_WEIGHT,
```

with:

```rust
            weight = goal_ctx.weight,
```

- [ ] **Step 3: Write the failing integration test for `rescue_weight = 0`**

Add to the `agent.rs` test module, near the existing `preflight_rescues_relevant_old_turn_over_newer_offgoal` test (around line 3408):

```rust
#[tokio::test]
async fn preflight_rescue_weight_zero_is_pure_recency() {
    use ulid::Ulid;
    use zoid_core::event::{Event, EventKind};
    use zoid_core::retrieval::FakeEmbedder;

    let fat = "x".repeat(3000);
    let goalish = "alpha beta gamma delta";
    let offgoal = "zulu yankee xray whiskey";
    let utext = |uid: u128| -> String {
        if matches!(uid, 1 | 11 | 13 | 15) { format!("{goalish} n{uid}") } else { format!("{offgoal} n{uid}") }
    };
    let mut seed = Vec::new();
    for i in 0..8u128 {
        let uid = i * 2 + 1;
        seed.push(Event::new(Ulid::from(uid), None, uid as i64,
            EventKind::UserMessage { text: utext(uid) }));
        seed.push(Event::new(Ulid::from(i * 2 + 2), None, (i * 2 + 2) as i64,
            EventKind::AssistantMessage { text: fat.clone() }));
    }
    let session = zoid_core::session::SessionHandle::spawn(":memory:").unwrap();
    for e in &seed { session.append(e.clone()).await.unwrap(); }

    let emb = FakeEmbedder::new(16);
    for uid in [1u128, 3, 5, 7, 9, 11] {
        let v = emb.embed(&[utext(uid).as_str()]).unwrap().remove(0);
        session.write_embedding(Ulid::from(uid), "fake".into(), v).await.unwrap();
    }

    let mut cfg = chat_turn_config();
    cfg.eviction = zoid_core::eviction::EvictionPolicy {
        enabled: true,
        capacity: 1_000_000,
        context_target: 5_000,
        band_headroom_pct: 20,
        recent_n: 2,
        max_output: None,
        rescue_weight: Some(0.0), // weight 0 ⇒ pure recency, no rescue
    };
    cfg.embedder = Some(std::sync::Arc::new(FakeEmbedder::new(16)));

    let out = run_gate_only(cfg, session, seed).await;
    let evicted: Vec<Ulid> = out.iter().filter_map(|e| match &e.kind {
        EventKind::TurnsEvicted { ids, .. } => Some(ids.clone()), _ => None,
    }).flatten().collect();

    assert!(!evicted.is_empty(), "a wave fired");
    assert!(evicted.contains(&Ulid::from(1u128)), "weight 0 ⇒ oldest evicted (no rescue)");
}
```

> **Note:** `run_gate_only` is the test helper already defined for the `preflight_rescues_relevant_old_turn_over_newer_offgoal` test (4b Task 6). If the helper is named differently, reuse whatever that test calls. The test seed is identical to the rescue test except `rescue_weight: Some(0.0)` — with weight 0, the on-goal old turn (id 1) is NOT rescued and IS evicted, matching pure recency.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p zoid --features local-embed -- preflight_rescue_weight_zero`
Expected: PASS.

- [ ] **Step 5: Run the existing rescue test to confirm no regression**

Run: `cargo test -p zoid --features local-embed -- preflight_rescues preflight_without_embedder`
Expected: PASS (both existing tests still pass — `rescue_weight: None` in their policies ⇒ `resolve_rescue_weight(None) = DEFAULT_RESCUE_WEIGHT` = 12.0, identical to the old bare const).

- [ ] **Step 6: Run the full release-gate test suite**

Run: `cargo test --workspace --features zoid/local-embed --no-fail-fast`
Expected: success — no regressions.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid/src/agent.rs
git commit -m "feat(zoid): wire resolve_rescue_weight into preflight_gate

preflight_gate now calls resolve_rescue_weight(config.eviction.
rescue_weight) instead of the bare DEFAULT_RESCUE_WEIGHT const.
Tracing logs the resolved weight. Add integration test: rescue_weight
= 0.0 ⇒ pure recency (oldest on-goal turn evicted, no rescue)."
```

---

## Self-Review

Run after all tasks: `cargo test --workspace --features zoid/local-embed --no-fail-fast` (AGENTS.md release gate). Confirm:
- `resolve_rescue_weight` tests pass (T1).
- `[eviction]` config parse/merge tests pass (T2).
- `preflight_rescue_weight_zero_is_pure_recency` passes (T3).
- Existing `preflight_rescues_relevant_old_turn_over_newer_offgoal` and `preflight_without_embedder_evicts_the_old_turn` still pass (regression guard).
- Existing 4b property tests (`bounded_reach_weight_zero_is_pure_recency`, `band_preservation_rescue_never_shrinks_quota`) still pass.
- `empty_layer_changes_nothing` (config.rs:652) still passes (`Config: PartialEq` retained).