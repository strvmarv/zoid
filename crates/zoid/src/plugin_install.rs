//! Effectful plugin installer: validate effects, materialize the mode
//! clean-slate, write the plugin provenance sidecar, and return the Safe
//! effects for the caller to apply to App state. App-state-free so it is
//! unit-testable with a tempdir (mirrors the now-deleted bespoke Superpowers
//! installer's `finish_install`).

use std::path::{Path, PathBuf};

use zoid_core::wizard::UpstreamScan;
use zoid_plugin::effect::{Effect, RiskTier};
use zoid_plugin::plan::InstallPlan;
use zoid_plugin::provenance::{AppliedEffect, PluginProvSource, PluginProvenance, PluginStamp};

use crate::mode_wizard::materialize;

#[derive(Debug)]
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
    manifest_ref: &str,
    origin: &str,
) -> Result<InstalledPlugin, String> {
    // v1 gate: any Dangerous effect requires the (deferred) confirmation prompt,
    // and ALL SetConfig effects (Safe or Dangerous) are rejected because config
    // application itself is deferred in v1 — a Safe-key SetConfig (e.g.
    // `skills.source_dirs`) must not slip through and be recorded as "applied"
    // when nothing was ever written to config.toml.
    // Reject BEFORE touching the filesystem so a rejected install leaves nothing.
    for e in &plan.effects {
        if e.risk() == RiskTier::Dangerous {
            return Err(format!(
                "effect requires confirmation, not yet supported in this zoid version: {e:?}"
            ));
        }
        if matches!(e, Effect::SetConfig { .. }) {
            return Err(format!(
                "config effects are not yet supported in this zoid version: {e:?}"
            ));
        }
    }

    // Clean-slate so a failed re-install leaves nothing rather than a corrupted
    // mode (same rationale as the now-deleted bespoke Superpowers installer's
    // `finish_install`).
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
            // Unreachable: SetConfig is rejected at the v1 gate above, so no
            // SetConfig ever reaches this mapping.
            Effect::SetConfig { .. } => unreachable!("SetConfig is rejected at the v1 gate"),
        })
        .collect();

    let sidecar = PluginProvenance {
        schema: 1,
        plugin: PluginStamp {
            id: plugin_id.to_string(),
            manifest_ref: manifest_ref.to_string(),
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

    // Filter to genuinely-safe effects so the field name stays honest even if
    // the gate above is later relaxed to prompt-and-continue for Dangerous
    // effects. Today, after the v1 gate above, the survivors are Activate and
    // OnboardingHint, both Safe — so behavior is unchanged.
    let safe_effects: Vec<Effect> = plan
        .effects
        .iter()
        .filter(|e| e.risk() == RiskTier::Safe)
        .cloned()
        .collect();
    Ok(InstalledPlugin {
        dest: dest_dir.to_path_buf(),
        safe_effects,
    })
}

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
        let out = finish_plugin_install(&plan, &scan, &dest, "superpowers", "d884ae0", "bundled").unwrap();
        assert_eq!(out.dest, dest);
        assert!(dest.join("mode.md").is_file());
        assert!(dest.join("brainstorming/SKILL.md").is_file());
        // mode provenance (from materialize) AND plugin provenance both present.
        assert!(dest.join(".zoid-provenance.json").is_file());
        let side = std::fs::read_to_string(dest.join(".zoid-plugin.json")).unwrap();
        let pv: zoid_plugin::provenance::PluginProvenance = serde_json::from_str(&side).unwrap();
        assert_eq!(pv.plugin.id, "superpowers");
        assert_eq!(pv.plugin.manifest_ref, "d884ae0"); // declared pin, not the fetched tree sha
        assert_eq!(pv.source.origin, "bundled");
        assert_eq!(out.safe_effects, vec![Effect::Activate]);
    }

    #[test]
    fn rejects_dangerous_effect_in_v1() {
        let scan = scan();
        let plan = build_plan(&manifest(vec![Effect::SetConfig { key: "provider".into(), value: toml::Value::String("x".into()) }]), &scan).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        let err = finish_plugin_install(&plan, &scan, &dest, "superpowers", "d884ae0", "bundled").unwrap_err();
        assert!(err.contains("requires confirmation") || err.contains("not yet supported"), "got: {err}");
        // Nothing materialized on rejection.
        assert!(!dest.exists());
    }

    #[test]
    fn rejects_safe_setconfig_in_v1() {
        // Pins the whole-branch-review bug: a Safe-key SetConfig (e.g.
        // `skills.source_dirs`) must NOT slip through the v1 gate just
        // because it's not Dangerous — config application is deferred
        // entirely in v1, so it must be rejected too.
        let scan = scan();
        let plan = build_plan(
            &manifest(vec![Effect::SetConfig {
                key: "skills.source_dirs".into(),
                value: toml::Value::String("x".into()),
            }]),
            &scan,
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        let err = finish_plugin_install(&plan, &scan, &dest, "superpowers", "d884ae0", "bundled").unwrap_err();
        assert!(err.contains("not yet supported"), "got: {err}");
        // Nothing materialized on rejection.
        assert!(!dest.exists());
    }

    #[test]
    fn reinstall_is_clean_slate() {
        let scan = scan();
        let plan = build_plan(&manifest(vec![Effect::Activate]), &scan).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("modes").join("superpowers");
        finish_plugin_install(&plan, &scan, &dest, "superpowers", "d884ae0", "bundled").unwrap();
        std::fs::write(dest.join("STALE.md"), "old").unwrap();
        finish_plugin_install(&plan, &scan, &dest, "superpowers", "d884ae0", "bundled").unwrap();
        assert!(!dest.join("STALE.md").exists());
    }
}
