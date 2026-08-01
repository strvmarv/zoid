# Eviction Master Switch — Design

**Date:** 2026-07-28
**Status:** Design (approved)

## Goal

Add an explicit `eviction.enabled` boolean master switch to the `[eviction]`
TOML config section that directly controls `EvictionPolicy.enabled` at
turn-build time, replacing the current implicit derivation from
`compact_threshold_pct > 0`. The switch appears as a Bool toggle in the
Economy section of the config screen. Disabling eviction does not affect
compaction, which continues to work independently.

## Background

The eviction system (`zoid-core/src/eviction.rs`) already has an
`EvictionPolicy.enabled: bool` field with `EvictionPolicy::disabled()` for the
zero-arg test constructors. But there is no user-facing config field that maps
to it. Instead, `enabled` is derived at turn-build time from
`compact_threshold_pct > 0` (`main.rs:7167`) — a compaction threshold of 0 is
the de facto eviction-off switch.

A separate `auto_evict_cold: bool` in `EconomyConfig` (defaults `true`)
controls whether cold turns are automatically evicted. It is already exposed in
the config screen as "auto-evict cold" but does not control the master
`EvictionPolicy.enabled`.

## Confirmed design decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| New explicit master switch | `eviction.enabled: bool` in `EvictionConfig` | Direct control, no implicit derivation. |
| Default | `true` | Matches today's behavior — eviction is on by default. |
| TOML section | `[eviction]` | Semantically an eviction setting; `[eviction]` already exists for `rescue_weight`. |
| Config UI section | Economy | All economy/eviction knobs appear in the Economy section. |
| Compaction coupling | Decoupled | Disabling eviction does not affect compaction. `compact_threshold_pct` controls compaction only. |
| `auto_evict_cold` when eviction off | Stays visible and editable | Takes effect when eviction is re-enabled. No conditional visibility. |
| `compact_threshold_pct` relationship | No longer controls eviction | Sole authority is the new `eviction.enabled` field. The `> 0` derivation is removed. |

## Config types

### `EvictionConfig` — new `enabled` field

`crates/zoid-core/src/config.rs` — add `enabled: bool` to `EvictionConfig`
(currently only has `rescue_weight: Option<f32>`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EvictionConfig {
    /// Master switch for the eviction controller. `false` = total bypass
    /// (byte-identical to pre-ACM behavior). Default `true`.
    pub enabled: bool,
    /// Rescue weight in turn-index units. None ⇒ DEFAULT_RESCUE_WEIGHT.
    pub rescue_weight: Option<f32>,
}
```

`Default` (via `#[derive(Default)]`): `enabled` defaults to `true` — **but
`bool`'s derived default is `false`**, so `EvictionConfig` must implement
`Default` manually:

```rust
impl Default for EvictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rescue_weight: None,
        }
    }
}
```

The `#[derive(Default)]` is removed and replaced with this manual impl.

### `PartialEviction` — new `enabled: Option<bool>`

```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEviction {
    pub enabled: Option<bool>,
    pub rescue_weight: Option<f32>,
}
```

### `Provenance` — new `eviction_enabled: Source`

```rust
pub struct Provenance {
    // ... existing fields ...
    pub eviction_enabled: Source,
    // ... rest ...
}
```

### `merge` function

In the `merge` function (`config.rs:543`), add after the `eviction.rescue_weight`
merge block:

```rust
if let Some(v) = p.eviction.enabled {
    cfg.eviction.enabled = v;
    prov.eviction_enabled = *src;
}
```

And add `eviction_enabled: Source::Default` to the initial `Provenance`
construction at the top of `merge`.

### TOML

```toml
[eviction]
enabled = false       # master switch (default true)
rescue_weight = 16.0   # existing
```

## Runtime wiring

### The single critical change — `main.rs:7166-7167`

Current:

```rust
turn_config.eviction = zoid_core::eviction::EvictionPolicy {
    enabled: app.economy.compact_threshold_pct > 0, // master switch (back-compat)
    capacity: app.shell.ctx_ceiling,
    context_target: app.context_target,
    band_headroom_pct: app.economy.band_headroom_pct,
    recent_n: app.economy.recent_n,
    max_output: None,
    rescue_weight: app.config.eviction.rescue_weight,
};
```

New:

```rust
turn_config.eviction = zoid_core::eviction::EvictionPolicy {
    enabled: app.config.eviction.enabled,
    capacity: app.shell.ctx_ceiling,
    context_target: app.context_target,
    band_headroom_pct: app.economy.band_headroom_pct,
    recent_n: app.economy.recent_n,
    max_output: None,
    rescue_weight: app.config.eviction.rescue_weight,
};
```

The comment `// master switch (back-compat)` is removed — the switch is now
explicit, not a back-compat derivation.

### What does NOT change in the runtime path

- `policy_from_config` (`main.rs:1269`) — still derives the compaction threshold
  from `compact_threshold_pct`. Compaction works independently of eviction.
- `auto_evict_cold` — still feeds into `policy_from_config`. Still shown, still
  editable, still takes effect when eviction is re-enabled.
- `EvictionPolicy::disabled()` — still used by test constructors and in
  `eviction.rs` tests.
- Eviction scoring, rescue, breadcrumb, `evicted_ids` — all downstream of
  `EvictionPolicy.enabled`. No change.

## Config UI

### New row in the Economy section — `config_view.rs`

In `build_sections`, add a new `FieldRow` at the **top** of the Economy section's
`rows` vec (before "context target"):

```rust
FieldRow {
    label: "eviction",
    value: onoff(cfg.eviction.enabled),
    kind: FieldKind::Bool,
    source: prov.eviction_enabled,
    env_shadowed: prov.eviction_enabled == Source::Env,
    secret_key: None,
},
```

This follows the exact pattern of every other Bool row (e.g., "auto-evict cold"
→ `economy.auto_evict_cold`, "reduced motion" → `reduced_motion`).

**What the user sees in the config screen:**

```
Economy
  eviction            on        ← new (Bool toggle)
  context target      300000
  auto-evict cold     on
  compact at %        80
  band headroom %     20
  recent turns        4
```

No conditional visibility — all rows stay visible and editable regardless of
the eviction switch state.

### TOML write-back — `main.rs` (~line 3949)

Add a new arm to the `label` match in the `current_toml_value` function:

```rust
"eviction" => (
    "eviction.enabled",
    TomlValue::Bool(app.config.eviction.enabled),
),
```

This follows the exact pattern of "auto-evict cold" → `economy.auto_evict_cold`.

## What does NOT change

| Component | File | Change |
|-----------|------|--------|
| Eviction controller logic | `zoid-core/src/eviction.rs` | None — `enabled` field already exists and is checked |
| Compaction | `main.rs:1269` (`policy_from_config`) | None — still driven by `compact_threshold_pct` independently |
| `auto_evict_cold` | `EconomyConfig`, config UI | None — still shown, still editable |
| Eviction scoring/rescue/breadcrumb | `eviction.rs` | None |
| `EvictionPolicy::disabled()` | `eviction.rs` | None — still used by tests |
| Persistence / model context | — | None — purely a config + wiring change |

## Edge cases

- **`EvictionConfig::default()` must set `enabled: true`.** Rust's derived
  `Default` for `bool` is `false`, which would silently disable eviction for all
  existing users on upgrade. The manual `Default` impl is critical.
- **Existing users with `compact_threshold_pct = 0`.** Today this implicitly
  disables eviction. After this change, `compact_threshold_pct = 0` no longer
  controls eviction — it only disables compaction. Such users will now have
  eviction enabled (the new default `true`). This is a deliberate behavior
  change: the user asked for an explicit switch, and the default is "on."
  Users who want eviction off must set `[eviction] enabled = false`.
- **`auto_evict_cold` when eviction is off.** The toggle stays visible and
  editable. It has no runtime effect while eviction is disabled, but its value
  is preserved and takes effect immediately when eviction is re-enabled. No
  special handling needed — `auto_evict_cold` feeds `policy_from_config`, which
  is only consulted when eviction is enabled.

## Testing

### `config.rs` tests

Mirroring `wake_enabled_defaults_true_and_merges`:

```rust
#[test]
fn eviction_enabled_defaults_true() {
    let (cfg, prov) = merge(&[]);
    assert!(cfg.eviction.enabled, "eviction.enabled defaults to true");
    assert_eq!(prov.eviction_enabled, Source::Default);
}

#[test]
fn eviction_enabled_parses_and_merges() {
    let (p, _warn) = parse_toml("[eviction]\nenabled = false").unwrap();
    assert_eq!(p.eviction.enabled, Some(false));
    let (cfg, prov) = merge(&[(Source::UserGlobal, p)]);
    assert!(!cfg.eviction.enabled);
    assert_eq!(prov.eviction_enabled, Source::UserGlobal);
}

#[test]
fn eviction_enabled_overrides_across_layers() {
    let (user, _) = parse_toml("[eviction]\nenabled = false").unwrap();
    let (proj, _) = parse_toml("[eviction]\nenabled = true").unwrap();
    let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
    assert!(cfg.eviction.enabled, "project layer wins");
}

#[test]
fn eviction_enabled_absent_section() {
    let (p, _) = parse_toml("model = \"a\"").unwrap();
    let (cfg, _) = merge(&[(Source::UserGlobal, p)]);
    assert!(cfg.eviction.enabled, "absent [eviction] → default true");
}
```

### `config_view.rs` tests

The existing `builds_four_sections_with_env_shadow` and
`provider_and_model_rows_are_pick_kind` tests construct `Provenance` literals
that must be updated with the new `eviction_enabled: Source::Default` field.

New assertion (added to `builds_four_sections_with_env_shadow` or a new test):

```rust
let economy = sections.iter().find(|s| s.title == "Economy").unwrap();
let eviction_row = &economy.rows[0];
assert_eq!(eviction_row.label, "eviction");
assert!(matches!(eviction_row.kind, FieldKind::Bool));
assert_eq!(eviction_row.value, "on"); // default true
```

### `main.rs` tests

The existing `policy_from_config` tests at line 7719+ cover compaction and are
unaffected. No new `main.rs` test is strictly needed — the wiring is a single
line reading `app.config.eviction.enabled`, and the `EvictionPolicy` struct
already exists. If a test is desired, it would assert that the `EvictionPolicy`
built in the turn path has `enabled` matching `app.config.eviction.enabled` —
but this is a one-line field assignment, not complex logic.

## Scope

This is a config-pipeline + single-wiring change. Files touched:

- `crates/zoid-core/src/config.rs` — `EvictionConfig` + manual `Default` impl,
  `PartialEviction`, `Provenance`, `merge` function
- `crates/zoid-tui/src/config_view.rs` — one new `FieldRow` in the Economy
  section, updated `Provenance` literals in existing tests
- `crates/zoid/src/main.rs` — one line change (`enabled:` source), one new TOML
  write-back arm
- Tests in `config.rs` and `config_view.rs`

No new dependencies, no eviction logic changes, no persistence changes, no
model-context impact.