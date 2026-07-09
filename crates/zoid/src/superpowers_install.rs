//! Deterministic, model-free install of the canonical obra/superpowers skill
//! set as a zoid mode. Reuses the URL-import wizard's fetch + materialize; the
//! only bespoke logic is the pinned mapping and the generated mode.md body.

use std::path::{Path, PathBuf};

use zoid_core::skill::parse_skill_md;
use zoid_core::wizard::{MappingEntry, ModeMapping, UpstreamScan};

use crate::mode_wizard::materialize;

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
}
