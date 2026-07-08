# MCP Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make zoid an MCP (Model Context Protocol) client so tools from user-configured stdio MCP servers appear alongside zoid's built-in tools and are callable by the model.

**Architecture:** A new dependency-light `zoid-mcp` crate hand-rolls a JSON-RPC-over-stdio client behind an `McpTransport` seam. An `McpManager` connects configured servers in the background and exposes their tools as `McpTool` spec-carriers. The agent loop gains a fourth intercepted `ToolKind::Mcp` dispatch arm that awaits the manager — reusing the exact pattern by which `Emitting`/`Interactive` tools are intercepted before the synchronous `Local` path.

**Tech Stack:** Rust, tokio (`process`, `io-util`, `sync`), serde_json, async-trait — all already in the workspace.

## Global Constraints

- **No new workspace dependencies.** Only change: add features `process` and `io-util` to the existing workspace `tokio` dependency (`Cargo.toml`).
- **Never add a `Co-Authored-By` or any co-author trailer to commit messages** (global CLAUDE.md).
- **stdio transport only, tools capability only.** No HTTP, no resources/prompts/sampling/roots.
- **Trust-on-configure:** MCP tool calls are NOT gated. `ToolGate`/`AllowAll` is unchanged; do not add approval prompts.
- **Tool namespacing:** a discovered tool `foo` on server `srv` is exposed to the model as `srv__foo` (double underscore).
- **Config format/locations:** ecosystem `.mcp.json` = `{"mcpServers": {name: {command, args, env}}}`. Read user `~/.config/zoid/mcp.json` (via existing `resolve_config_dir`) then project `./.mcp.json`; project overrides user by server name.
- **`${VAR}` expansion** in `args` and `env` values, resolved from zoid's environment; unset → empty string.
- **Never log `env` values** (they carry secrets). Log server names and tool names only.
- **Protocol version:** send `"2025-06-18"`; accept the server's negotiated version if we recognize it, else fail that server.
- **`McpManager` must implement `Debug`** (it is carried on `TurnConfig`, which `#[derive(Debug)]`) — a minimal manual impl is fine.
- A failed/crashed server must never crash zoid: it contributes zero tools and a `Failed`/`Disconnected` status.

## File Structure

- `crates/zoid-mcp/Cargo.toml` — new crate manifest + a fixture bin target for tests.
- `crates/zoid-mcp/src/lib.rs` — re-exports; `McpManager`, `McpTool`, `ServerState`, `ServerStatus`.
- `crates/zoid-mcp/src/jsonrpc.rs` — JSON-RPC 2.0 encode/classify (Task 1).
- `crates/zoid-mcp/src/config.rs` — `.mcp.json` parse/discover/expand (Task 2).
- `crates/zoid-mcp/src/transport.rs` — `McpTransport` trait, `StdioTransport`, `TransportHandle` (Task 3).
- `crates/zoid-mcp/src/client.rs` — `McpClient` connection actor + handshake/list/call (Task 4).
- `crates/zoid-mcp/src/manager.rs` — `McpManager`, `McpTool` (Task 5).
- `crates/zoid-mcp/src/bin/zoid_mcp_fake_server.rs` — hermetic fixture server (Task 7).
- `crates/zoid-tools/src/lib.rs:50` — add `ToolKind::Mcp` (Task 5).
- `crates/zoid/src/agent.rs` — `TurnConfig` field + dispatch arm (Task 6).
- `crates/zoid/src/main.rs` — startup manager + per-turn tool merge (Task 6).
- `crates/zoid-tui/src/state.rs`, `route.rs`, `render.rs` — read-only `/mcp` overlay (Task 8).
- `crates/zoid-mcp/Cargo.toml` (workspace `Cargo.toml`) — add crate to `[workspace] members`.

---

### Task 1: Scaffold `zoid-mcp` crate + JSON-RPC codec

**Files:**
- Create: `crates/zoid-mcp/Cargo.toml`
- Create: `crates/zoid-mcp/src/lib.rs`
- Create: `crates/zoid-mcp/src/jsonrpc.rs`
- Modify: `Cargo.toml` (workspace root — add member + tokio features)

**Interfaces:**
- Produces: `jsonrpc::encode_request(id: u64, method: &str, params: Option<Value>) -> String`, `jsonrpc::encode_notification(method: &str, params: Option<Value>) -> String`, `jsonrpc::encode_error_response(id: Value, code: i64, message: &str) -> String`, `jsonrpc::classify(line: &str) -> Result<jsonrpc::Inbound>`, `enum jsonrpc::Inbound { Response { id: u64, result: Result<Value, RpcError> }, ServerRequest { id: Value, method: String }, Notification { method: String } }`, `struct jsonrpc::RpcError { code: i64, message: String }`. Every encoder returns a single line with **no embedded newline**.

- [ ] **Step 1: Add the crate to the workspace and enable tokio features**

In the root `Cargo.toml`, add `"crates/zoid-mcp"` to `[workspace] members`, and change the workspace tokio line to include `process` and `io-util`:

```toml
tokio = { version = "1", features = ["macros", "rt", "rt-multi-thread", "sync", "time", "process", "io-util"] }
```

- [ ] **Step 2: Write `crates/zoid-mcp/Cargo.toml`**

```toml
[package]
name = "zoid-mcp"
version = "0.1.2"
edition = "2021"

[dependencies]
zoid-tools = { path = "../zoid-tools" }
zoid-provider = { path = "../zoid-provider" }
tokio = { workspace = true }
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 3: Write the failing test in `crates/zoid-mcp/src/jsonrpc.rs`**

```rust
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
pub enum Inbound {
    Response { id: u64, result: Result<Value, RpcError> },
    ServerRequest { id: Value, method: String },
    Notification { method: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_is_one_line_and_well_formed() {
        let line = encode_request(7, "tools/list", Some(json!({"cursor": "c1"})));
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/list");
        assert_eq!(v["params"]["cursor"], "c1");
    }

    #[test]
    fn classify_distinguishes_response_notification_and_server_request() {
        // A successful response to our request id 7.
        match classify(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).unwrap() {
            Inbound::Response { id: 7, result: Ok(v) } => assert_eq!(v["ok"], true),
            other => panic!("expected response, got {other:?}"),
        }
        // An error response.
        match classify(r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32601,"message":"nope"}}"#).unwrap() {
            Inbound::Response { id: 8, result: Err(e) } => assert_eq!(e.code, -32601),
            other => panic!("expected error response, got {other:?}"),
        }
        // A notification (no id).
        match classify(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#).unwrap() {
            Inbound::Notification { method } => assert_eq!(method, "notifications/tools/list_changed"),
            other => panic!("expected notification, got {other:?}"),
        }
        // A server->client request (id + method).
        match classify(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#).unwrap() {
            Inbound::ServerRequest { id, method } => {
                assert_eq!(id, json!("abc"));
                assert_eq!(method, "ping");
            }
            other => panic!("expected server request, got {other:?}"),
        }
    }
}
```

- [ ] **Step 4: Run it to confirm it fails to compile**

Run: `cargo test -p zoid-mcp jsonrpc`
Expected: FAIL — `encode_request` / `classify` not found.

- [ ] **Step 5: Implement the codec in `crates/zoid-mcp/src/jsonrpc.rs`** (above the `#[cfg(test)]` block)

```rust
pub fn encode_request(id: u64, method: &str, params: Option<Value>) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("jsonrpc".into(), Value::from("2.0"));
    obj.insert("id".into(), Value::from(id));
    obj.insert("method".into(), Value::from(method));
    if let Some(p) = params {
        obj.insert("params".into(), p);
    }
    Value::Object(obj).to_string() // to_string never emits newlines
}

pub fn encode_notification(method: &str, params: Option<Value>) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("jsonrpc".into(), Value::from("2.0"));
    obj.insert("method".into(), Value::from(method));
    if let Some(p) = params {
        obj.insert("params".into(), p);
    }
    Value::Object(obj).to_string()
}

pub fn encode_error_response(id: Value, code: i64, message: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

/// Classify one inbound JSON-RPC line. Responses carry our numeric `id`;
/// server-initiated requests carry an `id` + `method`; notifications carry a
/// `method` and no `id`.
pub fn classify(line: &str) -> anyhow::Result<Inbound> {
    let v: Value = serde_json::from_str(line)?;
    let has_method = v.get("method").and_then(|m| m.as_str()).is_some();
    let id = v.get("id").cloned();
    match (id, has_method) {
        (Some(id), true) => Ok(Inbound::ServerRequest {
            id,
            method: v["method"].as_str().unwrap().to_string(),
        }),
        (None, true) => Ok(Inbound::Notification {
            method: v["method"].as_str().unwrap().to_string(),
        }),
        (Some(id), false) => {
            let id = id
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("response id not a u64: {id}"))?;
            if let Some(err) = v.get("error") {
                let e: RpcError = serde_json::from_value(err.clone())?;
                Ok(Inbound::Response { id, result: Err(e) })
            } else {
                let result = v.get("result").cloned().unwrap_or(Value::Null);
                Ok(Inbound::Response { id, result: Ok(result) })
            }
        }
        (None, false) => Err(anyhow::anyhow!("malformed JSON-RPC line: {line}")),
    }
}
```

- [ ] **Step 6: Write `crates/zoid-mcp/src/lib.rs`**

```rust
//! zoid-mcp — a minimal MCP (Model Context Protocol) client: connects to
//! stdio MCP servers and surfaces their tools to the agent loop.
pub mod jsonrpc;
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p zoid-mcp jsonrpc`
Expected: PASS (2 tests).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/zoid-mcp/Cargo.toml crates/zoid-mcp/src/lib.rs crates/zoid-mcp/src/jsonrpc.rs
git commit -m "feat(mcp): scaffold zoid-mcp crate with JSON-RPC codec"
```

---

### Task 2: `.mcp.json` parsing, discovery, and `${VAR}` expansion

**Files:**
- Create: `crates/zoid-mcp/src/config.rs`
- Modify: `crates/zoid-mcp/src/lib.rs` (add `pub mod config;`)

**Interfaces:**
- Produces: `struct config::McpServerConfig { command: String, args: Vec<String>, env: std::collections::BTreeMap<String, String> }`; `config::parse_mcp_json(text: &str) -> anyhow::Result<Vec<(String, McpServerConfig)>>` (sorted by name); `config::expand_vars(s: &str, get: &dyn Fn(&str) -> Option<String>) -> String`; `config::discover(user_dir: &Path, cwd: &Path, get_env: &dyn Fn(&str) -> Option<String>) -> Vec<(String, McpServerConfig)>` (reads `user_dir/mcp.json` then `cwd/.mcp.json`, project overrides user by name, expands `${VAR}` in args + env values).

- [ ] **Step 1: Write the failing tests in `crates/zoid-mcp/src/config.rs`**

```rust
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
```

Add `tempfile` to `[dev-dependencies]` in `crates/zoid-mcp/Cargo.toml`:

```toml
tempfile = { workspace = true }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-mcp config`
Expected: FAIL — `parse_mcp_json` / `expand_vars` / `discover` not found.

- [ ] **Step 3: Implement `crates/zoid-mcp/src/config.rs`** (above the tests)

```rust
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
            Err(e) => {
                tracing::warn!("zoid-mcp: ignoring {}: {e}", path.display());
                Vec::new()
            }
        },
        Err(_) => Vec::new(), // absent file is not an error
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
```

Add `pub mod config;` to `crates/zoid-mcp/src/lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-mcp config`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-mcp/src/config.rs crates/zoid-mcp/src/lib.rs crates/zoid-mcp/Cargo.toml
git commit -m "feat(mcp): parse and discover .mcp.json with \${VAR} expansion"
```

---

### Task 3: Transport trait + `StdioTransport`

**Files:**
- Create: `crates/zoid-mcp/src/transport.rs`
- Modify: `crates/zoid-mcp/src/lib.rs` (add `pub mod transport;`)

**Interfaces:**
- Produces: `struct transport::TransportHandle { outbound: tokio::sync::mpsc::Sender<String>, inbound: tokio::sync::mpsc::Receiver<String> }`; the transport seam `trait transport::McpTransport: Send + Sync { fn connect(&self, cfg: &config::McpServerConfig) -> anyhow::Result<TransportHandle>; }`; `struct transport::StdioTransport` implementing it — spawns the child, forwards `outbound` lines to stdin (appending `\n`), reads stdout lines into `inbound` (closes it on EOF), drains stderr on its own task, and sets `kill_on_drop(true)`. `TransportHandle` is the transport-agnostic seam: the client actor is written against it, so a future `HttpTransport` implements the same trait and the client is unchanged.

- [ ] **Step 1: Write the failing test in `crates/zoid-mcp/src/transport.rs`**

```rust
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;
    use std::collections::BTreeMap;

    // `cat` echoes each stdin line back on stdout: enough to prove the
    // spawn + line-framing + read path without a real MCP server.
    #[tokio::test]
    async fn stdio_roundtrips_a_line_through_cat() {
        let cfg = McpServerConfig {
            command: "cat".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        let mut h = StdioTransport.connect(&cfg).unwrap();
        h.outbound.send(r#"{"hello":1}"#.to_string()).await.unwrap();
        let line = h.inbound.recv().await.expect("a line back");
        assert_eq!(line, r#"{"hello":1}"#);
    }

    #[tokio::test]
    async fn inbound_closes_on_child_exit() {
        let cfg = McpServerConfig {
            command: "true".into(), // exits immediately, closing stdout
            args: vec![],
            env: BTreeMap::new(),
        };
        let mut h = StdioTransport.connect(&cfg).unwrap();
        assert!(h.inbound.recv().await.is_none(), "EOF => channel closed");
    }
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-mcp transport`
Expected: FAIL — `StdioTransport` / `TransportHandle` not found.

- [ ] **Step 3: Implement `crates/zoid-mcp/src/transport.rs`**

```rust
use crate::config::McpServerConfig;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// The two line-oriented halves of a live MCP connection. The client actor
/// writes requests to `outbound` and reads server lines from `inbound`
/// (which closes when the server's stdout hits EOF).
pub struct TransportHandle {
    pub outbound: mpsc::Sender<String>,
    pub inbound: mpsc::Receiver<String>,
}

/// The transport seam. v1 ships only `StdioTransport`; a future `HttpTransport`
/// implements the same trait and returns the same `TransportHandle`, so the
/// client actor never changes.
pub trait McpTransport: Send + Sync {
    fn connect(&self, cfg: &McpServerConfig) -> anyhow::Result<TransportHandle>;
}

pub struct StdioTransport;

impl McpTransport for StdioTransport {
    /// Spawn `cfg.command` and wire its stdio into line channels. The child
    /// inherits zoid's environment, with `cfg.env` layered on top. Stderr is
    /// drained on its own task so a full pipe can't block the child.
    fn connect(&self, cfg: &McpServerConfig) -> anyhow::Result<TransportHandle> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
        let (in_tx, in_rx) = mpsc::channel::<String>(64);

        // Writer: outbound lines -> child stdin (append newline framing).
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() { break; }
                if stdin.write_all(b"\n").await.is_err() { break; }
                let _ = stdin.flush().await;
            }
        });

        // Reader: child stdout lines -> inbound channel (drops on EOF).
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if in_tx.send(line).await.is_err() { break; }
            }
            // in_tx dropped here => inbound closes.
        });

        // Stderr drain: keep the pipe empty; surface as trace diagnostics.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "zoid_mcp::server_stderr", "{line}");
            }
        });

        // Reap the child in the background when it exits.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        Ok(TransportHandle { outbound: out_tx, inbound: in_rx })
    }
}
```

Add `pub mod transport;` to `crates/zoid-mcp/src/lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-mcp transport`
Expected: PASS (2 tests on unix). On non-unix these tests are cfg-compiled out.

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-mcp/src/transport.rs crates/zoid-mcp/src/lib.rs
git commit -m "feat(mcp): stdio transport with line framing and stderr drain"
```

---

### Task 4: `McpClient` — connection actor, handshake, list, call

**Files:**
- Create: `crates/zoid-mcp/src/client.rs`
- Modify: `crates/zoid-mcp/src/lib.rs` (add `pub mod client;`)

**Interfaces:**
- Consumes: `jsonrpc::{encode_request, encode_notification, encode_error_response, classify, Inbound, RpcError}`, `transport::TransportHandle`.
- Produces: `struct client::DiscoveredTool { name: String, description: String, input_schema: Value }`; `client::McpClient` with `async fn connect(handle: TransportHandle) -> McpClient` (spawns the actor), `async fn initialize(&self) -> anyhow::Result<()>`, `async fn list_tools(&self) -> anyhow::Result<Vec<DiscoveredTool>>` (follows `nextCursor`), `async fn call_tool(&self, tool: &str, args: &Value) -> zoid_tools::ToolOutput`. Requests time out after 30s and after `initialize` fails the client is unusable. The actor answers inbound server requests with a JSON-RPC "method not found" (`-32601`) and ignores notifications, so neither can stall a pending call.

- [ ] **Step 1: Write the failing tests in `crates/zoid-mcp/src/client.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportHandle;
    use serde_json::{json, Value};
    use tokio::sync::mpsc;

    fn reply_to(line: &str, result: Value) -> String {
        let v: Value = serde_json::from_str(line).unwrap();
        json!({"jsonrpc":"2.0","id": v["id"], "result": result}).to_string()
    }

    #[tokio::test]
    async fn initialize_then_list_tools_paginates() {
        let (srv_out, cli_in) = mpsc::channel::<String>(16);
        let (cli_out, mut srv_in) = mpsc::channel::<String>(16);
        let client = McpClient::connect(TransportHandle { outbound: cli_out, inbound: cli_in }).await;

        // Drive the server side concurrently.
        let server = tokio::spawn(async move {
            // initialize
            let line = srv_in.recv().await.unwrap();
            srv_out.send(reply_to(&line, json!({"protocolVersion":"2025-06-18","capabilities":{}}))).await.unwrap();
            // the client sends notifications/initialized (no reply expected)
            let _initialized = srv_in.recv().await.unwrap();
            // tools/list page 1
            let line = srv_in.recv().await.unwrap();
            srv_out.send(reply_to(&line, json!({
                "tools":[{"name":"a","description":"A","inputSchema":{"type":"object"}}],
                "nextCursor":"p2"
            }))).await.unwrap();
            // tools/list page 2
            let line = srv_in.recv().await.unwrap();
            srv_out.send(reply_to(&line, json!({
                "tools":[{"name":"b","description":"B","inputSchema":{"type":"object"}}]
            }))).await.unwrap();
        });

        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(), vec!["a", "b"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn call_tool_maps_is_error_and_tolerates_inbound_noise() {
        let (srv_out, cli_in) = mpsc::channel::<String>(16);
        let (cli_out, mut srv_in) = mpsc::channel::<String>(16);
        let client = McpClient::connect(TransportHandle { outbound: cli_out, inbound: cli_in }).await;

        let server = tokio::spawn(async move {
            let line = srv_in.recv().await.unwrap();
            // Before replying, inject a notification and a server->client request:
            // neither must stall the pending call.
            srv_out.send(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#.to_string()).await.unwrap();
            srv_out.send(r#"{"jsonrpc":"2.0","id":"srv1","method":"ping"}"#.to_string()).await.unwrap();
            srv_out.send(reply_to(&line, json!({
                "content":[{"type":"text","text":"boom"}],
                "isError": true
            }))).await.unwrap();
        });

        let out = client.call_tool("do", &json!({"x":1})).await;
        assert!(out.is_error);
        assert_eq!(out.text, "boom");
        server.await.unwrap();
    }
}
```

(Both tests drive an in-process fake server over plain mpsc channels and call `McpClient::connect(...).await` directly — no subprocess, fully deterministic.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-mcp client`
Expected: FAIL — `McpClient` not found.

- [ ] **Step 3: Implement `crates/zoid-mcp/src/client.rs`** (above the tests)

```rust
use crate::jsonrpc::{self, Inbound};
use crate::transport::TransportHandle;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zoid_tools::ToolOutput;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

enum Cmd {
    Request { line: String, id: u64, reply: oneshot::Sender<Result<Value, jsonrpc::RpcError>> },
    Notify { line: String },
}

pub struct McpClient {
    cmd_tx: mpsc::Sender<Cmd>,
    next_id: AtomicU64,
}

impl McpClient {
    /// Spawn the connection actor over `handle` and return a usable client.
    pub async fn connect(handle: TransportHandle) -> McpClient {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(64);
        tokio::spawn(actor(handle, cmd_rx));
        McpClient { cmd_tx, next_id: AtomicU64::new(1) }
    }

    async fn request(&self, method: &str, params: Option<Value>) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let line = jsonrpc::encode_request(id, method, params);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(Cmd::Request { line, id, reply: reply_tx })
            .await
            .map_err(|_| anyhow::anyhow!("mcp connection closed"))?;
        let result = tokio::time::timeout(REQUEST_TIMEOUT, reply_rx)
            .await
            .map_err(|_| anyhow::anyhow!("mcp request '{method}' timed out"))?
            .map_err(|_| anyhow::anyhow!("mcp connection dropped"))?;
        result.map_err(|e| anyhow::anyhow!("mcp error {}: {}", e.code, e.message))
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> anyhow::Result<()> {
        let line = jsonrpc::encode_notification(method, params);
        self.cmd_tx
            .send(Cmd::Notify { line })
            .await
            .map_err(|_| anyhow::anyhow!("mcp connection closed"))
    }

    /// Perform the MCP handshake. Accepts the server's negotiated protocol
    /// version if we recognize it, else errors.
    pub async fn initialize(&self) -> anyhow::Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "zoid", "version": env!("CARGO_PKG_VERSION") }
        });
        let result = self.request("initialize", Some(params)).await?;
        let negotiated = result.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
        // v1 recognizes exactly the version we requested; anything else is a
        // server we can't speak to.
        if negotiated != PROTOCOL_VERSION {
            anyhow::bail!("unsupported MCP protocol version from server: {negotiated:?}");
        }
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    /// List every tool, following `nextCursor` pagination to completion.
    pub async fn list_tools(&self) -> anyhow::Result<Vec<DiscoveredTool>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.take().map(|c| json!({ "cursor": c }));
            let result = self.request("tools/list", params).await?;
            if let Some(arr) = result.get("tools").and_then(|t| t.as_array()) {
                for t in arr {
                    out.push(DiscoveredTool {
                        name: t.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        description: t.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"})),
                    });
                }
            }
            match result.get("nextCursor").and_then(|v| v.as_str()) {
                Some(c) if !c.is_empty() => cursor = Some(c.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Call one tool. Protocol/transport failures and `isError` both map to a
    /// `ToolOutput` (the model sees the message and can recover).
    pub async fn call_tool(&self, tool: &str, args: &Value) -> ToolOutput {
        let params = json!({ "name": tool, "arguments": args });
        match self.request("tools/call", Some(params)).await {
            Ok(result) => {
                let text = result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                if is_error { ToolOutput::err(text) } else { ToolOutput::ok(text) }
            }
            Err(e) => ToolOutput::err(format!("mcp tool '{tool}' failed: {e}")),
        }
    }
}

/// The connection actor: owns the transport halves and the pending-request map.
async fn actor(mut handle: TransportHandle, mut cmd_rx: mpsc::Receiver<Cmd>) {
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, jsonrpc::RpcError>>> = HashMap::new();
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Cmd::Request { line, id, reply }) => {
                    if handle.outbound.send(line).await.is_err() {
                        let _ = reply.send(Err(jsonrpc::RpcError { code: 0, message: "transport closed".into() }));
                    } else {
                        pending.insert(id, reply);
                    }
                }
                Some(Cmd::Notify { line }) => { let _ = handle.outbound.send(line).await; }
                None => break, // client dropped
            },
            line = handle.inbound.recv() => match line {
                Some(line) => match jsonrpc::classify(&line) {
                    Ok(Inbound::Response { id, result }) => {
                        if let Some(tx) = pending.remove(&id) { let _ = tx.send(result); }
                    }
                    Ok(Inbound::Notification { .. }) => { /* v1 ignores server notifications */ }
                    Ok(Inbound::ServerRequest { id, method }) => {
                        // We advertise no server-callable capabilities; refuse cleanly.
                        let resp = jsonrpc::encode_error_response(id, -32601, &format!("method not supported: {method}"));
                        let _ = handle.outbound.send(resp).await;
                    }
                    Err(e) => tracing::warn!("zoid-mcp: unparseable line: {e}"),
                },
                None => break, // server closed stdout (EOF / crash)
            },
        }
    }
    // Fail every in-flight request so awaiters don't hang forever.
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(jsonrpc::RpcError { code: 0, message: "mcp server disconnected".into() }));
    }
}
```

Add `pub mod client;` to `crates/zoid-mcp/src/lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p zoid-mcp client`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/zoid-mcp/src/client.rs crates/zoid-mcp/src/lib.rs
git commit -m "feat(mcp): connection actor with handshake, paginated list, call"
```

---

### Task 5: `ToolKind::Mcp`, `McpTool`, and `McpManager`

**Files:**
- Modify: `crates/zoid-tools/src/lib.rs:50` (add `Mcp` variant)
- Create: `crates/zoid-mcp/src/manager.rs`
- Modify: `crates/zoid-mcp/src/lib.rs` (add `pub mod manager;` + re-exports)

**Interfaces:**
- Consumes: `client::{McpClient, DiscoveredTool}`, `config::McpServerConfig`, `transport::StdioTransport`, `zoid_tools::{Tool, ToolKind, ToolOutput}`, `zoid_provider::ToolSpec`.
- Produces: `enum manager::ServerState { Connecting, Ready, Failed, Disconnected }`; `struct manager::ServerStatus { name: String, state: ServerState, tool_count: usize }`; `struct manager::McpManager` (impl `Debug`) with `fn new() -> McpManager`, `fn spawn_connect_all(self: &std::sync::Arc<McpManager>, servers: Vec<(String, McpServerConfig)>)`, `fn mcp_tools(&self) -> Vec<Box<dyn Tool>>` (namespaced `server__tool`), `async fn call_tool(&self, namespaced: &str, args: &Value) -> ToolOutput`, `fn status(&self) -> Vec<ServerStatus>`; `struct manager::McpTool` implementing `Tool` with `kind() == ToolKind::Mcp`.
- Namespacing/routing uses a `routes: BTreeMap<String, (String, String)>` (`"srv__foo" -> ("srv","foo")`) built at discovery, so `call_tool` never parses names.

- [ ] **Step 1: Add the enum variant (no behavior yet)**

In `crates/zoid-tools/src/lib.rs`, extend `ToolKind`:

```rust
pub enum ToolKind {
    Local,
    Emitting,
    Interactive,
    /// Routed to an MCP server over async I/O; intercepted by the agent loop
    /// before the synchronous path, so `run()` is never called (like Emitting).
    Mcp,
}
```

- [ ] **Step 2: Write the failing tests in `crates/zoid-mcp/src/manager.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DiscoveredTool;
    use serde_json::json;

    fn ready_entry(tools: &[&str]) -> ServerEntry {
        ServerEntry {
            state: ServerState::Ready,
            client: None, // routing/spec tests don't need a live client
            tools: tools
                .iter()
                .map(|n| DiscoveredTool {
                    name: n.to_string(),
                    description: format!("{n} desc"),
                    input_schema: json!({"type": "object"}),
                })
                .collect(),
        }
    }

    #[test]
    fn mcp_tools_are_namespaced_and_collisions_disambiguated() {
        let m = McpManager::new();
        m.insert_for_test("a", ready_entry(&["search"]));
        m.insert_for_test("b", ready_entry(&["search"]));
        let names: Vec<String> = m.mcp_tools().iter().map(|t| t.name().to_string()).collect();
        assert!(names.contains(&"a__search".to_string()));
        assert!(names.contains(&"b__search".to_string()));
    }

    #[test]
    fn status_reports_state_and_tool_count() {
        let m = McpManager::new();
        m.insert_for_test("a", ready_entry(&["x", "y"]));
        let s = m.status();
        let a = s.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a.tool_count, 2);
        assert_eq!(a.state, ServerState::Ready);
    }

    #[tokio::test]
    async fn call_unknown_route_is_error_not_panic() {
        let m = McpManager::new();
        let out = m.call_tool("ghost__tool", &json!({})).await;
        assert!(out.is_error);
        assert!(out.text.contains("unknown"));
    }
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cargo test -p zoid-mcp manager`
Expected: FAIL — `McpManager` not found.

- [ ] **Step 4: Implement `crates/zoid-mcp/src/manager.rs`** (above the tests)

```rust
use crate::client::{DiscoveredTool, McpClient};
use crate::config::McpServerConfig;
use crate::transport::StdioTransport;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use zoid_provider::ToolSpec;
use zoid_tools::{Tool, ToolKind, ToolOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState { Connecting, Ready, Failed, Disconnected }

#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub name: String,
    pub state: ServerState,
    pub tool_count: usize,
}

pub(crate) struct ServerEntry {
    pub(crate) state: ServerState,
    pub(crate) client: Option<Arc<McpClient>>,
    pub(crate) tools: Vec<DiscoveredTool>,
}

#[derive(Default)]
struct ManagerState {
    servers: BTreeMap<String, ServerEntry>,
    /// "srv__tool" -> (server, tool)
    routes: BTreeMap<String, (String, String)>,
}

pub struct McpManager {
    inner: Mutex<ManagerState>,
}

impl std::fmt::Debug for McpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpManager")
    }
}

fn namespaced(server: &str, tool: &str) -> String {
    format!("{server}__{tool}")
}

impl McpManager {
    pub fn new() -> McpManager {
        McpManager { inner: Mutex::new(ManagerState::default()) }
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, name: &str, entry: ServerEntry) {
        let mut st = self.inner.lock().unwrap();
        for t in &entry.tools {
            st.routes.insert(namespaced(name, &t.name), (name.to_string(), t.name.clone()));
        }
        st.servers.insert(name.to_string(), entry);
    }

    /// Kick off a background connect task per server. Returns immediately;
    /// tools appear as each server finishes initialize + tools/list.
    pub fn spawn_connect_all(self: &Arc<Self>, servers: Vec<(String, McpServerConfig)>) {
        for (name, cfg) in servers {
            {
                let mut st = self.inner.lock().unwrap();
                st.servers.insert(name.clone(), ServerEntry {
                    state: ServerState::Connecting,
                    client: None,
                    tools: Vec::new(),
                });
            }
            let this = Arc::clone(self);
            tokio::spawn(async move {
                match Self::connect_one(&cfg).await {
                    Ok((client, tools)) => {
                        let mut st = this.inner.lock().unwrap();
                        for t in &tools {
                            st.routes.insert(namespaced(&name, &t.name), (name.clone(), t.name.clone()));
                        }
                        if let Some(e) = st.servers.get_mut(&name) {
                            e.state = ServerState::Ready;
                            e.client = Some(Arc::new(client));
                            e.tools = tools;
                        }
                        tracing::info!("zoid-mcp: server '{name}' ready ({} tools)", st.servers[&name].tools.len());
                    }
                    Err(e) => {
                        let mut st = this.inner.lock().unwrap();
                        if let Some(entry) = st.servers.get_mut(&name) { entry.state = ServerState::Failed; }
                        tracing::warn!("zoid-mcp: server '{name}' failed to start: {e}");
                    }
                }
            });
        }
    }

    async fn connect_one(cfg: &McpServerConfig) -> anyhow::Result<(McpClient, Vec<DiscoveredTool>)> {
        use crate::transport::McpTransport;
        let handle = StdioTransport.connect(cfg)?;
        let client = McpClient::connect(handle).await;
        client.initialize().await?;
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    /// Snapshot the ready tools as `Box<dyn Tool>` spec-carriers.
    pub fn mcp_tools(&self) -> Vec<Box<dyn Tool>> {
        let st = self.inner.lock().unwrap();
        let mut out: Vec<Box<dyn Tool>> = Vec::new();
        for (name, entry) in &st.servers {
            if entry.state != ServerState::Ready { continue; }
            for t in &entry.tools {
                out.push(Box::new(McpTool {
                    namespaced: namespaced(name, &t.name),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                }));
            }
        }
        out
    }

    /// Route a namespaced call to its server's client. Never holds the lock
    /// across the await.
    pub async fn call_tool(&self, namespaced_name: &str, args: &Value) -> ToolOutput {
        let (server, tool, client) = {
            let st = self.inner.lock().unwrap();
            match st.routes.get(namespaced_name) {
                Some((s, t)) => {
                    let client = st.servers.get(s).and_then(|e| e.client.clone());
                    (s.clone(), t.clone(), client)
                }
                None => return ToolOutput::err(format!("unknown mcp tool: {namespaced_name}")),
            }
        };
        match client {
            Some(c) => c.call_tool(&tool, args).await,
            None => ToolOutput::err(format!("mcp server '{server}' is not connected")),
        }
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        let st = self.inner.lock().unwrap();
        st.servers
            .iter()
            .map(|(name, e)| ServerStatus { name: name.clone(), state: e.state, tool_count: e.tools.len() })
            .collect()
    }
}

/// A discovered MCP tool presented to the model. A pure spec-carrier: the agent
/// loop intercepts `ToolKind::Mcp` and routes execution through `McpManager`,
/// so `run()` is never called.
pub struct McpTool {
    namespaced: String,
    description: String,
    parameters: Value,
}

impl Tool for McpTool {
    fn name(&self) -> &str { &self.namespaced }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.namespaced.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }
    fn run(&self, _args: &Value, _cwd: &Path) -> ToolOutput {
        // Unreachable: Mcp-kind tools are intercepted before the sync path.
        ToolOutput::err("internal: MCP tool run() called directly")
    }
    fn kind(&self) -> ToolKind { ToolKind::Mcp }
}
```

Update `crates/zoid-mcp/src/lib.rs`:

```rust
pub mod manager;
pub use config::McpServerConfig;
pub use manager::{McpManager, McpTool, ServerState, ServerStatus};
```

- [ ] **Step 5: Run to verify pass (and confirm zoid-tools still builds)**

Run: `cargo test -p zoid-mcp manager && cargo test -p zoid-tools`
Expected: PASS. `zoid-tools` existing tests still pass (adding an enum variant is non-breaking; no existing `match` is exhaustive over `ToolKind` without a wildcard — the agent loop uses `_`).

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-tools/src/lib.rs crates/zoid-mcp/src/manager.rs crates/zoid-mcp/src/lib.rs
git commit -m "feat(mcp): McpManager, McpTool, and ToolKind::Mcp"
```

---

### Task 6: Wire MCP into the agent loop and the binary

**Files:**
- Modify: `crates/zoid/Cargo.toml` (add `zoid-mcp` dep)
- Modify: `crates/zoid/src/agent.rs` (`TurnConfig` field + dispatch arm + constructor)
- Modify: `crates/zoid/src/subagent.rs:149` (set `mcp: None`)
- Modify: `crates/zoid/src/main.rs` (startup manager + per-turn tool merge)

**Interfaces:**
- Consumes: `zoid_mcp::McpManager`, `zoid_tools::ToolKind::Mcp`.
- Produces: `TurnConfig.mcp: Option<std::sync::Arc<zoid_mcp::McpManager>>`; the `Some(ToolKind::Mcp)` dispatch arm; startup construction of the manager on the bin's app state.

- [ ] **Step 1: Add the dependency**

In `crates/zoid/Cargo.toml`, under `[dependencies]`:

```toml
zoid-mcp = { path = "../zoid-mcp" }
```

- [ ] **Step 2: Write the failing test in `crates/zoid/src/agent.rs`** (in the existing `#[cfg(test)] mod tests`)

```rust
#[tokio::test]
async fn mcp_kind_tool_routes_to_manager_and_errors_cleanly() {
    // A manager with a configured-but-unconnected server: calling its tool
    // must surface a ToolOutput error (never panic, never hit the Local path).
    let mgr = std::sync::Arc::new(zoid_mcp::McpManager::new());
    // No servers connected => any mcp tool name is unknown.
    let out = mgr.call_tool("srv__thing", &serde_json::json!({})).await;
    assert!(out.is_error);
    assert!(out.text.contains("unknown mcp tool"));
}
```

This asserts the routing contract the dispatch arm relies on. **Additionally**, add a turn-loop test that genuinely exercises the new arm: model the harness on the existing `run_agent_turn` tests in `crates/zoid/src/agent.rs` (near line 1712) that drive a fake `Provider`. Build a fake provider that emits one `ProviderEvent::ToolCall { name: "srv__missing", .. }` then `Done`; construct a `TurnConfig` with `mcp: Some(Arc::new(McpManager::new()))` and a `tools` vec containing an `McpTool`-kind entry named `srv__missing` (so the `kind` lookup returns `Mcp`); run the turn; assert the produced events contain a `ToolResult` with `is_error == true` and text mentioning `unknown mcp tool`. This proves the `Some(ToolKind::Mcp)` arm is taken (not the `_` Local path) and never panics.

- [ ] **Step 3: Run to confirm it compiles/fails appropriately**

Run: `cargo test -p zoid mcp_kind_tool_routes -- --nocapture`
Expected: FAIL to compile until `zoid-mcp` is a dependency (Step 1) — then PASS once the dep is added. (This test only exercises `zoid-mcp`; Steps 4-6 add the wiring the integration test needs.)

- [ ] **Step 4: Add the `TurnConfig` field**

In `crates/zoid/src/agent.rs`, add to `struct TurnConfig` (after `eviction`):

```rust
    /// Connected MCP servers whose tools this turn may call. `None` for
    /// subagents and tests (no MCP). Carried here (not as a fn parameter) so
    /// the turn-function signatures are unchanged.
    pub mcp: Option<std::sync::Arc<zoid_mcp::McpManager>>,
```

Set it in `chat_turn_config_with` (the `TurnConfig { .. }` literal): add `mcp: None,`. Then in `crates/zoid/src/subagent.rs` at the `TurnConfig {` literal (~line 149), add `mcp: None,`.

- [ ] **Step 5: Add the dispatch arm**

In `crates/zoid/src/agent.rs`, in the `match kind { .. }` block, add this arm immediately **before** the final `_ =>` (Local) arm:

```rust
                Some(zoid_tools::ToolKind::Mcp) => {
                    let _ = ui.send(AgentUpdate::ToolStarted { name: tc.name.clone() }).await;
                    let out = match config.mcp.as_ref() {
                        Some(m) => m.call_tool(&tc.name, &tc.args).await,
                        None => zoid_tools::ToolOutput::err(format!(
                            "mcp tool '{}' requested but no MCP manager is active",
                            tc.name
                        )),
                    };
                    let tool_ok = !out.is_error;
                    let tool_fail_msg = out.is_error.then(|| out.text.clone());
                    emit(
                        &session,
                        &mut events,
                        ui,
                        &config.branch,
                        EventKind::ToolResult {
                            id: tc.id,
                            name: tc.name,
                            output: out.text,
                            is_error: out.is_error,
                        },
                        session_id,
                        now,
                    )
                    .await?;
                    tracing::info!(
                        kind = "tool",
                        name = tool_name.as_str(),
                        ms = tool_start.elapsed().as_millis() as u64,
                        ok = tool_ok,
                        "tool executed"
                    );
                    if let Some(msg) = tool_fail_msg {
                        let ctx = format!("tool {tool_name}");
                        tracing::warn!(ctx = ctx.as_str(), message = msg.as_str(), "tool failed");
                    }
                }
```

- [ ] **Step 6: Wire the binary startup + per-turn merge in `crates/zoid/src/main.rs`**

(a) Add an app-state field (find the app/`App` struct that owns `companion_hub`, `skills`, etc.) — add:

```rust
    /// Background MCP manager (None if no servers are configured). Its tools are
    /// merged into the Chat tool set each turn.
    mcp: Option<std::sync::Arc<zoid_mcp::McpManager>>,
```

(b) During startup (near where `load_config()` / `resolve_config_dir` run and the app is assembled), construct the manager and start background connects:

```rust
    let mcp = {
        let cfg_dir = resolve_config_dir(|k| std::env::var(k).ok());
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let servers = zoid_mcp::config::discover(&cfg_dir, &cwd, &|k| std::env::var(k).ok());
        if servers.is_empty() {
            None
        } else {
            let m = std::sync::Arc::new(zoid_mcp::McpManager::new());
            m.spawn_connect_all(servers);
            Some(m)
        }
    };
    // ...assign `mcp` into the app struct where the other fields are set.
```

(c) In the turn-dispatch block (`crates/zoid/src/main.rs:4272`-`4281`), after the existing tool assembly and before `let tools = std::sync::Arc::new(tools);`, merge MCP tools; and after `turn_config` is built, attach the manager:

```rust
    if let Some(m) = &app.mcp {
        tools.extend(m.mcp_tools());
    }
    let tools = std::sync::Arc::new(tools);
    let mut turn_config = zoid::agent::chat_turn_config_with(&profile, &menu);
    turn_config.mcp = app.mcp.clone();
    // ...existing policy/eviction assignments unchanged...
```

- [ ] **Step 7: Build and run the focused test + the whole zoid crate**

Run: `cargo test -p zoid mcp_kind_tool_routes && cargo build -p zoid`
Expected: PASS + clean build. (`cargo build` proves the `TurnConfig` field, all its constructors, and the dispatch arm compile together.)

- [ ] **Step 8: Commit**

```bash
git add crates/zoid/Cargo.toml crates/zoid/src/agent.rs crates/zoid/src/subagent.rs crates/zoid/src/main.rs
git commit -m "feat(mcp): route ToolKind::Mcp in the turn loop; wire manager into the binary"
```

---

### Task 7: Hermetic end-to-end integration (fixture server)

**Files:**
- Create: `crates/zoid-mcp/src/bin/zoid_mcp_fake_server.rs`
- Modify: `crates/zoid-mcp/Cargo.toml` (declare the `[[bin]]`)
- Create: `crates/zoid-mcp/tests/end_to_end.rs`

**Interfaces:**
- Consumes: `zoid_mcp::{McpManager, McpServerConfig}` (via `zoid_mcp::config::McpServerConfig`), `env!("CARGO_BIN_EXE_zoid_mcp_fake_server")`.
- The fixture speaks stdio JSON-RPC: `initialize` → returns `protocolVersion: "2025-06-18"`; `tools/list` → one tool `echo` (single page); `tools/call` for `echo` → returns the `arguments` echoed as text; `tools/call` for `crash` → the process exits (simulating a mid-call crash). It ignores `notifications/initialized`.

- [ ] **Step 1: Declare the fixture bin in `crates/zoid-mcp/Cargo.toml`**

```toml
[[bin]]
name = "zoid_mcp_fake_server"
path = "src/bin/zoid_mcp_fake_server.rs"
```

Dependency bins are NOT compiled into `zoid` (only `zoid-mcp`'s lib is), so this fixture never ships in the product binary; it is built only for `zoid-mcp`'s own tests.

- [ ] **Step 2: Write the fixture `crates/zoid-mcp/src/bin/zoid_mcp_fake_server.rs`**

```rust
//! A minimal MCP stdio server used only by zoid-mcp integration tests.
use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn reply(id: &Value, result: Value) -> String {
    json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }
        let v: Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned();
        match (method, id) {
            ("initialize", Some(id)) => {
                let out = reply(&id, json!({"protocolVersion":"2025-06-18","capabilities":{}}));
                writeln!(stdout, "{out}").unwrap();
            }
            ("notifications/initialized", _) => { /* no reply */ }
            ("tools/list", Some(id)) => {
                let out = reply(&id, json!({"tools":[{
                    "name":"echo","description":"echoes arguments",
                    "inputSchema":{"type":"object"}
                }]}));
                writeln!(stdout, "{out}").unwrap();
            }
            ("tools/call", Some(id)) => {
                let name = v.get("params").and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                if name == "crash" { std::process::exit(1); } // mid-call crash
                let args = v.get("params").and_then(|p| p.get("arguments")).cloned().unwrap_or(json!({}));
                let out = reply(&id, json!({
                    "content":[{"type":"text","text": args.to_string()}],
                    "isError": false
                }));
                writeln!(stdout, "{out}").unwrap();
            }
            _ => {}
        }
        stdout.flush().unwrap();
    }
}
```

- [ ] **Step 3: Write the failing integration test `crates/zoid-mcp/tests/end_to_end.rs`**

```rust
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use zoid_mcp::config::McpServerConfig;
use zoid_mcp::{McpManager, ServerState};

fn fixture_cfg() -> (String, McpServerConfig) {
    (
        "fake".to_string(),
        McpServerConfig {
            command: env!("CARGO_BIN_EXE_zoid_mcp_fake_server").to_string(),
            args: vec![],
            env: BTreeMap::new(),
        },
    )
}

async fn wait_ready(m: &McpManager) {
    for _ in 0..50 {
        if m.status().iter().any(|s| s.name == "fake" && s.state == ServerState::Ready) { return; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("fixture server never became ready: {:?}", m.status());
}

#[tokio::test]
async fn discovers_and_calls_a_real_stdio_server() {
    let m = Arc::new(McpManager::new());
    m.spawn_connect_all(vec![fixture_cfg()]);
    wait_ready(&m).await;

    // The echo tool is discovered under its namespaced name.
    let names: Vec<String> = m.mcp_tools().iter().map(|t| t.name().to_string()).collect();
    assert!(names.contains(&"fake__echo".to_string()), "got {names:?}");

    // A round-trip call echoes the arguments back.
    let out = m.call_tool("fake__echo", &json!({"hi": "there"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("there"));
}

#[tokio::test]
async fn crash_mid_call_is_a_clean_error() {
    let m = Arc::new(McpManager::new());
    m.spawn_connect_all(vec![fixture_cfg()]);
    wait_ready(&m).await;

    // The `crash` tool makes the server exit during the call: we must get a
    // ToolOutput error, not a hang or panic.
    let out = m.call_tool("fake__echo", &json!({})).await; // warm-up (proves alive)
    assert!(!out.is_error, "{}", out.text);
    // Route a call to the crash tool by name through the same server.
    let crash = m.call_tool_direct_for_test("fake", "crash", &json!({})).await;
    assert!(crash.is_error);
}
```

Because `call_tool` only accepts namespaced names discovered via `tools/list` (and `crash` is not advertised), add a tiny test-only helper on `McpManager` in `crates/zoid-mcp/src/manager.rs`:

```rust
    /// Test-only: call an arbitrary tool name on a named server, bypassing the
    /// discovered-route table (used to exercise the crash path).
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn call_tool_direct_for_test(&self, server: &str, tool: &str, args: &Value) -> ToolOutput {
        let client = { self.inner.lock().unwrap().servers.get(server).and_then(|e| e.client.clone()) };
        match client {
            Some(c) => c.call_tool(tool, args).await,
            None => ToolOutput::err("server not connected"),
        }
    }
```

Add a `test-helpers` feature to `crates/zoid-mcp/Cargo.toml` so the integration test (a separate crate) can reach it:

```toml
[features]
test-helpers = []
```

And run the integration test with it enabled (Step 5).

- [ ] **Step 4: Run to confirm failure**

Run: `cargo test -p zoid-mcp --features test-helpers --test end_to_end`
Expected: FAIL — fixture bin / helper not yet built (until Steps 1-3 are all in).

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p zoid-mcp --features test-helpers --test end_to_end`
Expected: PASS (2 tests) — discovery + round-trip, and crash-mid-call → clean error.

- [ ] **Step 6: Commit**

```bash
git add crates/zoid-mcp/Cargo.toml crates/zoid-mcp/src/bin/zoid_mcp_fake_server.rs crates/zoid-mcp/src/manager.rs crates/zoid-mcp/tests/end_to_end.rs
git commit -m "test(mcp): hermetic end-to-end against an in-repo fixture server"
```

---

### Task 8: Read-only `/mcp` status overlay (TUI)

**Files:**
- Modify: `crates/zoid-tui/src/state.rs` (add `Overlay::Mcp` + a `mcp_status: Vec<McpStatusRow>` field + the `McpStatusRow` type)
- Modify: `crates/zoid-tui/src/route.rs` (route `Overlay::Mcp` keys: Esc closes)
- Modify: `crates/zoid-tui/src/render.rs` (render the overlay)
- Modify: `crates/zoid/src/main.rs` (sync `manager.status()` into `state.mcp_status`; open the overlay from the palette)

**Interfaces:**
- Consumes: `zoid_mcp::{ServerStatus, ServerState}` (mapped into the TUI's own `McpStatusRow` — zoid-tui does NOT depend on zoid-mcp).
- Produces: `struct state::McpStatusRow { name: String, state: String, tool_count: usize }`; `Overlay::Mcp`; `route::route_mcp_key`.

- [ ] **Step 1: Write the failing route test in `crates/zoid-tui/src/route.rs`** (in the existing tests module)

```rust
#[test]
fn esc_closes_the_mcp_overlay() {
    let mut s = crate::state::State::default();
    s.overlay = Overlay::Mcp;
    let _ = route_key(&mut s, key(KeyCode::Esc));
    assert_eq!(s.overlay, Overlay::None);
}
```

(Match the crate's existing test helpers `route_key` / `key(..)` — see the neighboring `cancel_does_not_pre_empt_an_open_overlay` test for the exact constructors.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -p zoid-tui esc_closes_the_mcp_overlay`
Expected: FAIL — `Overlay::Mcp` does not exist.

- [ ] **Step 3: Add the state**

In `crates/zoid-tui/src/state.rs`, add `Mcp` to `enum Overlay`:

```rust
pub enum Overlay {
    None,
    Palette,
    Objects,
    Verbs,
    Sessions,
    Config,
    ProviderSwitch,
    Mcp,
}
```

Add the row type and a `State` field (place the field next to the other overlay-backing collections; initialize to empty in `Default`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStatusRow {
    pub name: String,
    pub state: String, // "connecting" | "ready" | "failed" | "disconnected"
    pub tool_count: usize,
}
```

```rust
    /// Read-only snapshot of MCP servers, refreshed by the bin each tick.
    pub mcp_status: Vec<McpStatusRow>,
```

- [ ] **Step 4: Route the overlay in `crates/zoid-tui/src/route.rs`**

Add to the overlay dispatch (next to `Overlay::Sessions => ...`):

```rust
        Overlay::Mcp => return route_mcp_key(state, key),
```

And the handler (Esc/`q` close; it's read-only so nothing else mutates):

```rust
fn route_mcp_key(state: &mut crate::state::State, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            state.overlay = crate::state::Overlay::None;
            Action::Redraw
        }
        _ => Action::None,
    }
}
```

(Use the crate's actual `Action` variants — mirror what `route_sessions_key` returns for close/redraw/no-op.)

- [ ] **Step 5: Render it in `crates/zoid-tui/src/render.rs`**

Add a branch where the other overlays render (near the `Overlay::Config` render call), drawing a titled list. Each row: `name  state  (N tools)`. Provide a helper and a render test:

```rust
#[cfg(test)]
#[test]
fn mcp_overlay_lists_servers() {
    let mut state = crate::state::State::default();
    state.overlay = crate::state::Overlay::Mcp;
    state.mcp_status = vec![
        crate::state::McpStatusRow { name: "filesystem".into(), state: "ready".into(), tool_count: 3 },
        crate::state::McpStatusRow { name: "git".into(), state: "failed".into(), tool_count: 0 },
    ];
    let buf = render_to_test_buffer(&state); // use the crate's existing test render helper
    let text = buffer_text(&buf);            // ditto
    assert!(text.contains("filesystem"));
    assert!(text.contains("ready"));
    assert!(text.contains("git"));
    assert!(text.contains("failed"));
}
```

(Wire `render_to_test_buffer` / `buffer_text` to whatever the crate already uses for render tests — grep for an existing `render` unit test in `render.rs` and reuse its buffer harness.)

- [ ] **Step 6: Sync status + open from the palette in `crates/zoid/src/main.rs`**

Where the bin already refreshes live TUI state each tick (the branch/worktree poll added in recent commits), map the manager status in:

```rust
    if let Some(m) = &app.mcp {
        app.tui.mcp_status = m
            .status()
            .into_iter()
            .map(|s| zoid_tui::state::McpStatusRow {
                name: s.name,
                state: match s.state {
                    zoid_mcp::ServerState::Connecting => "connecting",
                    zoid_mcp::ServerState::Ready => "ready",
                    zoid_mcp::ServerState::Failed => "failed",
                    zoid_mcp::ServerState::Disconnected => "disconnected",
                }
                .to_string(),
                tool_count: s.tool_count,
            })
            .collect();
    }
```

Add a command-palette entry "MCP servers" that sets `state.overlay = Overlay::Mcp` (mirror how the palette opens `Overlay::Sessions`/`Overlay::Config` — see `route.rs:472`).

- [ ] **Step 7: Run the TUI tests**

Run: `cargo test -p zoid-tui mcp`
Expected: PASS (route + render tests). Then `cargo build -p zoid` to confirm the bin sync compiles.

- [ ] **Step 8: Commit**

```bash
git add crates/zoid-tui/src/state.rs crates/zoid-tui/src/route.rs crates/zoid-tui/src/render.rs crates/zoid/src/main.rs
git commit -m "feat(mcp): read-only /mcp server status overlay"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` (whole workspace) — clean.
- [ ] `cargo test` (whole workspace) — green, including `zoid-mcp --features test-helpers`.
- [ ] Manual smoke (fish-safe; wrap env-y commands in `bash -c`): create a `./.mcp.json` pointing at a real server (e.g. `npx -y @modelcontextprotocol/server-filesystem .`), launch zoid, open `/mcp`, confirm the server shows `ready` with a tool count, and that the model can call `filesystem__read_file`.
