# Claude-Plugin Importer + Plugin Generalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make any Claude Code plugin importable into zoid by generalizing the mode-body generator, adding a `skills` manifest kind, and building a deterministic hybrid converter (`crates/zoid-plugin-import`) that reads Claude plugin/marketplace manifests and emits zoid artifacts.

**Architecture:** Two pure, IO-free changes in the existing `zoid-plugin` crate (manifest-driven mode body; `skills` kind), install-side changes in the `zoid` bin (skills materialize into per-pack dirs under the convention skills root, discovered by a one-level scanner descent; `--mode`/`--skills` override flags), and a new workspace bin whose pure `classify`/`emit` core is fed by an effectful `fetch` shell. Emitted manifests are round-tripped through `zoid_plugin::parse_manifest` + `validate()` so nothing is produced that the installer cannot consume.

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
- `crates/zoid/src/plugin_install.rs` — `finish_skills_install` (materialize into a **per-pack private dir** `<cfg>/skills/<plugin_id>/`, no overlay); also patch its `#[cfg(test)]` `ModeRecipe` literal (~line 139).
- `crates/zoid/src/skill_import.rs` — descend one level so `<cfg>/skills/<pack>/<skill>/SKILL.md` is discovered (per-pack dirs are scan roots).
- `crates/zoid-tui/src/command.rs` — `:plugin install` keeps `Command::PluginInstall(String)` carrying the **raw arg incl. flags** (no enum change).
- `crates/zoid/src/main.rs` — at the `PluginInstall` dispatch (~line 5100 / `install_plugin`), parse `--mode`/`--skills` and route to `finish_plugin_install` vs `finish_skills_install`.

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

Update **every** `ModeRecipe { … }` struct literal for the two new fields (the compiler will flag them). There are exactly three (grep `ModeRecipe {`): the `plan.rs` `manifest()` test helper, the `manifest.rs` doc-comment example if any, and — critically — the `#[cfg(test)]` helper in `crates/zoid/src/plugin_install.rs` (~line 139). Add `body_intro: None, body_outro: None` to each. (Missing the `plugin_install.rs` one compiles under `-p zoid-plugin` but breaks `cargo test --workspace` in Task 12 — S1.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid-plugin && cargo build -p zoid --tests`
Expected: PASS (new tests green; the `zoid` bin test build still compiles with the patched helper).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/manifest.rs crates/zoid-plugin/src/plan.rs crates/zoid/src/plugin_install.rs
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

- [ ] **Step 4: Set the exact Superpowers strings on the GOLDEN's source — the in-code `manifest()` helper**

⚠️ The golden test `mode_body_matches_golden_snapshot` (`plan.rs:~198`) builds from the in-code `manifest()` **test helper** (`plan.rs:~143`) — **not** from `superpowers.toml`. So the byte-identical guarantee lives on that helper. In `plan.rs`'s `manifest()` helper, set the `ModeRecipe`'s `body_intro`/`body_outro` to the exact strings from `superpowers_body_golden.txt` (text before the first `- ` bullet → `body_intro`; text after the last bullet → `body_outro`, verbatim incl. newlines):

```rust
        mode: Some(ModeRecipe {
            loader: "using-superpowers/SKILL.md".into(),
            strip_prefix: "skills/".into(),
            body: BodyStrategy::FromSkillFrontmatter,
            description: "Superpowers — curated".into(),
            body_intro: Some("You are operating in \"Superpowers\" mode, imported from obra/superpowers.\n\nBefore any task, check if an available skill applies and invoke it with invoke_skill. The skills are:\n".into()),
            body_outro: Some("\nAlways check for an applicable skill before starting work. If multiple skills apply, invoke the most specific one first. After completing work, invoke verification-before-completion before claiming success.\n\nSkill work produces specs, plans, and debugging notes. Keep the running narration terse, and when the work is done do NOT reframe the whole effort in long paragraphs: close with a short recap of what changed and any next step.\n".into()),
        }),
```

> The generator emits `intro + "\n" + bullets + outro`. If the golden shows a one-newline diff, adjust the trailing `\n` of `body_intro` / leading `\n` of `body_outro` (never the generator) until byte-identical. Copy the exact bytes from the golden file rather than retyping.

- [ ] **Step 5: Also carry the strings in the bundled `superpowers.toml`, and guard the real product path**

Add the same `body_intro`/`body_outro` (TOML triple-quoted; note TOML trims one leading newline after `"""`) to the `[mode]` table of `manifests/superpowers.toml`, so the **bundled** install path (not just the test helper) reproduces the body. Then add a test in `bundled.rs` (or `plan.rs`) that proves it:

```rust
#[test]
fn bundled_superpowers_reproduces_the_golden_body() {
    let m = crate::bundled::bundled_manifest("superpowers").unwrap();
    let plan = crate::plan::build_plan(&m, &scan()).unwrap(); // reuse plan.rs scan() (make it pub(crate) if needed)
    let golden = include_str!("../tests/superpowers_body_golden.txt");
    assert_eq!(plan.mapping.mode_body, golden,
        "bundled superpowers.toml body drifted from the golden");
}
```

- [ ] **Step 6: Run the golden + new tests**

Run: `cargo test -p zoid-plugin`
Expected: PASS — `mode_body_matches_golden_snapshot` **and** `bundled_superpowers_reproduces_the_golden_body` both green (byte-identical), plus Task 2 Steps 1–2 tests.

- [ ] **Step 7: Commit**

```bash
git add crates/zoid-plugin/src/plan.rs crates/zoid-plugin/src/bundled.rs crates/zoid-plugin/manifests/superpowers.toml
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

## Task 4: Install a skills-kind plan into a per-pack private dir

**Files:**
- Modify: `crates/zoid/src/plugin_install.rs`
- Test: `crates/zoid/src/plugin_install.rs` (inline, tempdir)

**Interfaces:**
- Consumes: `InstallPlan` (skills kind), `zoid_core::wizard::UpstreamScan`, `crate::mode_wizard::materialize`.
- Produces: `finish_skills_install(plan: &InstallPlan, scan: &UpstreamScan, skills_root: &Path, plugin_id: &str, manifest_ref: &str, origin: &str) -> Result<InstalledPlugin, String>` — materializes each skill into the pack's **own private dir** `skills_root/<plugin_id>/` (so `materialize`'s file-set reconciliation stays scoped to this pack), writes no `mode.md`, and records `.zoid-plugin.json` inside that pack dir.

> **Why per-pack (C3):** `mode_wizard::materialize` writes one `.zoid-provenance.json` at its `dest_dir` and, on a subsequent call, **deletes every canonical path in the old sidecar not present in the new mapping**. If two packs shared `<cfg>/skills`, installing pack B would delete pack A's files. Each pack gets its own dir; Task 4b teaches the scanner to descend into them.

- [ ] **Step 1: Write the failing test (isolation is the key assertion)**

In the `tests` module of `plugin_install.rs`:

```rust
fn skills_manifest(id: &str) -> zoid_plugin::manifest::PluginManifest {
    use zoid_plugin::manifest::{PluginManifest, PluginSource};
    PluginManifest {
        id: id.into(), schema: 1, kind: vec!["skills".into()],
        name: id.into(), description: "d".into(),
        source: Some(PluginSource { repo: "o/r".into(), ref_: "SHA".into(), subtree: "skills".into() }),
        mode: None, install: vec![Effect::Activate],
    }
}

#[test]
fn skills_install_uses_private_pack_dir_no_mode_md() {
    let scan = scan(); // existing helper: brainstorming/SKILL.md etc.
    let plan = zoid_plugin::plan::build_plan(&skills_manifest("doctools"), &scan).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let skills_root = tmp.path().join("skills");
    let out = finish_skills_install(&plan, &scan, &skills_root, "doctools", "SHA", "url").unwrap();
    // Skill landed under the PRIVATE pack dir: <skills_root>/doctools/brainstorming/SKILL.md
    assert!(skills_root.join("doctools").join("brainstorming").join("SKILL.md").is_file());
    assert!(!skills_root.join("doctools").join("mode.md").exists());
    // Per-pack sidecar lives inside the pack dir.
    assert!(skills_root.join("doctools").join(".zoid-plugin.json").is_file());
    assert!(out.safe_effects.contains(&Effect::Activate));
}

#[test]
fn two_skills_packs_do_not_delete_each_other() {
    let scan = scan();
    let tmp = tempfile::tempdir().unwrap();
    let skills_root = tmp.path().join("skills");
    let plan_a = zoid_plugin::plan::build_plan(&skills_manifest("packA"), &scan).unwrap();
    finish_skills_install(&plan_a, &scan, &skills_root, "packA", "SHA", "url").unwrap();
    let plan_b = zoid_plugin::plan::build_plan(&skills_manifest("packB"), &scan).unwrap();
    finish_skills_install(&plan_b, &scan, &skills_root, "packB", "SHA", "url").unwrap();
    // Pack A survived installing Pack B (the C3 regression guard).
    assert!(skills_root.join("packA").join("brainstorming").join("SKILL.md").is_file());
    assert!(skills_root.join("packB").join("brainstorming").join("SKILL.md").is_file());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid skills_install_uses_private_pack_dir two_skills_packs_do_not_delete`
Expected: FAIL — `finish_skills_install` is undefined.

- [ ] **Step 3: Implement `finish_skills_install`**

`materialize` writes canonical entries relative to a destination root and drops a `.zoid-provenance.json` there, reconciling against any prior sidecar **at that same root**. So each pack gets its **own** root `skills_root/<plugin_id>/`; `brainstorming/SKILL.md` lands at `<skills_root>/<plugin_id>/brainstorming/SKILL.md`, and reconciliation only ever touches that one pack. Add:

```rust
/// Install a skills-kind plan into the pack's OWN dir under the convention
/// skills root. No `mode.md` overlay, no mode activation. The pack dir
/// `<skills_root>/<plugin_id>/` is discovered by the Task-4b scanner change
/// (which scans `<cfg>/skills/<pack>/<skill>/SKILL.md`). v1 writes no config
/// (SetConfig is gated off), so the on-disk convention IS the seam.
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
    // Per-pack private dir: scopes materialize's file-set reconciliation to
    // this pack alone (see C3), and mirrors how modes use <cfg>/modes/<id>/.
    let pack_dir = skills_root.join(plugin_id);
    if pack_dir.exists() {
        std::fs::remove_dir_all(&pack_dir)
            .map_err(|e| format!("remove old pack {}: {e}", pack_dir.display()))?;
    }
    std::fs::create_dir_all(&pack_dir)
        .map_err(|e| format!("create pack dir {}: {e}", pack_dir.display()))?;
    let fetched_at = chrono::Utc::now().to_rfc3339();
    materialize(&plan.mapping, scan, &pack_dir, &fetched_at).map_err(|e| e.problems.join("; "))?;

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
        // C2: PluginProvenance.files is Vec<ProvenanceEntry>. The per-file
        // list already lives in the pack dir's .zoid-provenance.json (written
        // by materialize); mirror finish_plugin_install and leave this empty
        // to avoid two sources of truth. Uninstall removes the whole pack_dir.
        files: Vec::new(),
        effects_applied: applied,
    };
    let json = serde_json::to_string_pretty(&sidecar).map_err(|e| format!("serialize sidecar: {e}"))?;
    std::fs::write(pack_dir.join(".zoid-plugin.json"), json)
        .map_err(|e| format!("write sidecar: {e}"))?;

    let safe_effects: Vec<Effect> = plan.effects.iter().filter(|e| e.risk() == RiskTier::Safe).cloned().collect();
    Ok(InstalledPlugin { dest: pack_dir, safe_effects })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p zoid skills_install_uses_private_pack_dir two_skills_packs_do_not_delete`
Expected: PASS — including the cross-pack regression guard.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/plugin_install.rs
git commit -m "feat(zoid): install skills-kind plans into per-pack private dirs"
```

---

## Task 4b: Scanner descends into per-pack skill dirs

**Files:**
- Modify: `crates/zoid/src/skill_import.rs`
- Test: `crates/zoid/src/skill_import.rs` (inline, tempdir)

**Interfaces:**
- Consumes: the on-disk layout `<skills_root>/<pack>/<skill>/SKILL.md` written by Task 4.
- Produces: `import_skills` (or a new `import_skills_recursive`) discovers skills one level deeper — both bare `<root>/<skill>/SKILL.md` (existing) **and** `<root>/<pack>/<skill>/SKILL.md` (new), so per-pack packs are found without a config write.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn imports_skills_from_per_pack_subdirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Bare skill (existing convention).
    let bare = root.join("bare-skill");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("SKILL.md"), "---\nname: bare-skill\ndescription: d\n---\nb\n").unwrap();
    // Per-pack skill: <root>/packA/nested/SKILL.md
    let nested = root.join("packA").join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("SKILL.md"), "---\nname: nested\ndescription: d\n---\nn\n").unwrap();
    // A pack sidecar dir must NOT be mistaken for a skill (no SKILL.md in it).
    let skills = import_skills(&[root.to_path_buf()]);
    let names: std::collections::HashSet<String> = skills.iter().map(|s| s.name.clone()).collect();
    assert!(names.contains("bare-skill"));
    assert!(names.contains("nested"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid imports_skills_from_per_pack_subdirs`
Expected: FAIL — `import_skills` only scans immediate `*/SKILL.md`, so `nested` is not found.

- [ ] **Step 3: Extend `import_skills` to descend one level when a child has no `SKILL.md`**

In the entry loop of `import_skills`, when an immediate child dir has **no** `SKILL.md` of its own, treat it as a pack dir and scan its immediate `*/SKILL.md` children too:

```rust
        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let md = skill_dir.join("SKILL.md");
            if md.is_file() {
                push_skill(&mut out, &skill_dir, &md);
                continue;
            }
            // No SKILL.md here → maybe a pack dir; scan one level deeper.
            if let Ok(inner) = std::fs::read_dir(&skill_dir) {
                for e2 in inner.flatten() {
                    let sub = e2.path();
                    let sub_md = sub.join("SKILL.md");
                    if sub.is_dir() && sub_md.is_file() {
                        push_skill(&mut out, &sub, &sub_md);
                    }
                }
            }
        }
```

Refactor the existing read-parse-push body into a small `fn push_skill(out: &mut Vec<Skill>, skill_dir: &Path, md: &Path)` (moves the current `read_to_string` + `parse_skill_md` + `canonicalize` + `out.push(Skill{..})` block verbatim) so both call sites share it. A pack's `.zoid-plugin.json` / `.zoid-provenance.json` are files (not dirs) and are naturally ignored.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p zoid import_ skills`
Expected: PASS — the existing `import_reads_valid_skills_and_skips_malformed` still green, plus the new nested test.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/skill_import.rs
git commit -m "feat(zoid): discover skills in per-pack subdirs of the skills root"
```

---

## Task 5: `:plugin install` gains `--mode` / `--skills` override flags

**Files:**
- Verify (likely no change): `crates/zoid-tui/src/command.rs` — `:plugin install <rest>` already stores `Command::PluginInstall(rest.trim())` (a raw `String`). Confirm it keeps the **whole** tail including flags (it does: `s["plugin install ".len()..].trim()`), so `--mode`/`--skills` ride along untouched. **Do not change the enum.**
- Create/modify: `crates/zoid/src/plugin_install.rs` — add pure `parse_plugin_install_args`.
- Modify: `crates/zoid/src/main.rs` — at the `Command::PluginInstall(arg)` dispatch (~line 5100 → `install_plugin`), split flags off `arg`, flip the resolved manifest's kind, and route to `finish_plugin_install` vs `finish_skills_install`.
- Test: `crates/zoid/src/plugin_install.rs` (inline, for the pure parser).

**Interfaces:**
- Produces: `parse_plugin_install_args(raw: &str) -> (String, KindOverride)` where `pub enum KindOverride { None, Mode, Skills }`. The dispatcher consumes it to flip `manifest.kind` before `build_plan` and to pick the finisher.

- [ ] **Step 1: Write the failing test (pure parser)**

In `plugin_install.rs` tests:

```rust
#[test]
fn parses_plugin_install_mode_and_skills_flags() {
    assert_eq!(parse_plugin_install_args("superpowers --mode"), ("superpowers".into(), KindOverride::Mode));
    assert_eq!(parse_plugin_install_args("anthropics/skills --skills"), ("anthropics/skills".into(), KindOverride::Skills));
    assert_eq!(parse_plugin_install_args("superpowers"), ("superpowers".into(), KindOverride::None));
    // Last flag wins; ref is the sole non-flag token.
    assert_eq!(parse_plugin_install_args("x --skills --mode"), ("x".into(), KindOverride::Mode));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p zoid parses_plugin_install_mode_and_skills`
Expected: FAIL — `KindOverride` / `parse_plugin_install_args` undefined.

- [ ] **Step 3: Implement the pure parser**

Keeping the enum untouched (C4) means the flag split happens here, off the raw string:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindOverride { None, Mode, Skills }

/// Split a raw `:plugin install` argument string into (plugin_ref, override).
/// Flags may appear on either side of the ref; the last of --mode/--skills
/// wins (permissive, matching zoid's flag stance). The first non-flag token is
/// the ref; later bare tokens are ignored (the ref is a single id/url).
pub fn parse_plugin_install_args(raw: &str) -> (String, KindOverride) {
    let mut plugin_ref = String::new();
    let mut over = KindOverride::None;
    for tok in raw.split_whitespace() {
        match tok {
            "--mode" => over = KindOverride::Mode,
            "--skills" => over = KindOverride::Skills,
            other if other.starts_with("--") => {} // unknown flag: ignore
            other if plugin_ref.is_empty() => plugin_ref = other.to_string(),
            _ => {}
        }
    }
    (plugin_ref, over)
}
```

Then in `main.rs`'s `install_plugin` (reached from the `Command::PluginInstall(arg)` arm ~line 5100): call `let (plugin_ref, over) = parse_plugin_install_args(&arg);` and use `plugin_ref` where the code currently uses the whole `arg`. After the manifest is resolved and before `build_plan`, apply the override: `KindOverride::Mode` → `manifest.kind = vec!["mode".into()]` (if it had no `[mode]` table, `build_plan`'s generic default body covers it); `KindOverride::Skills` → `manifest.kind = vec!["skills".into()]`, `manifest.mode = None`. Then branch: skills-kind → `finish_skills_install(&plan, &scan, &skills_root, id, ref_, origin)` with `skills_root = <cfg>/skills`; else the existing `finish_plugin_install` with `dest = <cfg>/modes/<id>`.

- [ ] **Step 4: Run test to verify it passes; confirm no enum ripple**

Run: `cargo test -p zoid parses_plugin_install_mode && cargo build -p zoid-tui -p zoid`
Expected: PASS + clean build (command.rs/palette.rs/render.rs untouched — the enum still carries one `String`).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/plugin_install.rs crates/zoid/src/main.rs
git commit -m "feat(zoid): --mode/--skills override at the :plugin install dispatch"
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
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
reqwest = { workspace = true }
tokio = { workspace = true }
```

> N1: `zoid-core` and `toml` are intentionally omitted — the converter formats TOML as strings and re-validates via `zoid_plugin::parse_manifest` (which owns the `toml` dep). Add `zoid-core` only if a later task needs `UpstreamScan` directly.

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
fn loader_match_is_anchored_not_substring() {
    // S2: `reusing-context` contains "using-" but is NOT a loader.
    let t = tree(&["skills/reusing-context/SKILL.md", "skills/foo/SKILL.md"]);
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

/// A loader/index skill name. Tightened (S2): anchored matches only — a bare
/// `contains("using-")` would misclassify `reusing-context`, `focusing-…`, etc.
fn is_loader_name(name: &str) -> bool {
    name.starts_with("using-") || name == "find-skills" || name.ends_with("-overview")
}

fn find_loader(files: &[String]) -> Option<String> {
    // A loader is a skills/<name>/SKILL.md whose <name> is a loader name.
    for f in files {
        let Some(rel) = f.strip_prefix("skills/") else { continue };
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() != 2 || segs[1] != "SKILL.md" { continue; }
        if is_loader_name(segs[0]) {
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
            // N2: an empty subtree must not yield strip_prefix = "/".
            let strip = if subtree.is_empty() { String::new() } else { format!("{subtree}/") };
            Some(format!(
                "[plugin]\nid = \"{id}\"\nschema = 1\nkind = [\"mode\"]\nname = \"{name}\"\ndescription = \"{desc}\"\n\n\
                 [source]\nrepo = \"{repo}\"\nref = \"{sha}\"\nsubtree = \"{subtree}\"\n\n\
                 [mode]\nloader = \"{loader_rel}\"\nstrip_prefix = \"{strip}\"\nbody = \"from-skill-frontmatter\"\ndescription = \"{desc}\"\n\n\
                 [[install]]\neffect = \"activate\"\n",
                id = slug(name), name = name, desc = description.replace('"', "'"),
                repo = repo, sha = sha, subtree = subtree, loader_rel = loader_rel, strip = strip,
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

## Self-Review Notes (author) — incl. gilfoyle review resolution (2026-07-13)

- **Spec §3.1a (body generalization):** Tasks 1–2. **C1 fixed:** the golden builds from the in-code `manifest()` helper, so Task 2 Step 4 sets intro/outro there (not on `superpowers.toml`); Step 5 adds a bundled-repro guard so the real product path is also proven byte-identical.
- **Spec §3.1b (skills kind):** Task 3.
- **Spec §3.3 (install side + `--mode`/`--skills`):** Tasks 4, 4b, 5.
  - **C3 fixed (design):** `mode_wizard::materialize` reconciles/deletes against a prior sidecar at its `dest_dir`, so a **shared** `<cfg>/skills` dir would make packs delete each other. Each pack now installs into its **own** dir `<cfg>/skills/<plugin_id>/` (Task 4), and Task 4b teaches the scanner to descend one level to discover them. This preserves the v1 no-`set_config` constraint. Supersedes the spec's "register via `skills.source_dirs`" wording — a spec note has been added.
  - **C2 fixed:** `PluginProvenance.files` is `Vec<ProvenanceEntry>`; Task 4 uses `files: Vec::new()` (per-file list lives in the pack dir's `.zoid-provenance.json`).
  - **C4 fixed:** `:plugin install` is parsed in `zoid-tui/src/command.rs` into `Command::PluginInstall(String)`. Task 5 keeps that enum unchanged (no palette/render/command-test ripple) and splits `--mode`/`--skills` at the `main.rs` dispatch via the pure `parse_plugin_install_args`.
  - **S1 fixed:** Task 1 also patches the `ModeRecipe` literal in `plugin_install.rs` (~139).
- **Spec §3.2 + §4 (converter + classification):** Tasks 6–11. **S2 fixed:** loader match is anchored (`starts_with("using-") || == "find-skills" || ends_with("-overview")`) with a negative test.
- **Spec §5 (testing):** fixtures + `roundtrip.rs` in Task 11; workspace gate in Task 12 (Step 3 asserts the golden file is byte-unmodified).
- **S3 (scope clarity):** the converter's emitted `.mcp.json` is a **Spec 2 catalog artifact** — zoid discovers MCP only from `<user>/mcp.json` and `<cwd>/.mcp.json` (`zoid-mcp::discover`), never from a plugin dir. Spec 1 does not install MCP; it only produces the normalized snippet for the catalog to host.
- **Type consistency:** `PluginTree`, `KindPref`, `TargetKind`, `Classification`, `Emitted` used identically across Tasks 8–11. `KindOverride` (install, Task 5) is intentionally distinct from `KindPref` (converter, Task 8) — different layers.
- **Known limitation (deferred to Spec 3):** per-pack dirs prevent cross-pack file deletion, but two packs can still register skills with the same `name`; the registry's first-wins dedup bounds this for v1's small wholesale packs.
