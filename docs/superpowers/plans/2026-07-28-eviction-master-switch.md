# Eviction Master Switch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an explicit `eviction.enabled` boolean master switch to the `[eviction]` TOML config section that directly controls `EvictionPolicy.enabled` at turn-build time, replacing the implicit `compact_threshold_pct > 0` derivation. Decoupled from compaction. Appears as a Bool toggle in the Economy config section.

**Architecture:** A new `enabled: bool` field (default `true`) flows through the standard config pipeline: `EvictionConfig` → `PartialEviction` → `Provenance` → `merge` → config_view row → TOML write-back. The runtime wiring is one line in `main.rs` (the `EvictionPolicy` constructor). Two `main.rs` hardcoded `match label` blocks need new arms: `ConfigToggle` (write path) and `current_toml_value` (read-back). An env-var override `ZOID_EVICTION_ENABLED` is added for consistency with other master switches.

**Tech Stack:** Rust, serde (TOML deserialization), ratatui (config screen rendering).

## Global Constraints

- `eviction.enabled` defaults to `true`. The `#[derive(Default)]` must be **removed** from `EvictionConfig` and replaced with a manual `impl Default` — `bool`'s derived default is `false`, which would silently disable eviction for all users.
- The new field lives in `[eviction]` TOML section (alongside `rescue_weight`), but the config UI row appears in the Economy section.
- `compact_threshold_pct` no longer controls eviction — it controls compaction only. The `> 0` derivation is removed.
- `auto_evict_cold` stays visible and editable when eviction is off. No conditional visibility.
- Env-var `ZOID_EVICTION_ENABLED` follows the `ZOID_COMPANION_ENABLED` pattern: `matches!(v.trim(), "1" | "true" | "yes")`.
- No new dependencies. No eviction logic changes, no persistence changes, no model-context impact.

---

## File Structure

| File | Responsibility | Action |
|------|---------------|--------|
| `crates/zoid-core/src/config.rs` | Config types, merge, provenance | `EvictionConfig` + `Default`, `PartialEviction`, `Provenance`, `merge` |
| `crates/zoid-tui/src/config_view.rs` | Config screen view model | One new `FieldRow` in Economy section, test updates |
| `crates/zoid/src/main.rs` | Runtime wiring, env parsing, config UI actions | `EvictionPolicy` line, `ConfigToggle` arm, `current_toml_value` arm, env var, test updates |

---

### Task 1: Config types — `EvictionConfig`, `PartialEviction`, `Provenance`, `merge`

**Files:**
- Modify: `crates/zoid-core/src/config.rs:138-145` (replace `EvictionConfig` struct + add manual `Default`)
- Modify: `crates/zoid-core/src/config.rs:473-477` (add `enabled` to `PartialEviction`)
- Modify: `crates/zoid-core/src/config.rs:395-418` (add `eviction_enabled` to `Provenance`)
- Modify: `crates/zoid-core/src/config.rs:543-567` (add `eviction_enabled` to `Provenance` initializer in `merge`)
- Modify: `crates/zoid-core/src/config.rs:703-705` (add `eviction.enabled` merge block after `rescue_weight`)
- Test: `crates/zoid-core/src/config.rs` (in `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `EvictionConfig.enabled: bool` (default `true`), `PartialEviction.enabled: Option<bool>`, `Provenance.eviction_enabled: Source`. Task 2 and Task 3 reference `app.config.eviction.enabled` and `prov.eviction_enabled`.

- [ ] **Step 1: Write the failing tests**

Add these tests to the `tests` module in `config.rs`, after the existing `eviction_overrides_across_layers` test (which ends at ~line 352):

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

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-core --lib config::tests::eviction_enabled_defaults_true`
Expected: FAIL — `EvictionConfig` has no `enabled` field; `Provenance` has no `eviction_enabled` field; compile errors.

- [ ] **Step 3: Modify `EvictionConfig` — add `enabled`, remove `#[derive(Default)]`, add manual `Default`**

Replace `crates/zoid-core/src/config.rs` lines 138–145:

Current:
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

New:
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvictionConfig {
    /// Master switch for the eviction controller. `false` = total bypass
    /// (byte-identical to pre-ACM behavior). Default `true`.
    pub enabled: bool,
    /// Rescue weight in turn-index units ("maximal relevance is worth this
    /// many turns of newness"). None ⇒ DEFAULT_RESCUE_WEIGHT const.
    /// Range: ~4–32; see 4b design §5. 0 disables rescue (= pure recency).
    /// Negative, NaN, and +∞ are clamped at the read site (§3.1).
    pub rescue_weight: Option<f32>,
}

impl Default for EvictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rescue_weight: None,
        }
    }
}
```

**Critical:** `#[derive(Default)]` is removed from the derive list. Leaving it in while also adding `impl Default` is a compile error (duplicate `Default` impls). `bool`'s derived default is `false`, which would silently disable eviction for all users — the manual impl sets `enabled: true`.

- [ ] **Step 4: Add `enabled` to `PartialEviction`**

Replace `crates/zoid-core/src/config.rs` lines 473–477:

Current:
```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEviction {
    pub rescue_weight: Option<f32>,
}
```

New:
```rust
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEviction {
    pub enabled: Option<bool>,
    pub rescue_weight: Option<f32>,
}
```

- [ ] **Step 5: Add `eviction_enabled` to `Provenance`**

In `crates/zoid-core/src/config.rs`, add `eviction_enabled: Source,` to the `Provenance` struct. Insert it after `companion_enabled: Source,` (line 417), before the closing `}`:

```rust
    pub companion_enabled: Source,
    pub eviction_enabled: Source,
}
```

- [ ] **Step 6: Add `eviction_enabled` to the `Provenance` initializer in `merge`**

In `crates/zoid-core/src/config.rs`, in the `merge` function (~line 543), add `eviction_enabled: Source::Default,` to the `Provenance` initializer. Insert it after `companion_enabled: Source::Default,` (line 566), before the closing `};`:

```rust
        companion_enabled: Source::Default,
        eviction_enabled: Source::Default,
    };
```

- [ ] **Step 7: Add the `eviction.enabled` merge block**

In `crates/zoid-core/src/config.rs`, in the `merge` function, add after the `rescue_weight` block (after line 705, before the closing `}` of the `for` loop at line 706):

```rust
        if let Some(v) = p.eviction.enabled {
            cfg.eviction.enabled = v;
            prov.eviction_enabled = *src;
        }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib config::tests::eviction_enabled`
Expected: PASS (all 4 new tests)

- [ ] **Step 9: Run full zoid-core test suite to verify no regressions**

Run: `cargo test -p zoid-core --lib`
Expected: PASS (all existing tests including `eviction_defaults_to_none`, `eviction_section_parses_and_merges`, etc.)

- [ ] **Step 10: Fix all `Provenance` literal construction sites that now fail to compile**

Adding `eviction_enabled` to `Provenance` breaks every struct literal that doesn't
list it. The `merge` initializer (Step 6) and the two `config_view.rs` test
literals (handled in Task 2 Step 1) are already covered. The remaining **7** sites
must each get `eviction_enabled: Source::Default,` inserted after
`companion_enabled: Source::Default,`:

1. `crates/zoid-tui/tests/shell_snapshot.rs:923` (`config_overlay_frame`)
2. `crates/zoid-tui/tests/shell_snapshot.rs:965` (`config_key_prompt_masks_entry`)
3. `crates/zoid-tui/tests/shell_snapshot.rs:1012` (`config_overlay_provider_picker`)
4. `crates/zoid-tui/tests/shell_snapshot.rs:1071` (`config_overlay_provider_picker_selection_styles`)
5. `crates/zoid-tui/tests/shell_snapshot.rs:1149` (`config_overlay_narrow_degrades`)
6. `crates/zoid-tui/tests/shell_snapshot.rs:1197` (`config_overlay_narrow_degrades_respects_focus`)
7. `crates/zoid/src/main.rs:8076` (`mod tests` `App` fixture, after `companion_enabled: Source::Default,` at line 8097)

In each, add the line:
```rust
                    companion_enabled: Source::Default,
                    eviction_enabled: Source::Default,
```

**No `.snap` snapshot files change** — all six snapshot tests use `config_section = 0` (Provider & Model), so the Economy section's new eviction row is never rendered. Only the `Provenance` literals fail to compile.

- [ ] **Step 11: Run full test suite (lib + integration) to verify all Provenance literals compile**

Run: `cargo test -p zoid-core --lib && cargo test -p zoid-tui --lib`
Expected: PASS (zoid-core lib tests pass; zoid-tui lib tests pass — integration tests in `tests/` are not compiled by `--lib`)

- [ ] **Step 12: Compile integration tests to verify the 7 Provenance literal fixes**

Run: `cargo test -p zoid-tui --no-run && cargo test -p zoid --no-run`
Expected: Compiles without errors (all `Provenance` literals now include `eviction_enabled`)

- [ ] **Step 13: Commit**

```bash
git add crates/zoid-core/src/config.rs
git commit -m "feat: add eviction.enabled master switch to config pipeline

New EvictionConfig.enabled (default true) with manual Default impl.
Flows through PartialEviction, Provenance, and merge — same pattern
as wake.enabled, companion.enabled, and thinking.enabled."
```

---

### Task 2: Config UI row and runtime wiring in `main.rs`

**Files:**
- Modify: `crates/zoid-tui/src/config_view.rs:185-229` (add eviction row to Economy section)
- Modify: `crates/zoid-tui/src/config_view.rs:287-314` and `374-399` (add `eviction_enabled` to test `Provenance` literals)
- Modify: `crates/zoid/src/main.rs:208-210` (add `ZOID_EVICTION_ENABLED` env var)
- Modify: `crates/zoid/src/main.rs:3949` area (add `current_toml_value` arm)
- Modify: `crates/zoid/src/main.rs:4843-4851` (add `ConfigToggle` arm)
- Modify: `crates/zoid/src/main.rs:7166-7167` (change `enabled:` source)
- Test: `crates/zoid-tui/src/config_view.rs` (updated tests + new assertion)

**Interfaces:**
- Consumes: `EvictionConfig.enabled`, `Provenance.eviction_enabled` from Task 1.
- Produces: A working config screen toggle that reads from and writes to `[eviction] enabled` in TOML.

- [ ] **Step 1: Write the failing test for the config UI row**

Add this assertion to the `builds_four_sections_with_env_shadow` test in `config_view.rs`, after the existing `auto_evict_row` assertions (after line 330):

```rust
        let eviction_row = &economy.rows[0];
        assert_eq!(eviction_row.label, "eviction");
        assert!(matches!(eviction_row.kind, FieldKind::Bool));
        assert_eq!(eviction_row.value, "on"); // default true
```

Also, add `eviction_enabled: Source::Default,` to the `Provenance` literal in that test (after `companion_enabled: Source::Default,` at line 313).

And add `eviction_enabled: Source::Default,` to the `Provenance` literal in `provider_and_model_rows_are_pick_kind` (after `companion_enabled: Source::Default,` at line 398).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-tui --lib config_view::tests::builds_four_sections_with_env_shadow`
Expected: FAIL — the eviction row assertion fails (`economy.rows[0].label == "eviction"` panics, since the row doesn't exist yet). The `Provenance` literal now compiles because Step 1 added the `eviction_enabled` field.

- [ ] **Step 3: Add the eviction `FieldRow` to the Economy section**

In `crates/zoid-tui/src/config_view.rs`, in the `economy` section of `build_sections` (line 185), add a new `FieldRow` at the **top** of the `rows` vec, before "context target":

```rust
    let economy = Section {
        title: "Economy".into(),
        rows: vec![
            FieldRow {
                label: "eviction",
                value: onoff(cfg.eviction.enabled),
                kind: FieldKind::Bool,
                source: prov.eviction_enabled,
                env_shadowed: prov.eviction_enabled == Source::Env,
                secret_key: None,
            },
            FieldRow {
                label: "context target",
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-tui --lib config_view::tests::builds_four_sections_with_env_shadow`
Expected: PASS

- [ ] **Step 5: Run full zoid-tui test suite**

Run: `cargo test -p zoid-tui --lib`
Expected: PASS (all tests including `provider_and_model_rows_are_pick_kind` which also has the updated `Provenance` literal)

- [ ] **Step 6: Add `ZOID_EVICTION_ENABLED` env-var parsing**

In `crates/zoid/src/main.rs`, after the `ZOID_COMPANION_ENABLED` block (line 210, before `layers.push`), add:

```rust
    if let Ok(v) = std::env::var("ZOID_EVICTION_ENABLED") {
        envp.eviction.enabled = Some(matches!(v.trim(), "1" | "true" | "yes"));
    }
```

- [ ] **Step 7: Add `current_write` arm (save-to-repo path)**

In `crates/zoid/src/main.rs`, the `current_write` function (~line 3934) builds a
`Some(match label { ... })` that `Action::ConfigSaveToRepo` uses to persist the
live value into `.zoid/config.toml`. Without a matching arm, saving the
eviction row falls through to `_ => None` and silently does nothing. Add a new
arm before the `"auto-evict cold"` arm:

```rust
        "eviction" => (
            "eviction.enabled",
            TomlValue::Bool(app.config.eviction.enabled),
        ),
        "auto-evict cold" => (
```

- [ ] **Step 8: Add `ConfigToggle` write-path arm**

In `crates/zoid/src/main.rs`, in the `Action::ConfigToggle` block (~line 4843), add a new arm to the `match label` block. Add it before the `"auto-evict cold"` arm:

```rust
                let write = match label {
                    "eviction" => Some((
                        "eviction.enabled",
                        !app.config.eviction.enabled,
                    )),
                    "auto-evict cold" => Some((
```

**This is the actual write path.** Without this arm, pressing the toggle key on the new row does nothing (falls through to `_ => None`).

- [ ] **Step 9: Change the `EvictionPolicy.enabled` source**

In `crates/zoid/src/main.rs` line 7167, change:

```rust
        enabled: app.economy.compact_threshold_pct > 0, // master switch (back-compat)
```

to:

```rust
        enabled: app.config.eviction.enabled,
```

- [ ] **Step 10: Add `main.rs` test assertions**

In the `config_field_target_and_value_mapping` test (~line 7779), add after the existing `"auto-evict cold"` Bool assertion:

```rust
        assert!(field_target("eviction", &FieldKind::Bool).is_none());
```

Note: the `current_write` and `ConfigToggle` arms are hardcoded `match label`
blocks verified by the Task 3 grep and by manual verification. Unit-level
testing of these arms requires an `App` fixture (the existing
`config_field_target_and_value_mapping` test only calls free functions without
an `App`). This is a pre-existing test gap — no Bool row's `current_write` or
`ConfigToggle` arm is unit-tested today. The new row follows the same pattern.

- [ ] **Step 11: Run full test suite**

Run: `cargo test -p zoid-tui --lib && cargo test -p zoid-core --lib`
Expected: PASS (all tests)

- [ ] **Step 12: Commit**

```bash
git add crates/zoid-tui/src/config_view.rs crates/zoid/src/main.rs
git commit -m "feat: wire eviction master switch into config UI and runtime

- Config UI: new 'eviction' Bool row at top of Economy section
- main.rs: ConfigToggle arm (write), current_toml_value arm (read-back)
- main.rs: ZOID_EVICTION_ENABLED env var (mirrors ZOID_COMPANION_ENABLED)
- main.rs: EvictionPolicy.enabled now reads app.config.eviction.enabled
  (replaces compact_threshold_pct > 0 derivation)"
```

---

### Task 3: Verify build and full test suite

**Files:** None modified — verification only.

- [ ] **Step 1: Build the full binary**

Run: `cargo build -p zoid`
Expected: Compiles without errors or warnings

- [ ] **Step 2: Run the complete test suite**

Run: `cargo test`
Expected: PASS (all crates, all tests)

- [ ] **Step 3: Verify no `compact_threshold_pct > 0` eviction derivation remains**

Run: `grep -rn 'compact_threshold_pct > 0' crates/ --include='*.rs'`
Expected: No results — the old `enabled: app.economy.compact_threshold_pct > 0` line must be gone. (The only `compact_threshold_pct` references remaining should be in `policy_from_config` for compaction, which uses `== 0`, not `> 0`.)