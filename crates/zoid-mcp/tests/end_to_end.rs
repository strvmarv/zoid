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

// Uses `call_tool_direct_for_test`, which is only compiled under the
// `test-helpers` feature — gate the test to match so a plain
// `cargo test --workspace` (feature off) still compiles this integration crate.
#[cfg(feature = "test-helpers")]
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

    // After the crash, the server's client actor sees EOF and the manager must
    // surface `Disconnected` (spec §D/§E). Poll briefly for the flag to settle.
    let mut disconnected = false;
    for _ in 0..50 {
        if m.status().iter().any(|s| s.name == "fake" && s.state == ServerState::Disconnected) {
            disconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(disconnected, "crashed server must read Disconnected: {:?}", m.status());
}
