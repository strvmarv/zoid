//! zoid-mcp — a minimal MCP (Model Context Protocol) client: connects to
//! stdio MCP servers and surfaces their tools to the agent loop.
pub mod client;
pub mod config;
pub mod jsonrpc;
pub mod manager;
pub mod transport;
pub use config::McpServerConfig;
pub use manager::{McpManager, McpTool, ServerState, ServerStatus};
