//! Filesystem source adapter for modes — the effectful half (the pure model is
//! `zoid_core::mode`). Walks convention + configured dirs, and for each subfolder
//! with a `mode.md` builds a `Mode::Ready` (its `*/SKILL.md` become the mode's
//! scoped skills) or, on a parse failure, a `Mode::Broken` named by the folder.
//! Bad inputs are skipped/degraded, never fatal — mirroring `skill_import.rs`.

use std::path::{Path, PathBuf};

use zoid_core::agent_profile::AgentProfile;
use zoid_core::mode::{overlay_prompt, Mode, ModeRegistry};
use zoid_core::skill::{parse_skill_md, SkillRegistry};

use crate::skill_import::import_skills;

/// Ordered dirs to scan: the two convention dirs (`<cfg>/modes`, `<cwd>/.zoid/modes`)
/// then configured `source_dirs` (leading `~`/`~/` expanded). Pure path arithmetic.
pub fn resolve_mode_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![user_cfg_dir.join("modes"), cwd.join(".zoid").join("modes")];
    for s in source_dirs {
        dirs.push(expand_tilde(s, home));
    }
    dirs
}

fn expand_tilde(s: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if s == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = s.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Build the mode registry: `Chat` (from `base`) at index 0, then one mode per
/// `<dir>/<name>/mode.md`. A folder without `mode.md` is ignored; a malformed
/// `mode.md` becomes `Mode::Broken` named by its folder. Scoped skills come from
/// the folder's `*/SKILL.md` (reusing the skill importer). First-wins by mode
/// name across dirs. Never panics.
pub fn build_mode_registry(base: &AgentProfile, dirs: &[PathBuf]) -> ModeRegistry {
    let mut modes = vec![Mode::chat(base.clone())];
    let mut seen: Vec<String> = vec![base.name.clone()];
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping modes dir {}: {e}", dir.display());
                continue;
            }
        };
        // Sort by folder name for deterministic cycle order.
        let mut folders: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            let manifest = folder.join("mode.md");
            if !manifest.is_file() {
                continue; // not a mode
            }
            let folder_name = folder
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("<mode>")
                .to_string();
            let mode = load_mode(base, &folder, &manifest, &folder_name);
            let name = mode.name().to_string();
            if seen.iter().any(|n| n == &name) {
                eprintln!(
                    "zoid: skipping duplicate mode '{name}' at {}",
                    folder.display()
                );
                continue;
            }
            seen.push(name);
            modes.push(mode);
        }
    }
    ModeRegistry::new(modes)
}

/// Load one mode folder into `Ready` or `Broken`. Total — a read/parse failure
/// yields `Broken` named by the folder (so it stays visible in the cycle).
fn load_mode(base: &AgentProfile, folder: &Path, manifest: &Path, folder_name: &str) -> Mode {
    let text = match std::fs::read_to_string(manifest) {
        Ok(t) => t,
        Err(e) => {
            return Mode::Broken {
                name: folder_name.to_string(),
                error: format!("cannot read {}: {e}", manifest.display()),
            }
        }
    };
    let parsed = match parse_skill_md(&text) {
        Ok(p) => p,
        Err(reason) => {
            return Mode::Broken {
                name: folder_name.to_string(),
                error: format!("{}: {reason}", manifest.display()),
            }
        }
    };
    // Scoped skills: the mode folder's immediate `*/SKILL.md` children.
    let skills = SkillRegistry::new(import_skills(&[folder.to_path_buf()]));
    let profile = AgentProfile {
        name: parsed.name,
        description: parsed.description,
        system_prompt: overlay_prompt(&base.system_prompt, &parsed.body),
        tools: vec![], // SEAMED — a mode's own tool allow-list is not honored this slice
        model: None,   // SEAMED — a mode's model override is not honored this slice
    };
    Mode::Ready { profile, skills }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AgentProfile {
        AgentProfile {
            name: "default".into(),
            description: "base".into(),
            system_prompt: "BASE".into(),
            tools: vec![],
            model: None,
        }
    }

    fn write(dir: &Path, rel: &str, contents: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    fn reg_get<'a>(reg: &'a ModeRegistry, name: &str) -> &'a Mode {
        reg.modes().iter().find(|m| m.name() == name).unwrap()
    }

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_mode_dirs(
            &["~/m".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/modes"),
                PathBuf::from("/proj/.zoid/modes"),
                PathBuf::from("/home/u/m"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn chat_is_always_index_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert_eq!(reg.names().first().map(String::as_str), Some("default")); // Chat = base profile name
    }

    #[test]
    fn ready_mode_composes_overlay_and_scopes_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "superpowers/mode.md",
            "---\nname: Superpowers\ndescription: sp\n---\nUSE SKILLS\n",
        );
        write(
            root,
            "superpowers/brainstorming/SKILL.md",
            "---\nname: brainstorming\ndescription: d\n---\nBODY\n",
        );
        let reg = build_mode_registry(&base(), &[root.to_path_buf()]);
        assert_eq!(
            reg.names(),
            vec!["default".to_string(), "Superpowers".to_string()]
        );
        match &reg_get(&reg, "Superpowers") {
            Mode::Ready { profile, skills } => {
                assert_eq!(profile.system_prompt, "BASE\n\nUSE SKILLS\n"); // overlay = base + body
                assert_eq!(skills.names(), vec!["brainstorming".to_string()]);
            }
            _ => panic!("Superpowers must be Ready"),
        }
    }

    #[test]
    fn malformed_mode_md_is_broken_named_by_folder() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "busted/mode.md", "no frontmatter here\n");
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert!(matches!(reg_get(&reg, "busted"), Mode::Broken { .. }));
    }

    #[test]
    fn folder_without_mode_md_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "just-skills/x/SKILL.md",
            "---\nname: x\ndescription: d\n---\nb\n",
        );
        let reg = build_mode_registry(&base(), &[tmp.path().to_path_buf()]);
        assert_eq!(reg.names(), vec!["default".to_string()]); // only Chat
    }

    #[test]
    fn bad_skill_inside_good_mode_keeps_mode_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "m/mode.md", "---\nname: M\ndescription: d\n---\n\n");
        write(
            root,
            "m/good/SKILL.md",
            "---\nname: good\ndescription: d\n---\nb\n",
        );
        write(root, "m/bad/SKILL.md", "no frontmatter\n");
        let reg = build_mode_registry(&base(), &[root.to_path_buf()]);
        match reg_get(&reg, "M") {
            Mode::Ready { skills, .. } => assert_eq!(skills.names(), vec!["good".to_string()]),
            _ => panic!("M must be Ready"),
        }
    }

    #[test]
    fn missing_dir_is_skipped_without_panic() {
        let reg = build_mode_registry(&base(), &[PathBuf::from("/nonexistent/zoid/modes/xyz")]);
        assert_eq!(reg.names(), vec!["default".to_string()]);
    }
}
