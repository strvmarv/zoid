use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: BTreeMap<String, RawServer>,
}

#[derive(Deserialize)]
struct RawServer {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Parse a `.mcp.json` document into `(name, config)` pairs sorted by name.
pub fn parse_mcp_json(text: &str) -> anyhow::Result<Vec<(String, McpServerConfig)>> {
    let raw: RawFile = serde_json::from_str(text)?;
    Ok(raw
        .mcp_servers
        .into_iter()
        .map(|(name, s)| {
            (
                name,
                McpServerConfig {
                    command: s.command,
                    args: s.args,
                    env: s.env,
                },
            )
        })
        .collect())
}

/// Expand `${VAR}` occurrences using `get`. Unset variables expand to "".
/// UTF-8-safe: slices only on `find`-returned char boundaries.
pub fn expand_vars(s: &str, get: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("${") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        if let Some(end) = after.find('}') {
            out.push_str(&get(&after[..end]).unwrap_or_default());
            rest = &after[end + 1..];
        } else {
            // Unterminated `${` — emit it literally and continue past it.
            out.push_str("${");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn expand_cfg(mut cfg: McpServerConfig, get: &dyn Fn(&str) -> Option<String>) -> McpServerConfig {
    cfg.args = cfg.args.iter().map(|a| expand_vars(a, get)).collect();
    cfg.env = cfg
        .env
        .into_iter()
        .map(|(k, v)| (k, expand_vars(&v, get)))
        .collect();
    cfg
}

fn read_file(path: &Path) -> Vec<(String, McpServerConfig)> {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse_mcp_json(&text) {
            Ok(servers) => servers,
            Err(_) => {
                // Never forward the serde error text: on a type mismatch it
                // echoes the offending JSON scalar verbatim, which can be an
                // `env` value (a secret). Log the path only.
                tracing::warn!("zoid-mcp: ignoring {} (invalid config)", path.display());
                Vec::new()
            }
        },
        // A missing file is not an error; other IO failures (permissions,
        // is-a-directory) are worth a breadcrumb — log the kind only, never
        // the contents.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::debug!("zoid-mcp: cannot read {}: {}", path.display(), e.kind());
            Vec::new()
        }
    }
}

/// Discover servers from `user_dir/mcp.json` then `cwd/.mcp.json`; project
/// entries override user entries with the same name. `${VAR}` is expanded in
/// args and env values from `get_env`.
pub fn discover(
    user_dir: &Path,
    cwd: &Path,
    get_env: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, McpServerConfig)> {
    let mut merged: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    for (name, cfg) in read_file(&user_dir.join("mcp.json")) {
        merged.insert(name, cfg);
    }
    for (name, cfg) in read_file(&cwd.join(".mcp.json")) {
        merged.insert(name, cfg); // project overrides
    }
    merged
        .into_iter()
        .map(|(name, cfg)| (name, expand_cfg(cfg, get_env)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ecosystem_shape() {
        let text = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/src"],
                    "env": { "TOKEN": "abc" }
                }
            }
        }"#;
        let servers = parse_mcp_json(text).unwrap();
        assert_eq!(servers.len(), 1);
        let (name, cfg) = &servers[0];
        assert_eq!(name, "filesystem");
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@modelcontextprotocol/server-filesystem", "/src"]);
        assert_eq!(cfg.env.get("TOKEN").unwrap(), "abc");
    }

    #[test]
    fn missing_or_empty_args_env_default() {
        let text = r#"{"mcpServers": {"x": {"command": "run"}}}"#;
        let servers = parse_mcp_json(text).unwrap();
        assert!(servers[0].1.args.is_empty());
        assert!(servers[0].1.env.is_empty());
    }

    #[test]
    fn expands_dollar_brace_vars() {
        let get = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        assert_eq!(expand_vars("${HOME}/x", &get), "/home/u/x");
        // Unset variable expands to empty.
        assert_eq!(expand_vars("a${NOPE}b", &get), "ab");
        // A literal with no vars is unchanged.
        assert_eq!(expand_vars("plain", &get), "plain");
    }

    #[test]
    fn expand_vars_is_utf8_safe_and_tolerates_unterminated() {
        // Multi-byte text on both sides of the marker, and a non-ASCII
        // replacement value: slicing must land on char boundaries, never panic.
        assert_eq!(expand_vars("€${X}日本", &|_| Some("→".into())), "€→日本");
        // An unterminated `${` is emitted literally, no panic.
        assert_eq!(expand_vars("a${unterminated", &|_| None), "a${unterminated");
    }

    #[test]
    fn project_overrides_user_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(user.join("mcp.json"),
            r#"{"mcpServers": {"git": {"command": "user-git"}, "fs": {"command": "fs"}}}"#).unwrap();
        std::fs::write(proj.join(".mcp.json"),
            r#"{"mcpServers": {"git": {"command": "proj-git"}}}"#).unwrap();
        let get = |_: &str| None;
        let servers = discover(&user, &proj, &get);
        let git = servers.iter().find(|(n, _)| n == "git").unwrap();
        assert_eq!(git.1.command, "proj-git"); // project wins
        assert!(servers.iter().any(|(n, _)| n == "fs")); // user-only kept
    }
}
