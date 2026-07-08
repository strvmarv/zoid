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

/// Replace any character a provider tool name may not contain with `_`.
/// Provider tool names must match `^[A-Za-z0-9_-]{1,64}$`; a server named with
/// a space/dot in `.mcp.json` (or an over-long `server__tool`) would otherwise
/// produce a spec the provider API rejects — and since all tool specs travel in
/// one request, that would fail the *whole turn's* tool calling, not just the
/// one MCP tool.
fn sanitize_segment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn namespaced(server: &str, tool: &str) -> String {
    let mut raw = format!("{}__{}", sanitize_segment(server), sanitize_segment(tool));
    // Clamp to the provider's 64-char cap. `sanitize_segment` emits only ASCII,
    // so byte index 64 is always a char boundary and `truncate` cannot panic.
    raw.truncate(64);
    raw
}

/// A Ready server whose client has died reads as Disconnected. Test entries
/// (client == None) keep their stored state.
fn effective_state(entry: &ServerEntry) -> ServerState {
    match (entry.state, &entry.client) {
        (ServerState::Ready, Some(c)) if !c.is_alive() => ServerState::Disconnected,
        (s, _) => s,
    }
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
                // Per-server connect budget (spec §D default 10s), so a wedged
                // `initialize` can't sit in `Connecting` for the 30s request TTL.
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    Self::connect_one(&cfg),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("connect timed out")));
                match result {
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
            if effective_state(entry) != ServerState::Ready { continue; }
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

    pub fn status(&self) -> Vec<ServerStatus> {
        let st = self.inner.lock().unwrap();
        st.servers
            .iter()
            .map(|(name, e)| ServerStatus { name: name.clone(), state: effective_state(e), tool_count: e.tools.len() })
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

impl McpTool {
    /// Construct a spec-carrier for a discovered tool. Public so the agent-loop
    /// test (Task 6) can build one without a live server.
    pub fn new(namespaced: String, description: String, parameters: Value) -> McpTool {
        McpTool { namespaced, description, parameters }
    }
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
    fn namespaced_names_are_always_provider_valid() {
        fn provider_valid(s: &str) -> bool {
            !s.is_empty()
                && s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        // Ordinary names pass through unchanged.
        assert_eq!(namespaced("git", "status"), "git__status");
        // Characters the provider forbids (space, dot, slash) become '_'.
        let n = namespaced("my server.v2", "read/file");
        assert_eq!(n, "my_server_v2__read_file");
        assert!(provider_valid(&n));
        // Over-long names are clamped to the 64-char cap.
        let long = namespaced(&"s".repeat(60), &"t".repeat(60));
        assert_eq!(long.len(), 64);
        assert!(provider_valid(&long));
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
