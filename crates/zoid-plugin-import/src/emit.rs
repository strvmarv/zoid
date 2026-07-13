use crate::classify::{Classification, TargetKind};
use serde_json::{json, Map, Value};

pub struct Emitted {
    pub plugin_toml: Option<String>,
    pub mcp_json: Option<String>,
    pub report: String,
}

/// Escape a string for a TOML basic (double-quoted) string per the TOML spec:
/// backslash and double-quote are escaped, and control chars use their TOML
/// escapes (\n, \t, \r) or \uXXXX. Everything else is passed through.
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
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
                id = slug(name), name = name, desc = toml_escape(description),
                repo = repo, sha = sha, subtree = subtree, loader_rel = loader_rel, strip = strip,
            ))
        }
        TargetKind::Skills => Some(format!(
            "[plugin]\nid = \"{id}\"\nschema = 1\nkind = [\"skills\"]\nname = \"{name}\"\ndescription = \"{desc}\"\n\n\
             [source]\nrepo = \"{repo}\"\nref = \"{sha}\"\nsubtree = \"{subtree}\"\n\n\
             [[install]]\neffect = \"activate\"\n",
            id = slug(name), name = name, desc = toml_escape(description),
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn emits_valid_manifest_with_backslash_and_quote_in_description() {
        let desc = r#"matches \d+ and a "quote""#;
        let e = emit("Superpowers", desc, "obra/superpowers", "SHA", "skills",
            &cls(TargetKind::Mode { loader: "skills/using-superpowers/SKILL.md".into() }), None).unwrap();
        let toml = e.plugin_toml.expect("plugin_toml should be Some");
        let m = zoid_plugin::manifest::parse_manifest(&toml).unwrap();
        m.validate().unwrap();
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
}
