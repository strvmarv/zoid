use crate::claude::PluginJson;

pub struct PluginTree {
    pub files: Vec<String>,
    pub mcp_json: Option<String>,
    pub plugin_json: PluginJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindPref {
    Auto,
    Mode,
    Skills,
}

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
        if segs.len() != 2 || segs[1] != "SKILL.md" {
            continue;
        }
        if is_loader_name(segs[0]) {
            return Some(f.clone());
        }
    }
    None
}

fn has_skills(files: &[String]) -> bool {
    files.iter().any(|f| {
        f.strip_prefix("skills/")
            .map(|r| {
                let s: Vec<&str> = r.split('/').collect();
                s.len() == 2 && s[1] == "SKILL.md"
            })
            .unwrap_or(false)
    })
}

fn http_servers(mcp_json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(mcp_json) else {
        return out;
    };
    // Accept bare map or { mcpServers: { ... } }.
    let map = v.get("mcpServers").unwrap_or(&v);
    if let Some(obj) = map.as_object() {
        for (name, cfg) in obj {
            let is_http = cfg.get("type").and_then(|t| t.as_str()) == Some("http")
                || (cfg.get("url").is_some() && cfg.get("command").is_none());
            if is_http {
                out.push(name.clone());
            }
        }
    }
    out
}

fn has_stdio_server(mcp_json: &str) -> bool {
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(mcp_json) else {
        return false;
    };
    let map = v.get("mcpServers").unwrap_or(&v);
    map.as_object()
        .map(|o| o.values().any(|c| c.get("command").is_some()))
        .unwrap_or(false)
}

pub fn classify(tree: &PluginTree, pref: KindPref) -> Classification {
    let mut dropped = Vec::new();
    for f in &tree.files {
        if f.starts_with("commands/") {
            dropped.push(format!("commands: {f}"));
        }
        if f.starts_with("agents/") {
            dropped.push(format!("agents: {f}"));
        }
        if f.starts_with("hooks/") || f.ends_with("hooks.json") {
            dropped.push(format!("hooks: {f}"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str]) -> PluginTree {
        PluginTree {
            files: files.iter().map(|s| s.to_string()).collect(),
            mcp_json: None,
            plugin_json: crate::claude::PluginJson { name: "p".into(), description: "d".into() },
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
}
