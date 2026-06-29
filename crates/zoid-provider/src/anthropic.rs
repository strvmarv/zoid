//! The real streaming Anthropic provider (reqwest + SSE).
//! Task 6: request body. Task 7: SSE parsing. Task 8: the provider + selection.

use crate::{CompletionRequest, Message, MsgRole};
use serde_json::{json, Value};

/// Default model when `$ZOID_MODEL` is unset (latest Claude Sonnet).
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// Build the Anthropic Messages API request body for a streaming completion.
pub fn request_body(req: &CompletionRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            json!({
                "role": match m.role { MsgRole::User => "user", MsgRole::Assistant => "assistant" },
                "content": m.content,
            })
        })
        .collect();

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "messages": messages,
    });
    if let Some(sys) = &req.system {
        body["system"] = json!(sys);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_messages_body_with_stream_flag() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![
                Message { role: MsgRole::User, content: "hi".into() },
                Message { role: MsgRole::Assistant, content: "hello".into() },
            ],
            max_tokens: 1024,
        };
        let body = request_body(&req);
        assert_eq!(body, json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 1024,
            "stream": true,
            "messages": [
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
    }

    #[test]
    fn includes_system_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        let body = request_body(&req);
        assert_eq!(body["system"], json!("be terse"));
    }
}
