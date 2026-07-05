# Config graceful degradation with surfaced warnings

**Date:** 2026-07-05
**Status:** Approved (design)

## Problem

A single unknown key in a config file silently discards the **entire** config
layer. `PartialEconomy` / `PartialConfig` / `PartialSkills` are declared
`#[serde(deny_unknown_fields)]` (config.rs), so any unknown key makes
`toml::from_str` fail the whole document. In `load_config`'s `read()` closure
(zoid/src/main.rs) that `Err` is mapped to `eprintln!("zoid: ignoring …")` +
`None`, dropping every valid key in the file alongside the bad one.

Two independent failures compound:

1. **Blast radius.** A stale/renamed key (`context_ceiling`, renamed to
   `context_target`) took `model`, `provider`, and `base_url` down with it —
   the user's model selection silently reverted to defaults.
2. **Visibility.** The warning is `eprintln!` to stderr *before* the TUI's
   alternate screen takes over, so it scrolls out of sight. For a full-screen
   TUI, stderr is structurally the wrong channel.

## Goal

One unknown key must never discard a whole config layer. Load every valid key;
report the ignored ones where the user will actually see them — while
preserving the typo-surfacing that `deny_unknown_fields` was added to provide.

## Design

### 1. `parse_toml` becomes lenient and reports unknown keys (`zoid-core/src/config.rs`)

- Signature changes to `parse_toml(s) -> anyhow::Result<(PartialConfig, Vec<String>)>`.
  The `Vec<String>` holds dotted unknown-key paths (e.g. `"economy.context_ceiling"`).
- Remove `#[serde(deny_unknown_fields)]` from `PartialConfig`, `PartialEconomy`,
  and `PartialSkills` so known keys always deserialize even when an unknown key
  is present.
- A hard `Err` is now reserved for **genuinely malformed TOML** (syntax errors)
  and **wrong-typed known keys** — never for unknown keys. See §4.

**Unknown-key capture — approach A (`serde_ignored`):**

```rust
let de = toml::Deserializer::new(s);
let mut unknown: Vec<String> = Vec::new();
let cfg: PartialConfig =
    serde_ignored::deserialize(de, |path| unknown.push(path.to_string()))?;
Ok((cfg, unknown))
```

`serde_ignored` (dtolnay-maintained) invokes the callback with the full dotted
path of every field serde ignored, while letting known fields deserialize
normally. This reconciles the two properties that removing `deny_unknown_fields`
alone would trade off: valid keys still load **and** every ignored key is still
reported. Adds one dependency (`serde_ignored`) to the workspace and to
`zoid-core`.

- Update existing callers in `config.rs` tests and `main.rs` that call
  `parse_toml(...)` to destructure the new tuple (`.unwrap()` sites become
  `let (pc, _warn) = parse_toml(...).unwrap();`).

### 2. `load_config` aggregates warnings across layers (`zoid/src/main.rs`)

- The `read()` closure returns `Option<(PartialConfig, Vec<String>)>`:
  - Lenient parse of unknown keys → always `Some` (layer loads).
  - Only a **syntax error** (or wrong-typed known key) keeps the current
    `eprintln!("zoid: ignoring {path}: {e}")` + `None` drop.
- Each returned warning is prefixed with its source file for context, e.g.
  `"config.toml: ignored unknown key economy.context_ceiling"`, and aggregated
  across the UserGlobal / Project / Local layers.
- `load_config` signature gains the aggregate: `-> (Config, Provenance, Vec<String>)`.

### 3. Surface via the transient status hint (`zoid/src/main.rs` startup)

- `ShellState.status_hint: Option<String>` (state.rs) already exists as a
  one-line status-bar notice, seeded `None` in `ShellState::new()`. It is a
  public field, so startup can override it after construction.
- After `ShellState` is built, if the aggregated warnings are non-empty, seed
  `shell.status_hint`:
  - one key → `"config: 1 key ignored (economy.context_ceiling)"`
  - many keys → `"config: N keys ignored — see log"`
- Also keep a durable record: each full warning is emitted via `tracing::warn!`
  (harmless, and it lands in `ZOID_LOG` and the in-memory obs layer).

### 4. Error-handling boundaries (explicit)

| Case | Behavior |
|------|----------|
| Unknown key | **Graceful** — valid keys load, key reported as a warning. *(the fix)* |
| Malformed TOML syntax | Layer dropped with the existing "ignoring" message (genuinely unusable). |
| Wrong type on a *known* key (e.g. `recent_n = "four"`) | Hard layer-drop, as today. Silently ignoring a mistyped *real* setting is worse than saying so. Documented boundary, revisitable later. |

### 5. Testing (TDD)

**`zoid-core/src/config.rs`:**
- Unknown top-level key → known keys load; warning contains the key path.
- Unknown `[economy]` key → sibling economy keys load; warning is `economy.<key>`.
- **Regression:** `context_ceiling` present alongside `model` / `provider` →
  both load; warning names `economy.context_ceiling`.
- Malformed TOML syntax → `Err`.
- Wrong-typed known key → `Err`.
- No unknown keys → empty warnings vec (no behavior change).
- **Inverted existing test:** `unknown_key_is_rejected` (config.rs) currently
  asserts `parse_toml("bogus = 1").is_err()`. Under this design that input is
  now `Ok` with a `"bogus"` warning — the test must be **rewritten** (rename to
  `unknown_key_is_warned_not_rejected`) to assert the warning, not an error.

**`zoid/src/main.rs`:**
- `load_config` prefixes each warning with the source file name.
- `status_hint` is seeded when warnings are present, left `None` when not
  (extract a small pure helper for the summary string so it is unit-testable
  without launching the TUI).

## Out of scope (noted, not touched)

- `ZOID_CONTEXT_CEILING` env var (main.rs:118) still uses the old "ceiling"
  word while mapping to `context_target` — a separate naming inconsistency.
- No "did you mean `context_target`?" suggestion — naming the ignored key
  suffices (YAGNI).

## Files touched

- `Cargo.toml` — add `serde_ignored` to workspace deps.
- `crates/zoid-core/Cargo.toml` — depend on `serde_ignored`.
- `crates/zoid-core/src/config.rs` — lenient `parse_toml`, drop
  `deny_unknown_fields`, new tests.
- `crates/zoid/src/main.rs` — `read()` / `load_config` return warnings; seed
  `status_hint`; helper + tests.
