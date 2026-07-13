# Claude-Plugin Importer + Plugin Generalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make any Claude Code plugin importable into zoid by generalizing the mode-body generator, adding a `skills` manifest kind, and building a deterministic hybrid converter (`crates/zoid-plugin-import`) that reads Claude plugin/marketplace manifests and emits zoid artifacts.

**Architecture:** Two pure, IO-free changes in the existing `zoid-plugin` crate (manifest-driven mode body; `skills` kind), one install-side change in the `zoid` bin (skills materialize into the convention skills dir; `--mode`/`--skills` override flags), and a new workspace bin whose pure `classify`/`emit` core is fed by an effectful `fetch` shell. Emitted manifests are round-tripped through `zoid_plugin::parse_manifest` + `validate()` so nothing is produced that the installer cannot consume.

**Tech Stack:** Rust 2021, `serde`/`serde_json`/`toml`, `reqwest` (rustls) + `tokio` for GitHub API fetch, `git` CLI for `ls-remote`, `zoid-plugin` + `zoid-core` for types, `insta`/golden files + plain `#[test]` for tests.

## Global Constraints

- Rust edition 2021; workspace-inherited version (currently `0.4.0`); do not bump the version in this plan.
- `zoid-plugin` stays **pure/IO-free** (no `std::fs`, no network) — mirrors `zoid-core::wizard`.
- New deps must set `default-features = false` where TLS is involved: `reqwest = { workspace = true }` (already `rustls-tls`, no openssl) — the musl release build must stay clean.
- `zoid-plugin-import` is a **dev/tooling bin**, excluded from the release gate feature set; it must still `cargo build --workspace` and `cargo test --workspace` cleanly.
- Unknown manifest **keys** warn/ignore (serde default), unknown **effect names** and **kinds** are hard errors — preserve this existing stance.
- The Superpowers mode-body output must remain **byte-identical**: `crates/zoid-plugin/tests/superpowers_body_golden.txt` is the guardrail and must not change.
- Never log MCP `env` values (secrets) — match `zoid-mcp/config.rs` hygiene in any code that touches `.mcp.json`.
- Commit after every task with a conventional-commit message. No `Co-Authored-By` trailer.

---

## File Structure

**Modified (zoid-plugin, pure):**
- `crates/zoid-plugin/src/manifest.rs` — add `body_intro`/`body_outro` to `ModeRecipe` + raw parse; accept `skills` kind in `validate()`.
- `crates/zoid-plugin/src/plan.rs` — body generator reads manifest intro/outro (else generic default); add `skills`-kind plan branch.
- `crates/zoid-plugin/manifests/superpowers.toml` — carry the exact intro/outro strings.

**Modified (zoid bin):**
- `crates/zoid/src/plugin_install.rs` — `finish_skills_install` (materialize into convention skills dir, no overlay).
- `crates/zoid/src/cli.rs` — parse `--mode`/`--skills` on `:plugin install`.

**Created (new bin):**
- `crates/zoid-plugin-import/Cargo.toml`
- `crates/zoid-plugin-import/src/main.rs` — CLI + front-ends (`bulk`, `repo`).
- `crates/zoid-plugin-import/src/claude.rs` — parse Claude `marketplace.json` + `plugin.json`.
- `crates/zoid-plugin-import/src/classify.rs` — pure capability→kind classification.
- `crates/zoid-plugin-import/src/emit.rs` — build zoid `plugin.toml` + normalized `.mcp.json` + report.
- `crates/zoid-plugin-import/src/fetch.rs` — GitHub tree/blob fetch at a pinned sha; `git ls-remote`.
- `crates/zoid-plugin-import/tests/fixtures/…` — copied real plugin dirs.
- `crates/zoid-plugin-import/tests/roundtrip.rs` — golden round-trip tests.
- Root `Cargo.toml` — add `crates/zoid-plugin-import` to `[workspace].members`.

---

## Task 1: Manifest carries mode body intro/outro

**Files:**
- Modify: `crates/zoid-plugin/src/manifest.rs`
- Test: `crates/zoid-plugin/src/manifest.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `ModeRecipe { loader: String, strip_prefix: String, body: BodyStrategy, description: String, body_intro: Option<String>, body_outro: Option<String> }`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `manifest.rs`:

```rust
#[test]
fn parses_mode_body_intro_outro() {
    let src = GOOD.replace(
        "body = \"from-skill-frontmatter\"",
        "body = \"from-skill-frontmatter\"\nbody_intro = \"INTRO\"\nbody_outro = \"OUTRO\"",
    );
    let m = parse_manifest(&src).unwrap();
    let mode = m.mode.as_ref().unwrap();
    assert_eq!(mode.body_intro.as_deref(), Some("INTRO"));
    assert_eq!(mode.body_outro.as_deref(), Some("OUTRO"));
}

#[test]
fn mode_body_intro_outro_default_to_none() {
    let m = parse_manifest(GOOD).unwrap();
    let mode = m.mode.as_ref().unwrap();
    assert!(mode.body_intro.is_none());
    assert!(mode.body_outro.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid-plugin parses_mode_body_intro_outro`
Expected: FAIL — `body_intro`/`body_outro` fields do not exist on `ModeRecipe`.

- [ ] **Step 3: Add the fields to `ModeRecipe`, `RawMode`, and the conversion**

In the public `ModeRecipe` struct, add:

```rust
pub struct ModeRecipe {
    pub loader: String,
    pub strip_prefix: String,
    pub body: BodyStrategy,
    pub description: String,
    pub body_intro: Option<String>,
    pub body_outro: Option<String>,
}
```

In `RawMode`, add:

```rust
    #[serde(default)]
    body_intro: Option<String>,
    #[serde(default)]
    body_outro: Option<String>,
```

In `parse_manifest`, in the `raw.mode.map(...)` closure, add the two fields:

```rust
        mode: raw.mode.map(|m| ModeRecipe {
            loader: m.loader,
            strip_prefix: m.strip_prefix,
            body: body.expect("body set when mode present"),
            description: m.description,
            body_intro: m.body_intro,
            body_outro: m.body_outro,
        }),
```

Update the existing `manifest()` test helpers in `plan.rs` if the compiler flags the two new fields as missing (add `body_intro: None, body_outro: None`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-plugin`
Expected: PASS (new tests green; existing tests still compile/pass).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/manifest.rs crates/zoid-plugin/src/plan.rs
git commit -m "feat(zoid-plugin): add optional mode body_intro/body_outro to manifest"
```

---

## Task 2: Body generator reads manifest intro/outro, else generic default

**Files:**
- Modify: `crates/zoid-plugin/src/plan.rs`
- Modify: `crates/zoid-plugin/manifests/superpowers.toml`
- Test: `crates/zoid-plugin/src/plan.rs` (inline), plus the existing golden test.

**Interfaces:**
- Consumes: `ModeRecipe.body_intro`, `ModeRecipe.body_outro`, `PluginManifest.name`, `PluginManifest.source`.
- Produces: `generate_body_from_frontmatter(manifest: &PluginManifest, scan: &UpstreamScan, loader_full: &str, strip_prefix: &str) -> String` (signature gains `manifest`).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `plan.rs`:

```rust
#[test]
fn body_uses_manifest_intro_outro_when_present() {
    let mut m = manifest();
    let mode = m.mode.as_mut().unwrap();
    mode.body_intro = Some("CUSTOM INTRO\n".to_string());
    mode.body_outro = Some("\nCUSTOM OUTRO\n".to_string());
    let plan = build_plan(&m, &scan()).unwrap();
    assert!(plan.mapping.mode_body.starts_with("CUSTOM INTRO"));
    assert!(plan.mapping.mode_body.contains("- brainstorming: Use before creative work"));
    assert!(plan.mapping.mode_body.trim_end().ends_with("CUSTOM OUTRO"));
}

#[test]
fn body_falls_back_to_generic_default_using_name_and_repo() {
    let mut m = manifest();
    m.name = "Robotics".to_string();
    m.source = Some(crate::manifest::PluginSource {
        repo: "arpitg1304/robotics-agent-skills".into(),
        ref_: "SHA".into(),
        subtree: "skills".into(),
    });
    // No intro/outro on the recipe.
    let plan = build_plan(&m, &scan()).unwrap();
    assert!(plan.mapping.mode_body.contains("operating in \"Robotics\" mode"));
    assert!(plan.mapping.mode_body.contains("imported from arpitg1304/robotics-agent-skills"));
    assert!(plan.mapping.mode_body.contains("invoke_skill"));
    // The generic default must NOT carry Superpowers-specific text.
    assert!(!plan.mapping.mode_body.contains("verification-before-completion"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-plugin body_uses_manifest_intro_outro body_falls_back_to_generic`
Expected: FAIL — generator ignores manifest fields; still emits hardcoded Superpowers text.

- [ ] **Step 3: Rewrite the generator to source intro/outro from the manifest**

Replace `generate_body_from_frontmatter` and its call site. New call site inside `build_plan`:

```rust
    let mode_body = match mode.body {
        BodyStrategy::FromSkillFrontmatter => {
            generate_body_from_frontmatter(manifest, scan, &loader_full, &mode.strip_prefix)
        }
    };
```

New function:

```rust
fn generate_body_from_frontmatter(
    manifest: &PluginManifest,
    scan: &UpstreamScan,
    loader_full: &str,
    strip_prefix: &str,
) -> String {
    let mut skills: Vec<(String, String)> = Vec::new();
    for f in &scan.files {
        if f.upstream_path == loader_full {
            continue;
        }
        let Some(rel) = f.upstream_path.strip_prefix(strip_prefix) else {
            continue;
        };
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() != 2 || segs[1] != "SKILL.md" {
            continue;
        }
        if let Ok(p) = parse_skill_md(&f.content) {
            skills.push((p.name, p.description));
        }
    }
    skills.sort_by(|a, b| a.0.cmp(&b.0));

    let mode = manifest.mode.as_ref().expect("mode present in build_plan");
    let repo = manifest
        .source
        .as_ref()
        .map(|s| s.repo.as_str())
        .unwrap_or("an upstream repository");

    let intro = mode.body_intro.clone().unwrap_or_else(|| {
        format!(
            "You are operating in \"{}\" mode, imported from {}.\n\n\
             Before any task, check if an available skill applies and invoke it with \
             invoke_skill. The skills are:\n",
            manifest.name, repo
        )
    });
    let outro = mode.body_outro.clone().unwrap_or_else(|| {
        "\nAlways check for an applicable skill before starting work. If multiple skills \
         apply, invoke the most specific one first.\n"
            .to_string()
    });

    let mut body = String::new();
    body.push_str(&intro);
    body.push('\n');
    for (name, desc) in &skills {
        body.push_str(&format!("- {name}: {desc}\n"));
    }
    body.push_str(&outro);
    body
}
```

- [ ] **Step 4: Move Superpowers' exact strings into its manifest to preserve the golden**

Open `crates/zoid-plugin/tests/superpowers_body_golden.txt` and copy the text **before** the first `- ` bullet into `body_intro`, and the text **after** the last bullet into `body_outro`, verbatim (including newlines). Add to the `[mode]` table of `manifests/superpowers.toml`:

```toml
body_intro = """
You are operating in "Superpowers" mode, imported from obra/superpowers.

Before any task, check if an available skill applies and invoke it with invoke_skill. The skills are:
"""
body_outro = """

Always check for an applicable skill before starting work. If multiple skills apply, invoke the most specific one first. After completing work, invoke verification-before-completion before claiming success.

Skill work produces specs, plans, and debugging notes. Keep the running narration terse, and when the work is done do NOT reframe the whole effort in long paragraphs: close with a short recap of what changed and any next step.
"""
```

> Note: the generator inserts one `\n` before the bullets and expects `intro` to end without the blank line and `outro` to begin with the blank line. If the golden test in Step 5 shows a one-newline diff, adjust the trailing/leading newline of `body_intro`/`body_outro` (not the generator) until byte-identical.

- [ ] **Step 5: Run the golden + new tests**

Run: `cargo test -p zoid-plugin`
Expected: PASS — `mode_body_matches_golden_snapshot` still green (byte-identical), plus the two new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-plugin/src/plan.rs crates/zoid-plugin/manifests/superpowers.toml
git commit -m "feat(zoid-plugin): manifest-driven mode body with generic name/repo default"
```

---

## Task 3: Add the `skills` manifest kind

**Files:**
- Modify: `crates/zoid-plugin/src/manifest.rs` (`validate()`)
- Modify: `crates/zoid-plugin/src/plan.rs` (`build_plan` skills branch)
- Test: both files' inline test modules.

**Interfaces:**
- Consumes: `PluginManifest.kind: Vec<String>` (now may contain `"skills"`).
- Produces: `build_plan` returns an `InstallPlan` whose `mapping.entries` contain **no** `canonical_path == "mode.md"` and whose `mode_body` is empty when kind is `skills`.

- [ ] **Step 1: Write the failing tests**

In `manifest.rs` tests:

```rust
#[test]
fn accepts_skills_kind_without_mode_table() {
    let src = r#"
[plugin]
id = "doctools"
schema = 1
kind = ["skills"]
name = "Doc Tools"
description = "on-demand skills"

[source]
repo = "anthropics/skills"
ref = "SHA"
subtree = "skills"
"#;
    let m = parse_manifest(src).unwrap();
    m.validate().unwrap();
    assert_eq!(m.kind, vec!["skills".to_string()]);
    assert!(m.mode.is_none());
}
```

In `plan.rs` tests:

```rust
#[test]
fn build_plan_skills_kind_has_no_mode_md_and_empty_body() {
    let mut m = manifest();
    m.kind = vec!["skills".into()];
    m.mode = None;
    let plan = build_plan(&m, &scan()).unwrap();
    assert!(plan.mapping.mode_body.is_empty());
    let pairs: Vec<(&str, &str)> = plan.mapping.materialize_entries();
    assert!(!pairs.iter().any(|(c, _)| *c == "mode.md"));
    // Skill files are still materialized under their stripped canonical paths.
    assert!(pairs.iter().any(|(c, _)| *c == "brainstorming/SKILL.md"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p zoid-plugin accepts_skills_kind build_plan_skills_kind`
Expected: FAIL — `validate()` rejects `skills`; `build_plan` errors when `mode` is `None`.

- [ ] **Step 3: Accept `skills` in `validate()`**

In `manifest.rs::validate`, replace the kind loop and the mode-requires-table check:

```rust
        for k in &self.kind {
            if k != "mode" && k != "skills" {
                return Err(format!(
                    "plugin '{}' declares unsupported kind '{}' (v1 supports 'mode' and 'skills')",
                    self.id, k
                ));
            }
        }
        if self.kind.iter().any(|k| k == "mode") && self.mode.is_none() {
            return Err(format!(
                "plugin '{}' declares kind 'mode' but has no [mode] table",
                self.id
            ));
        }
```

Update the existing `rejects_unknown_kind` test's injected kind from `"wormhole"` (still unknown → still rejected; no change needed, just confirm it stays `wormhole`).

- [ ] **Step 4: Add the skills branch to `build_plan`**

At the top of `build_plan`, branch on kind before the mode logic:

```rust
pub fn build_plan(manifest: &PluginManifest, scan: &UpstreamScan) -> Result<InstallPlan, String> {
    if manifest.kind.iter().any(|k| k == "skills") && !manifest.kind.iter().any(|k| k == "mode") {
        return build_skills_plan(manifest, scan);
    }
    // ... existing mode logic unchanged ...
```

Add the new function:

```rust
fn build_skills_plan(manifest: &PluginManifest, scan: &UpstreamScan) -> Result<InstallPlan, String> {
    // Skills packs have no loader/overlay: every `<skill>/SKILL.md` (plus its
    // sibling files) is materialized under its canonical (stripped) path.
    let strip = scan_strip_prefix(manifest, scan);
    let mut entries = Vec::new();
    for f in &scan.files {
        let canonical = match f.upstream_path.strip_prefix(strip.as_str()) {
            Some(c) => c.to_string(),
            None => continue,
        };
        entries.push(MappingEntry::Materialize {
            canonical_path: canonical,
            source: f.upstream_path.clone(),
            summary: String::new(),
        });
    }
    Ok(InstallPlan {
        mapping: ModeMapping {
            mode_name: manifest.name.clone(),
            mode_description: manifest.description.clone(),
            mode_body: String::new(),
            entries,
        },
        effects: manifest.install.clone(),
    })
}

/// The prefix stripped from upstream paths for a skills pack. A skills manifest
/// has no `[mode]`, so derive it from the scan's subtree (e.g. "skills/").
fn scan_strip_prefix(_manifest: &PluginManifest, scan: &UpstreamScan) -> String {
    if scan.subtree_path.is_empty() {
        String::new()
    } else {
        format!("{}/", scan.subtree_path)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p zoid-plugin`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-plugin/src/manifest.rs crates/zoid-plugin/src/plan.rs
git commit -m "feat(zoid-plugin): add skills manifest kind (no mode overlay)"
```

---

## Task 4: Install a skills-kind plan into the convention skills dir

**Files:**
- Modify: `crates/zoid/src/plugin_install.rs`
- Test: `crates/zoid/src/plugin_install.rs` (inline, tempdir)

**Interfaces:**
- Consumes: `InstallPlan` (skills kind), `zoid_core::wizard::UpstreamScan`, `crate::mode_wizard::materialize`.
- Produces: `finish_skills_install(plan: &InstallPlan, scan: &UpstreamScan, skills_root: &Path, plugin_id: &str, manifest_ref: &str, origin: &str) -> Result<InstalledPlugin, String>` — materializes each skill under `skills_root` (the convention dir `<cfg>/skills`), writes no `mode.md`, and records a `.zoid-plugin.json` sidecar under `skills_root/.zoid-plugins/<plugin_id>/`.

- [ ] **Step 1: Write the failing test**

In the `tests` module of `plugin_install.rs`:

```rust
#[test]
fn skills_install_materializes_into_convention_dir_no_mode_md() {
    use zoid_plugin::manifest::{PluginManifest, PluginSource};
    let scan = scan(); // existing helper: brainstorming/SKILL.md etc.
    let m = PluginManifest {
        id: "doctools".into(),
        schema: 1,
        kind: vec!["skills".into()],
        name: "Doc Tools".into(),
        description: "d".into(),
        source: Some(PluginSource { repo: "anthropics/skills".into(), ref_: "SHA".into(), subtree: "skills".into() }),
        mode: None,
        install: vec![Effect::Activate],
    };
    let plan = zoid_plugin::plan::build_plan(&m, &scan).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let skills_root = tmp.path().join("skills");
    let out = finish_skills_install(&plan, &scan, &skills_root, "doctools", "SHA", "url").unwrap();
    // A skill landed as <skills_root>/brainstorming/SKILL.md
    assert!(skills_root.join("brainstorming").join("SKILL.md").is_file());
    // No mode.md anywhere under skills_root.
    assert!(!skills_root.join("mode.md").exists());
    // Sidecar recorded for uninstall.
    assert!(skills_root.join(".zoid-plugins").join("doctools").join(".zoid-plugin.json").is_file());
    assert!(out.safe_effects.contains(&Effect::Activate));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid finish_skills_install 2>&1 | head` (or the test name)
Expected: FAIL — `finish_skills_install` is undefined.

- [ ] **Step 3: Implement `finish_skills_install`**

`materialize` writes canonical entries relative to a destination root and drops a `.zoid-provenance.json`. Reuse it with `skills_root` as the destination so `brainstorming/SKILL.md` lands at `<skills_root>/brainstorming/SKILL.md` — exactly where `resolve_skill_dirs` scans. Add:

```rust
/// Install a skills-kind plan into the convention skills dir. Unlike a mode
/// install, there is no `mode.md` overlay and no activation of a mode; the
/// materialized `<skill>/SKILL.md` files are auto-discovered by
/// `skill_import::resolve_skill_dirs` (which scans `<cfg>/skills/*/SKILL.md`).
/// v1 does NOT write config (SetConfig is gated off), so relying on the
/// convention dir is the seam.
pub fn finish_skills_install(
    plan: &InstallPlan,
    scan: &UpstreamScan,
    skills_root: &Path,
    plugin_id: &str,
    manifest_ref: &str,
    origin: &str,
) -> Result<InstalledPlugin, String> {
    // Same v1 effect gate as finish_plugin_install.
    for e in &plan.effects {
        if e.risk() == RiskTier::Dangerous {
            return Err(format!("effect requires confirmation, not yet supported: {e:?}"));
        }
        if matches!(e, Effect::SetConfig { .. }) {
            return Err(format!("config effects are not yet supported: {e:?}"));
        }
    }
    std::fs::create_dir_all(skills_root)
        .map_err(|e| format!("create skills dir {}: {e}", skills_root.display()))?;
    let fetched_at = chrono::Utc::now().to_rfc3339();
    materialize(&plan.mapping, scan, skills_root, &fetched_at).map_err(|e| e.problems.join("; "))?;

    let sidecar_dir = skills_root.join(".zoid-plugins").join(plugin_id);
    std::fs::create_dir_all(&sidecar_dir)
        .map_err(|e| format!("create sidecar dir: {e}"))?;
    let applied: Vec<AppliedEffect> = plan
        .effects
        .iter()
        .map(|e| match e {
            Effect::Activate => AppliedEffect::Activate,
            Effect::OnboardingHint { text } => AppliedEffect::OnboardingHint { text: text.clone() },
            Effect::SetConfig { .. } => unreachable!("SetConfig rejected at the gate"),
        })
        .collect();
    let sidecar = PluginProvenance {
        schema: 1,
        plugin: PluginStamp { id: plugin_id.to_string(), manifest_ref: manifest_ref.to_string(), installed_at: fetched_at.clone() },
        source: PluginProvSource { repo: scan.repo.clone(), ref_: scan.resolved_ref.clone(), subtree: scan.subtree_path.clone(), origin: origin.to_string() },
        files: plan.mapping.canonical_paths().iter().map(|p| p.to_string()).collect(),
        effects_applied: applied,
    };
    let json = serde_json::to_string_pretty(&sidecar).map_err(|e| format!("serialize sidecar: {e}"))?;
    std::fs::write(sidecar_dir.join(".zoid-plugin.json"), json)
        .map_err(|e| format!("write sidecar: {e}"))?;

    let safe_effects: Vec<Effect> = plan.effects.iter().filter(|e| e.risk() == RiskTier::Safe).cloned().collect();
    Ok(InstalledPlugin { dest: skills_root.to_path_buf(), safe_effects })
}
```

> `PluginProvenance.files` is populated here (unlike the mode path) because for skills there is no separate mode `.zoid-provenance.json` root to own the per-plugin file list for uninstall. If `PluginProvSource.files` field is named differently, match the struct in `zoid-plugin/src/provenance.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid finish_skills_install`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/plugin_install.rs
git commit -m "feat(zoid): install skills-kind plans into the convention skills dir"
```

---

## Task 5: `:plugin install` gains `--mode` / `--skills` override flags

**Files:**
- Modify: `crates/zoid/src/cli.rs`
- Test: `crates/zoid/src/cli.rs` (inline)

**Interfaces:**
- Produces: the parsed `:plugin install` command exposes an override enum `KindOverride { None, Mode, Skills }` (or two bools) that the install dispatcher reads to pick `finish_plugin_install` vs `finish_skills_install`, and to flip the manifest kind before `build_plan`.

- [ ] **Step 1: Write the failing test**

Locate the existing `:plugin`/`install` parse test in `cli.rs` (near `parses_uninstall_and_purge`) and add:

```rust
#[test]
fn parses_plugin_install_mode_and_skills_flags() {
    assert_eq!(parse_plugin_install("superpowers --mode"), Some(("superpowers".into(), KindOverride::Mode)));
    assert_eq!(parse_plugin_install("anthropics/skills --skills"), Some(("anthropics/skills".into(), KindOverride::Skills)));
    assert_eq!(parse_plugin_install("superpowers"), Some(("superpowers".into(), KindOverride::None)));
}
```

> Adapt to the file's actual parse-entry shape: if `:plugin install` is parsed inside a larger `parse_command`, add a focused helper `parse_plugin_install(rest: &str) -> Option<(String, KindOverride)>` and test that, then call it from the command parser.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid parses_plugin_install_mode_and_skills`
Expected: FAIL — `KindOverride` / `parse_plugin_install` undefined.

- [ ] **Step 3: Implement the parse**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindOverride { None, Mode, Skills }

/// Parse the argument tail of `:plugin install`. Returns the plugin ref (id or
/// url) and any kind override. `--mode` and `--skills` are mutually exclusive;
/// if both appear, the last one wins (documented, not an error, to match zoid's
/// permissive flag stance).
pub fn parse_plugin_install(rest: &str) -> Option<(String, KindOverride)> {
    let mut plugin_ref: Option<String> = None;
    let mut over = KindOverride::None;
    for tok in rest.split_whitespace() {
        match tok {
            "--mode" => over = KindOverride::Mode,
            "--skills" => over = KindOverride::Skills,
            other if other.starts_with("--") => return None, // unknown flag
            other => {
                if plugin_ref.is_some() {
                    return None; // a second bare token is a parse error
                }
                plugin_ref = Some(other.to_string());
            }
        }
    }
    plugin_ref.map(|r| (r, over))
}
```

Wire `KindOverride` into the install dispatcher: after resolving the manifest, if `over == Mode` set `manifest.kind = vec!["mode".into()]` (and if it had no `[mode]`, `build_plan` uses the generic default body); if `over == Skills` set `manifest.kind = vec!["skills".into()]` and `manifest.mode = None`. Then choose `finish_plugin_install` (mode) or `finish_skills_install` (skills) by the resulting kind.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid parses_plugin_install`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/cli.rs
git commit -m "feat(zoid): --mode/--skills override on :plugin install"
```

---

## Task 6: Scaffold the `zoid-plugin-import` bin

**Files:**
- Create: `crates/zoid-plugin-import/Cargo.toml`
- Create: `crates/zoid-plugin-import/src/main.rs`
- Modify: root `Cargo.toml` (`[workspace].members`)

**Interfaces:**
- Produces: a runnable bin `zoid-plugin-import` with `--help`; modules `claude`, `classify`, `emit`, `fetch` declared (empty stubs filled by later tasks).

- [ ] **Step 1: Write the failing test**

Create `crates/zoid-plugin-import/src/main.rs`:

```rust
mod claude;
mod classify;
mod emit;
mod fetch;

fn main() {
    eprintln!("zoid-plugin-import: use `bulk <marketplace.json>` or `repo <owner/name[/subpath]>`");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(2 + 2, 4);
    }
}
```

Create empty module files so it compiles:

```rust
// src/claude.rs
// src/classify.rs
// src/emit.rs
// src/fetch.rs
```

- [ ] **Step 2: Create the Cargo.toml and register the member**

`crates/zoid-plugin-import/Cargo.toml`:

```toml
[package]
name = "zoid-plugin-import"
edition = "2021"
version.workspace = true
publish = false

[dependencies]
zoid-plugin = { path = "../zoid-plugin" }
zoid-core = { path = "../zoid-core" }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
anyhow = { workspace = true }
reqwest = { workspace = true }
tokio = { workspace = true }
```

In root `Cargo.toml`, add `"crates/zoid-plugin-import"` to `[workspace].members`.

- [ ] **Step 3: Run to verify it builds**

Run: `cargo build -p zoid-plugin-import && cargo test -p zoid-plugin-import`
Expected: PASS (builds; trivial test green).

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-plugin-import Cargo.toml
git commit -m "chore(zoid-plugin-import): scaffold converter bin crate"
```

---

## Task 7: Parse Claude `marketplace.json` + `plugin.json`

**Files:**
- Modify: `crates/zoid-plugin-import/src/claude.rs`
- Create: `crates/zoid-plugin-import/tests/fixtures/marketplace_snippet.json`
- Test: `crates/zoid-plugin-import/src/claude.rs` (inline)

**Interfaces:**
- Produces:
  - `MarketplaceEntry { name: String, description: String, source: PluginSourceRef }`
  - `enum PluginSourceRef { InRepo { path: String }, GitSubdir { url: String, path: String, sha: String }, Github { repo: String, sha: String } }`
  - `fn parse_marketplace(json: &str) -> anyhow::Result<Vec<MarketplaceEntry>>`
  - `fn parse_plugin_json(json: &str) -> anyhow::Result<PluginJson>` where `PluginJson { name: String, description: String }`

- [ ] **Step 1: Write the failing test + fixture**

Fixture `tests/fixtures/marketplace_snippet.json` (real shapes seen in the official marketplace):

```json
{
  "name": "test-market",
  "plugins": [
    { "name": "a-external", "description": "d1",
      "source": { "source": "git-subdir", "url": "https://github.com/o/r.git", "path": "plugins/a", "ref": "main", "sha": "1111111111111111111111111111111111111111" } },
    { "name": "b-inrepo", "description": "d2", "source": "./plugins/b" },
    { "name": "c-github", "description": "d3",
      "source": { "source": "github", "repo": "o2/r2", "commit": "deadbeef", "sha": "2222222222222222222222222222222222222222" } }
  ]
}
```

Test in `claude.rs`:

```rust
#[test]
fn parses_all_three_source_shapes() {
    let json = include_str!("../tests/fixtures/marketplace_snippet.json");
    let entries = parse_marketplace(json).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(matches!(&entries[0].source, PluginSourceRef::GitSubdir { sha, .. } if sha.len() == 40));
    assert!(matches!(&entries[1].source, PluginSourceRef::InRepo { path } if path == "./plugins/b"));
    assert!(matches!(&entries[2].source, PluginSourceRef::Github { repo, .. } if repo == "o2/r2"));
}

#[test]
fn parses_plugin_json() {
    let p = parse_plugin_json(r#"{"name":"github","description":"gh"}"#).unwrap();
    assert_eq!(p.name, "github");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin-import claude`
Expected: FAIL — types/functions undefined.

- [ ] **Step 3: Implement the parser**

```rust
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSourceRef {
    InRepo { path: String },
    GitSubdir { url: String, path: String, sha: String },
    Github { repo: String, sha: String },
}

#[derive(Debug, Clone)]
pub struct MarketplaceEntry {
    pub name: String,
    pub description: String,
    pub source: PluginSourceRef,
}

#[derive(Deserialize)]
struct RawMarket { plugins: Vec<RawEntry> }

#[derive(Deserialize)]
struct RawEntry {
    name: String,
    #[serde(default)]
    description: String,
    source: RawSource,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSource {
    Str(String),
    Obj {
        source: String,
        #[serde(default)] url: Option<String>,
        #[serde(default)] path: Option<String>,
        #[serde(default)] repo: Option<String>,
        #[serde(default)] sha: Option<String>,
    },
}

pub fn parse_marketplace(json: &str) -> anyhow::Result<Vec<MarketplaceEntry>> {
    let raw: RawMarket = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for e in raw.plugins {
        let source = match e.source {
            RawSource::Str(p) => PluginSourceRef::InRepo { path: p },
            RawSource::Obj { source, url, path, repo, sha } => match source.as_str() {
                "git-subdir" => PluginSourceRef::GitSubdir {
                    url: url.ok_or_else(|| anyhow::anyhow!("git-subdir missing url"))?,
                    path: path.unwrap_or_default(),
                    sha: sha.ok_or_else(|| anyhow::anyhow!("git-subdir missing sha"))?,
                },
                "github" => PluginSourceRef::Github {
                    repo: repo.ok_or_else(|| anyhow::anyhow!("github missing repo"))?,
                    sha: sha.ok_or_else(|| anyhow::anyhow!("github missing sha"))?,
                },
                other => anyhow::bail!("unknown source kind '{other}'"),
            },
        };
        out.push(MarketplaceEntry { name: e.name, description: e.description, source });
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct PluginJson { pub name: String, #[serde(default)] pub description: String }

pub fn parse_plugin_json(json: &str) -> anyhow::Result<PluginJson> {
    Ok(serde_json::from_str(json)?)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-plugin-import claude`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin-import/src/claude.rs crates/zoid-plugin-import/tests/fixtures/marketplace_snippet.json
git commit -m "feat(zoid-plugin-import): parse Claude marketplace.json + plugin.json"
```

---

## Task 8: Pure capability classification

**Files:**
- Modify: `crates/zoid-plugin-import/src/classify.rs`
- Test: `crates/zoid-plugin-import/src/classify.rs` (inline)

**Interfaces:**
- Consumes: a `PluginTree { files: Vec<String>, mcp_json: Option<String>, plugin_json: PluginJson }` (paths relative to the plugin root), and a `KindPref { Auto, Mode, Skills }`.
- Produces:
  - `enum TargetKind { Mode { loader: String }, Skills, McpOnly, Unsupported }`
  - `struct Classification { kind: TargetKind, dropped: Vec<String>, mcp_skipped_http: Vec<String> }`
  - `fn classify(tree: &PluginTree, pref: KindPref) -> Classification`

- [ ] **Step 1: Write the failing tests**

```rust
fn tree(files: &[&str]) -> PluginTree {
    PluginTree {
        files: files.iter().map(|s| s.to_string()).collect(),
        mcp_json: None,
        plugin_json: super::claude::PluginJson { name: "p".into(), description: "d".into() },
    }
}

#[test]
fn loader_present_defaults_to_mode() {
    let t = tree(&["skills/using-p/SKILL.md", "skills/foo/SKILL.md"]);
    let c = classify(&t, KindPref::Auto);
    assert!(matches!(c.kind, TargetKind::Mode { ref loader } if loader == "skills/using-p/SKILL.md"));
}

#[test]
fn no_loader_defaults_to_skills() {
    let t = tree(&["skills/foo/SKILL.md", "skills/bar/SKILL.md"]);
    assert!(matches!(classify(&t, KindPref::Auto).kind, TargetKind::Skills));
}

#[test]
fn pref_overrides_default() {
    let t = tree(&["skills/using-p/SKILL.md"]);
    assert!(matches!(classify(&t, KindPref::Skills).kind, TargetKind::Skills));
    let t2 = tree(&["skills/foo/SKILL.md"]);
    assert!(matches!(classify(&t2, KindPref::Mode).kind, TargetKind::Mode { .. }));
}

#[test]
fn commands_and_agents_are_dropped() {
    let t = tree(&["skills/foo/SKILL.md", "commands/x.md", "agents/y.md"]);
    let c = classify(&t, KindPref::Auto);
    assert!(c.dropped.iter().any(|d| d.contains("commands")));
    assert!(c.dropped.iter().any(|d| d.contains("agents")));
}

#[test]
fn http_mcp_server_is_skipped_stdio_kept() {
    let mut t = tree(&[]);
    t.mcp_json = Some(r#"{ "gh": { "type": "http", "url": "https://x" }, "pw": { "command": "npx", "args": ["-y","@playwright/mcp"] } }"#.into());
    let c = classify(&t, KindPref::Auto);
    assert!(c.mcp_skipped_http.iter().any(|s| s == "gh"));
    assert!(matches!(c.kind, TargetKind::McpOnly));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin-import classify`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement classification**

```rust
use crate::claude::PluginJson;

pub struct PluginTree {
    pub files: Vec<String>,
    pub mcp_json: Option<String>,
    pub plugin_json: PluginJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindPref { Auto, Mode, Skills }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKind {
    Mode { loader: String },
    Skills,
    McpOnly,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub kind: TargetKind,
    pub dropped: Vec<String>,
    pub mcp_skipped_http: Vec<String>,
}

const LOADER_HINTS: &[&str] = &["using-", "find-skills", "-overview"];

fn find_loader(files: &[String]) -> Option<String> {
    // A loader is a skills/<name>/SKILL.md whose <name> matches a hint.
    for f in files {
        let Some(rel) = f.strip_prefix("skills/") else { continue };
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() != 2 || segs[1] != "SKILL.md" { continue; }
        let name = segs[0];
        if LOADER_HINTS.iter().any(|h| name.starts_with(h) || name.ends_with(h) || name.contains(h)) {
            return Some(f.clone());
        }
    }
    None
}

fn has_skills(files: &[String]) -> bool {
    files.iter().any(|f| {
        f.strip_prefix("skills/")
            .map(|r| { let s: Vec<&str> = r.split('/').collect(); s.len() == 2 && s[1] == "SKILL.md" })
            .unwrap_or(false)
    })
}

fn http_servers(mcp_json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(mcp_json) else { return out };
    // Accept bare map or { mcpServers: { ... } }.
    let map = v.get("mcpServers").unwrap_or(&v);
    if let Some(obj) = map.as_object() {
        for (name, cfg) in obj {
            let is_http = cfg.get("type").and_then(|t| t.as_str()) == Some("http")
                || cfg.get("url").is_some() && cfg.get("command").is_none();
            if is_http { out.push(name.clone()); }
        }
    }
    out
}

fn has_stdio_server(mcp_json: &str) -> bool {
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(mcp_json) else { return false };
    let map = v.get("mcpServers").unwrap_or(&v);
    map.as_object()
        .map(|o| o.values().any(|c| c.get("command").is_some()))
        .unwrap_or(false)
}

pub fn classify(tree: &PluginTree, pref: KindPref) -> Classification {
    let mut dropped = Vec::new();
    for f in &tree.files {
        if f.starts_with("commands/") { dropped.push(format!("commands: {f}")); }
        if f.starts_with("agents/") { dropped.push(format!("agents: {f}")); }
        if f.starts_with("hooks/") || f.ends_with("hooks.json") { dropped.push(format!("hooks: {f}")); }
    }
    let mcp_skipped_http = tree.mcp_json.as_deref().map(http_servers).unwrap_or_default();
    let has_stdio = tree.mcp_json.as_deref().map(has_stdio_server).unwrap_or(false);

    let kind = if has_skills(&tree.files) {
        let loader = find_loader(&tree.files);
        match pref {
            KindPref::Skills => TargetKind::Skills,
            KindPref::Mode => TargetKind::Mode { loader: loader.unwrap_or_default() },
            KindPref::Auto => match loader {
                Some(l) => TargetKind::Mode { loader: l },
                None => TargetKind::Skills,
            },
        }
    } else if has_stdio {
        TargetKind::McpOnly
    } else {
        TargetKind::Unsupported
    };
    Classification { kind, dropped, mcp_skipped_http }
}
```

> `KindPref::Mode` with no loader yields `Mode { loader: "" }`; `emit` treats an empty loader as "synthesize the generic default body and pick any skill as the overlay entrypoint is unnecessary" — see Task 9. This matches the install-time `--mode` promotion of a loader-less pack.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-plugin-import classify`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin-import/src/classify.rs
git commit -m "feat(zoid-plugin-import): pure capability classification"
```

---

## Task 9: Emit zoid `plugin.toml` + normalized `.mcp.json` + report

**Files:**
- Modify: `crates/zoid-plugin-import/src/emit.rs`
- Test: `crates/zoid-plugin-import/src/emit.rs` (inline)

**Interfaces:**
- Consumes: `MarketplaceEntry`/`PluginSourceRef` (Task 7), `Classification`/`TargetKind` (Task 8), and a resolved `(repo: String, ref_sha: String, subtree: String)`.
- Produces:
  - `struct Emitted { plugin_toml: Option<String>, mcp_json: Option<String>, report: String }`
  - `fn emit(name: &str, description: &str, repo: &str, sha: &str, subtree: &str, class: &Classification, mcp_json_src: Option<&str>) -> anyhow::Result<Emitted>`
  - Every emitted `plugin_toml` is re-parsed via `zoid_plugin::manifest::parse_manifest` + `validate()` before return (hard error otherwise).

- [ ] **Step 1: Write the failing tests**

```rust
use crate::classify::{Classification, TargetKind};

fn cls(kind: TargetKind) -> Classification {
    Classification { kind, dropped: vec![], mcp_skipped_http: vec![] }
}

#[test]
fn emits_valid_mode_manifest_that_reparses() {
    let e = emit("Superpowers", "d", "obra/superpowers", "SHA", "skills",
        &cls(TargetKind::Mode { loader: "skills/using-superpowers/SKILL.md".into() }), None).unwrap();
    let toml = e.plugin_toml.unwrap();
    let m = zoid_plugin::manifest::parse_manifest(&toml).unwrap();
    m.validate().unwrap();
    assert_eq!(m.kind, vec!["mode".to_string()]);
    assert_eq!(m.mode.as_ref().unwrap().loader, "using-superpowers/SKILL.md"); // subtree-stripped
}

#[test]
fn emits_valid_skills_manifest() {
    let e = emit("Doc Tools", "d", "anthropics/skills", "SHA", "skills",
        &cls(TargetKind::Skills), None).unwrap();
    let m = zoid_plugin::manifest::parse_manifest(&e.plugin_toml.unwrap()).unwrap();
    m.validate().unwrap();
    assert_eq!(m.kind, vec!["skills".to_string()]);
    assert!(m.mode.is_none());
}

#[test]
fn normalizes_stdio_mcp_and_reports_http_skips() {
    let src = r#"{ "gh": { "type": "http", "url": "u" }, "pw": { "command": "npx", "args": ["-y","@playwright/mcp"] } }"#;
    let c = Classification { kind: TargetKind::McpOnly, dropped: vec![], mcp_skipped_http: vec!["gh".into()] };
    let e = emit("pw", "d", "microsoft/playwright-mcp", "SHA", "", &c, Some(src)).unwrap();
    let mcp = e.mcp_json.unwrap();
    // Wrapped under mcpServers, stdio server kept, http server dropped.
    assert!(mcp.contains("\"mcpServers\""));
    assert!(mcp.contains("\"pw\""));
    assert!(!mcp.contains("\"gh\""));
    assert!(e.report.contains("gh"));
    assert!(e.plugin_toml.is_none()); // McpOnly emits no plugin.toml
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin-import emit`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement emission**

```rust
use crate::classify::{Classification, TargetKind};
use serde_json::{json, Map, Value};

pub struct Emitted {
    pub plugin_toml: Option<String>,
    pub mcp_json: Option<String>,
    pub report: String,
}

fn slug(name: &str) -> String {
    name.to_lowercase().chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>().trim_matches('-').to_string()
}

pub fn emit(
    name: &str, description: &str, repo: &str, sha: &str, subtree: &str,
    class: &Classification, mcp_json_src: Option<&str>,
) -> anyhow::Result<Emitted> {
    let mut report = String::new();
    report.push_str(&format!("# {name} ({repo}@{})\n", &sha[..sha.len().min(8)]));
    for d in &class.dropped { report.push_str(&format!("- DROPPED {d}\n")); }
    for s in &class.mcp_skipped_http { report.push_str(&format!("- SKIPPED http MCP server '{s}' (needs HttpTransport)\n")); }

    let plugin_toml = match &class.kind {
        TargetKind::Mode { loader } => {
            let loader_rel = strip_subtree(loader, subtree);
            Some(format!(
                "[plugin]\nid = \"{id}\"\nschema = 1\nkind = [\"mode\"]\nname = \"{name}\"\ndescription = \"{desc}\"\n\n\
                 [source]\nrepo = \"{repo}\"\nref = \"{sha}\"\nsubtree = \"{subtree}\"\n\n\
                 [mode]\nloader = \"{loader_rel}\"\nstrip_prefix = \"{subtree}/\"\nbody = \"from-skill-frontmatter\"\ndescription = \"{desc}\"\n\n\
                 [[install]]\neffect = \"activate\"\n",
                id = slug(name), name = name, desc = description.replace('"', "'"),
                repo = repo, sha = sha, subtree = subtree, loader_rel = loader_rel,
            ))
        }
        TargetKind::Skills => Some(format!(
            "[plugin]\nid = \"{id}\"\nschema = 1\nkind = [\"skills\"]\nname = \"{name}\"\ndescription = \"{desc}\"\n\n\
             [source]\nrepo = \"{repo}\"\nref = \"{sha}\"\nsubtree = \"{subtree}\"\n\n\
             [[install]]\neffect = \"activate\"\n",
            id = slug(name), name = name, desc = description.replace('"', "'"),
            repo = repo, sha = sha, subtree = subtree,
        )),
        TargetKind::McpOnly | TargetKind::Unsupported => None,
    };

    // Validate anything we emit round-trips through the installer's parser.
    if let Some(toml) = &plugin_toml {
        let m = zoid_plugin::manifest::parse_manifest(toml)
            .map_err(|e| anyhow::anyhow!("emitted plugin.toml does not parse: {e}"))?;
        m.validate().map_err(|e| anyhow::anyhow!("emitted plugin.toml invalid: {e}"))?;
    }

    let mcp_json = match (mcp_json_src, &class.kind) {
        (Some(src), _) => normalize_mcp(src, &class.mcp_skipped_http)?,
        _ => None,
    };

    Ok(Emitted { plugin_toml, mcp_json, report })
}

fn strip_subtree(loader: &str, subtree: &str) -> String {
    if subtree.is_empty() { return loader.to_string(); }
    loader.strip_prefix(&format!("{subtree}/")).unwrap_or(loader).to_string()
}

/// Normalize a Claude `.mcp.json` (bare map or mcpServers-wrapped) into zoid's
/// `{ mcpServers: { name: { command, args, env } } }`, keeping only stdio
/// (command-based) servers and dropping the http-skipped ones.
fn normalize_mcp(src: &str, http_skipped: &[String]) -> anyhow::Result<Option<String>> {
    let v: Value = serde_json::from_str(src)?;
    let map = v.get("mcpServers").cloned().unwrap_or(v);
    let Some(obj) = map.as_object() else { return Ok(None) };
    let mut out = Map::new();
    for (name, cfg) in obj {
        if http_skipped.contains(name) { continue; }
        let Some(command) = cfg.get("command").and_then(|c| c.as_str()) else { continue };
        let args = cfg.get("args").cloned().unwrap_or_else(|| json!([]));
        let env = cfg.get("env").cloned().unwrap_or_else(|| json!({}));
        out.insert(name.clone(), json!({ "command": command, "args": args, "env": env }));
    }
    if out.is_empty() { return Ok(None); }
    let wrapped = json!({ "mcpServers": Value::Object(out) });
    Ok(Some(serde_json::to_string_pretty(&wrapped)?))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-plugin-import emit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin-import/src/emit.rs
git commit -m "feat(zoid-plugin-import): emit validated plugin.toml + normalized mcp.json"
```

---

## Task 10: GitHub fetch + `git ls-remote` (effectful shell)

**Files:**
- Modify: `crates/zoid-plugin-import/src/fetch.rs`
- Test: `crates/zoid-plugin-import/src/fetch.rs` (inline unit test for the pure URL builder only; network calls are not unit-tested)

**Interfaces:**
- Produces:
  - `fn tree_url(repo: &str, sha: &str) -> String` (pure) → `https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1`
  - `async fn fetch_tree_paths(repo: &str, sha: &str) -> anyhow::Result<Vec<String>>`
  - `async fn fetch_blob(repo: &str, sha: &str, path: &str) -> anyhow::Result<String>`
  - `fn resolve_head_sha(repo: &str, branch: &str) -> anyhow::Result<String>` (shells `git ls-remote https://github.com/{repo} {branch}`)

- [ ] **Step 1: Write the failing test (pure part only)**

```rust
#[test]
fn tree_url_is_recursive_api_url() {
    assert_eq!(
        tree_url("obra/superpowers", "abc123"),
        "https://api.github.com/repos/obra/superpowers/git/trees/abc123?recursive=1"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin-import tree_url_is_recursive`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement fetch**

```rust
use anyhow::Context;

pub fn tree_url(repo: &str, sha: &str) -> String {
    format!("https://api.github.com/repos/{repo}/git/trees/{sha}?recursive=1")
}

fn client() -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().user_agent("zoid-plugin-import");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {tok}"))?);
        b = b.default_headers(h);
    }
    Ok(b.build()?)
}

pub async fn fetch_tree_paths(repo: &str, sha: &str) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = client()?.get(tree_url(repo, sha)).send().await?
        .error_for_status()?.json().await?;
    let tree = v.get("tree").and_then(|t| t.as_array()).context("no tree array")?;
    Ok(tree.iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("blob"))
        .filter_map(|e| e.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
        .collect())
}

pub async fn fetch_blob(repo: &str, sha: &str, path: &str) -> anyhow::Result<String> {
    let url = format!("https://raw.githubusercontent.com/{repo}/{sha}/{path}");
    Ok(client()?.get(url).send().await?.error_for_status()?.text().await?)
}

pub fn resolve_head_sha(repo: &str, branch: &str) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(["ls-remote", &format!("https://github.com/{repo}"), branch])
        .output()?;
    anyhow::ensure!(out.status.success(), "git ls-remote failed for {repo} {branch}");
    let line = String::from_utf8(out.stdout)?;
    let sha = line.split_whitespace().next().context("empty ls-remote output")?;
    Ok(sha.to_string())
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p zoid-plugin-import tree_url_is_recursive`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin-import/src/fetch.rs
git commit -m "feat(zoid-plugin-import): github tree/blob fetch + git ls-remote sha resolve"
```

---

## Task 11: Wire the CLI front-ends + golden round-trip test

**Files:**
- Modify: `crates/zoid-plugin-import/src/main.rs`
- Create: `crates/zoid-plugin-import/tests/fixtures/frontend-design/…` (copied real plugin), `.../superpowers-min/…`, `.../github-mcp/.mcp.json`
- Create: `crates/zoid-plugin-import/tests/roundtrip.rs`

**Interfaces:**
- Consumes: all prior modules.
- Produces: `zoid-plugin-import bulk <marketplace.json> [--out DIR]` and `zoid-plugin-import repo <owner/name[/subpath]> [--mode|--skills] [--out DIR]`; a pure orchestration fn `plan_from_tree(name, description, repo, sha, subtree, tree, mcp_json, pref) -> (Emitted)` that `roundtrip.rs` drives without network.

- [ ] **Step 1: Copy real fixtures**

From the local cache, copy minimal real trees (only the files needed):

```bash
mkdir -p crates/zoid-plugin-import/tests/fixtures/frontend-design/skills/frontend-design
cp ~/.claude/plugins/marketplaces/claude-plugins-official/plugins/frontend-design/skills/frontend-design/SKILL.md \
   crates/zoid-plugin-import/tests/fixtures/frontend-design/skills/frontend-design/SKILL.md
mkdir -p crates/zoid-plugin-import/tests/fixtures/github-mcp
cp ~/.claude/plugins/marketplaces/claude-plugins-official/external_plugins/github/.mcp.json \
   crates/zoid-plugin-import/tests/fixtures/github-mcp/.mcp.json
```

- [ ] **Step 2: Write the failing round-trip test**

`crates/zoid-plugin-import/tests/roundtrip.rs`:

```rust
use zoid_plugin_import::{classify::{classify, KindPref, PluginTree}, claude::PluginJson, emit::emit};

// NOTE: expose modules from a lib target (Step 4) so tests can import them.

#[test]
fn frontend_design_imports_as_skills_and_reparses() {
    let tree = PluginTree {
        files: vec!["skills/frontend-design/SKILL.md".into()],
        mcp_json: None,
        plugin_json: PluginJson { name: "frontend-design".into(), description: "UI".into() },
    };
    let c = classify(&tree, KindPref::Auto);
    let e = emit("frontend-design", "UI", "anthropics/claude-plugins", "SHA", "skills", &c, None).unwrap();
    let toml = e.plugin_toml.expect("skills plugin.toml");
    let m = zoid_plugin::manifest::parse_manifest(&toml).unwrap();
    m.validate().unwrap();
    assert_eq!(m.kind, vec!["skills".to_string()]);
}

#[test]
fn github_mcp_is_http_and_skipped() {
    let src = include_str!("fixtures/github-mcp/.mcp.json");
    let mut tree = PluginTree { files: vec![], mcp_json: Some(src.to_string()),
        plugin_json: PluginJson { name: "github".into(), description: "gh".into() } };
    let c = classify(&tree, KindPref::Auto);
    assert!(!c.mcp_skipped_http.is_empty());
    let e = emit("github", "gh", "anthropics/claude-plugins", "SHA", "", &c, Some(src)).unwrap();
    assert!(e.plugin_toml.is_none());
    assert!(e.mcp_json.is_none()); // only http server present → nothing to normalize
    assert!(e.report.to_lowercase().contains("http"));
    let _ = &mut tree;
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p zoid-plugin-import --test roundtrip`
Expected: FAIL — modules not exposed as a lib; functions not importable.

- [ ] **Step 4: Add a lib target so modules are importable, wire `main`**

Create `crates/zoid-plugin-import/src/lib.rs`:

```rust
pub mod claude;
pub mod classify;
pub mod emit;
pub mod fetch;
```

Change `main.rs` to use the lib and implement the front-ends:

```rust
use zoid_plugin_import::{classify::{classify, KindPref, PluginTree}, claude, emit::emit, fetch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("repo") => run_repo(&args[1..]).await,
        Some("bulk") => run_bulk(&args[1..]).await,
        _ => { eprintln!("usage: zoid-plugin-import <repo|bulk> ..."); std::process::exit(2); }
    }
}

fn parse_pref(args: &[String]) -> KindPref {
    if args.iter().any(|a| a == "--mode") { KindPref::Mode }
    else if args.iter().any(|a| a == "--skills") { KindPref::Skills }
    else { KindPref::Auto }
}

async fn run_repo(args: &[String]) -> anyhow::Result<()> {
    let spec = args.first().anyhow_context("missing <owner/name[/subpath]>")?;
    let pref = parse_pref(args);
    // Split owner/name[/subpath]; subtree defaults to "skills".
    let mut parts = spec.splitn(3, '/');
    let owner = parts.next().unwrap();
    let name = parts.next().anyhow_context("expected owner/name")?;
    let repo = format!("{owner}/{name}");
    let subtree = parts.next().unwrap_or("skills").trim_end_matches('/').to_string();
    let sha = fetch::resolve_head_sha(&repo, "HEAD")?;
    let files = fetch::fetch_tree_paths(&repo, &sha).await?;
    let mcp_json = if files.iter().any(|f| f == ".mcp.json") {
        Some(fetch::fetch_blob(&repo, &sha, ".mcp.json").await?)
    } else { None };
    // For classification we need skill file paths RELATIVE to the plugin root;
    // for a repo-root plugin they already are. Build the tree.
    let plugin = claude::PluginJson { name: name.to_string(), description: String::new() };
    let tree = PluginTree { files, mcp_json: mcp_json.clone(), plugin_json: plugin };
    let c = classify(&tree, pref);
    let e = emit(name, "", &repo, &sha, &subtree, &c, mcp_json.as_deref())?;
    print_emitted(&repo, &e);
    Ok(())
}

async fn run_bulk(args: &[String]) -> anyhow::Result<()> {
    let path = args.first().anyhow_context("missing <marketplace.json>")?;
    let entries = claude::parse_marketplace(&std::fs::read_to_string(path)?)?;
    for entry in entries {
        // Resolve repo+sha from the source ref (pinned in the marketplace).
        let (repo, sha, subtree) = match &entry.source {
            claude::PluginSourceRef::GitSubdir { url, path, sha } => {
                (url.trim_end_matches(".git").trim_start_matches("https://github.com/").to_string(),
                 sha.clone(), format!("{}/skills", path.trim_end_matches('/')))
            }
            claude::PluginSourceRef::Github { repo, sha } => (repo.clone(), sha.clone(), "skills".into()),
            claude::PluginSourceRef::InRepo { .. } => { eprintln!("skip in-repo {} (bulk needs the marketplace repo sha)", entry.name); continue; }
        };
        let files = match fetch::fetch_tree_paths(&repo, &sha).await { Ok(f) => f, Err(e) => { eprintln!("skip {}: {e}", entry.name); continue; } };
        let mcp_json = if files.iter().any(|f| f.ends_with(".mcp.json")) {
            fetch::fetch_blob(&repo, &sha, ".mcp.json").await.ok()
        } else { None };
        let tree = PluginTree { files, mcp_json: mcp_json.clone(), plugin_json: claude::PluginJson { name: entry.name.clone(), description: entry.description.clone() } };
        let c = classify(&tree, KindPref::Auto);
        match emit(&entry.name, &entry.description, &repo, &sha, subtree.trim_end_matches("/skills"), &c, mcp_json.as_deref()) {
            Ok(e) => print_emitted(&repo, &e),
            Err(e) => eprintln!("emit {}: {e}", entry.name),
        }
    }
    Ok(())
}

fn print_emitted(repo: &str, e: &emit::Emitted) {
    println!("== {repo} ==\n{}", e.report);
    if let Some(t) = &e.plugin_toml { println!("--- plugin.toml ---\n{t}"); }
    if let Some(m) = &e.mcp_json { println!("--- .mcp.json ---\n{m}"); }
}

trait AnyhowContext<T> { fn anyhow_context(self, msg: &str) -> anyhow::Result<T>; }
impl<T> AnyhowContext<T> for Option<T> {
    fn anyhow_context(self, msg: &str) -> anyhow::Result<T> { self.ok_or_else(|| anyhow::anyhow!(msg.to_string())) }
}
```

Update `main.rs` top to only contain `fn main` + helpers (modules now live in `lib.rs`). Keep `Cargo.toml` producing both a lib and a bin (default layout with `src/lib.rs` + `src/main.rs` does this automatically).

- [ ] **Step 5: Run all crate tests**

Run: `cargo test -p zoid-plugin-import`
Expected: PASS (unit + `roundtrip.rs`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-plugin-import
git commit -m "feat(zoid-plugin-import): wire bulk/repo front-ends + golden round-trip tests"
```

---

## Task 12: Workspace-wide verification

**Files:** none (verification only)

- [ ] **Step 1: Build the whole workspace**

Run: `cargo build --workspace`
Expected: PASS — no warnings-as-errors; `zoid-plugin-import` compiles.

- [ ] **Step 2: Run the full test suite (no-fail-fast)**

Run: `cargo test --workspace --no-fail-fast`
Expected: PASS — including the Superpowers golden (`mode_body_matches_golden_snapshot`) and the new tests.

- [ ] **Step 3: Confirm the golden did not drift**

Run: `git status --porcelain crates/zoid-plugin/tests/superpowers_body_golden.txt`
Expected: **empty** (the golden file is unmodified — proof the Superpowers body is byte-identical).

- [ ] **Step 4: Commit any final touch-ups (if none, skip)**

```bash
git commit --allow-empty -m "chore(plugin-import): workspace build + test green"
```

---

## Self-Review Notes (author)

- **Spec §3.1a (body generalization):** Tasks 1–2. Golden preserved (Task 2 Step 4/5, Task 12 Step 3).
- **Spec §3.1b (skills kind):** Task 3.
- **Spec §3.3 (install side + `--mode`/`--skills`):** Tasks 4–5. Note the deliberate deviation from the spec's "register via `skills.source_dirs` set_config": v1 gates SetConfig off, so skills materialize into the convention dir instead (documented in Task 4 Step 3).
- **Spec §3.2 + §4 (converter + classification):** Tasks 6–11.
- **Spec §5 (testing):** fixtures + `roundtrip.rs` in Task 11; workspace gate in Task 12.
- **Type consistency:** `PluginTree`, `KindPref`, `TargetKind`, `Classification`, `Emitted` names are used identically across Tasks 8–11. `KindOverride` (install CLI, Task 5) is intentionally distinct from `KindPref` (converter, Task 8) — different layers.
- **Known limitation (deferred to Spec 3):** flat convention-dir skills install can collide skill names across packs; registry first-wins dedup bounds the blast radius for v1's small wholesale packs.
