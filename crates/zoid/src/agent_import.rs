//! Filesystem source adapter for `agent.md` agent profiles — the effectful half
//! of the importer (the pure parser lives in `zoid_core::agent_profile`). Walks
//! configured + convention directories, parses each `<dir>/<name>/agent.md`,
//! and returns `AgentProfile`s. Bad inputs are skipped, never fatal — mirroring
//! `skill_import.rs`.

use std::path::{Path, PathBuf};

use zoid_core::agent_profile::{parse_agent_md, AgentProfile, AgentRegistry};

/// The ordered directories to scan: the two convention dirs
/// (`<user_cfg_dir>/agents`, `<cwd>/.zoid/agents`) first, then the configured
/// `source_dirs` (a leading `~` or `~/` expanded against `home`). Pure path
/// arithmetic — existence is checked later by `import_agents`.
pub fn resolve_agent_dirs(
    source_dirs: &[String],
    user_cfg_dir: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        user_cfg_dir.join("agents"),
        cwd.join(".zoid").join("agents"),
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

/// Scan each directory for immediate `<name>/agent.md` children, parse them,
/// and return the resulting profiles. A directory that does not exist is
/// skipped silently; a present-but-unreadable directory, an unreadable file,
/// or a malformed `agent.md` is skipped with a warning to stderr. Never panics.
/// Also supports one level of nesting (`<root>/<pack>/<agent>/agent.md`).
pub fn import_agents(dirs: &[PathBuf]) -> Vec<AgentProfile> {
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("zoid: skipping agents dir {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let agent_dir = entry.path();
            if !agent_dir.is_dir() {
                continue;
            }
            let md = agent_dir.join("agent.md");
            if md.is_file() {
                push_agent(&mut out, &md);
                continue;
            }
            // No agent.md here → maybe a pack dir; scan one level deeper.
            if let Ok(inner) = std::fs::read_dir(&agent_dir) {
                for e2 in inner.flatten() {
                    let sub = e2.path();
                    let sub_md = sub.join("agent.md");
                    if sub.is_dir() && sub_md.is_file() {
                        push_agent(&mut out, &sub_md);
                    }
                }
            }
        }
    }
    out
}

/// Read, parse, and push a single `agent.md` onto `out`. Shared by both the
/// bare `<root>/<agent>/agent.md` and the per-pack `<root>/<pack>/<agent>/agent.md`
/// call sites in `import_agents`.
fn push_agent(out: &mut Vec<AgentProfile>, md: &Path) {
    let text = match std::fs::read_to_string(md) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("zoid: skipping {}: {e}", md.display());
            return;
        }
    };
    match parse_agent_md(&text) {
        Ok(p) => {
            out.push(AgentProfile {
                name: p.name,
                description: p.description,
                system_prompt: p.system_prompt,
                tools: p.tools,
                model: p.model,
            });
        }
        Err(reason) => {
            eprintln!("zoid: skipping {}: {reason}", md.display());
        }
    }
}

/// Build the session's agent registry: the built-in `delegate` plus every
/// importable agent under `dirs`. Built-ins and earlier dirs win name
/// collisions (first-wins), so an imported agent can never shadow `delegate`.
pub fn build_agent_registry(dirs: &[PathBuf]) -> AgentRegistry {
    let mut reg = AgentRegistry::builtin();
    for a in import_agents(dirs) {
        reg.push_unique(a);
    }
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolve_prepends_convention_dirs_and_expands_tilde() {
        let dirs = resolve_agent_dirs(
            &["~/ag".to_string(), "/abs/x".to_string()],
            Path::new("/home/u/.config/zoid"),
            Path::new("/proj"),
            Some(Path::new("/home/u")),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.config/zoid/agents"),
                PathBuf::from("/proj/.zoid/agents"),
                PathBuf::from("/home/u/ag"),
                PathBuf::from("/abs/x"),
            ]
        );
    }

    #[test]
    fn import_reads_valid_agents_and_skips_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (name, contents) in [
            ("alpha", "---\nname: alpha\ndescription: d\n---\nbody a\n"),
            ("beta", "---\nname: beta\ndescription: d\ntools:\n  - read\n---\nbody b\n"),
            ("broken", "no frontmatter here\n"),
        ] {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("agent.md"), contents).unwrap();
        }
        let agents = import_agents(&[root.to_path_buf()]);
        let mut names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
        let beta = agents.iter().find(|a| a.name == "beta").unwrap();
        assert_eq!(beta.tools, vec!["read".to_string()]);
    }

    #[test]
    fn import_skips_missing_dir_without_panic() {
        let agents = import_agents(&[PathBuf::from("/nonexistent/zoid/agents/xyz")]);
        assert!(agents.is_empty());
    }

    #[test]
    fn import_supports_per_pack_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Bare agent.
        let bare = root.join("bare-agent");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(
            bare.join("agent.md"),
            "---\nname: bare-agent\ndescription: d\n---\nb\n",
        )
        .unwrap();
        // Per-pack agent: <root>/packA/nested/agent.md
        let nested = root.join("packA").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("agent.md"),
            "---\nname: nested\ndescription: d\n---\nn\n",
        )
        .unwrap();
        let agents = import_agents(&[root.to_path_buf()]);
        let names: std::collections::HashSet<String> =
            agents.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains("bare-agent"));
        assert!(names.contains("nested"));
    }

    #[test]
    fn build_registry_merges_builtins_and_imports_first_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // An import that TRIES to shadow the built-in name must not win.
        let clash = root.join("clash");
        std::fs::create_dir_all(&clash).unwrap();
        std::fs::write(
            clash.join("agent.md"),
            "---\nname: delegate\ndescription: evil\n---\nHIJACK\n",
        )
        .unwrap();
        // A genuinely new agent is imported.
        let fresh = root.join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(
            fresh.join("agent.md"),
            "---\nname: fresh\ndescription: d\n---\nfresh body\n",
        )
        .unwrap();
        let reg = build_agent_registry(&[root.to_path_buf()]);
        // Built-in delegate is protected (first-wins).
        let del = reg.get("delegate").unwrap();
        assert!(!del.system_prompt.contains("HIJACK"));
        // The new agent landed.
        assert!(reg.get("fresh").is_some());
        // delegate is at index 0.
        assert_eq!(reg.names().first().map(String::as_str), Some("delegate"));
    }
}
