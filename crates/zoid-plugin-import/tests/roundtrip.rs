use zoid_plugin_import::{classify::{classify, KindPref, PluginTree}, claude::PluginJson, emit::emit};

// NOTE: expose modules from a lib target (Step 4) so tests can import them.

#[test]
fn frontend_design_imports_as_skills_and_reparses() {
    let tree = PluginTree {
        files: vec!["skills/frontend-design/SKILL.md".into()],
        mcp_json: None,
        plugin_json: PluginJson { name: "frontend-design".into(), description: "UI".into() },
    };
    let c = classify(&tree, KindPref::Auto);
    let e = emit("frontend-design", "UI", "anthropics/claude-plugins", "SHA", "skills", &c, None).unwrap();
    let toml = e.plugin_toml.expect("skills plugin.toml");
    let m = zoid_plugin::manifest::parse_manifest(&toml).unwrap();
    m.validate().unwrap();
    assert_eq!(m.kind, vec!["skills".to_string()]);
}

#[test]
fn github_mcp_is_http_and_skipped() {
    let src = include_str!("fixtures/github-mcp/.mcp.json");
    let mut tree = PluginTree { files: vec![], mcp_json: Some(src.to_string()),
        plugin_json: PluginJson { name: "github".into(), description: "gh".into() } };
    let c = classify(&tree, KindPref::Auto);
    assert!(!c.mcp_skipped_http.is_empty());
    let e = emit("github", "gh", "anthropics/claude-plugins", "SHA", "", &c, Some(src)).unwrap();
    assert!(e.plugin_toml.is_none());
    assert!(e.mcp_json.is_none()); // only http server present → nothing to normalize
    assert!(e.report.to_lowercase().contains("http"));
    let _ = &mut tree;
}
