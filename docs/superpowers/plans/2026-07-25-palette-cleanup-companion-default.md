# Palette Cleanup + Companion Default — Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove `delegate` and `drawer` from the `:`-prefix palette stage1, and add `[companion] enabled` config field (default false) with live overlay toggle.

**Spec:** `docs/superpowers/specs/2026-07-25-palette-cleanup-companion-default-design.md`

**Tech Stack:** Rust (`zoid-core`, `zoid`, `zoid-tui` crates). No new deps.

## Global Constraints

- No coverage reduction. All existing tests must pass.
- `cargo nextest run --workspace --features zoid/local-embed --no-fail-fast` is the gate.
- No co-author trailer in commits (repo `AGENTS.md`).
- Snapshot tests use `cargo insta accept` to regenerate `.snap` files.

---

## File Structure

| File | Change | Task |
|---|---|---|
| `crates/zoid-tui/src/palette.rs` | Remove 2 items from `stage1_items()`, update test | 1 |
| `crates/zoid-core/src/config.rs` | `CompanionConfig.enabled`, `PartialCompanion.enabled`, merge, `Provenance` | 2 |
| `crates/zoid/src/main.rs` | Boot OR, env var, `ConfigToggle` arm, `apply_config_write` live-apply, `test_app` Provenance | 2, 3 |
| `crates/zoid-tui/src/config_view.rs` | Companion row in Interface section, Provenance literals in tests | 3 |
| `crates/zoid-tui/tests/shell_snapshot.rs` | All Provenance literals + snapshot regen | 3 |

---

### Task 1: Palette cleanup — remove `delegate` and `drawer` from stage1

**Goal:** Remove the `delegate` and `drawer` entries from `stage1_items()` in `crates/zoid-tui/src/palette.rs`. Update the test that asserts the exact stage1 label list. Regenerate the snapshot.

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs`
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (snapshot regen only)

- [ ] **Step 1: Remove the 2 entries from `stage1_items()`**

In `crates/zoid-tui/src/palette.rs`, find `fn stage1_items()` (line ~118).
Remove these two `PaletteItem` entries:

```rust
PaletteItem {
    label: "delegate".into(),
    command: Command::Delegate(String::new()),
},
```

and:

```rust
PaletteItem {
    label: "drawer".into(),
    command: Command::Unknown("drawer".into()),
},
```

The resulting `stage1_items()` vec should have 8 entries: `session`, `mode`, `companion`, `compact`, `config`, `help`, `q`, `quit`.

- [ ] **Step 2: Update the `direct_items_stage1_bare_colon` test**

Find `fn direct_items_stage1_bare_colon()` (line ~701). It asserts the exact 10-label vec. Remove `"drawer"` and `"delegate"` from the expected labels. The new assertion should check for 8 labels in order: `session`, `mode`, `companion`, `compact`, `config`, `help`, `q`, `quit`.

- [ ] **Step 3: Regenerate the `palette_direct_stage1_frame` snapshot**

Run the test to trigger the snapshot failure, then accept:
```bash
cargo insta test --workspace --features zoid/local-embed -- palette_direct_stage1_frame
cargo insta accept
```

- [ ] **Step 4: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(palette): remove delegate and drawer from :stage1"
```

---

### Task 2: Config — `companion.enabled` field + provenance + env var

**Goal:** Add `enabled: bool` (default false) to `CompanionConfig`, `PartialCompanion`, the merge, `Provenance`, and the `ZOID_COMPANION_ENABLED` env var. Update all 10 `Provenance` construction sites.

**Files:**
- Modify: `crates/zoid-core/src/config.rs`
- Modify: `crates/zoid/src/main.rs` (env var + `test_app` Provenance)
- Modify: `crates/zoid-tui/src/config_view.rs` (2 test Provenance literals)
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (6 Provenance literals)

- [ ] **Step 1: Add `enabled` to `CompanionConfig` and `Default`**

In `crates/zoid-core/src/config.rs`, add `enabled: bool` as the first field of `CompanionConfig` (line ~62). Default: `false`.

```rust
pub struct CompanionConfig {
    pub enabled: bool,
    pub port: u16,
    pub open: bool,
}
```

Update `Default` (line ~69):
```rust
Self { enabled: false, port: 0, open: true }
```

- [ ] **Step 2: Add `enabled` to `PartialCompanion`**

Find `PartialCompanion` (search for `struct PartialCompanion`). Add:
```rust
pub enabled: Option<bool>,
```

- [ ] **Step 3: Add merge logic + `Provenance` field**

In the merge function, add (alongside the existing `companion.port`/`open` merge):
```rust
if let Some(v) = p.companion.enabled {
    cfg.companion.enabled = v;
    prov.companion_enabled = *src;
}
```

Add `companion_enabled: Source` to `Provenance`.

- [ ] **Step 4: Update all 10 `Provenance { ... }` construction sites**

Add `companion_enabled: Source::Default` to each exhaustive literal:

1. `crates/zoid-core/src/config.rs:531` — `merge()` initializer
2. `crates/zoid/src/main.rs:~7753` — `test_app()`
3. `crates/zoid-tui/src/config_view.rs:~282` — `builds_four_sections_with_env_shadow`
4. `crates/zoid-tui/src/config_view.rs:~366` — `provider_and_model_rows_are_pick_kind`
5-10. `crates/zoid-tui/tests/shell_snapshot.rs` — 6 literals (lines ~923, ~965, ~1012, ~1071, ~1149, ~1197)

Search for `Provenance {` to find them all. Each needs the new field added.

- [ ] **Step 5: Add `ZOID_COMPANION_ENABLED` env var**

In `crates/zoid/src/main.rs`, after the existing `ZOID_COMPANION_OPEN` env handling (line ~207), add:
```rust
if let Ok(v) = std::env::var("ZOID_COMPANION_ENABLED") {
    envp.companion.enabled = Some(matches!(v.trim(), "1" | "true" | "yes"));
}
```

- [ ] **Step 6: Add test coverage for `enabled` parsing/merging**

In `crates/zoid-core/src/config.rs`, find `companion_section_parses_and_merges` (line ~283). Add assertions for `enabled`:

```rust
// Default is false
let (cfg, _) = merge(&vec![(Source::Default, PartialConfig::default())]);
assert!(!cfg.companion.enabled, "companion.enabled defaults to false");

// TOML overrides
let (pc, _) = parse_toml("[companion]\nenabled = true\nport = 9123\nopen = false").unwrap();
let (cfg, _) = merge(&vec![(Source::UserGlobal, pc)]);
assert!(cfg.companion.enabled, "companion.enabled overridden via TOML");
```

- [ ] **Step 7: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(config): companion.enabled field (default false) + ZOID_COMPANION_ENABLED env"
```

---

### Task 3: Boot logic + settings overlay + live toggle

**Goal:** Wire the companion config field into boot logic, the settings overlay (read + write), and the live toggle (start/stop the server when toggled).

**Files:**
- Modify: `crates/zoid/src/main.rs`
- Modify: `crates/zoid-tui/src/config_view.rs`
- Modify: `crates/zoid-tui/tests/shell_snapshot.rs` (snapshot regen)

- [ ] **Step 1: Boot logic OR**

In `crates/zoid/src/main.rs` (line ~2377), change:
```rust
if companion_at_boot {
    enable_companion(&mut app);
}
```
to:
```rust
if companion_at_boot || app.config.companion.enabled {
    enable_companion(&mut app);
}
```

- [ ] **Step 2: Add companion row to the Interface section in `config_view.rs`**

In `crates/zoid-tui/src/config_view.rs`, find the `interface` Section (line ~230). Add a companion row after `reduced motion`:

```rust
let interface = Section {
    title: "Interface".into(),
    rows: vec![
        FieldRow {
            label: "reduced motion",
            value: onoff(cfg.reduced_motion),
            kind: FieldKind::Bool,
            source: prov.reduced_motion,
            env_shadowed: prov.reduced_motion == Source::Env,
            secret_key: None,
        },
        FieldRow {
            label: "companion",
            value: onoff(cfg.companion.enabled),
            kind: FieldKind::Bool,
            source: prov.companion_enabled,
            env_shadowed: prov.companion_enabled == Source::Env,
            secret_key: None,
        },
    ],
};
```

- [ ] **Step 3: Add `"companion"` arm to `ConfigToggle`**

In `crates/zoid/src/main.rs`, find `Action::ConfigToggle` (line ~4515). Add a `"companion"` arm:

```rust
"companion" => Some(("companion.enabled", !app.config.companion.enabled)),
```

Add it alongside the existing `"reduced motion"` and `"thinking"` arms.

- [ ] **Step 4: Live-apply companion in `apply_config_write`**

In `crates/zoid/src/main.rs`, find `fn apply_config_write` (line ~3813). After the existing live-apply for `reduced_motion` (line ~3839), add:

```rust
// Live-apply companion: start or stop the server to match the new config.
if app.config.companion.enabled && !app.shell.companion_on {
    enable_companion(app);
} else if !app.config.companion.enabled && app.shell.companion_on {
    disable_companion(app);
}
```

- [ ] **Step 5: Regenerate ALL config-overlay snapshots**

The companion row in the Interface section changes every snapshot that
renders the config overlay via `build_sections`. There are 3 snapshot
tests affected:
- `config_overlay_frame`
- `config_overlay_provider_picker`
- `config_overlay_narrow_degrades`

Regenerate all of them:
```bash
cargo insta test --workspace --features zoid/local-embed -- config_overlay
cargo insta accept
```

Also verify `config_key_prompt_masks_entry` — if the masked-entry view
replaces the fields column, its snapshot may stay unchanged. Check the
`.new` file (if any) and `--reject` if it's a false positive.

- [ ] **Step 6: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(companion): boot OR, settings overlay row, live toggle"
```

---

### Task 4: Animate subagent glyph in the subagents drawer

**Goal:** Replace the static `glyph::RUNNING` ('◐') in `render_subagents_body` with the per-frame animated `state.spinner`, so running subagents look alive during long delegations.

**Files:**
- Modify: `crates/zoid-tui/src/render.rs`

- [ ] **Step 1: Pass `state.spinner` to `render_subagents_body`**

In `crates/zoid-tui/src/render.rs`, find the `DrawerId::Subagents` call (line ~593):
```rust
DrawerId::Subagents => render_subagents_body(frame, body_rect, &state.subagent_rows),
```
Change to:
```rust
DrawerId::Subagents => render_subagents_body(frame, body_rect, &state.subagent_rows, state.spinner),
```

- [ ] **Step 2: Update `render_subagents_body` signature and glyph**

Find `fn render_subagents_body` (line ~874). Add `spinner: char` parameter:
```rust
fn render_subagents_body(frame: &mut Frame, area: Rect, rows: &[crate::state::SubagentRow], spinner: char) {
```

Replace `glyph::RUNNING` at line ~892 with `spinner`:
```rust
Span::styled(format!("{} ", spinner), Style::new().fg(color::WARN)),
```

`state.spinner` is already computed per-frame with `reduced_motion` awareness (frozen on frame 0 when reduced motion is on), so no extra handling is needed.

- [ ] **Step 3: Run the gate + commit**

```bash
cargo nextest run --workspace --features zoid/local-embed --no-fail-fast
git commit -m "feat(tui): animate subagent glyph in subagents drawer"
```

If any snapshot tests fail due to the spinner glyph change, regenerate with `cargo insta accept`.

---

## Self-Review

**Gilfoyle review (spec) issues addressed:**
- Blocker 1 (overlay toggle no-op): Task 3 Step 3 adds the `ConfigToggle` arm, Step 4 adds the live-apply
- Blocker 2 (Provenance sites): Task 2 Step 4 enumerates all 10 sites
- Test breakage (palette): Task 1 Steps 2-3 update the test + snapshot
- Test breakage (config): Task 2 Step 6 adds `enabled` assertions
- Test breakage (config_view): Task 2 Step 4 fixes Provenance literals, Task 3 Step 5 regenerates ALL config-overlay snapshots (3 snapshots, not just 1)
- Env var: Task 2 Step 5 adds `ZOID_COMPANION_ENABLED`
- Naming: Spec §1 clarifies `:` stage1 vs Ctrl+P `all_items()`

**Subagent animation (Task 4):**
- Uses existing `state.spinner` (per-frame, reduced-motion aware) — no new state
- Single-file change (`render.rs`) — pass `spinner` param, replace `glyph::RUNNING`
- No new tests (render-side visual change)