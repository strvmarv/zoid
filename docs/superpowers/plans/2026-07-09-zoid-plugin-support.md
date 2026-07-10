# zoid Plugin Support (v1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace zoid's hardcoded `superpowers_install.rs` recipe with a generic, manifest-driven plugin installer (`:plugin install <id|url>`), with Superpowers demoted to a bundled `.zoid/plugin.toml` manifest.

**Architecture:** A new pure `zoid-plugin` crate holds the manifest schema, effect model + risk classification, source-resolution decision, and the plan builder (manifest + scan → `ModeMapping` + effects). The `zoid` bin gains `plugin_install.rs`, which reuses the existing `github_fetch` + `mode_wizard::materialize` machinery to fetch, materialize, and record provenance. The async fetch→apply orchestration mirrors today's `SuperpowersScan` path exactly.

**Tech Stack:** Rust 2021, `serde`, `toml`, `serde_json`, `tokio`, `tempfile` (dev), the existing `zoid-core::wizard` value types.

## Global Constraints

- Workspace edition: **2021**. New crate `zoid-plugin` is a workspace member.
- `zoid-plugin` is **pure**: no filesystem or network access (mirrors `zoid-core`). Only `serde`, `toml`, `serde_json`, and `zoid-core`.
- Provenance/manifest JSON must contain **no absolute host paths** (canonical paths are subtree-relative), matching the existing `.zoid-provenance.json` invariant.
- Superpowers pinned ref stays **`d884ae04edebef577e82ff7c4e143debd0bbec99`** — it moves from a Rust `const` into `manifests/superpowers.toml` `[source].ref`, unchanged.
- Do NOT add a `Co-Authored-By` trailer to commits (repo convention).
- TDD: every task writes the failing test first, watches it fail, then implements. Commit at the end of each task.

## Scope (v1) and explicit deferrals

**In scope:** `:plugin install <id|url>`, `:mode install superpowers` retargeted as an alias, the bundled Superpowers manifest, the generic installer, the `.zoid-plugin.json` provenance sidecar (written), and typed seams for future artifact kinds + effects. Deletion of the bespoke recipe.

**Deferred to a follow-up plan (seamed, not implemented here):** `:plugin uninstall`, `:plugin update`, `:plugin list` overlay, the interactive approval prompt for Dangerous effects, and live application of `Effect::SetConfig`. In v1 the installer applies only **Safe** effects (`Activate`, `OnboardingHint`); any Dangerous or unknown effect is rejected at plan validation with a clear message. The risk-classification code is built and unit-tested so the follow-up plan only has to add the prompt + config writer.

## File Structure

- Create: `crates/zoid-plugin/Cargo.toml` — new pure crate manifest.
- Create: `crates/zoid-plugin/src/lib.rs` — module wiring + re-exports.
- Create: `crates/zoid-plugin/src/effect.rs` — `Effect`, `RiskTier`, `classify_config_key`.
- Create: `crates/zoid-plugin/src/manifest.rs` — `PluginManifest` + parse + validate.
- Create: `crates/zoid-plugin/src/resolve.rs` — `ManifestSource` + `resolve_source`.
- Create: `crates/zoid-plugin/src/plan.rs` — `InstallPlan` + `build_plan` + body-strategy port.
- Create: `crates/zoid-plugin/src/provenance.rs` — `PluginProvenance` serde types.
- Create: `crates/zoid-plugin/src/bundled.rs` — `bundled_manifest(id)`.
- Create: `crates/zoid-plugin/manifests/superpowers.toml` — bundled manifest.
- Create: `crates/zoid/src/plugin_install.rs` — effectful installer.
- Modify: `Cargo.toml` (root) — add `crates/zoid-plugin` to `members`.
- Modify: `crates/zoid/Cargo.toml` — add `zoid-plugin` dependency.
- Modify: `crates/zoid/src/lib.rs` — declare `pub mod plugin_install;`.
- Modify: `crates/zoid-tui/src/command.rs` — add `Command::PluginInstall`, retarget parser.
- Modify: `crates/zoid-tui/src/palette.rs:205` — palette row wording.
- Modify: `crates/zoid-tui/src/onboarding.rs:26` — onboarding line wording.
- Modify: `crates/zoid/src/agent.rs` (near `AgentUpdate` ~line 213) — add `PluginScan` variant.
- Modify: `crates/zoid/src/main.rs` — kickoff/apply/dispatch; delete old superpowers path.
- Delete (recipe): `crates/zoid/src/superpowers_install.rs` mapping logic (Task 11).

---

### Task 1: Scaffold `zoid-plugin` crate + effect model

**Files:**
- Create: `crates/zoid-plugin/Cargo.toml`
- Create: `crates/zoid-plugin/src/lib.rs`
- Create: `crates/zoid-plugin/src/effect.rs`
- Modify: `Cargo.toml` (root workspace `members`)

**Interfaces:**
- Produces: `pub enum Effect { Activate, OnboardingHint { text: String }, SetConfig { key: String, value: toml::Value } }`; `pub enum RiskTier { Safe, Dangerous }`; `Effect::risk(&self) -> RiskTier`; `pub fn classify_config_key(key: &str) -> RiskTier`.

- [ ] **Step 1: Add the crate to the workspace**

In root `Cargo.toml`, add `"crates/zoid-plugin"` to the `[workspace] members` array (keep alphabetical-ish with the other `crates/*` entries).

- [ ] **Step 2: Write `crates/zoid-plugin/Cargo.toml`**

```toml
[package]
name = "zoid-plugin"
version = "0.3.1"
edition = "2021"

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
toml = { workspace = true }
zoid-core = { path = "../zoid-core" }

[dev-dependencies]
```

If `toml`/`serde`/`serde_json` are not yet workspace deps, use the same version spec the sibling crates use (check `crates/zoid-core/Cargo.toml`); replace `{ workspace = true }` with the concrete version if the workspace doesn't define them.

- [ ] **Step 3: Write the failing test in `effect.rs`**

```rust
//! Install-time effects a plugin manifest may declare, and their risk tier.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_and_hint_are_safe() {
        assert_eq!(Effect::Activate.risk(), RiskTier::Safe);
        assert_eq!(
            Effect::OnboardingHint { text: "hi".into() }.risk(),
            RiskTier::Safe
        );
    }

    #[test]
    fn known_config_keys_are_safe_everything_else_dangerous() {
        assert_eq!(classify_config_key("skills.source_dirs"), RiskTier::Safe);
        assert_eq!(classify_config_key("modes.source_dirs"), RiskTier::Safe);
        // Fail-closed: anything not on the allowlist is Dangerous.
        assert_eq!(classify_config_key("provider"), RiskTier::Dangerous);
        assert_eq!(classify_config_key("base_url"), RiskTier::Dangerous);
        assert_eq!(classify_config_key("approval.mode"), RiskTier::Dangerous);
    }

    #[test]
    fn set_config_risk_follows_key_classification() {
        let safe = Effect::SetConfig {
            key: "skills.source_dirs".into(),
            value: toml::Value::String("x".into()),
        };
        let dangerous = Effect::SetConfig {
            key: "provider".into(),
            value: toml::Value::String("x".into()),
        };
        assert_eq!(safe.risk(), RiskTier::Safe);
        assert_eq!(dangerous.risk(), RiskTier::Dangerous);
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p zoid-plugin effect`
Expected: FAIL — `Effect`, `RiskTier`, `classify_config_key` not defined.

- [ ] **Step 5: Implement the effect model in `effect.rs`**

```rust
/// One install-time effect a plugin manifest may declare in `[[install]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Make the freshly-installed mode active.
    Activate,
    /// Emit an onboarding/status line after install.
    OnboardingHint { text: String },
    /// Write a config.toml key. Applying this is deferred to a follow-up plan;
    /// v1 rejects it at plan validation (it classifies as needing confirmation).
    SetConfig { key: String, value: toml::Value },
}

/// Whether an effect may apply silently (`Safe`) or needs explicit confirmation
/// (`Dangerous`). Classification lives with the effect so new effects declare
/// their own tier rather than every call-site re-deciding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    Safe,
    Dangerous,
}

impl Effect {
    pub fn risk(&self) -> RiskTier {
        match self {
            Effect::Activate | Effect::OnboardingHint { .. } => RiskTier::Safe,
            Effect::SetConfig { key, .. } => classify_config_key(key),
        }
    }
}

/// Fail-closed config-key classifier: only an allowlist of known-safe keys is
/// `Safe`; everything else (provider, base_url, approval, secrets-adjacent) is
/// `Dangerous`.
pub fn classify_config_key(key: &str) -> RiskTier {
    const SAFE_KEYS: &[&str] = &["skills.source_dirs", "modes.source_dirs"];
    if SAFE_KEYS.contains(&key) {
        RiskTier::Safe
    } else {
        RiskTier::Dangerous
    }
}
```

- [ ] **Step 6: Write `crates/zoid-plugin/src/lib.rs`**

```rust
//! Pure, IO-free plugin schema + planning for zoid (spec:
//! docs/superpowers/specs/2026-07-09-zoid-plugin-support-design.md).

pub mod effect;
pub mod manifest;
pub mod plan;
pub mod provenance;
pub mod resolve;
pub mod bundled;

pub use effect::{classify_config_key, Effect, RiskTier};
```

Comment out the not-yet-created modules (`manifest`, `plan`, `provenance`, `resolve`, `bundled`) for now so the crate compiles; each later task uncomments its module.

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p zoid-plugin effect`
Expected: PASS (3 tests).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/zoid-plugin
git commit -m "feat(plugin): scaffold zoid-plugin crate + effect risk model"
```

---

### Task 2: Manifest schema + parse + validate

**Files:**
- Create/Modify: `crates/zoid-plugin/src/manifest.rs`
- Modify: `crates/zoid-plugin/src/lib.rs` (uncomment `pub mod manifest;`)

**Interfaces:**
- Consumes: `Effect` (Task 1).
- Produces:
  - `pub struct PluginManifest { pub id: String, pub schema: u32, pub kind: Vec<String>, pub name: String, pub description: String, pub source: Option<PluginSource>, pub mode: Option<ModeRecipe>, pub install: Vec<Effect> }`
  - `pub struct PluginSource { pub repo: String, pub ref_: String, pub subtree: String }`
  - `pub struct ModeRecipe { pub loader: String, pub strip_prefix: String, pub body: BodyStrategy, pub description: String }`
  - `pub enum BodyStrategy { FromSkillFrontmatter }`
  - `pub fn parse_manifest(toml_src: &str) -> Result<PluginManifest, String>`
  - `PluginManifest::validate(&self) -> Result<(), String>`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[plugin]
id = "superpowers"
schema = 1
kind = ["mode"]
name = "Superpowers"
description = "Skill-driven workflows"

[source]
repo = "obra/superpowers"
ref = "d884ae04"
subtree = "skills"

[mode]
loader = "using-superpowers/SKILL.md"
strip_prefix = "skills/"
body = "from-skill-frontmatter"
description = "Superpowers — curated skills"

[[install]]
effect = "activate"

[[install]]
effect = "onboarding_hint"
text = "Superpowers installed."
"#;

    #[test]
    fn parses_a_good_manifest() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!(m.id, "superpowers");
        assert_eq!(m.kind, vec!["mode".to_string()]);
        assert_eq!(m.source.as_ref().unwrap().ref_, "d884ae04");
        let mode = m.mode.as_ref().unwrap();
        assert_eq!(mode.loader, "using-superpowers/SKILL.md");
        assert_eq!(mode.strip_prefix, "skills/");
        assert!(matches!(mode.body, BodyStrategy::FromSkillFrontmatter));
        assert_eq!(m.install.len(), 2);
        assert_eq!(m.install[0], Effect::Activate);
        assert_eq!(
            m.install[1],
            Effect::OnboardingHint { text: "Superpowers installed.".into() }
        );
        m.validate().unwrap();
    }

    #[test]
    fn rejects_unknown_kind() {
        let src = GOOD.replace(r#"kind = ["mode"]"#, r#"kind = ["mode", "wormhole"]"#);
        let m = parse_manifest(&src).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.contains("wormhole"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_effect() {
        let src = GOOD.replace(r#"effect = "activate""#, r#"effect = "rm_rf""#);
        let err = parse_manifest(&src).unwrap_err();
        assert!(err.contains("rm_rf") || err.contains("effect"), "got: {err}");
    }

    #[test]
    fn mode_kind_requires_a_mode_table() {
        let src = GOOD.replace("[mode]", "[unused]");
        let m = parse_manifest(&src).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.contains("mode"), "got: {err}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin manifest`
Expected: FAIL — module/types undefined.

- [ ] **Step 3: Implement `manifest.rs`**

```rust
//! The `.zoid/plugin.toml` manifest schema (schema = 1) + parse + validate.

use serde::Deserialize;

use crate::effect::Effect;

#[derive(Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub schema: u32,
    pub kind: Vec<String>,
    pub name: String,
    pub description: String,
    pub source: Option<PluginSource>,
    pub mode: Option<ModeRecipe>,
    pub install: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginSource {
    pub repo: String,
    pub ref_: String,
    pub subtree: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeRecipe {
    pub loader: String,
    pub strip_prefix: String,
    pub body: BodyStrategy,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodyStrategy {
    FromSkillFrontmatter,
}

// --- Raw serde shapes (mirror the TOML layout), converted into the public
// types above so the public API isn't coupled to serde field naming. ---

#[derive(Deserialize)]
struct RawManifest {
    plugin: RawPlugin,
    source: Option<RawSource>,
    mode: Option<RawMode>,
    #[serde(default)]
    install: Vec<RawEffect>,
}

#[derive(Deserialize)]
struct RawPlugin {
    id: String,
    schema: u32,
    kind: Vec<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct RawSource {
    repo: String,
    #[serde(rename = "ref")]
    ref_: String,
    subtree: String,
}

#[derive(Deserialize)]
struct RawMode {
    loader: String,
    #[serde(default)]
    strip_prefix: String,
    body: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct RawEffect {
    effect: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<toml::Value>,
}

/// Parse a manifest from TOML source. Unknown *keys* are ignored by serde
/// (forward-compat, mirroring config.toml's warn-not-reject stance); unknown
/// *effect names* are a hard error (an unrecognized effect must never be
/// silently dropped).
pub fn parse_manifest(toml_src: &str) -> Result<PluginManifest, String> {
    let raw: RawManifest =
        toml::from_str(toml_src).map_err(|e| format!("plugin.toml parse error: {e}"))?;

    let body = match raw.mode.as_ref().map(|m| m.body.as_str()) {
        None => None,
        Some("from-skill-frontmatter") => Some(BodyStrategy::FromSkillFrontmatter),
        Some(other) => return Err(format!("unknown mode body strategy '{other}'")),
    };

    let mut install = Vec::new();
    for e in raw.install {
        let effect = match e.effect.as_str() {
            "activate" => Effect::Activate,
            "onboarding_hint" => Effect::OnboardingHint {
                text: e.text.unwrap_or_default(),
            },
            "set_config" => Effect::SetConfig {
                key: e
                    .key
                    .ok_or_else(|| "set_config effect missing 'key'".to_string())?,
                value: e
                    .value
                    .ok_or_else(|| "set_config effect missing 'value'".to_string())?,
            },
            other => return Err(format!("unknown install effect '{other}'")),
        };
        install.push(effect);
    }

    Ok(PluginManifest {
        id: raw.plugin.id,
        schema: raw.plugin.schema,
        kind: raw.plugin.kind,
        name: raw.plugin.name,
        description: raw.plugin.description,
        source: raw.source.map(|s| PluginSource {
            repo: s.repo,
            ref_: s.ref_,
            subtree: s.subtree,
        }),
        mode: raw.mode.map(|m| ModeRecipe {
            loader: m.loader,
            strip_prefix: m.strip_prefix,
            body: body.expect("body set when mode present"),
            description: m.description,
        }),
        install,
    })
}

impl PluginManifest {
    /// Validate that this manifest is installable by *this* zoid version.
    /// Unknown artifact kinds and a `mode` kind without a `[mode]` table are
    /// rejected here (typed seams: future kinds fail cleanly, never silently).
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!(
                "plugin '{}' declares schema {} (this zoid supports schema 1)",
                self.id, self.schema
            ));
        }
        for k in &self.kind {
            if k != "mode" {
                return Err(format!(
                    "plugin '{}' declares unsupported kind '{}' (v1 supports only 'mode')",
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
        Ok(())
    }
}
```

- [ ] **Step 4: Uncomment `pub mod manifest;` in `lib.rs` and run the test**

Run: `cargo test -p zoid-plugin manifest`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/manifest.rs crates/zoid-plugin/src/lib.rs
git commit -m "feat(plugin): manifest schema, TOML parse, and validate"
```

---

### Task 3: Source resolution decision

**Files:**
- Create/Modify: `crates/zoid-plugin/src/resolve.rs`
- Modify: `crates/zoid-plugin/src/lib.rs` (uncomment `pub mod resolve;`)

**Interfaces:**
- Produces:
  - `pub enum PluginRef { Id(String), Url(String) }`
  - `pub enum ManifestSource { Bundled, Repo, WizardFallback }`
  - `pub fn classify_ref(arg: &str) -> PluginRef`
  - `pub fn resolve_source(r: &PluginRef, bundled_ids: &[&str], repo_has_manifest: bool, bundled_for_url: bool) -> ManifestSource`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ref_distinguishes_url_from_id() {
        assert_eq!(classify_ref("superpowers"), PluginRef::Id("superpowers".into()));
        assert_eq!(
            classify_ref("github.com/obra/superpowers/tree/main/skills"),
            PluginRef::Url("github.com/obra/superpowers/tree/main/skills".into())
        );
        assert!(matches!(
            classify_ref("https://github.com/o/r/tree/main/x"),
            PluginRef::Url(_)
        ));
    }

    #[test]
    fn id_resolves_to_bundled_when_known() {
        let r = PluginRef::Id("superpowers".into());
        assert_eq!(
            resolve_source(&r, &["superpowers"], false, false),
            ManifestSource::Bundled
        );
    }

    #[test]
    fn unknown_id_has_no_source() {
        // An unknown bare id can't be a URL and isn't bundled → wizard fallback
        // is meaningless without a URL; caller treats WizardFallback for an Id as
        // an error. resolve_source still returns WizardFallback; caller decides.
        let r = PluginRef::Id("nope".into());
        assert_eq!(
            resolve_source(&r, &["superpowers"], false, false),
            ManifestSource::WizardFallback
        );
    }

    #[test]
    fn url_prefers_repo_manifest_then_bundled_then_wizard() {
        let r = PluginRef::Url("github.com/o/r/tree/main/skills".into());
        assert_eq!(resolve_source(&r, &[], true, false), ManifestSource::Repo);
        assert_eq!(resolve_source(&r, &[], false, true), ManifestSource::Bundled);
        assert_eq!(
            resolve_source(&r, &[], false, false),
            ManifestSource::WizardFallback
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin resolve`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement `resolve.rs`**

```rust
//! Pure source-resolution decision for `:plugin install <arg>`. The bin performs
//! the actual fetch and passes the observed facts (does the repo carry a
//! manifest? is there a bundled manifest for this URL?) into `resolve_source`,
//! keeping this module IO-free and table-testable.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRef {
    Id(String),
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSource {
    Bundled,
    Repo,
    WizardFallback,
}

/// A bare token is an id; anything that looks like a github URL is a Url.
pub fn classify_ref(arg: &str) -> PluginRef {
    let a = arg.trim();
    let looks_url = a.starts_with("github.com/")
        || a.starts_with("http://github.com/")
        || a.starts_with("https://github.com/");
    if looks_url {
        PluginRef::Url(a.to_string())
    } else {
        PluginRef::Id(a.to_string())
    }
}

/// Decide which manifest source to use. For an `Id`: bundled if known, else
/// `WizardFallback` (caller reports "unknown plugin"). For a `Url`: repo
/// manifest wins, then a bundled manifest keyed to that URL, then the
/// model-driven wizard.
pub fn resolve_source(
    r: &PluginRef,
    bundled_ids: &[&str],
    repo_has_manifest: bool,
    bundled_for_url: bool,
) -> ManifestSource {
    match r {
        PluginRef::Id(id) => {
            if bundled_ids.contains(&id.as_str()) {
                ManifestSource::Bundled
            } else {
                ManifestSource::WizardFallback
            }
        }
        PluginRef::Url(_) => {
            if repo_has_manifest {
                ManifestSource::Repo
            } else if bundled_for_url {
                ManifestSource::Bundled
            } else {
                ManifestSource::WizardFallback
            }
        }
    }
}
```

- [ ] **Step 4: Uncomment `pub mod resolve;` and run**

Run: `cargo test -p zoid-plugin resolve`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/resolve.rs crates/zoid-plugin/src/lib.rs
git commit -m "feat(plugin): pure source-resolution decision"
```

---

### Task 4: Plan builder (manifest + scan → ModeMapping + effects)

This is the crux: the generic re-implementation of `superpowers_mapping` + `generate_mode_body`, driven by manifest fields. The body generator is **ported verbatim** from `superpowers_install.rs::generate_mode_body` so Task 5's byte-identical regression test passes.

**Files:**
- Create/Modify: `crates/zoid-plugin/src/plan.rs`
- Modify: `crates/zoid-plugin/src/lib.rs` (uncomment `pub mod plan;`)

**Interfaces:**
- Consumes: `PluginManifest`, `ModeRecipe`, `BodyStrategy` (Task 2); `Effect` (Task 1); `zoid_core::wizard::{UpstreamScan, ModeMapping, MappingEntry}`; `zoid_core::skill::parse_skill_md`.
- Produces:
  - `pub struct InstallPlan { pub mapping: ModeMapping, pub effects: Vec<Effect> }`
  - `pub fn build_plan(manifest: &PluginManifest, scan: &UpstreamScan) -> Result<InstallPlan, String>`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::Effect;
    use crate::manifest::{BodyStrategy, ModeRecipe, PluginManifest};
    use zoid_core::wizard::{MappingEntry, ScannedFile, UpstreamScan};

    fn skill_md(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody for {name}\n")
    }

    fn scan() -> UpstreamScan {
        UpstreamScan {
            url: "u".into(),
            repo: "obra/superpowers".into(),
            resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile { upstream_path: "skills/using-superpowers/SKILL.md".into(), sha: "a".into(), content: skill_md("using-superpowers", "loader") },
                ScannedFile { upstream_path: "skills/brainstorming/SKILL.md".into(), sha: "c".into(), content: skill_md("brainstorming", "Use before creative work") },
                ScannedFile { upstream_path: "skills/brainstorming/visual-companion.md".into(), sha: "d".into(), content: "vc".into() },
            ],
        }
    }

    fn manifest() -> PluginManifest {
        PluginManifest {
            id: "superpowers".into(),
            schema: 1,
            kind: vec!["mode".into()],
            name: "Superpowers".into(),
            description: "disp".into(),
            source: None,
            mode: Some(ModeRecipe {
                loader: "using-superpowers/SKILL.md".into(),
                strip_prefix: "skills/".into(),
                body: BodyStrategy::FromSkillFrontmatter,
                description: "Superpowers — curated".into(),
            }),
            install: vec![Effect::Activate],
        }
    }

    #[test]
    fn build_plan_maps_loader_to_mode_md_and_strips_prefix() {
        let plan = build_plan(&manifest(), &scan()).unwrap();
        assert_eq!(plan.mapping.mode_name, "Superpowers");
        assert_eq!(plan.mapping.mode_description, "Superpowers — curated");
        let pairs: Vec<(&str, &str)> = plan.mapping.materialize_entries();
        assert!(pairs.contains(&("mode.md", "skills/using-superpowers/SKILL.md")));
        assert!(pairs.contains(&("brainstorming/SKILL.md", "skills/brainstorming/SKILL.md")));
        assert!(pairs.contains(&("brainstorming/visual-companion.md", "skills/brainstorming/visual-companion.md")));
        // loader is NOT emitted as its own canonical file.
        assert!(!pairs.iter().any(|(c, _)| *c == "using-superpowers/SKILL.md"));
        assert_eq!(plan.effects, vec![Effect::Activate]);
    }

    #[test]
    fn build_plan_body_lists_skills_alphabetically_excluding_loader() {
        let plan = build_plan(&manifest(), &scan()).unwrap();
        assert!(plan.mapping.mode_body.contains("- brainstorming: Use before creative work"));
        assert!(!plan.mapping.mode_body.contains("- using-superpowers:"));
        assert!(plan.mapping.mode_body.contains("verification-before-completion before claiming success"));
    }

    #[test]
    fn build_plan_errors_when_loader_absent() {
        let mut s = scan();
        s.files.retain(|f| f.upstream_path != "skills/using-superpowers/SKILL.md");
        assert!(build_plan(&manifest(), &s).is_err());
    }

    #[test]
    fn build_plan_errors_when_no_mode_recipe() {
        let mut m = manifest();
        m.mode = None;
        assert!(build_plan(&m, &scan()).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin plan`
Expected: FAIL — `build_plan`/`InstallPlan` undefined.

- [ ] **Step 3: Implement `plan.rs`**

The mode.md summary string is `format!("{} mode overlay (generated)", mode_name)`, which for `mode_name = "Superpowers"` yields exactly `"Superpowers mode overlay (generated)"` — the same literal the old `superpowers_mapping` used. Non-loader entries get an empty summary, matching the old recipe. The full loader path is `{subtree}/{loader}` reconstructed from the scan's `subtree_path`.

```rust
//! Build an install plan (a ModeMapping + ordered effects) from a manifest and
//! a fetched upstream scan. This is the generic form of the old bespoke
//! superpowers recipe; the body generator is ported verbatim so output is
//! byte-identical for the superpowers case.

use crate::effect::Effect;
use crate::manifest::{BodyStrategy, PluginManifest};
use zoid_core::skill::parse_skill_md;
use zoid_core::wizard::{MappingEntry, ModeMapping, UpstreamScan};

pub struct InstallPlan {
    pub mapping: ModeMapping,
    pub effects: Vec<Effect>,
}

pub fn build_plan(manifest: &PluginManifest, scan: &UpstreamScan) -> Result<InstallPlan, String> {
    let mode = manifest
        .mode
        .as_ref()
        .ok_or_else(|| format!("plugin '{}' has no [mode] recipe", manifest.id))?;

    // Full upstream path of the loader = {subtree}/{loader}. The scan's paths
    // include the subtree prefix, so reconstruct it to match.
    let loader_full = if scan.subtree_path.is_empty() {
        mode.loader.clone()
    } else {
        format!("{}/{}", scan.subtree_path, mode.loader)
    };
    if !scan.files.iter().any(|f| f.upstream_path == loader_full) {
        return Err(format!("upstream is missing loader {loader_full}"));
    }

    let mode_name = manifest.name.clone();
    let mut entries = vec![MappingEntry::Materialize {
        canonical_path: "mode.md".to_string(),
        source: loader_full.clone(),
        summary: format!("{mode_name} mode overlay (generated)"),
    }];
    for f in &scan.files {
        if f.upstream_path == loader_full {
            continue; // consumed as mode.md
        }
        let canonical = match f.upstream_path.strip_prefix(mode.strip_prefix.as_str()) {
            Some(c) => c.to_string(),
            None => continue, // outside the stripped subtree; skip defensively
        };
        entries.push(MappingEntry::Materialize {
            canonical_path: canonical,
            source: f.upstream_path.clone(),
            summary: String::new(),
        });
    }

    let mode_body = match mode.body {
        BodyStrategy::FromSkillFrontmatter => {
            generate_body_from_frontmatter(scan, &loader_full, &mode.strip_prefix)
        }
    };

    Ok(InstallPlan {
        mapping: ModeMapping {
            mode_name,
            mode_description: mode.description.clone(),
            mode_body,
            entries,
        },
        effects: manifest.install.clone(),
    })
}

/// Ported verbatim (behavior-preserving) from
/// `superpowers_install.rs::generate_mode_body`: the skill bullet list is the
/// name+description frontmatter of each top-level `<skill>/SKILL.md` under the
/// stripped subtree (loader excluded), alphabetical by name.
fn generate_body_from_frontmatter(scan: &UpstreamScan, loader_full: &str, strip_prefix: &str) -> String {
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
            continue; // only a skill's top-level SKILL.md
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

Note: the body preamble text is hardcoded to `"Superpowers"` in the original, so it is preserved verbatim here for byte-identical output. Generalizing that preamble to arbitrary plugins is a follow-up concern (tracked in the deferred section), not a v1 requirement.

- [ ] **Step 4: Uncomment `pub mod plan;` and run**

Run: `cargo test -p zoid-plugin plan`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/plan.rs crates/zoid-plugin/src/lib.rs
git commit -m "feat(plugin): generic plan builder (mapping + effects) from manifest"
```

---

### Task 5: Bundled superpowers manifest + byte-identical regression guard

**Files:**
- Create: `crates/zoid-plugin/manifests/superpowers.toml`
- Create/Modify: `crates/zoid-plugin/src/bundled.rs`
- Modify: `crates/zoid-plugin/src/lib.rs` (uncomment `pub mod bundled;`)
- Modify: `crates/zoid/src/superpowers_install.rs` — add ONE regression test (do not delete the recipe yet).

**Interfaces:**
- Produces: `pub fn bundled_manifest(id: &str) -> Option<PluginManifest>`; `pub fn bundled_ids() -> &'static [&'static str]`.

- [ ] **Step 1: Write `manifests/superpowers.toml`**

The `[mode].description` must be the EXACT string from `superpowers_install.rs::MODE_DESCRIPTION` (lines 19-21) for byte-identical output:

```toml
[plugin]
id = "superpowers"
schema = 1
kind = ["mode"]
name = "Superpowers"
description = "A curated skill set for structured software engineering workflows."

[source]
repo = "obra/superpowers"
ref = "d884ae04edebef577e82ff7c4e143debd0bbec99"
subtree = "skills"

[mode]
loader = "using-superpowers/SKILL.md"
strip_prefix = "skills/"
body = "from-skill-frontmatter"
description = "Superpowers — a curated skill set for structured software engineering workflows (TDD, debugging, code review, planning, parallel agents, git worktrees, verification), imported from obra/superpowers."

[[install]]
effect = "activate"

[[install]]
effect = "onboarding_hint"
text = "Superpowers mode installed and active."
```

Verify the `[mode].description` value character-for-character against `MODE_DESCRIPTION` in `crates/zoid/src/superpowers_install.rs:19-21` (the `\`-continued string concatenates to a single line). If they differ, the Step 4 regression test will fail — fix the manifest to match.

- [ ] **Step 2: Write the failing test in `bundled.rs`**

```rust
//! Bundled (first-party) manifests shipped inside zoid, embedded at compile time.

use crate::manifest::{parse_manifest, PluginManifest};

const SUPERPOWERS_TOML: &str = include_str!("../manifests/superpowers.toml");

pub fn bundled_ids() -> &'static [&'static str] {
    &["superpowers"]
}

pub fn bundled_manifest(id: &str) -> Option<PluginManifest> {
    match id {
        "superpowers" => Some(parse_manifest(SUPERPOWERS_TOML).expect("bundled superpowers.toml parses")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superpowers_is_bundled_and_valid() {
        let m = bundled_manifest("superpowers").unwrap();
        m.validate().unwrap();
        assert_eq!(m.source.as_ref().unwrap().ref_, "d884ae04edebef577e82ff7c4e143debd0bbec99");
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(bundled_manifest("nope").is_none());
    }
}
```

- [ ] **Step 3: Uncomment `pub mod bundled;` and run**

Run: `cargo test -p zoid-plugin bundled`
Expected: PASS (2 tests).

- [ ] **Step 4: Write the byte-identical regression test in `superpowers_install.rs` tests module**

This proves the generic `build_plan` reproduces the old bespoke `superpowers_mapping` output exactly, BEFORE we delete the old recipe. Add to the existing `#[cfg(test)] mod tests` in `crates/zoid/src/superpowers_install.rs`:

```rust
#[test]
fn generic_plan_matches_bespoke_mapping_byte_for_byte() {
    let scan = fixture();
    let bespoke = superpowers_mapping(&scan).unwrap();
    let manifest = zoid_plugin::bundled::bundled_manifest("superpowers").unwrap();
    let generic = zoid_plugin::plan::build_plan(&manifest, &scan).unwrap();
    assert_eq!(generic.mapping.mode_name, bespoke.mode_name);
    assert_eq!(generic.mapping.mode_description, bespoke.mode_description);
    assert_eq!(generic.mapping.mode_body, bespoke.mode_body);
    assert_eq!(generic.mapping.entries, bespoke.entries);
}
```

Add `zoid-plugin = { path = "../zoid-plugin" }` to `crates/zoid/Cargo.toml` `[dependencies]` (Task 6 also needs it; adding it here is fine).

- [ ] **Step 5: Run the regression test**

Run: `cargo test -p zoid generic_plan_matches_bespoke_mapping_byte_for_byte`
Expected: PASS. If `mode_description` or `mode_body` differs, reconcile the manifest string (Step 1) until identical — do not change `build_plan` to match; the manifest data is the source of truth.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-plugin crates/zoid/Cargo.toml crates/zoid/src/superpowers_install.rs
git commit -m "feat(plugin): bundle superpowers manifest + byte-identical regression guard"
```

---

### Task 6: Plugin provenance sidecar types

**Files:**
- Create/Modify: `crates/zoid-plugin/src/provenance.rs`
- Modify: `crates/zoid-plugin/src/lib.rs` (uncomment `pub mod provenance;`)

**Interfaces:**
- Produces:
  - `pub struct PluginProvenance { pub schema: u32, pub plugin: PluginStamp, pub source: PluginProvSource, pub files: Vec<zoid_core::wizard::ProvenanceEntry>, pub effects_applied: Vec<AppliedEffect> }`
  - `pub struct PluginStamp { pub id: String, pub manifest_ref: String, pub installed_at: String }`
  - `pub struct PluginProvSource { pub repo: String, pub ref_: String, pub subtree: String, pub origin: String }`
  - `pub enum AppliedEffect { Activate, OnboardingHint { text: String }, SetConfig { key: String, prev: serde_json::Value, new: serde_json::Value } }`

- [ ] **Step 1: Write the failing round-trip test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::wizard::ProvenanceEntry;

    #[test]
    fn provenance_round_trips_and_has_no_host_paths() {
        let p = PluginProvenance {
            schema: 1,
            plugin: PluginStamp {
                id: "superpowers".into(),
                manifest_ref: "d884ae0".into(),
                installed_at: "2026-07-09T00:00:00Z".into(),
            },
            source: PluginProvSource {
                repo: "obra/superpowers".into(),
                ref_: "d884ae0".into(),
                subtree: "skills".into(),
                origin: "bundled".into(),
            },
            files: vec![ProvenanceEntry {
                canonical_path: "brainstorming/SKILL.md".into(),
                upstream_path: "skills/brainstorming/SKILL.md".into(),
                upstream_sha: "sha".into(),
                upstream_ref: "d884ae0".into(),
                upstream_snapshot: "snap".into(),
            }],
            effects_applied: vec![
                AppliedEffect::Activate,
                AppliedEffect::OnboardingHint { text: "hi".into() },
            ],
        };
        let json = serde_json::to_string_pretty(&p).unwrap();
        assert!(!json.contains("/home/"));
        let back: PluginProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-plugin provenance`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement `provenance.rs`**

```rust
//! The `.zoid-plugin.json` sidecar (schema 1): a superset of the mode
//! provenance that also records the ordered effects applied at install, so a
//! future uninstall can revert them (prev values captured for SetConfig).

use serde::{Deserialize, Serialize};
use zoid_core::wizard::ProvenanceEntry;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginProvenance {
    pub schema: u32,
    pub plugin: PluginStamp,
    pub source: PluginProvSource,
    pub files: Vec<ProvenanceEntry>,
    pub effects_applied: Vec<AppliedEffect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStamp {
    pub id: String,
    pub manifest_ref: String,
    pub installed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginProvSource {
    pub repo: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub subtree: String,
    /// "bundled" | "repo" | "url" — where the manifest came from.
    pub origin: String,
}

/// An effect as actually applied, with enough info to revert it. `SetConfig`
/// captures the prior value so uninstall restores the exact prior state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppliedEffect {
    Activate,
    OnboardingHint { text: String },
    SetConfig {
        key: String,
        prev: serde_json::Value,
        new: serde_json::Value,
    },
}
```

- [ ] **Step 4: Uncomment `pub mod provenance;` and run**

Run: `cargo test -p zoid-plugin provenance`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-plugin/src/provenance.rs crates/zoid-plugin/src/lib.rs
git commit -m "feat(plugin): plugin provenance sidecar types (.zoid-plugin.json)"
```

---

### Task 7: Effectful installer core (`plugin_install.rs`)

FS-effectful but App-state-free, so it is unit-testable with a tempdir (mirrors `superpowers_install::finish_install`). Validates the plan's effects (rejects Dangerous/unsupported in v1), materializes files clean-slate, writes the plugin sidecar, and returns the Safe effects for the caller to apply to `App`.

**Files:**
- Create: `crates/zoid/src/plugin_install.rs`
- Modify: `crates/zoid/src/lib.rs` (add `pub mod plugin_install;`)

**Interfaces:**
- Consumes: `zoid_plugin::{plan::InstallPlan, effect::{Effect, RiskTier}, provenance::*}`; `zoid_core::wizard::UpstreamScan`; `crate::mode_wizard::materialize`.
- Produces:
  - `pub struct InstalledPlugin { pub dest: std::path::PathBuf, pub safe_effects: Vec<Effect> }`
  - `pub fn finish_plugin_install(plan: &InstallPlan, scan: &UpstreamScan, dest_dir: &Path, plugin_id: &str, origin: &str) -> Result<InstalledPlugin, String>`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::wizard::{ScannedFile, UpstreamScan};
    use zoid_plugin::effect::Effect;
    use zoid_plugin::manifest::{BodyStrategy, ModeRecipe, PluginManifest};
    use zoid_plugin::plan::build_plan;

    fn skill_md(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody\n")
    }
    fn scan() -> UpstreamScan {
        UpstreamScan {
            url: "u".into(), repo: "obra/superpowers".into(), resolved_ref: "SHA".into(),
            subtree_path: "skills".into(),
            files: vec![
                ScannedFile { upstream_path: "skills/using-superpowers/SKILL.md".into(), sha: "a".into(), content: skill_md("using-superpowers", "loader") },
                ScannedFile { upstream_path: "skills/brainstorming/SKILL.md".into(), sha: "c".into(), content: skill_md("brainstorming", "creative") },
            ],
        }
    }
    fn manifest(effects: Vec<Effect>) -> PluginManifest {
        PluginManifest {
            id: "superpowers".into(), schema: 1, kind: vec!["mode".into()],
            name: "Superpowers".into(), description: "d".into(), source: None,
            mode: Some(ModeRecipe { loader: "using-superpowers/SKILL.md".into(), strip_prefix: "skills/".into(), body: BodyStrategy::FromSkillFrontmatter, description: "desc".into() }),
            install: effects,
        }
    }

    #[test]
    fn installs_mode_writes_sidecar_and_returns_safe_effects() {
        let scan = scan();
        let plan = build_plan(&manifest(vec![Effect::Activate]), &scan).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        let out = finish_plugin_install(&plan, &scan, &dest, "superpowers", "bundled").unwrap();
        assert_eq!(out.dest, dest);
        assert!(dest.join("mode.md").is_file());
        assert!(dest.join("brainstorming/SKILL.md").is_file());
        // mode provenance (from materialize) AND plugin provenance both present.
        assert!(dest.join(".zoid-provenance.json").is_file());
        let side = std::fs::read_to_string(dest.join(".zoid-plugin.json")).unwrap();
        let pv: zoid_plugin::provenance::PluginProvenance = serde_json::from_str(&side).unwrap();
        assert_eq!(pv.plugin.id, "superpowers");
        assert_eq!(pv.source.origin, "bundled");
        assert_eq!(out.safe_effects, vec![Effect::Activate]);
    }

    #[test]
    fn rejects_dangerous_effect_in_v1() {
        let scan = scan();
        let plan = build_plan(&manifest(vec![Effect::SetConfig { key: "provider".into(), value: toml::Value::String("x".into()) }]), &scan).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        let err = finish_plugin_install(&plan, &scan, &dest, "superpowers", "bundled").unwrap_err();
        assert!(err.contains("requires confirmation") || err.contains("not yet supported"), "got: {err}");
        // Nothing materialized on rejection.
        assert!(!dest.exists());
    }

    #[test]
    fn reinstall_is_clean_slate() {
        let scan = scan();
        let plan = build_plan(&manifest(vec![Effect::Activate]), &scan).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        finish_plugin_install(&plan, &scan, &dest, "superpowers", "bundled").unwrap();
        std::fs::write(dest.join("STALE.md"), "old").unwrap();
        finish_plugin_install(&plan, &scan, &dest, "superpowers", "bundled").unwrap();
        assert!(!dest.join("STALE.md").exists());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid finish_plugin_install`
Expected: FAIL — undefined.

- [ ] **Step 3: Implement `plugin_install.rs`**

```rust
//! Effectful plugin installer: validate effects, materialize the mode
//! clean-slate, write the plugin provenance sidecar, and return the Safe
//! effects for the caller to apply to App state. App-state-free so it is
//! unit-testable with a tempdir (mirrors superpowers_install::finish_install).

use std::path::{Path, PathBuf};

use zoid_core::wizard::UpstreamScan;
use zoid_plugin::effect::{Effect, RiskTier};
use zoid_plugin::plan::InstallPlan;
use zoid_plugin::provenance::{AppliedEffect, PluginProvSource, PluginProvenance, PluginStamp};

use crate::mode_wizard::materialize;

pub struct InstalledPlugin {
    pub dest: PathBuf,
    pub safe_effects: Vec<Effect>,
}

/// `dest_dir` = `<cfg>/modes/<plugin_id>`; the caller resolves it.
/// `origin` = "bundled" | "repo" | "url".
pub fn finish_plugin_install(
    plan: &InstallPlan,
    scan: &UpstreamScan,
    dest_dir: &Path,
    plugin_id: &str,
    origin: &str,
) -> Result<InstalledPlugin, String> {
    // v1 gate: any Dangerous effect requires the (deferred) confirmation prompt.
    // Reject BEFORE touching the filesystem so a rejected install leaves nothing.
    for e in &plan.effects {
        if e.risk() == RiskTier::Dangerous {
            return Err(format!(
                "effect requires confirmation, not yet supported in this zoid version: {e:?}"
            ));
        }
    }

    // Clean-slate so a failed re-install leaves nothing rather than a corrupted
    // mode (same rationale as superpowers_install::finish_install).
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)
            .map_err(|e| format!("remove old install {}: {e}", dest_dir.display()))?;
    }
    let fetched_at = chrono::Utc::now().to_rfc3339();
    materialize(&plan.mapping, scan, dest_dir, &fetched_at).map_err(|e| e.problems.join("; "))?;

    // Build applied-effect records (all Safe in v1) and the plugin sidecar.
    let applied: Vec<AppliedEffect> = plan
        .effects
        .iter()
        .map(|e| match e {
            Effect::Activate => AppliedEffect::Activate,
            Effect::OnboardingHint { text } => AppliedEffect::OnboardingHint { text: text.clone() },
            // Unreachable in v1 (rejected above), but map for completeness.
            Effect::SetConfig { key, value } => AppliedEffect::SetConfig {
                key: key.clone(),
                prev: serde_json::Value::Null,
                new: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
            },
        })
        .collect();

    let sidecar = PluginProvenance {
        schema: 1,
        plugin: PluginStamp {
            id: plugin_id.to_string(),
            manifest_ref: scan.resolved_ref.clone(),
            installed_at: fetched_at.clone(),
        },
        source: PluginProvSource {
            repo: scan.repo.clone(),
            ref_: scan.resolved_ref.clone(),
            subtree: scan.subtree_path.clone(),
            origin: origin.to_string(),
        },
        // The mode's per-file provenance already lives in .zoid-provenance.json
        // (written by materialize). We keep files empty here to avoid two
        // sources of truth; uninstall reads .zoid-provenance.json for files.
        files: Vec::new(),
        effects_applied: applied,
    };
    let sidecar_json = serde_json::to_string_pretty(&sidecar)
        .map_err(|e| format!("serialize plugin sidecar: {e}"))?;
    std::fs::write(dest_dir.join(".zoid-plugin.json"), sidecar_json)
        .map_err(|e| format!("write plugin sidecar: {e}"))?;

    let safe_effects = plan.effects.clone();
    Ok(InstalledPlugin {
        dest: dest_dir.to_path_buf(),
        safe_effects,
    })
}
```

- [ ] **Step 4: Add `pub mod plugin_install;` to `crates/zoid/src/lib.rs` and run**

Run: `cargo test -p zoid finish_plugin_install`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid/src/plugin_install.rs crates/zoid/src/lib.rs
git commit -m "feat(plugin): effectful installer core (materialize + sidecar + effect gate)"
```

---

### Task 8: Command parsing — `:plugin install` + `:mode install superpowers` alias

**Files:**
- Modify: `crates/zoid-tui/src/command.rs`

**Interfaces:**
- Produces: `Command::PluginInstall(String)`. Removes `Command::ModeInstallSuperpowers` (retargeted to `PluginInstall("superpowers")`).

- [ ] **Step 1: Update the failing tests**

Replace the existing `parses_mode_install_superpowers` and `mode_install_does_not_shadow_switch_to_a_mode_named_install` tests, and add plugin tests:

```rust
#[test]
fn mode_install_superpowers_aliases_to_plugin_install() {
    assert_eq!(
        parse_command(":mode install superpowers"),
        Command::PluginInstall("superpowers".into())
    );
    assert_eq!(
        parse_command("mode install superpowers"),
        Command::PluginInstall("superpowers".into())
    );
}

#[test]
fn mode_install_does_not_shadow_switch_to_a_mode_named_install() {
    assert_eq!(
        parse_command(":mode install foo"),
        Command::SwitchMode("install foo".into())
    );
}

#[test]
fn parses_plugin_install_id_and_url() {
    assert_eq!(
        parse_command(":plugin install superpowers"),
        Command::PluginInstall("superpowers".into())
    );
    assert_eq!(
        parse_command(":plugin install github.com/o/r/tree/main/skills"),
        Command::PluginInstall("github.com/o/r/tree/main/skills".into())
    );
    assert_eq!(
        parse_command(":plugin install"),
        Command::PluginInstall(String::new())
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p zoid-tui command`
Expected: FAIL — `PluginInstall` undefined / old variant mismatch.

- [ ] **Step 3: Update the `Command` enum**

Remove the `ModeInstallSuperpowers` variant (lines 23-25) and add:

```rust
    /// Install a plugin by bundled id or github URL (`:plugin install <arg>`).
    /// `:mode install superpowers` is a retained alias that produces this with
    /// arg = "superpowers". Empty string = usage hint.
    PluginInstall(String),
```

- [ ] **Step 4: Update the parser**

Replace the `"mode install superpowers" => Command::ModeInstallSuperpowers,` arm (line 78) with an alias, and add the `:plugin` namespace. Place the alias arm BEFORE the generic `mode ` switch arm, and add the plugin arms near the other namespaces:

```rust
        // --- :mode namespace ---
        "mode reload" => Command::ReloadModes,
        // ... existing import/update arms ...
        "mode install superpowers" => Command::PluginInstall("superpowers".into()),
        "mode" => Command::SwitchMode(String::new()),
        s if s.starts_with("mode ") => Command::SwitchMode(s["mode ".len()..].trim().to_string()),
        // --- :plugin namespace ---
        s if s.starts_with("plugin install ") => {
            Command::PluginInstall(s["plugin install ".len()..].trim().to_string())
        }
        "plugin install" => Command::PluginInstall(String::new()),
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p zoid-tui command`
Expected: PASS. (The compiler will also flag `ModeInstallSuperpowers` uses in `main.rs`/`palette.rs` — those are fixed in Tasks 9-10; if `zoid-tui` compiles standalone it's green here.)

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tui/src/command.rs
git commit -m "feat(plugin): :plugin install command + :mode install superpowers alias"
```

---

### Task 9: Wire the installer into main.rs + agent update

**Files:**
- Modify: `crates/zoid/src/agent.rs` (near `AgentUpdate` ~line 213)
- Modify: `crates/zoid/src/main.rs` (kickoff + apply + dispatch)

**Interfaces:**
- Consumes: `zoid_plugin::{resolve::*, bundled::*, plan::build_plan}`, `crate::plugin_install::finish_plugin_install`, `crate::github_fetch`.
- Produces: `AgentUpdate::PluginScan { id: String, origin: String, res: Result<UpstreamScan, String> }`; `install_plugin(app, arg)`; `apply_plugin_scan(app, ...) -> bool`.

- [ ] **Step 1: Add the `AgentUpdate` variant**

In `agent.rs`, alongside `SuperpowersScan(Result<UpstreamScan, String>)`, add:

```rust
    /// A completed plugin fetch, ready to materialize on the main loop.
    PluginScan {
        id: String,
        origin: String,
        res: Result<zoid_core::wizard::UpstreamScan, String>,
    },
```

- [ ] **Step 2: Write the kickoff `install_plugin` in main.rs**

v1 resolves only bundled ids and explicit bundled-for-url; repo `.zoid/` detection and wizard fallback are wired minimally (bundled-only) with clear messages for the deferred paths:

```rust
/// Kick off a plugin install: resolve the manifest source, fetch the pinned
/// tree off-thread, and hand the scan back via AgentUpdate::PluginScan.
fn install_plugin(app: &mut App, arg: String) {
    use zoid_plugin::resolve::{classify_ref, resolve_source, ManifestSource, PluginRef};
    if arg.trim().is_empty() {
        app.shell.status_hint = Some("usage: :plugin install <id|github-url>".into());
        return;
    }
    if app.installing_plugin {
        app.shell.status_hint = Some("a plugin install is already in progress…".into());
        return;
    }
    let r = classify_ref(&arg);
    let source = resolve_source(&r, zoid_plugin::bundled::bundled_ids(), false, false);

    // v1 supports the Bundled source (by id). Repo/.zoid and wizard fallback are
    // deferred; report them clearly rather than silently doing nothing.
    let (manifest, id) = match (&r, source) {
        (PluginRef::Id(id), ManifestSource::Bundled) => {
            (zoid_plugin::bundled::bundled_manifest(id).expect("bundled id resolves"), id.clone())
        }
        (PluginRef::Id(id), _) => {
            app.shell.status_hint = Some(format!("unknown plugin '{id}' (no bundled manifest)"));
            return;
        }
        (PluginRef::Url(_), _) => {
            app.shell.status_hint =
                Some("installing plugins from a URL is not supported yet; use a bundled id".into());
            return;
        }
    };
    if let Err(e) = manifest.validate() {
        app.shell.status_hint = Some(e);
        return;
    }
    let Some(src) = manifest.source.clone() else {
        app.shell.status_hint = Some(format!("plugin '{id}' has no [source] to fetch"));
        return;
    };
    let url = format!("github.com/{}/tree/{}/{}", src.repo, src.ref_, src.subtree);
    let parsed = match zoid::github_fetch::parse_github_url(&url) {
        Ok(p) => p,
        Err(e) => {
            app.shell.status_hint = Some(e);
            return;
        }
    };
    app.installing_plugin = true;
    app.shell.status_hint = Some(format!("installing plugin '{id}'…"));
    let ui_tx = app.ui_tx.clone();
    let id_for_msg = id.clone();
    tokio::spawn(async move {
        let api = zoid::github_fetch::HttpGithubApi::new();
        let res = zoid::github_fetch::fetch_tree(&api, &parsed)
            .await
            .map_err(|e| format!("plugin fetch failed: {e}"));
        let _ = ui_tx
            .send(zoid::agent::AgentUpdate::PluginScan {
                id: id_for_msg,
                origin: "bundled".into(),
                res,
            })
            .await;
    });
}
```

Add an `installing_plugin: bool` field to `App` (next to `installing_superpowers`), initialized `false`.

- [ ] **Step 3: Write `apply_plugin_scan` in main.rs**

```rust
/// Apply a completed plugin fetch on the main loop: build the plan, materialize
/// into `<modes-dir>/<id>`, apply Safe effects (activate / onboarding hint),
/// rebuild the registry. Returns `true` iff a mode was installed and activated.
fn apply_plugin_scan(
    app: &mut App,
    id: String,
    origin: String,
    res: Result<zoid_core::wizard::UpstreamScan, String>,
) -> bool {
    app.installing_plugin = false;
    let scan = match res {
        Ok(s) => s,
        Err(e) => {
            app.shell.status_hint = Some(e);
            return false;
        }
    };
    let manifest = match zoid_plugin::bundled::bundled_manifest(&id) {
        Some(m) => m,
        None => {
            app.shell.status_hint = Some(format!("bundled manifest for '{id}' vanished"));
            return false;
        }
    };
    let plan = match zoid_plugin::plan::build_plan(&manifest, &scan) {
        Ok(p) => p,
        Err(e) => {
            app.shell.status_hint = Some(format!("plugin plan failed: {e}"));
            return false;
        }
    };
    let Some(dest) = app.mode_dirs.first().map(|d| d.join(&id)) else {
        app.shell.status_hint = Some("no modes directory configured".into());
        return false;
    };
    let installed = match zoid::plugin_install::finish_plugin_install(&plan, &scan, &dest, &id, &origin) {
        Ok(out) => out,
        Err(e) => {
            app.shell.status_hint = Some(format!("plugin install failed: {e}"));
            return false;
        }
    };

    // Rebuild registry so the new mode is visible.
    let prev = app.modes.active_name().to_string();
    app.modes = zoid::mode_import::build_mode_registry(&app.base_profile, &app.mode_dirs);

    // Apply Safe effects.
    let mut activated = false;
    let mode_display = manifest.name.clone();
    for e in &installed.safe_effects {
        match e {
            zoid_plugin::effect::Effect::Activate => {
                if app.modes.names().iter().any(|n| n == &mode_display) {
                    app.modes.set_active(&mode_display);
                    activated = true;
                }
            }
            zoid_plugin::effect::Effect::OnboardingHint { text } => {
                app.shell.status_hint = Some(text.clone());
            }
            zoid_plugin::effect::Effect::SetConfig { .. } => { /* deferred; never reaches here in v1 */ }
        }
    }
    if !activated {
        app.modes.set_active(&prev); // preserve prior active if we didn't activate
    }
    sync_mode_mirror(app);
    if app.shell.status_hint.is_none() {
        app.shell.status_hint = Some(format!("plugin '{id}' installed."));
    }
    activated
}
```

- [ ] **Step 4: Dispatch the command + the update**

In `exec_command`, replace the `Command::ModeInstallSuperpowers` arm with:

```rust
        Command::PluginInstall(arg) => {
            install_plugin(app, arg);
            Ok(false)
        }
```

Where the main loop matches `AgentUpdate::SuperpowersScan(res)` (~main.rs:2846), add a sibling arm:

```rust
                zoid::agent::AgentUpdate::PluginScan { id, origin, res } => {
                    if apply_plugin_scan(app, id, origin, res) {
                        persist_active_mode(app).await;
                    }
                }
```

- [ ] **Step 5: Build and run the whole workspace test suite**

Run: `cargo build -p zoid`
Expected: compiles (old `ModeInstallSuperpowers` references now gone from command.rs; `install_superpowers`/`apply_superpowers_scan` may still exist and are removed in Task 10).
Run: `cargo test -p zoid`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid/src/agent.rs crates/zoid/src/main.rs
git commit -m "feat(plugin): wire generic installer into main loop (PluginScan)"
```

---

### Task 10: Retarget palette + onboarding wording

**Files:**
- Modify: `crates/zoid-tui/src/palette.rs` (~line 205)
- Modify: `crates/zoid-tui/src/onboarding.rs` (~line 26)

- [ ] **Step 1: Update the palette row test (if one exists) / update the row**

At `palette.rs:205`, change the `install superpowers` row so its produced command is `Command::PluginInstall("superpowers".into())` and its label reads `plugin install superpowers`. If the palette builds `Command::ModeInstallSuperpowers` there, replace it with `Command::PluginInstall("superpowers".into())`.

- [ ] **Step 2: Update the onboarding line**

At `onboarding.rs:26`, change the instructional text from `Run :mode install superpowers …` to:

```rust
    "Run :plugin install superpowers to install the Superpowers skill set (structured TDD, debugging, planning, and review workflows).",
```

Keep the existing gating in `main.rs:2263` (`first_time_user && !modes.contains("Superpowers")`) unchanged — the installed mode is still named `Superpowers`.

- [ ] **Step 3: Build + run**

Run: `cargo build -p zoid-tui && cargo test -p zoid-tui`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/zoid-tui/src/palette.rs crates/zoid-tui/src/onboarding.rs
git commit -m "feat(plugin): retarget palette + onboarding to :plugin install superpowers"
```

---

### Task 11: Delete the bespoke recipe

Only after Tasks 5 + 9 prove the generic path works and reproduces the old output.

**Files:**
- Modify/Delete: `crates/zoid/src/superpowers_install.rs`
- Modify: `crates/zoid/src/main.rs` (remove `install_superpowers`, `apply_superpowers_scan`, `SuperpowersScan` handling)
- Modify: `crates/zoid/src/agent.rs` (remove `AgentUpdate::SuperpowersScan`)
- Modify: `crates/zoid/src/lib.rs` (remove `pub mod superpowers_install;` if the file is deleted)

- [ ] **Step 1: Remove the old async orchestration from main.rs**

Delete `install_superpowers` (lines ~4463-4491) and `apply_superpowers_scan` (lines ~4501-4535), and the `AgentUpdate::SuperpowersScan(res)` match arm in the main loop (~2846). Remove the `installing_superpowers` App field and its initializer.

- [ ] **Step 2: Remove the `SuperpowersScan` variant from agent.rs**

Delete the `SuperpowersScan(Result<UpstreamScan, String>)` variant.

- [ ] **Step 3: Delete the recipe file (keep nothing bespoke)**

The byte-identical regression test added in Task 5 lives in this file and references `superpowers_mapping`. Move that test's *value* into `zoid-plugin` by deleting it here (the equivalence is already proven and committed; the generic path now stands alone). Then delete `crates/zoid/src/superpowers_install.rs` entirely and remove `pub mod superpowers_install;` from `crates/zoid/src/lib.rs`.

If anything else still imports `zoid::superpowers_install::*`, replace those imports with the `zoid_plugin` equivalents.

- [ ] **Step 4: Build the whole workspace**

Run: `cargo build`
Expected: compiles with no references to `superpowers_install` or `SuperpowersScan`.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test`
Expected: PASS across all crates.

- [ ] **Step 6: Manual smoke test (documented, run if a GitHub token is available)**

Run the TUI, type `:plugin install superpowers`, confirm the Superpowers mode installs to `<cfg>/modes/superpowers`, becomes active, and that both `.zoid-provenance.json` and `.zoid-plugin.json` exist in that dir. Then type `:mode install superpowers` and confirm it behaves identically (alias).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(plugin): delete bespoke superpowers_install recipe (superseded by manifest)"
```

---

## Deferred to follow-up plan (tracked seams)

These are intentionally out of v1. Each has a typed seam already in place:

1. **`:plugin uninstall <id>`** — read `.zoid-plugin.json`, revert `effects_applied` in reverse (restore `SetConfig.prev`), `remove_dir_all` the mode dir, rebuild registry.
2. **`:plugin update <id>`** — reuse `zoid_core::wizard::classify_update` on the mode's `.zoid-provenance.json` files; re-run effects idempotently.
3. **`:plugin list`** — read-only overlay mirroring `Overlay::Mcp`, listing installed plugins + source/ref + active state.
4. **Interactive Dangerous-effect approval** — replace `finish_plugin_install`'s hard reject with the `detailed_approval_summary`-style overlay; wire `Effect::SetConfig` application (config.toml read-modify-write capturing `prev`).
5. **Repo `.zoid/plugin.toml` fetch + wizard fallback** — flesh out `install_plugin`'s `Url` arm: fetch the tree, look for `.zoid/plugin.toml`, parse it (origin "repo"), else bundled-for-url, else fall through to the existing `:mode import` wizard.
6. **Generalize the mode-body preamble** — the `"Superpowers"`-specific preamble in `generate_body_from_frontmatter` becomes a manifest-driven template once a second mode plugin exists.
7. **MCP as a plugin kind** — `Effect::InstallMcp` / `kind = ["mcp"]`, classified Dangerous, wired to `zoid-mcp::McpManager`.

## Self-Review

- **Spec coverage:** crate layout (§3.1) → Task 1; manifest schema (§3.2) → Task 2; effect+risk (§3.3) → Task 1; resolution (§3.4) → Task 3; installer flow (§3.5) → Tasks 4/7/9; provenance (§3.6) → Tasks 6/7; command surface (§4) install+alias → Tasks 8-10 (uninstall/update/list deferred, noted); superpowers migration (§5) → Tasks 5/11; seams (§6) → validate() + effect gate + deferred section; testing (§7) → per-task tests + byte-identical guard (Task 5) + provenance round-trip (Task 6). Deferred items are explicitly listed, not dropped.
- **Placeholder scan:** no TBD/TODO; every code step carries complete code; the one ported function (`generate_body_from_frontmatter`) is shown in full.
- **Type consistency:** `PluginManifest`/`ModeRecipe`/`BodyStrategy`/`Effect`/`RiskTier`/`InstallPlan`/`build_plan`/`finish_plugin_install`/`PluginProvenance`/`AppliedEffect`/`Command::PluginInstall`/`AgentUpdate::PluginScan` are used consistently across tasks with matching signatures. `materialize`, `parse_github_url`, `fetch_tree`, `build_mode_registry`, `sync_mode_mirror`, `persist_active_mode` match the current codebase signatures verified during planning.
