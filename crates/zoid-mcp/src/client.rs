use crate::jsonrpc::{self, Inbound};
use crate::transport::TransportHandle;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zoid_tools::ToolOutput;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The protocol version we advertise. We still accept a server that negotiates
/// a different *known* version — the shapes this client uses are wire-identical.
const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

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
    /// Cleared by the actor when the connection ends (EOF / crash / client
    /// dropped). Lets the manager lazily detect a disconnected server.
    alive: Arc<AtomicBool>,
}

impl McpClient {
    /// Spawn the connection actor over `handle` and return a usable client.
    pub async fn connect(handle: TransportHandle) -> McpClient {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(64);
        let alive = Arc::new(AtomicBool::new(true));
        tokio::spawn(actor(handle, cmd_rx, alive.clone()));
        McpClient { cmd_tx, next_id: AtomicU64::new(1), alive }
    }

    /// False once the connection has ended (EOF / crash / drop).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
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
        // Accept any non-empty version the server negotiates; only warn on ones
        // we haven't explicitly validated. Real servers commonly reply
        // 2024-11-05 / 2025-03-26 — refusing them would defeat the whole point.
        if negotiated.is_empty() {
            anyhow::bail!("server did not return a protocolVersion");
        }
        if !KNOWN_VERSIONS.contains(&negotiated) {
            tracing::warn!("zoid-mcp: server negotiated unrecognized protocol version {negotiated:?}; proceeding");
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
async fn actor(mut handle: TransportHandle, mut cmd_rx: mpsc::Receiver<Cmd>, alive: Arc<AtomicBool>) {
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
    // Connection ended: mark dead, then fail every in-flight request so
    // awaiters don't hang forever.
    alive.store(false, Ordering::Relaxed);
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(jsonrpc::RpcError { code: 0, message: "mcp server disconnected".into() }));
    }
}

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
        let client = McpClient::connect(TransportHandle { outbound: cli_out, inbound: cli_in, _child: None }).await;

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
        let client = McpClient::connect(TransportHandle { outbound: cli_out, inbound: cli_in, _child: None }).await;

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

    #[tokio::test]
    async fn pending_request_fails_fast_when_server_disconnects_midcall() {
        use std::time::Duration;
        let (srv_out, cli_in) = mpsc::channel::<String>(16);
        let (cli_out, mut srv_in) = mpsc::channel::<String>(16);
        let client = McpClient::connect(TransportHandle { outbound: cli_out, inbound: cli_in, _child: None }).await;

        // The server receives the request line (so it is registered as pending
        // in the actor) then disconnects WITHOUT replying by dropping its
        // channel ends — closing the client's inbound (EOF).
        let server = tokio::spawn(async move {
            let _line = srv_in.recv().await.unwrap();
            drop(srv_out); // closes the client's inbound => actor sees EOF
        });

        // The in-flight call must resolve to an error FAST — the actor drains
        // pending on EOF, so this cannot hang until the 30s REQUEST_TIMEOUT.
        let out = tokio::time::timeout(Duration::from_secs(5), client.call_tool("do", &json!({})))
            .await
            .expect("disconnect mid-call must resolve well before the 30s request timeout");
        assert!(out.is_error, "disconnect mid-call must surface an error, not a success");
        server.await.unwrap();

        // The actor sets alive=false before draining pending, so the client
        // observes the connection as dead once the call has returned.
        assert!(!client.is_alive(), "client must report not-alive after server EOF");
    }
}
