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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::Effect;
    use crate::manifest::{BodyStrategy, ModeRecipe, PluginManifest};
    use zoid_core::wizard::{ScannedFile, UpstreamScan};

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
