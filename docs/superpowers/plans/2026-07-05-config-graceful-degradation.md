# Config Graceful Degradation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a config file with an unknown key load its valid keys anyway and report the ignored keys through the TUI status bar, instead of silently discarding the whole layer.

**Architecture:** `parse_toml` (zoid-core) stops using `deny_unknown_fields` and instead deserializes with `serde_ignored`, returning the deserialized `PartialConfig` plus the dotted paths of any ignored keys. The zoid binary threads those paths through `load_config`, logs each one file-qualified, and seeds the existing `ShellState.status_hint` with a one-line summary at startup.

**Tech Stack:** Rust, `serde` + `serde_ignored`, `toml` 0.8, ratatui-based TUI (`zoid-tui`).

## Global Constraints

- New dependency: `serde_ignored = "0.1"` — add to root `[workspace.dependencies]` and consume in `zoid-core` via `{ workspace = true }`. No other new deps.
- Behavior boundaries (spec §4): unknown key → graceful (load valid keys, warn); malformed TOML syntax → drop layer with existing message; wrong-typed **known** key → drop layer (hard `Err`, not graceful).
- Warning log line format: `"{file}: ignored unknown key {dotted_path}"` (e.g. `config.toml: ignored unknown key economy.context_ceiling`).
- Status-hint summary: exactly one key → `"config: 1 key ignored ({dotted_path})"`; more than one → `"config: {n} keys ignored — see log"`.
- `parse_toml` returns **bare** dotted key paths (it does not know the filename); file-qualification happens in the binary.
- Commit messages: no `Co-Authored-By`/co-author trailer (user rule). TDD, one deliverable per task.

## File Structure

- `Cargo.toml` (root) — add `serde_ignored` to `[workspace.dependencies]`.
- `crates/zoid-core/Cargo.toml` — consume `serde_ignored`.
- `crates/zoid-core/src/config.rs` — lenient `parse_toml` returning `(PartialConfig, Vec<String>)`; drop `deny_unknown_fields`; update in-crate callers; new/rewritten tests. **Owns the parse contract.**
- `crates/zoid/src/main.rs` — `load_config` returns warnings; two pure helpers (`layer_warning_line`, `config_warning_hint`); seed `status_hint`; update `parse_toml` test callers. **Owns wiring + surfacing.**

---

## Task 1: Lenient `parse_toml` with unknown-key capture (zoid-core)

**Files:**
- Modify: `Cargo.toml` (root `[workspace.dependencies]`)
- Modify: `crates/zoid-core/Cargo.toml`
- Modify: `crates/zoid-core/src/config.rs` (struct attrs ~101-125, `parse_toml` ~129-131, tests ~197-248 and ~303-329)
- Test: `crates/zoid-core/src/config.rs` (inline `#[cfg(test)]` modules)

**Interfaces:**
- Produces: `pub fn parse_toml(s: &str) -> anyhow::Result<(PartialConfig, Vec<String>)>` — second element is the bare dotted paths of ignored (unknown) keys, in document order. `PartialConfig` / `PartialEconomy` / `PartialSkills` unchanged in shape; only their serde attrs change (`deny_unknown_fields` removed).

- [ ] **Step 1: Add the `serde_ignored` dependency**

In root `Cargo.toml`, under `[workspace.dependencies]`, next to the existing `serde` line, add:

```toml
serde_ignored = "0.1"
```

In `crates/zoid-core/Cargo.toml`, under `[dependencies]`, next to `serde = { workspace = true }`, add:

```toml
serde_ignored = { workspace = true }
```

Run: `cargo build -p zoid-core`
Expected: PASS (dependency resolves; not yet used).

- [ ] **Step 2: Write the new/rewritten tests (they will not compile yet)**

In `crates/zoid-core/src/config.rs`, **replace** the existing `unknown_key_is_rejected` test (in `mod merge_tests`) with the following, and add the rest into `mod merge_tests`:

```rust
#[test]
fn unknown_key_is_warned_not_rejected() {
    let (pc, warn) = parse_toml("model = \"a\"\nbogus = 1").unwrap();
    assert_eq!(pc.model.as_deref(), Some("a")); // valid key still loads
    assert_eq!(warn, vec!["bogus".to_string()]);
}

#[test]
fn unknown_economy_key_loads_siblings_and_warns_dotted() {
    let (pc, warn) =
        parse_toml("[economy]\ncompact_threshold_pct = 70\ncontext_ceiling = 512000").unwrap();
    assert_eq!(pc.economy.compact_threshold_pct, Some(70)); // sibling loads
    assert_eq!(pc.economy.context_target, None); // renamed key NOT applied
    assert_eq!(warn, vec!["economy.context_ceiling".to_string()]);
}

#[test]
fn regression_stale_ceiling_does_not_drop_model_or_provider() {
    let toml = "model = \"glm-5.2\"\nprovider = \"ollama-cloud\"\n[economy]\ncontext_ceiling = 512000";
    let (pc, warn) = parse_toml(toml).unwrap();
    assert_eq!(pc.model.as_deref(), Some("glm-5.2"));
    assert_eq!(pc.provider.as_deref(), Some("ollama-cloud"));
    assert_eq!(warn, vec!["economy.context_ceiling".to_string()]);
}

#[test]
fn malformed_toml_is_still_err() {
    assert!(parse_toml("this is = = not toml").is_err());
}

#[test]
fn wrong_typed_known_key_is_still_err() {
    // recent_n expects an integer; a string is a hard error, not an unknown key.
    assert!(parse_toml("[economy]\nrecent_n = \"four\"").is_err());
}

#[test]
fn no_unknown_keys_yields_empty_warnings() {
    let (_pc, warn) = parse_toml("model = \"a\"").unwrap();
    assert!(warn.is_empty());
}
```

Also update the existing in-crate callers of `parse_toml` to destructure the new tuple (mechanical `let x = parse_toml(...).unwrap();` → `let (x, _) = parse_toml(...).unwrap();`):
- `mod merge_tests::later_layers_override_and_record_source`: the two `parse_toml(...).unwrap()` bindings for `user` and `proj` → `let (user, _) = ...;` and `let (proj, _) = ...;`.
- `mod merge_tests::parses_skills_source_dirs`: `let p = parse_toml(...).unwrap();` → `let (p, _) = parse_toml(...).unwrap();`.
- `mod merge_tests::merge_unions_source_dirs_across_layers`: the two `user`/`proj` bindings → `let (user, _) = ...;` / `let (proj, _) = ...;`.
- `mod write_tests::sets_top_level_and_nested_preserving_others`: `let p = parse_toml(&out).unwrap();` → `let (p, _) = parse_toml(&out).unwrap();`.
- `mod write_tests::unset_removes_key`: `parse_toml(&out).unwrap().model.is_none()` → `parse_toml(&out).unwrap().0.model.is_none()`.
- `mod write_tests::writes_into_empty_document`: `parse_toml(&out).unwrap().reduced_motion` → `parse_toml(&out).unwrap().0.reduced_motion`.

- [ ] **Step 3: Run tests to verify they fail (compile error = red)**

Run: `cargo test -p zoid-core --lib config`
Expected: FAIL — compile error, `parse_toml` returns `Result<PartialConfig, _>`, not a tuple (signature not changed yet).

- [ ] **Step 4: Make `parse_toml` lenient**

In `crates/zoid-core/src/config.rs`, change the serde attribute on all three partial structs from `#[serde(default, deny_unknown_fields)]` to `#[serde(default)]`:
- `PartialEconomy` (~line 102)
- `PartialSkills` (~line 112)
- `PartialConfig` (~line 118)

Then replace `parse_toml` (~lines 128-131):

```rust
/// Parse one TOML layer. Known keys deserialize normally; unknown keys are NOT
/// rejected — their dotted paths are collected and returned so callers can warn
/// (preserving typo-surfacing without discarding the whole layer). A genuine
/// syntax error, or a wrong-typed *known* key, is still an `Err`.
pub fn parse_toml(s: &str) -> anyhow::Result<(PartialConfig, Vec<String>)> {
    let de = toml::Deserializer::new(s);
    let mut unknown: Vec<String> = Vec::new();
    let cfg: PartialConfig =
        serde_ignored::deserialize(de, |path| unknown.push(path.to_string()))?;
    Ok((cfg, unknown))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-core --lib config`
Expected: PASS — all `config` and `merge_tests` / `write_tests` tests green, including `unknown_key_is_warned_not_rejected` and `regression_stale_ceiling_does_not_drop_model_or_provider`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/zoid-core/Cargo.toml crates/zoid-core/src/config.rs
git commit -m "feat(config): parse_toml keeps valid keys, reports unknown ones

Drop deny_unknown_fields; deserialize via serde_ignored so a stale/renamed
key no longer fails the whole layer. Returns the ignored dotted paths for the
caller to surface. Malformed syntax and wrong-typed known keys still error.

Claude-Session: https://claude.ai/code/session_01PRbGHHvB5VWRGAZBF7t8vH"
```

---

## Task 2: Thread warnings through `load_config` and surface via status hint (zoid bin)

**Files:**
- Modify: `crates/zoid/src/main.rs` — `load_config` (~87-133), startup call site (~1077 + shell at ~1090), live-reload call site (~1914), test `write_config_file_round_trips_through_temp_dir` (~3167-3185); add two helpers + their tests.

**Interfaces:**
- Consumes: `parse_toml(s) -> anyhow::Result<(PartialConfig, Vec<String>)>` from Task 1.
- Produces:
  - `fn load_config() -> (zoid_core::config::Config, zoid_core::config::Provenance, Vec<String>)` — third element is bare dotted paths of all ignored keys across file layers.
  - `fn layer_warning_line(file: &str, key: &str) -> String`
  - `fn config_warning_hint(keys: &[String]) -> Option<String>`

- [ ] **Step 1: Write failing tests for the two pure helpers**

In `crates/zoid/src/main.rs`, inside the existing `#[cfg(test)] mod tests` block (the one containing `write_config_file_round_trips_through_temp_dir`), add:

```rust
#[test]
fn layer_warning_line_is_file_qualified() {
    assert_eq!(
        layer_warning_line("config.toml", "economy.context_ceiling"),
        "config.toml: ignored unknown key economy.context_ceiling"
    );
}

#[test]
fn config_warning_hint_none_one_many() {
    assert_eq!(config_warning_hint(&[]), None);
    assert_eq!(
        config_warning_hint(&["economy.context_ceiling".to_string()]),
        Some("config: 1 key ignored (economy.context_ceiling)".to_string())
    );
    assert_eq!(
        config_warning_hint(&["a".to_string(), "b".to_string()]),
        Some("config: 2 keys ignored — see log".to_string())
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid --bin zoid config_warning_hint layer_warning_line`
Expected: FAIL — compile error, `layer_warning_line` / `config_warning_hint` not found.

- [ ] **Step 3: Add the two pure helpers**

In `crates/zoid/src/main.rs`, directly above `fn load_config(` (~line 87), add:

```rust
/// Format one unknown-key warning for the log, qualified by its source file.
fn layer_warning_line(file: &str, key: &str) -> String {
    format!("{file}: ignored unknown key {key}")
}

/// One-line status-bar summary of ignored config keys, or None when there were
/// none. A single key is named inline; several defer to the log.
fn config_warning_hint(keys: &[String]) -> Option<String> {
    match keys {
        [] => None,
        [one] => Some(format!("config: 1 key ignored ({one})")),
        _ => Some(format!("config: {} keys ignored — see log", keys.len())),
    }
}
```

- [ ] **Step 4: Run helper tests to verify they pass**

Run: `cargo test -p zoid --bin zoid config_warning_hint layer_warning_line`
Expected: PASS.

- [ ] **Step 5: Thread warnings through `load_config` and both call sites**

In `crates/zoid/src/main.rs`, change the `load_config` signature (line ~87) to:

```rust
fn load_config() -> (zoid_core::config::Config, zoid_core::config::Provenance, Vec<String>) {
```

Replace the `read` closure (lines ~91-100) with a warning-collecting version, and add a `warnings` accumulator just above it:

```rust
    let mut warnings: Vec<String> = Vec::new();
    let mut read = |p: PathBuf| -> Option<PartialConfig> {
        let text = std::fs::read_to_string(&p).ok()?;
        let file = p.file_name().and_then(|n| n.to_str()).unwrap_or("config.toml");
        match parse_toml(&text) {
            Ok((pc, unknown)) => {
                for k in unknown {
                    eprintln!("zoid: {}", layer_warning_line(file, &k));
                    warnings.push(k);
                }
                Some(pc)
            }
            Err(e) => {
                eprintln!("zoid: ignoring {}: {e}", p.display());
                None
            }
        }
    };
```

Change the final line of `load_config` (line ~132) from `merge(&layers)` to:

```rust
    let (cfg, prov) = merge(&layers);
    (cfg, prov, warnings)
```

Update the startup call site (line ~1077):

```rust
    let (config, prov, cfg_warnings) = load_config();
```

Immediately after `shell.reduced_motion = config.reduced_motion;` (line ~1091) add:

```rust
    shell.status_hint = config_warning_hint(&cfg_warnings);
```

Update the live-reload call site (line ~1914) — it does not surface, just ignore:

```rust
    let (c, p, _cfg_warnings) = load_config();
```

- [ ] **Step 6: Fix the `parse_toml` callers in the main.rs test**

In `write_config_file_round_trips_through_temp_dir` (~lines 3174, 3178, 3183), the three `parse_toml(...).unwrap()` results are used directly. Change each `let parsed = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();` to:

```rust
        let (parsed, _) = parse_toml(&std::fs::read_to_string(&path).unwrap()).unwrap();
```

(All three occurrences.)

- [ ] **Step 7: Build and test the whole workspace**

Run: `cargo test --workspace`
Expected: PASS — all crates compile and tests are green.

Run: `cargo build --workspace`
Expected: PASS — no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/main.rs
git commit -m "feat(config): surface ignored config keys in the status bar

load_config aggregates the ignored dotted paths, logs each file-qualified,
and seeds ShellState.status_hint with a one-line summary at startup, so a
renamed/typo key is visible in the TUI instead of scrolling past on stderr.

Claude-Session: https://claude.ai/code/session_01PRbGHHvB5VWRGAZBF7t8vH"
```

---

## Manual verification (after both tasks)

Reproduce the original bug on a scratch config, confirming valid keys survive:

```bash
mkdir -p /tmp/zoid-cfg-test
printf 'model = "glm-5.2"\nprovider = "ollama-cloud"\n[economy]\ncontext_ceiling = 512000\n' \
  > /tmp/zoid-cfg-test/config.toml
XDG_CONFIG_HOME=/tmp/zoid-cfg-test/.. HOME=/tmp/zoid-cfg-test cargo run -q -p zoid -- --help 2>&1 | head
```

Expected: stderr shows `zoid: config.toml: ignored unknown key economy.context_ceiling` (not `ignoring …config.toml`), and `model`/`provider` are retained (no whole-file rejection). In a real interactive launch, the status bar shows `config: 1 key ignored (economy.context_ceiling)`.

> Note: exact env-var overrides for the config dir depend on `resolve_config_dir`; if `--help` exits before config load, verify instead by launching normally with the scratch file placed at `~/.config/zoid/config.toml` and observing the status bar.
