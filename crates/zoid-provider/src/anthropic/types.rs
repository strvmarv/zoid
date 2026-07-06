//! Typed Anthropic Messages-API wire structs. Replaces the hand-built `json!`
//! blobs in the legacy `anthropic.rs::request_body`. Serialize-only on the
//! request side; the response side (`StreamEvent`) is added in Task 2.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: CacheKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CacheKind {
    /// 1-hour ephemeral cache (the current default).
    #[serde(rename = "ephemeral")]
    Ephemeral1h,
    /// 5-minute ephemeral cache (typed seam; no config knob exposes it yet).
    #[serde(rename = "ephemeral_5m")]
    Ephemeral5m,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain string shorthand: `"content": "hi"`. Used for interior messages
    /// without cache_control or tool blocks.
    Text(String),
    /// Block array: `"content": [{"type":"text",...}]`. Used when a message
    /// carries cache_control, tool_use, or tool_result blocks.
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// Anthropic's field name is `input_schema` (not OpenAI's `parameters`).
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SystemBlock {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct ThinkingConfig {
    pub r#type: ThinkingType,
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingType {
    Enabled,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: MessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlockStart,
    },
    ContentBlockDelta {
        index: u32,
        delta: Delta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        usage: Usage,
    },
    MessageStop,
    Error {
        error: ApiError,
    },
    Ping,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockStart {
    Text,
    ToolUse { id: String, name: String },
    Thinking,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MessageStart {
    pub usage: Usage,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MessageDeltaBody {
    /// `stop_reason` is present on the terminal `message_delta` only; absent on
    /// intermediate ones. `None` means "still streaming".
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiError {
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anthropic_request_serializes_minimal_body() {
        let req = AnthropicRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 1024,
            stream: true,
            messages: vec![AnthropicMessage {
                role: AnthropicRole::User,
                content: MessageContent::Text("hi".into()),
            }],
            system: None,
            tools: vec![],
            thinking: None,
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 1024,
                "stream": true,
                "messages": [{ "role": "user", "content": "hi" }]
            })
        );
        // empty tools/system/thinking must NOT appear on the wire
        assert!(body.get("tools").is_none());
        assert!(body.get("system").is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn content_block_tool_use_serializes() {
        let block = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: json!({"path": "a.txt"}),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "tool_use",
                "id": "toolu_1",
                "name": "read_file",
                "input": {"path": "a.txt"}
            })
        );
    }

    #[test]
    fn content_block_tool_result_serializes() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "file body".into(),
            is_error: None,
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "file body"
            })
        );
        assert!(v.get("is_error").is_none()); // skip_serializing_if
    }

    #[test]
    fn content_block_text_with_cache_control_serializes() {
        let block = ContentBlock::Text {
            text: "hi".into(),
            cache_control: Some(CacheControl {
                kind: CacheKind::Ephemeral1h,
            }),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "text",
                "text": "hi",
                "cache_control": {"type": "ephemeral"}
            })
        );
    }

    #[test]
    fn message_content_text_emits_plain_string() {
        let msg = AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text("hi".into()),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v, json!({"role": "user", "content": "hi"}));
    }

    #[test]
    fn message_content_blocks_emits_array() {
        let msg = AnthropicMessage {
            role: AnthropicRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "hello".into(),
                cache_control: None,
            }]),
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            v,
            json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "hello"}]
            })
        );
        assert!(v["content"][0].get("cache_control").is_none()); // skip_serializing_if
    }

    #[test]
    fn content_block_thinking_serializes() {
        // Thinking is typed + round-trips, even though it's discarded from
        // the event stream this slice (spec §7.2). The variant exists so the
        // parse side doesn't crash on it.
        let block = ContentBlock::Thinking {
            thinking: "reasoning".into(),
            signature: "sig".into(),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "thinking",
                "thinking": "reasoning",
                "signature": "sig"
            })
        );
    }

    #[test]
    fn stream_event_message_start_with_cache_tokens() {
        let frame = r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"cache_read_input_tokens":40,"cache_creation_input_tokens":3}}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::MessageStart { message } => {
                assert_eq!(message.usage.input_tokens, 7);
                assert_eq!(message.usage.cache_read_input_tokens, 40);
                assert_eq!(message.usage.cache_creation_input_tokens, 3);
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_start_tool_use() {
        let frame = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 0);
                match content_block {
                    ContentBlockStart::ToolUse { id, name } => {
                        assert_eq!(id, "toolu_1");
                        assert_eq!(name, "read_file");
                    }
                    other => panic!("expected ToolUse, got {other:?}"),
                }
            }
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_delta_text() {
        let frame = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                match delta {
                    Delta::TextDelta { text } => assert_eq!(text, "Hello"),
                    other => panic!("expected TextDelta, got {other:?}"),
                }
            }
            other => panic!("expected ContentBlockDelta, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_content_block_delta_input_json() {
        let frame = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::ContentBlockDelta {
                index,
                delta: Delta::InputJsonDelta { partial_json },
            } => {
                assert_eq!(index, 0);
                assert_eq!(partial_json, r#"{"path":"#);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_delta_with_stop_reason() {
        let frame = r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":4096}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason.as_deref(), Some("max_tokens"));
                assert_eq!(usage.output_tokens, 4096);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_message_stop() {
        let frame = r#"{"type":"message_stop"}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        assert!(matches!(ev, StreamEvent::MessageStop));
    }

    #[test]
    fn stream_event_error() {
        let frame =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        match ev {
            StreamEvent::Error { error } => assert_eq!(error.message, "Overloaded"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn stream_event_ping() {
        let frame = r#"{"type":"ping"}"#;
        let ev: StreamEvent = serde_json::from_str(frame).unwrap();
        assert!(matches!(ev, StreamEvent::Ping));
    }
}
