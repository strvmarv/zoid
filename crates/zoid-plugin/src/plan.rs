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
    if manifest.kind.iter().any(|k| k == "skills") && !manifest.kind.iter().any(|k| k == "mode") {
        return build_skills_plan(manifest, scan);
    }
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
            generate_body_from_frontmatter(manifest, scan, &loader_full, &mode.strip_prefix)
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

/// Builds the mode body from the manifest's `[mode]` recipe. The skill bullet
/// list is the name+description frontmatter of each top-level
/// `<skill>/SKILL.md` under the stripped subtree (loader excluded),
/// alphabetical by name. The intro/outro come from the manifest's
/// `body_intro`/`body_outro` when present; otherwise a generic default is
/// synthesized from the manifest's name and source repo. For Superpowers,
/// whose manifest carries the exact ported strings, this reproduces the
/// original bespoke installer's output byte-for-byte.
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
            continue; // only a skill's top-level SKILL.md, not sibling docs
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::effect::Effect;
    use crate::manifest::{BodyStrategy, ModeRecipe, PluginManifest};
    use zoid_core::wizard::{ScannedFile, UpstreamScan};

    fn skill_md(name: &str, desc: &str) -> String {
        format!("---\nname: {name}\ndescription: {desc}\n---\nbody for {name}\n")
    }

    pub(crate) fn scan() -> UpstreamScan {
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
                body_intro: Some("You are operating in \"Superpowers\" mode, imported from obra/superpowers.\n\nBefore any task, check if an available skill applies and invoke it with invoke_skill. The skills are:\n".into()),
                body_outro: Some("\nAlways check for an applicable skill before starting work. If multiple skills apply, invoke the most specific one first. After completing work, invoke verification-before-completion before claiming success.\n\nSkill work produces specs, plans, and debugging notes. Keep the running narration terse, and when the work is done do NOT reframe the whole effort in long paragraphs: close with a short recap of what changed and any next step.\n".into()),
            }),
            mcp: None,
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

    #[test]
    fn mode_body_matches_golden_snapshot() {
        let plan = build_plan(&manifest(), &scan()).unwrap();
        let golden = include_str!("../tests/superpowers_body_golden.txt");
        assert_eq!(plan.mapping.mode_body, golden,
            "body generator drifted; if intentional, regenerate the golden file");
    }

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
        let mode = m.mode.as_mut().unwrap();
        mode.body_intro = None;
        mode.body_outro = None;
        let plan = build_plan(&m, &scan()).unwrap();
        assert!(plan.mapping.mode_body.contains("operating in \"Robotics\" mode"));
        assert!(plan.mapping.mode_body.contains("imported from arpitg1304/robotics-agent-skills"));
        assert!(plan.mapping.mode_body.contains("invoke_skill"));
        // The generic default must NOT carry Superpowers-specific text.
        assert!(!plan.mapping.mode_body.contains("verification-before-completion"));
    }
}
