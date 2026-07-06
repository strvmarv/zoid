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
}
