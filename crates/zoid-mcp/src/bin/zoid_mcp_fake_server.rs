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
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned();
        match (method, id) {
            ("initialize", Some(id)) => {
                let out = reply(
                    &id,
                    json!({"protocolVersion":"2025-06-18","capabilities":{}}),
                );
                writeln!(stdout, "{out}").unwrap();
            }
            ("notifications/initialized", _) => { /* no reply */ }
            ("tools/list", Some(id)) => {
                let out = reply(
                    &id,
                    json!({"tools":[{
                        "name":"echo","description":"echoes arguments",
                        "inputSchema":{"type":"object"}
                    }]}),
                );
                writeln!(stdout, "{out}").unwrap();
            }
            ("tools/call", Some(id)) => {
                let name = v
                    .get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                if name == "crash" {
                    std::process::exit(1);
                } // mid-call crash
                let args = v
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or(json!({}));
                let out = reply(
                    &id,
                    json!({
                        "content":[{"type":"text","text": args.to_string()}],
                        "isError": false
                    }),
                );
                writeln!(stdout, "{out}").unwrap();
            }
            _ => {}
        }
        stdout.flush().unwrap();
    }
}
