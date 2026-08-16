use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
pub enum Inbound {
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    ServerRequest {
        id: Value,
        method: String,
    },
    Notification {
        method: String,
    },
}

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
                Ok(Inbound::Response {
                    id,
                    result: Ok(result),
                })
            }
        }
        (None, false) => Err(anyhow::anyhow!("malformed JSON-RPC line: {line}")),
    }
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
            Inbound::Response {
                id: 7,
                result: Ok(v),
            } => assert_eq!(v["ok"], true),
            other => panic!("expected response, got {other:?}"),
        }
        // An error response.
        match classify(r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32601,"message":"nope"}}"#)
            .unwrap()
        {
            Inbound::Response {
                id: 8,
                result: Err(e),
            } => assert_eq!(e.code, -32601),
            other => panic!("expected error response, got {other:?}"),
        }
        // A notification (no id).
        match classify(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#).unwrap()
        {
            Inbound::Notification { method } => {
                assert_eq!(method, "notifications/tools/list_changed")
            }
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
