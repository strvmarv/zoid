# Palette Cleanup + Companion Default — Design

> **Status:** DESIGN (brainstormed 2026-07-25). Ready for `writing-plans`.

---

## 1. Goal

Two independent changes, bundled for a single release:

1. **Remove 4 items from the Ctrl+P palette** — `delegate`, `drawer > repo`, `drawer > session`, `drawer > context`. The functionality stays accessible via `:` commands and keyboard shortcuts.
2. **Add `[companion] enabled` config field** — default `false`. The companion can be enabled via config TOML, the settings overlay, or the `--companion` flag. All three paths work.

---

## 2. Palette cleanup

### 2.1 What's removed

From `stage1_items()` in `crates/zoid-tui/src/palette.rs`:
- **`delegate`** — the `PaletteItem { label: "delegate", command: Command::Delegate(String::new()) }`
- **`drawer`** — the `PaletteItem { label: "drawer", command: Command::Unknown("drawer".into()) }`

Removing `drawer` from stage1 means the three stage2 sub-items (`drawer > repo`, `drawer > session`, `drawer > context`) are no longer reachable via the palette. The `stage2_items("drawer", ...)` match arm stays for `:drawer` routing.

### 2.2 What stays

- **`:delegate <task>`** from the input — still works (routed by `parse_command`)
- **`:drawer repo` / `:drawer session` / `:drawer context`** — still work
- **Tab cycling** — still cycles drawers
- **`stage2_items("drawer", ...)`** — stays as dead code; `:drawer` routing uses it

### 2.3 Files

- `crates/zoid-tui/src/palette.rs` — remove 2 `PaletteItem` entries from `stage1_items()`

---

## 3. Companion config

### 3.1 Config field

Add `enabled: bool` to `CompanionConfig` in `crates/zoid-core/src/config.rs`:

```rust
pub struct CompanionConfig {
    /// Enable the companion browser server at boot. Default false.
    pub enabled: bool,
    /// TCP port for the companion server; 0 = OS-assigned ephemeral.
    pub port: u16,
    /// Auto-open the browser when the companion is enabled.
    pub open: bool,
}
```

Default: `enabled: false`.

### 3.2 TOML parsing

Add `enabled: Option<bool>` to `PartialCompanion`. Merge:
```rust
if let Some(v) = p.companion.enabled {
    cfg.companion.enabled = v;
    prov.companion_enabled = *src;
}
```

Add `companion_enabled: Source` to `Provenance` (3 construction sites).

### 3.3 Boot logic

In `crates/zoid/src/main.rs`, the companion-at-boot check (line 2377):
```rust
if companion_at_boot {
    enable_companion(&mut app);
}
```
becomes:
```rust
if companion_at_boot || app.config.companion.enabled {
    enable_companion(&mut app);
}
```

The `--companion` flag and the config field are OR'd — either path enables the companion.

### 3.4 Settings overlay

Add companion rows to the **Interface** section in `crates/zoid-tui/src/config_view.rs`:

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

Companion goes in Interface (not a new section) — it's a UI feature, same as reduced motion. The `port` and `open` fields are not shown (advanced; set via TOML only).

### 3.5 Files

- `crates/zoid-core/src/config.rs` — `CompanionConfig.enabled`, `PartialCompanion.enabled`, merge, `Provenance`
- `crates/zoid/src/main.rs` — boot logic OR
- `crates/zoid-tui/src/config_view.rs` — companion row in Interface section

---

## 4. Out of scope

- Companion `port`/`open` in the settings overlay (advanced, TOML only)
- Removing `:delegate` or `:drawer` commands (still useful from the input)
- Auto-enabling companion based on session activity
- Companion toggle from the palette (the `:companion` command stays)