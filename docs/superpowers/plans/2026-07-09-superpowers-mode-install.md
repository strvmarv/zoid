# One-Action Superpowers Mode Install — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deterministic, model-free `:mode install superpowers` (plus a gated first-run onboarding keypress) that installs the `obra/superpowers` skill set as a zoid mode.

**Architecture:** Reuse the URL-import wizard's two effectful halves — `github_fetch::fetch_tree` (acquire) and `mode_wizard::materialize` (write canonical files + `.zoid-provenance.json`) — and replace only the AI mapping step with a pure, pinned `superpowers_mapping()`. Fetch runs off-thread; the result is handed back to the main loop via a new `AgentUpdate::SuperpowersScan`, which maps → materializes → reloads → switches.

**Tech Stack:** Rust, tokio, reqwest (behind the existing `GithubApi` trait), serde, ratatui/crossterm, `zoid_core::wizard` value types.

## Global Constraints

- Pinned source: repo `obra/superpowers`, ref `d884ae04edebef577e82ff7c4e143debd0bbec99`, subtree `skills`. URL form: `github.com/obra/superpowers/tree/<SHA>/skills`. (verbatim from spec §3)
- No bundling: content is fetched from upstream on explicit user action only; never auto-installed. (spec §1)
- Reuse `github_fetch` and `mode_wizard` unchanged in behavior; only widen visibility if a symbol is private to its module. (spec §4.1)
- Provenance output must be schema-v1 identical to the wizard's so `:mode update superpowers` works. (spec §8)
- Deterministic mapping only; no model/API key. (spec §2)
- Tests use the existing mockable `GithubApi`; **no real network in tests.** (spec §9)
- Tool names are lowercase (`invoke_skill`, `edit`, …) per repo convention.

---

### Task 1: Deterministic mapping + `mode.md` generator (pure)

Pure functions only — no FS, no network. This is the whole "brain" of the recipe.

**Files:**
- Create: `crates/zoid/src/superpowers_install.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod superpowers_install;`)
- Test: inline `#[cfg(test)]` in `superpowers_install.rs`

**Interfaces:**
- Consumes: `zoid_core::wizard::{UpstreamScan, ScannedFile, ModeMapping, MappingEntry}`; `zoid_core::skill::parse_skill_md`.
- Produces:
  - `pub const SUPERPOWERS_URL: &str` = `"github.com/obra/superpowers/tree/d884ae04edebef577e82ff7c4e143debd0bbec99/skills"`
  - `pub const USING_SUPERPOWERS_SRC: &str` = `"skills/using-superpowers/SKILL.md"`
  - `pub fn superpowers_mapping(scan: &UpstreamScan) -> Result<ModeMapping, String>`
  - `fn generate_mode_body(scan: &UpstreamScan) -> String` (private; exercised via `superpowers_mapping`)

- [ ] **Step 1: Register the module**

In `crates/zoid/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod superpowers_install;
```

- [ ] **Step 2: Write the failing test**

Create `crates/zoid/src/superpowers_install.rs` with only the test module and a fixture:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::wizard::{MappingEntry, ScannedFile, UpstreamScan};

    fn skill_md(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody for {name}\n")
    }

    fn fixture() -> UpstreamScan {
        UpstreamScan {
            url: "github.com/obra/superpowers/tree/SHA/skills".into(),
            repo: "obra/superpowers".into(),
            resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile { upstream_path: "skills/using-superpowers/SKILL.md".into(), sha: "a".into(), content: skill_md("using-superpowers", "loader") },
                ScannedFile { upstream_path: "skills/using-superpowers/references/codex-tools.md".into(), sha: "b".into(), content: "ref".into() },
                ScannedFile { upstream_path: "skills/brainstorming/SKILL.md".into(), sha: "c".into(), content: skill_md("brainstorming", "Use before creative work") },
                ScannedFile { upstream_path: "skills/brainstorming/visual-companion.md".into(), sha: "d".into(), content: "vc".into() },
                ScannedFile { upstream_path: "skills/test-driven-development/SKILL.md".into(), sha: "e".into(), content: skill_md("test-driven-development", "Use before writing impl") },
            ],
        }
    }

    #[test]
    fn maps_loader_to_mode_md_and_strips_skills_prefix() {
        let m = superpowers_mapping(&fixture()).unwrap();
        assert_eq!(m.mode_name, "Superpowers");
        let paths: Vec<(&str, &str)> = m.entries.iter().filter_map(|e| match e {
            MappingEntry::Materialize { canonical_path, source, .. } => Some((canonical_path.as_str(), source.as_str())),
            MappingEntry::Skip { .. } => None,
        }).collect();
        // mode.md comes from the loader skill; loader's own SKILL.md is NOT a separate canonical file.
        assert!(paths.contains(&("mode.md", "skills/using-superpowers/SKILL.md")));
        assert!(!paths.iter().any(|(c, _)| *c == "using-superpowers/SKILL.md"));
        // loader's sibling references ARE copied verbatim.
        assert!(paths.contains(&("using-superpowers/references/codex-tools.md", "skills/using-superpowers/references/codex-tools.md")));
        // other skills + their supporting files, prefix stripped.
        assert!(paths.contains(&("brainstorming/SKILL.md", "skills/brainstorming/SKILL.md")));
        assert!(paths.contains(&("brainstorming/visual-companion.md", "skills/brainstorming/visual-companion.md")));
    }

    #[test]
    fn mode_body_lists_skills_alphabetically_from_frontmatter() {
        let m = superpowers_mapping(&fixture()).unwrap();
        // brainstorming before test-driven-development; loader excluded from the list.
        let b_at = m.mode_body.find("- brainstorming: Use before creative work").unwrap();
        let t_at = m.mode_body.find("- test-driven-development: Use before writing impl").unwrap();
        assert!(b_at < t_at, "skills must be alphabetical");
        assert!(!m.mode_body.contains("- using-superpowers:"), "loader is not a listed skill");
        assert!(m.mode_body.contains("invoke it with invoke_skill"));
        assert!(m.mode_body.contains("verification-before-completion before claiming success"));
    }

    #[test]
    fn errors_when_loader_skill_absent() {
        let mut s = fixture();
        s.files.retain(|f| f.upstream_path != USING_SUPERPOWERS_SRC);
        assert!(superpowers_mapping(&s).is_err());
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid superpowers_install 2>&1 | tail -20`
Expected: FAIL — `cannot find function superpowers_mapping` (module has no impl yet).

- [ ] **Step 4: Write the implementation**

Prepend the impl above the test module in `crates/zoid/src/superpowers_install.rs`:

```rust
//! Deterministic, model-free install of the canonical obra/superpowers skill
//! set as a zoid mode. Reuses the URL-import wizard's fetch + materialize; the
//! only bespoke logic is the pinned mapping and the generated mode.md body.

use zoid_core::skill::parse_skill_md;
use zoid_core::wizard::{MappingEntry, ModeMapping, UpstreamScan};

/// Pinned upstream (ref frozen for reproducibility; bump = reviewed change).
pub const SUPERPOWERS_URL: &str =
    "github.com/obra/superpowers/tree/d884ae04edebef577e82ff7c4e143debd0bbec99/skills";

/// The loader skill whose SKILL.md becomes the mode's overlay (mode.md).
pub const USING_SUPERPOWERS_SRC: &str = "skills/using-superpowers/SKILL.md";

const MODE_DESCRIPTION: &str = "Superpowers — a curated skill set for structured \
software engineering workflows (TDD, debugging, code review, planning, parallel \
agents, git worktrees, verification), imported from obra/superpowers.";

/// Build the pinned, deterministic mapping: mode.md is synthesized from the
/// loader skill; every other `skills/<skill>/**` file is copied verbatim with
/// the `skills/` prefix stripped.
pub fn superpowers_mapping(scan: &UpstreamScan) -> Result<ModeMapping, String> {
    if !scan.files.iter().any(|f| f.upstream_path == USING_SUPERPOWERS_SRC) {
        return Err(format!("upstream is missing {USING_SUPERPOWERS_SRC}"));
    }
    let mut entries = vec![MappingEntry::Materialize {
        canonical_path: "mode.md".to_string(),
        source: USING_SUPERPOWERS_SRC.to_string(),
        summary: "Superpowers mode overlay (generated)".to_string(),
    }];
    for f in &scan.files {
        if f.upstream_path == USING_SUPERPOWERS_SRC {
            continue; // consumed as mode.md above
        }
        let Some(canonical) = f.upstream_path.strip_prefix("skills/") else {
            continue; // defensive: fetch_tree only returns paths under the subtree
        };
        entries.push(MappingEntry::Materialize {
            canonical_path: canonical.to_string(),
            source: f.upstream_path.clone(),
            summary: String::new(),
        });
    }
    Ok(ModeMapping {
        mode_name: "Superpowers".to_string(),
        mode_description: MODE_DESCRIPTION.to_string(),
        mode_body: generate_mode_body(scan),
        entries,
    })
}

/// The overlay body materialize writes after the synthesized frontmatter. The
/// skill bullet list is extracted mechanically from each top-level
/// `skills/<skill>/SKILL.md` frontmatter (loader excluded), alphabetical by name.
fn generate_mode_body(scan: &UpstreamScan) -> String {
    let mut skills: Vec<(String, String)> = Vec::new();
    for f in &scan.files {
        if f.upstream_path == USING_SUPERPOWERS_SRC {
            continue;
        }
        let Some(rel) = f.upstream_path.strip_prefix("skills/") else {
            continue;
        };
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() != 2 || segs[1] != "SKILL.md" {
            continue; // only a skill's top-level SKILL.md, not sibling docs
        }
        if let Ok(p) = parse_skill_md(&f.content) {
            skills.push((p.name, p.description));
        }
    }
    skills.sort_by(|a, b| a.0.cmp(&b.0));

    let mut body = String::new();
    body.push_str(
        "You are operating in \"Superpowers\" mode, imported from obra/superpowers.\n\n",
    );
    body.push_str(
        "Before any task, check if an available skill applies and invoke it with \
invoke_skill. The skills are:\n\n",
    );
    for (name, desc) in &skills {
        body.push_str(&format!("- {name}: {desc}\n"));
    }
    body.push_str(
        "\nAlways check for an applicable skill before starting work. If multiple \
skills apply, invoke the most specific one first. After completing work, invoke \
verification-before-completion before claiming success.\n",
    );
    body
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid superpowers_install 2>&1 | tail -20`
Expected: PASS — 3 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/superpowers_install.rs crates/zoid/src/lib.rs
git commit -m "feat(superpowers): deterministic mapping + mode.md generator"
```

---

### Task 2: `finish_install` + async orchestrator + `AgentUpdate::SuperpowersScan`

Wire the pure mapping to the reused writer, add the async fetch, and the main-loop hand-back that materializes → reloads → switches.

**Files:**
- Modify: `crates/zoid/src/superpowers_install.rs` (add `finish_install`)
- Modify: `crates/zoid/src/agent.rs:167-210` (add `SuperpowersScan` variant)
- Modify: `crates/zoid/src/main.rs` (add `App.installing_superpowers: bool`; `install_superpowers()`; handle the new AgentUpdate near the `ModelsFetched` handler ~2625)
- Test: inline `#[cfg(test)]` in `superpowers_install.rs` (fixture `UpstreamScan` + tempdir; no network — `finish_install` takes a scan directly)

**Interfaces:**
- Consumes: `superpowers_mapping` (Task 1); `mode_wizard::materialize(&ModeMapping, &UpstreamScan, dest_dir: &Path, fetched_at: &str) -> Result<PathBuf, MaterializeError>`; `github_fetch::{parse_github_url, HttpGithubApi, fetch_tree}`; `App.ui_tx`.
- Produces:
  - `pub fn finish_install(scan: &UpstreamScan, dest_dir: &Path) -> Result<std::path::PathBuf, String>`
  - `AgentUpdate::SuperpowersScan(Result<UpstreamScan, String>)`
  - `fn install_superpowers(app: &mut App)` (private to main.rs)

- [ ] **Step 1: Write the failing test** (end-to-end map+materialize against a fake fetch)

Append to the `#[cfg(test)] mod tests` in `superpowers_install.rs`:

```rust
    #[test]
    fn finish_install_writes_mode_md_skills_and_provenance() {
        let scan = fixture();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        let out = finish_install(&scan, &dest).expect("install ok");
        assert_eq!(out, dest);
        // mode.md synthesized (frontmatter + generated body).
        let mode_md = std::fs::read_to_string(dest.join("mode.md")).unwrap();
        assert!(mode_md.starts_with("---\nname: Superpowers\n"));
        assert!(mode_md.contains("- brainstorming: Use before creative work"));
        // a scoped skill + its supporting file landed.
        assert!(dest.join("brainstorming/SKILL.md").is_file());
        assert!(dest.join("brainstorming/visual-companion.md").is_file());
        // provenance sidecar: schema 1, pinned-ish source ref, mode.md entry present.
        let prov = std::fs::read_to_string(dest.join(".zoid-provenance.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&prov).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["mode_name"], "Superpowers");
        assert_eq!(v["source"]["repo"], "obra/superpowers");
        assert!(v["files"].as_array().unwrap().iter().any(|f| f["canonical_path"] == "mode.md"));
    }

    #[test]
    fn reinstall_is_clean_slate() {
        let scan = fixture();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        finish_install(&scan, &dest).unwrap();
        // Plant a stale file a later mapping would never produce.
        std::fs::write(dest.join("STALE.md"), "old").unwrap();
        finish_install(&scan, &dest).unwrap();
        assert!(!dest.join("STALE.md").exists(), "clean-slate wipes stale files");
        assert!(dest.join("mode.md").is_file());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid finish_install_writes 2>&1 | tail -20`
Expected: FAIL — `cannot find function finish_install`.

- [ ] **Step 3: Implement `finish_install`**

Add to `superpowers_install.rs` (imports at top: add `use std::path::{Path, PathBuf};` and `use crate::mode_wizard::materialize;`):

```rust
/// Map + write. Pure of app state so it is unit-testable; the caller resolves
/// `dest_dir` (`<cfg>/modes/superpowers`) and handles reload/switch.
///
/// Clean-slate: remove any prior install before writing. `materialize`'s own
/// rollback deletes only files written in the failing attempt (not dirs) and,
/// on a re-install, truncates the old files before deleting them — a failed
/// re-install could otherwise destroy a previously-good mode (review M3).
/// Removing `dest_dir` first makes a failed install leave *nothing* rather than
/// a corrupted mode; the pinned SHA makes a clean re-run cheap.
pub fn finish_install(scan: &UpstreamScan, dest_dir: &Path) -> Result<PathBuf, String> {
    let mapping = superpowers_mapping(scan)?;
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)
            .map_err(|e| format!("remove old install {}: {e}", dest_dir.display()))?;
    }
    let fetched_at = chrono::Utc::now().to_rfc3339();
    materialize(&mapping, scan, dest_dir, &fetched_at).map_err(|e| e.problems.join("; "))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid superpowers_install 2>&1 | tail -20`
Expected: PASS — 5 tests (3 mapping + `finish_install` + `reinstall`).

- [ ] **Step 5: Add the `AgentUpdate` variant**

In `crates/zoid/src/agent.rs`, inside `pub enum AgentUpdate { … }` (after `FeedbackOutcome`, before the closing `}` at line 210):

```rust
    /// Result of an async Superpowers install fetch. `Ok` carries the scanned
    /// upstream tree to finish (map + materialize) on the main loop; `Err`
    /// carries a user-facing message. Deterministic install — no model turn.
    SuperpowersScan(Result<zoid_core::wizard::UpstreamScan, String>),
```

- [ ] **Step 6: Add `install_superpowers()` and the main-loop handler in main.rs**

Add a free function near `exec_command` in `crates/zoid/src/main.rs`:

First add an in-flight flag to `struct App` (find its definition in `main.rs`) and initialize it `false` in the constructor:

```rust
    /// True while a Superpowers install fetch is in flight. Prevents a second
    /// trigger from racing a concurrent write on the same folder (review M4).
    installing_superpowers: bool,
```

Then the function:

```rust
/// Kick off the deterministic Superpowers install: fetch the pinned tree
/// off-thread, then hand the scan back to the main loop via SuperpowersScan.
fn install_superpowers(app: &mut App) {
    if app.installing_superpowers {
        app.shell.status_hint = Some("Superpowers install already in progress…".into());
        return;
    }
    app.shell.status_hint = Some("installing Superpowers…".into());
    let parsed = match zoid::github_fetch::parse_github_url(
        zoid::superpowers_install::SUPERPOWERS_URL,
    ) {
        Ok(p) => p,
        Err(e) => {
            app.shell.status_hint = Some(e);
            return;
        }
    };
    app.installing_superpowers = true;
    let ui_tx = app.ui_tx.clone();
    tokio::spawn(async move {
        let api = zoid::github_fetch::HttpGithubApi::new();
        let res = zoid::github_fetch::fetch_tree(&api, &parsed)
            .await
            .map_err(|e| format!("Superpowers fetch failed: {e}"));
        let _ = ui_tx
            .send(zoid::agent::AgentUpdate::SuperpowersScan(res))
            .await;
    });
}
```

In the `match update { … }` block that handles `AgentUpdate` in the main loop (same block as `AgentUpdate::ModelsFetched` ~line 2625), add:

```rust
                    AgentUpdate::SuperpowersScan(res) => {
                        app.installing_superpowers = false; // fetch attempt concluded
                        match res {
                            Err(e) => app.shell.status_hint = Some(e),
                            Ok(scan) => {
                                let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
                                let dest = cfg_dir.join("modes").join("superpowers");
                                match zoid::superpowers_install::finish_install(&scan, &dest) {
                                    Ok(_) => {
                                        // Reload the registry so the new mode is visible,
                                        // then make it active.
                                        let prev = app.modes.active_name().to_string();
                                        app.modes = zoid::mode_import::build_mode_registry(
                                            &app.base_profile,
                                            &app.mode_dirs,
                                        );
                                        let installed =
                                            app.modes.names().iter().any(|n| n == "Superpowers");
                                        app.modes.set_active(if installed {
                                            "Superpowers"
                                        } else {
                                            prev.as_str()
                                        });
                                        sync_mode_mirror(app);
                                        persist_active_mode(app).await;
                                        app.shell.status_hint =
                                            Some("Superpowers mode installed.".into());
                                    }
                                    Err(e) => {
                                        app.shell.status_hint =
                                            Some(format!("Superpowers install failed: {e}"));
                                    }
                                }
                            }
                        }
                    }
```

- [ ] **Step 7: Build + run the whole zoid crate tests**

Run: `source "$HOME/.cargo/env" && cargo build -p zoid 2>&1 | tail -5 && cargo test -p zoid superpowers_install 2>&1 | tail -10`
Expected: build OK; 5 tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/src/superpowers_install.rs crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(superpowers): async install orchestrator + main-loop materialize/reload"
```

---

### Task 3: `:mode install superpowers` command + dispatch + palette

**Files:**
- Modify: `crates/zoid-tui/src/command.rs` (enum variant + parse + tests)
- Modify: `crates/zoid/src/main.rs` (`exec_command` arm)
- Modify: palette Direct-command list (wherever `:mode import` is offered as a palette row — search `mode import` in `crates/zoid-tui/src/palette.rs`)
- Test: inline tests in `command.rs`

**Interfaces:**
- Consumes: `install_superpowers` (Task 2).
- Produces: `Command::ModeInstallSuperpowers`.

- [ ] **Step 1: Write the failing parse tests**

In `crates/zoid-tui/src/command.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn parses_mode_install_superpowers() {
        assert_eq!(
            parse_command(":mode install superpowers"),
            Command::ModeInstallSuperpowers
        );
        assert_eq!(
            parse_command("mode install superpowers"),
            Command::ModeInstallSuperpowers
        );
    }

    #[test]
    fn mode_install_does_not_shadow_switch_to_a_mode_named_install() {
        // "mode install foo" is NOT the superpowers installer — it stays a switch.
        assert_eq!(
            parse_command(":mode install foo"),
            Command::SwitchMode("install foo".into())
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid-tui parses_mode_install 2>&1 | tail -15`
Expected: FAIL — `Command::ModeInstallSuperpowers` not found.

- [ ] **Step 3: Add the enum variant**

In `command.rs`, in `pub enum Command`, after `ModeUpdate(String)`:

```rust
    /// Deterministically install the pinned obra/superpowers skill set as a
    /// mode (`:mode install superpowers`). No model turn, no API key.
    ModeInstallSuperpowers,
```

- [ ] **Step 4: Add the parse arm**

In `parse_command`, in the `:mode` namespace block, **before** the generic `s if s.starts_with("mode ")` switch arm (so it matches first):

```rust
        "mode install superpowers" => Command::ModeInstallSuperpowers,
```

- [ ] **Step 5: Run to verify parse passes**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid-tui parses_mode_install mode_install_does 2>&1 | tail -15`
Expected: PASS — 2 tests.

- [ ] **Step 6: Handle the command in `exec_command`**

In `crates/zoid/src/main.rs` `exec_command`, add an arm (near the other `Command::Mode*` arms):

```rust
        Command::ModeInstallSuperpowers => {
            install_superpowers(app);
            Ok(false)
        }
```

- [ ] **Step 7: Add the palette row**

The palette is **staged**: `crates/zoid-tui/src/palette.rs` has a `"mode" => { let mut rows = vec![ … ]; … }` block (~line 190) with terse `PaletteItem { label, command }` rows (`"reload"`, `"import"`, `"update"`). Add one sibling entry to that `rows` vec:

```rust
                PaletteItem {
                    label: "install superpowers".into(),
                    command: Command::ModeInstallSuperpowers,
                },
```

(Terse label to match the neighbors — not "Install Superpowers mode".)

- [ ] **Step 8: Build + test**

Run: `source "$HOME/.cargo/env" && cargo build -p zoid 2>&1 | tail -3 && cargo test -p zoid-tui command 2>&1 | tail -10`
Expected: build OK; command tests PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/zoid-tui/src/command.rs crates/zoid-tui/src/palette.rs crates/zoid/src/main.rs
git commit -m "feat(superpowers): :mode install superpowers command + palette row"
```

---

### Task 4: First-run onboarding install line

**No keypress, no route/state change** (review M1: a bare-`s` binding hijacks a new user's first keystroke; the empty-buffer guard is worthless because the buffer *is* empty at keystroke one). The install runs via `Command::ModeInstallSuperpowers` (command + palette, Task 3). The onboarding screen just adds an instructional line pointing at it, shown only when Superpowers isn't installed yet.

**Files:**
- Modify: `crates/zoid-tui/src/onboarding.rs` (`empty_state_lines` gains `offer_superpowers: bool`; add the instructional line; fix the internal test callers broken by the arity change)
- Modify: `crates/zoid/src/main.rs` (compute the offer bool at the empty-state call site ~line 2212 and pass it)
- Test: `onboarding.rs` unit test

**Interfaces:**
- Consumes: nothing new (install is `Command::ModeInstallSuperpowers` from Task 3).
- Produces: `empty_state_lines(first_time_user: bool, offer_superpowers: bool, width: usize)`.

- [ ] **Step 1: Write the failing onboarding test**

In `crates/zoid-tui/src/onboarding.rs` `#[cfg(test)] mod tests`, add (the test module already has `use super::*;`; `Line` is `ratatui::text::Line`):

```rust
    #[test]
    fn superpowers_offer_line_shown_only_when_offered() {
        let joined = |ls: &[ratatui::text::Line]| ls.iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        let with = empty_state_lines(true, true, 80);
        let without = empty_state_lines(true, false, 80);
        assert!(joined(&with).contains(":mode install superpowers"));
        assert!(!joined(&without).contains("Superpowers"));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test -p zoid-tui superpowers_offer_line 2>&1 | tail -15`
Expected: FAIL — `empty_state_lines` takes 2 args, not 3 (arity error).

- [ ] **Step 3: Change the signature + thread the flag**

In `crates/zoid-tui/src/onboarding.rs`:

```rust
pub fn empty_state_lines(first_time_user: bool, offer_superpowers: bool, width: usize) -> Vec<Line<'static>> {
    if first_time_user {
        new_user_lines(offer_superpowers, width)
    } else {
        returning_user_lines(width)
    }
}
```

Add a constant near the other `NEW_USER_*` consts:

```rust
const SUPERPOWERS_OFFER: &str =
    "Run :mode install superpowers to install the Superpowers skill set (brainstorming, TDD, systematic debugging, code review, planning…)";
```

Change `fn new_user_lines(width: usize)` → `fn new_user_lines(offer_superpowers: bool, width: usize)`, and just before its final `lines` return, append:

```rust
    if offer_superpowers {
        lines.push(Line::from(""));
        for w in wrap_title(indent, SUPERPOWERS_OFFER, width) {
            lines.push(Line::from(Span::styled(w, Style::new().fg(color::CHAT_ACCENT))));
        }
    }
```

- [ ] **Step 4: Fix the internal test callers broken by the arity change (m2)**

The new 3-arg signature breaks **every** existing `empty_state_lines(bool, width)` call in this file's own test module — not just one. Enumerate and fix all of them, passing `false` for the new middle arg (they don't assert the offer):

Run: `grep -n "empty_state_lines(" crates/zoid-tui/src/onboarding.rs`
Then update each call — currently `:103` (`empty_state_lines(true, false, 80)`), `:135` (`empty_state_lines(false, false, 80)`), and every width/wrap test below them — to the 3-arg form. (Do NOT stop at the first one the compiler names; fix them all in one pass.)

- [ ] **Step 5: Update the main.rs call site + compute the offer**

At `crates/zoid/src/main.rs` (~line 2212), in the empty-state intercept block:

```rust
                let offer_superpowers = app.shell.first_time_user
                    && !app.modes.names().iter().any(|n| n == "Superpowers");
                let lines = zoid_tui::onboarding::empty_state_lines(
                    app.shell.first_time_user,
                    offer_superpowers,
                    body_w,
                );
```

Also fix any other `empty_state_lines(` call site the compiler flags (pass `false` where there is no offer context).

- [ ] **Step 6: Build + run onboarding + snapshot tests**

Run: `source "$HOME/.cargo/env" && cargo build -p zoid 2>&1 | tail -3 && cargo test -p zoid-tui onboarding 2>&1 | tail -12 && cargo test -p zoid-tui --test shell_snapshot 2>&1 | tail -8`
Expected: build OK; onboarding tests PASS; snapshot tests PASS (if a snapshot fixture is a first-time user with the offer, the empty-state snapshot changes — review the diff and accept deliberately).

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-tui/src/onboarding.rs crates/zoid/src/main.rs
git commit -m "feat(superpowers): first-run onboarding install line"
```

---

### Task 5: Full-suite verification

- [ ] **Step 1: Whole workspace green**

Run: `source "$HOME/.cargo/env" && cargo test 2>&1 | grep -E "test result: FAILED|error\[" || echo ALL_GREEN`
Expected: `ALL_GREEN`.

- [ ] **Step 2: Manual smoke (real network, optional)**

Run the release build and execute `:mode install superpowers` (or click the palette row). Expect: status "Superpowers mode installed.", `~/.config/zoid/modes/superpowers/` populated, and `Superpowers` in the Shift+Tab cycle. Then `:mode update superpowers` should report "unchanged" — this proves provenance parity **and** verifies the pinned SHA round-trips through `fetch_tree`'s `resolved_ref` (review n2); if update reports drift on a fresh install, the pinned ref in the sidecar differs from the fetch and the `SUPERPOWERS_URL` const needs the tree SHA, not the commit SHA.

- [ ] **Step 3: Commit any snapshot updates**

```bash
git add -A && git commit -m "test(superpowers): update snapshots for onboarding offer" # only if snapshots changed
```

---

## Self-Review

**Spec coverage:**
- §3 pinned source → Task 1 consts. ✓
- §4 architecture (acquire/map/write/reload) → Task 1 (map) + Task 2 (acquire/write/reload). ✓
- §5 mode.md template + auto-extracted descriptions → Task 1 `generate_mode_body`. ✓
- §6.1 command + palette → Task 3. ✓
- §6.2 onboarding instructional line (no keypress) → Task 4. ✓
- §7 error handling + clean-slate cleanup on failure → Task 2 `finish_install` removes `dest` before write. ✓
- §8 idempotency/update (provenance parity) → Task 2 sidecar + reinstall tests; Task 5 smoke checks `:mode update`. ✓
- §9 testing (pure mapping, mockable API, onboarding conditional line, clean-slate reinstall, parse collision) → Tasks 1,2,3,4 tests. ✓
- §10 out of scope (no installer, no auto-install) → honored (opt-in only). ✓

**Placeholder scan:** No TBD/TODO. All code steps show real code, verified against the codebase.

**Type consistency:** `superpowers_mapping`/`finish_install`/`install_superpowers`/`AgentUpdate::SuperpowersScan`/`Command::ModeInstallSuperpowers`/`empty_state_lines(bool,bool,usize)`/`App.installing_superpowers` are used identically across tasks. `materialize(&ModeMapping,&UpstreamScan,&Path,&str)` and `parse_github_url`/`fetch_tree`/`HttpGithubApi` match `github_fetch.rs`/`mode_wizard.rs` verbatim.

## Gilfoyle review — resolution

Reviewed by the gilfoyle-tech-reviewer; verdict "ready-with-fixes". All signatures verified against the real code; `materialize` semantics and the deterministic mapping confirmed against the on-disk install. Fixes folded into this plan + the spec:
- **M1 (major) — bare-`s` keypress: removed.** The install is command + palette only; the onboarding screen shows an instructional line (`Run :mode install superpowers …`). Task 4 no longer touches `route.rs`/`state.rs` and adds no `Action`.
- **m1 — palette:** Task 3 Step 7 now targets the real staged `"mode"` `rows` vec with a terse `PaletteItem { label: "install superpowers", command }`.
- **m2 — onboarding test callers:** Task 4 Step 4 enumerates and fixes *all* internal `empty_state_lines` callers, not just the one the compiler names first.
- **m3 — clean-slate:** `finish_install` removes `dest` before writing (materialize's rollback leaves dirs and can truncate a good install on failure); spec §7 corrected; reinstall test added.
- **m4 — in-flight guard:** `App.installing_superpowers` prevents a second concurrent install racing the same folder.
- **n1 — `prev.as_str()`** in the handler's `set_active` else arm.
- **n2 — pinned-SHA round-trip** verified in the Task 5 smoke (update-reports-unchanged doubles as the ref-parity check).
