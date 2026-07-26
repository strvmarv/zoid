# Palette Cleanup + Companion Default — Design (Revised)

> **Status:** DESIGN (brainstormed + gilfoyle-reviewed, 2026-07-25). Ready for `writing-plans`.
>
> **Revision:** Addresses gilfoyle review: Blocker 1 (overlay toggle no-op), Blocker 2
> (9 Provenance construction sites, not 3), test breakage enumeration, env var decision,
> Ctrl+P vs `:` stage1 naming clarification.

---

## 1. Goal

Two independent changes, bundled for a single release:

1. **Remove `delegate` and `drawer` from the `:`-prefix Direct stage1** — the `:`-prefixed
   palette (opened by typing `:` in the input). The Ctrl+P fuzzy list (`all_items()`) is
   **unaffected** — "Delegate task…" and "Toggle <x> drawer" stay there. The `:delegate`
   and `:drawer` commands still work from the input.
2. **Add `[companion] enabled` config field** — default `false`. The companion can be
   enabled via config TOML, the settings overlay toggle, or the `--companion` flag.
   All three paths work, including live toggle from the overlay.

---

## 2. Palette cleanup

### 2.1 What's removed

From `stage1_items()` in `crates/zoid-tui/src/palette.rs`:
- **`delegate`** — the `PaletteItem { label: "delegate", command: Command::Delegate(String::new()) }`
- **`drawer`** — the `PaletteItem { label: "drawer", command: Command::Unknown("drawer".into()) }`

Removing `drawer` from stage1 means the three stage2 sub-items (`drawer > repo`, `drawer > session`, `drawer > context`) are no longer reachable from the `:` stage1. The `stage2_items("drawer", ...)` match arm stays — it's still live via `:drawer ` routing in `direct_items` (palette.rs:109), just no longer reachable from stage1.

### 2.2 What stays

- **`:delegate <task>`** from the input — still works (routed by `parse_command`)
- **`:drawer repo` / `:drawer session` / `:drawer context`** — still work
- **Tab cycling** — still cycles drawers
- **Ctrl+P `all_items()`** — "Delegate task…" and "Toggle <x> drawer" stay (not removed)
- **`stage2_items("drawer", ...)`** — stays, live via `:drawer ` routing
- **`ArgKind::Delegate`** — stays (used by `:delegate` arg entry)

### 2.3 Test updates required

- **`direct_items_stage1_bare_colon`** (palette.rs:701) — asserts the exact 10-label
  vec including `"drawer"` and `"delegate"`. Update to the new 8-label list.
- **`palette_direct_stage1_frame`** (shell_snapshot.rs:438) — `insta` snapshot of the
  `:` palette. Regenerate with `cargo insta accept`.

`direct_items_stage2_drawer` (palette.rs:732), `direct_items_stage3_delegate_is_empty_free_text`
(palette.rs:774), and `arg_kind_prompts_and_builds_for_all_variants` (palette.rs:798) all
stay green — they exercise `stage2`/`stage3`/`ArgKind`, which are preserved.

### 2.4 Files

- `crates/zoid-tui/src/palette.rs` — remove 2 `PaletteItem` entries from `stage1_items()`
- `crates/zoid-tui/src/palette.rs` — update `direct_items_stage1_bare_colon` test
- `crates/zoid-tui/tests/shell_snapshot.rs` — regenerate `palette_direct_stage1_frame` snapshot

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

Add `enabled: Option<bool>` to `PartialCompanion` (config.rs, `PartialCompanion` struct).

### 3.2 TOML parsing + provenance

Merge:
```rust
if let Some(v) = p.companion.enabled {
    cfg.companion.enabled = v;
    prov.companion_enabled = *src;
}
```

Add `companion_enabled: Source` to `Provenance`.

**All 9 exhaustive `Provenance { … }` construction sites** (no `..` spread — all
will fail E0063 without the new field):

1. `crates/zoid-core/src/config.rs:531` — `merge()` initializer
2. `crates/zoid/src/main.rs:7753` — `test_app()`
3. `crates/zoid-tui/src/config_view.rs:282` — `builds_four_sections_with_env_shadow` test
4. `crates/zoid-tui/src/config_view.rs:366` — `provider_and_model_rows_are_pick_kind` test
5. `crates/zoid-tui/tests/shell_snapshot.rs:923` — `config_overlay_frame`
6. `crates/zoid-tui/tests/shell_snapshot.rs:965` — `economy_overlay_frame`
7. `crates/zoid-tui/tests/shell_snapshot.rs:1012` — `economy_overlay_no_hotkeys`
8. `crates/zoid-tui/tests/shell_snapshot.rs:1071` — `session_picker_10_sessions`
9. `crates/zoid-tui/tests/shell_snapshot.rs:1149` — `session_picker_with_create_new_at_top`
10. `crates/zoid-tui/tests/shell_snapshot.rs:1197` — `session_picker_create_new_followed_by_resume`

(That's 10, not 9 — the original review miscounted. All need `companion_enabled: Source::Default`.)

**Env var:** Add `ZOID_COMPANION_ENABLED` env handling in `main.rs` (parallels
`ZOID_REDUCED_MOTION`), so the `env_shadowed` binding in the settings overlay is
truthful. Precedence: env > TOML > default.

**`port`/`open` stay un-provenanced** — they don't appear in the overlay. This is
deliberate; don't "fix" the asymmetry.

### 3.3 Boot logic

In `crates/zoid/src/main.rs` (line 2377):
```rust
if companion_at_boot || app.config.companion.enabled {
    enable_companion(&mut app);
}
```

The `--companion` flag and the config field are OR'd — either path enables the companion.

### 3.4 Settings overlay — read path

Add companion row to the **Interface** section in `crates/zoid-tui/src/config_view.rs`:

```rust
FieldRow {
    label: "companion",
    value: onoff(cfg.companion.enabled),
    kind: FieldKind::Bool,
    source: prov.companion_enabled,
    env_shadowed: prov.companion_enabled == Source::Env,
    secret_key: None,
},
```

Companion goes in Interface (not a new section) — it's a UI feature, same as reduced
motion. The `port` and `open` fields are not shown (advanced; set via TOML only).

### 3.5 Settings overlay — write path (live toggle)

**Blocker 1 fix.** The `ConfigToggle` handler in `crates/zoid/src/main.rs` (around
line 4515) has an explicit label→key match arm. Add a `"companion"` arm:

```rust
"companion" => Some(("companion.enabled", !app.config.companion.enabled)),
```

Then in `apply_config_write` (or in the toggle arm itself), after the TOML write +
config reload, live-apply the companion lifecycle:

```rust
// Live-apply companion: start or stop the server to match the new config.
if app.config.companion.enabled && !app.shell.companion_on {
    enable_companion(app);
} else if !app.config.companion.enabled && app.shell.companion_on {
    disable_companion(app);
}
```

This mirrors `reduced_motion`'s live-apply at line 3839. Without this, the overlay
would persist to TOML but not start/stop the server until next boot — a config-says-
one-thing, reality-does-another bug.

### 3.6 Test updates required

- **`companion_section_parses_and_merges`** (config.rs:283) — add `enabled` assertions
  (parse + merge + default false)
- **`builds_four_sections_with_env_shadow`** (config_view.rs:277) — section count stays
  4 (companion row added *inside* Interface, not a new section). But the `Provenance { … }`
  literal at `:282` needs `companion_enabled: Source::Default`.
- **`provider_and_model_rows_are_pick_kind`** (config_view.rs:366) — same Provenance fix
- **`config_overlay_frame`** (shell_snapshot.rs:915) — Provenance fix + regenerate snapshot
  (new companion row in Interface section)
- All other `shell_snapshot.rs` Provenance literals (items 5–10 above) — add
  `companion_enabled: Source::Default`

### 3.7 Files

- `crates/zoid-core/src/config.rs` — `CompanionConfig.enabled`, `PartialCompanion.enabled`,
  merge, `Provenance` field
- `crates/zoid/src/main.rs` — boot OR, env var, `ConfigToggle` arm, `apply_config_write`
  live-apply
- `crates/zoid-tui/src/config_view.rs` — companion row in Interface section, Provenance
  literals in tests
- `crates/zoid-tui/tests/shell_snapshot.rs` — all Provenance literals + snapshot regen

---

## 4. Animate the subagent glyph in the subagents drawer

### 4.1 What changes

The subagents drawer currently renders each running subagent with a static
`glyph::RUNNING` ('◐') glyph. During long delegations the drawer looks frozen.
Animate it using the existing `TOOL_FRAMES` moon-phase animation (◐◑◓◒), the
same animation used by the tool indicator in the status bar.

### 4.2 How

`render_subagents_body` (`crates/zoid-tui/src/render.rs:874`) currently takes
`&[SubagentRow]`. It needs the current spinner frame. The `ShellState` already
has `spinner: char` (updated per-frame at `main.rs:2801`), but it uses the
`SPINNER` array, not `TOOL_FRAMES`. Two options:

- **Option A:** Add a `tool_frame: char` to `ShellState` (computed alongside
  `spinner` using `TOOL_FRAMES` instead of `SPINNER`). Pass it to
  `render_subagents_body`.
- **Option B:** Pass `state.spinner` directly (it already animates per-frame).
  Use it instead of `glyph::RUNNING`.

**Recommendation:** Option B — `state.spinner` already animates per-frame
using the `SPINNER` array. The `SPINNER` and `TOOL_FRAMES` are both moon-phase
animations. Using `state.spinner` avoids a new field and reuses the existing
per-frame computation. The visual is slightly different (SPINNER is a 10-frame
cycle, TOOL_FRAMES is 4-frame), but both read as "working" and the subagent
glyph animates.

Change the render call at line 593:
```rust
DrawerId::Subagents => render_subagents_body(frame, body_rect, &state.subagent_rows, state.spinner),
```

Change `render_subagents_body` signature:
```rust
fn render_subagents_body(frame: &mut Frame, area: Rect, rows: &[SubagentRow], spinner: char) {
```

Replace `glyph::RUNNING` at line 892 with `spinner`:
```rust
Span::styled(format!("{} ", spinner), Style::new().fg(color::WARN)),
```

### 4.3 Reduced motion

`state.spinner` is already computed with `reduced_motion` awareness
(`spinner_frame` returns 0 when `reduced_motion` is true, freezing on the
first frame). No extra handling needed.

### 4.4 Files

- `crates/zoid-tui/src/render.rs` — pass `state.spinner` to
  `render_subagents_body`, replace `glyph::RUNNING` with `spinner`

### 4.5 Tests

No new tests needed — the animation is a render-side visual change. Existing
snapshot tests that include the subagents drawer will regenerate (if any).
The `subagent_rows` state is unchanged.

---

## 5. Out of scope

- Companion `port`/`open` in the settings overlay (advanced, TOML only)
- Removing `:delegate` or `:drawer` commands (still useful from the input)
- Removing "Delegate task…" or "Toggle <x> drawer" from Ctrl+P `all_items()` (they stay)
- Auto-enabling companion based on session activity
- Companion toggle from the palette (the `:companion` command stays)