//! The real streaming Anthropic provider (reqwest + SSE).
//! Task 6: request body. Task 7: SSE parsing. Task 8: the provider + selection.

use crate::{CompletionRequest, MsgRole, ProviderEvent, Usage};
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

/// Map one Anthropic SSE frame to a `ProviderEvent`. Unhandled or malformed
/// frames return `None` (the caller skips them). Never panics.
pub fn parse_event(event_type: &str, data: &str) -> Option<ProviderEvent> {
    match event_type {
        "content_block_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let text = v.get("delta")?.get("text")?.as_str()?;
            Some(ProviderEvent::TextDelta(text.to_string()))
        }
        "message_start" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let input = v.get("message")?.get("usage")?.get("input_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: 0 }))
        }
        "message_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let output = v.get("usage")?.get("output_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage { input_tokens: 0, output_tokens: output }))
        }
        "message_stop" => Some(ProviderEvent::Done),
        "error" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            Some(ProviderEvent::Error(msg.to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
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

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(parse_event("content_block_delta", data), Some(ProviderEvent::TextDelta("Hello".into())));
    }

    #[test]
    fn parses_message_stop_as_done() {
        assert_eq!(parse_event("message_stop", r#"{"type":"message_stop"}"#), Some(ProviderEvent::Done));
    }

    #[test]
    fn parses_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        assert_eq!(parse_event("message_delta", data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 0, output_tokens: 12 })));
    }

    #[test]
    fn parses_message_start_input_usage() {
        let data = r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#;
        assert_eq!(parse_event("message_start", data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 7, output_tokens: 0 })));
    }

    #[test]
    fn parses_error() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(parse_event("error", data), Some(ProviderEvent::Error("Overloaded".into())));
    }

    #[test]
    fn ignores_unhandled_frames() {
        assert_eq!(parse_event("ping", "{}"), None);
        assert_eq!(parse_event("content_block_start", r#"{"type":"content_block_start"}"#), None);
        assert_eq!(parse_event("content_block_stop", r#"{"type":"content_block_stop"}"#), None);
    }

    #[test]
    fn malformed_data_yields_none_not_panic() {
        assert_eq!(parse_event("content_block_delta", "not json"), None);
    }
}
