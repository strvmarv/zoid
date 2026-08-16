use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Write;
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

/// Outcome of a `merge_server` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    Inserted,
    SkippedExisting,
}

/// Additively merge one named stdio server into the `.mcp.json` at `path`.
/// Atomic (temp file + rename) and order-preserving. Skips an existing name
/// without writing; aborts (never clobbers) a malformed target file. `${VAR}`
/// placeholders in `server.env` are written verbatim.
pub fn merge_server(
    path: &Path,
    name: &str,
    server: &McpServerConfig,
) -> anyhow::Result<MergeOutcome> {
    use serde_json::{Map, Value};

    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} is not valid JSON: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "cannot read {}: {}",
                path.display(),
                e.kind()
            ))
        }
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} root is not a JSON object", path.display()))?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} 'mcpServers' is not a JSON object", path.display()))?;

    if servers.contains_key(name) {
        return Ok(MergeOutcome::SkippedExisting);
    }

    // Build the server object with a stable key order (command, args, env).
    let mut sv = Map::new();
    sv.insert("command".into(), Value::String(server.command.clone()));
    sv.insert(
        "args".into(),
        Value::Array(server.args.iter().cloned().map(Value::String).collect()),
    );
    let mut env = Map::new();
    for (k, v) in &server.env {
        env.insert(k.clone(), Value::String(v.clone()));
    }
    sv.insert("env".into(), Value::Object(env));
    servers.insert(name.to_string(), Value::Object(sv));

    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    // Atomic: write a temp file in the SAME directory, then rename over the target.
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("atomic rename failed: {e}"))?;
    Ok(MergeOutcome::Inserted)
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
        assert_eq!(
            cfg.args,
            vec!["-y", "@modelcontextprotocol/server-filesystem", "/src"]
        );
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
        std::fs::write(
            user.join("mcp.json"),
            r#"{"mcpServers": {"git": {"command": "user-git"}, "fs": {"command": "fs"}}}"#,
        )
        .unwrap();
        std::fs::write(
            proj.join(".mcp.json"),
            r#"{"mcpServers": {"git": {"command": "proj-git"}}}"#,
        )
        .unwrap();
        let get = |_: &str| None;
        let servers = discover(&user, &proj, &get);
        let git = servers.iter().find(|(n, _)| n == "git").unwrap();
        assert_eq!(git.1.command, "proj-git"); // project wins
        assert!(servers.iter().any(|(n, _)| n == "fs")); // user-only kept
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn cfg(cmd: &str) -> McpServerConfig {
        McpServerConfig {
            command: cmd.into(),
            args: vec!["-y".into()],
            env: BTreeMap::from([("TOKEN".to_string(), "${TOKEN}".to_string())]),
        }
    }

    #[test]
    fn inserts_into_missing_file_creating_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mcp.json");
        let out = merge_server(&path, "github", &cfg("npx")).unwrap();
        assert!(matches!(out, MergeOutcome::Inserted));
        let back = parse_mcp_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].0, "github");
        // ${VAR} written verbatim, not expanded.
        assert_eq!(back[0].1.env.get("TOKEN").unwrap(), "${TOKEN}");
    }

    #[test]
    fn preserves_siblings_and_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        // Hand-written, deliberately non-alphabetical order.
        std::fs::write(&path, "{\n  \"mcpServers\": {\n    \"zeta\": { \"command\": \"z\" },\n    \"alpha\": { \"command\": \"a\" }\n  }\n}\n").unwrap();
        merge_server(&path, "github", &cfg("npx")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // Original siblings kept in original (non-alphabetical) order; new one appended.
        let zi = text.find("zeta").unwrap();
        let ai = text.find("alpha").unwrap();
        let gi = text.find("github").unwrap();
        assert!(zi < ai && ai < gi, "order not preserved: {text}");
    }

    #[test]
    fn skips_existing_name_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(
            &path,
            "{ \"mcpServers\": { \"github\": { \"command\": \"mine\" } } }",
        )
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let out = merge_server(&path, "github", &cfg("npx")).unwrap();
        assert!(matches!(out, MergeOutcome::SkippedExisting));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "must not rewrite on skip"
        );
    }

    #[test]
    fn aborts_on_malformed_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "not json at all").unwrap();
        assert!(merge_server(&path, "github", &cfg("npx")).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "not json at all",
            "must not clobber"
        );
    }
}
