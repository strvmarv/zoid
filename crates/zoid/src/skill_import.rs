//! Filesystem source adapter for SKILL.md skills — the effectful half of the
//! importer (the pure parser lives in `zoid_core::skill`). Walks configured +
//! convention directories, parses each `<dir>/SKILL.md`, and returns `Skill`s
//! with an absolute `base_dir`. Bad inputs are skipped, never fatal — mirroring
//! the runtime's "a bad input returns a result, never aborts startup" rule.

use std::path::{Path, PathBuf};

use zoid_core::skill::{parse_skill_md, Skill};

/// The ordered directories to scan: the two convention dirs
/// (`<user_cfg_dir>/skills`, `<cwd>/.zoid/skills`) first, then the configured
/// `source_dirs` (a leading `~` or `~/` expanded against `home`). Pure path
/// arithmetic — existence is checked later by `import_skills`.
pub fn resolve_skill_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        user_cfg_dir.join("skills"),
        cwd.join(".zoid").join("skills"),
    ];
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

/// Scan each directory for immediate `*/SKILL.md` children, parse them, and
/// return the resulting skills (each with an absolute `base_dir`). A directory
/// that does not exist is skipped silently (a missing convention/source dir is
/// normal); a present-but-unreadable directory, an unreadable file, or a
/// malformed `SKILL.md` is skipped with a warning to stderr. Never panics.
pub fn import_skills(dirs: &[PathBuf]) -> Vec<Skill> {
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping skills dir {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let md = skill_dir.join("SKILL.md");
            if !md.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&md) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("zoid: skipping {}: {e}", md.display());
                    continue;
                }
            };
            match parse_skill_md(&text) {
                Ok(p) => {
                    let base = std::fs::canonicalize(&skill_dir).unwrap_or(skill_dir);
                    out.push(Skill {
                        name: p.name,
                        description: p.description,
                        body: p.body,
                        base_dir: Some(base),
                    });
                }
                Err(reason) => {
                    eprintln!("zoid: skipping {}: {reason}", md.display());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_skill_dirs(
            &["~/sp".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/skills"),
                PathBuf::from("/proj/.zoid/skills"),
                PathBuf::from("/home/u/sp"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn import_reads_valid_skills_and_skips_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (name, contents) in [
            ("alpha", "---\nname: alpha\ndescription: d\n---\nbody a\n"),
            ("beta", "---\nname: beta\ndescription: d\n---\nbody b\n"),
            ("broken", "no frontmatter here\n"),
        ] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), contents).unwrap();
        }

        let skills = import_skills(&[root.to_path_buf()]);
        let mut names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
        for s in &skills {
            assert!(s.base_dir.as_ref().unwrap().is_absolute());
        }
    }

    #[test]
    fn import_skips_missing_dir_without_panic() {
        let skills = import_skills(&[PathBuf::from("/nonexistent/zoid/skills/xyz")]);
        assert!(skills.is_empty());
    }
}
