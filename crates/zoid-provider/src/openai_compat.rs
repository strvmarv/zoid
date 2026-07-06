//! The generic OpenAI Chat Completions client (POST {base}/v1/chat/completions,
//! SSE streaming, tool-calling with fragment accumulation). Self-contained
//! like the `anthropic`/`ollama` modules; uses the crate's `Provider` seam.
//! No opencode-go-specifics — a generic leaf reusable by any OpenAI-compat
//! provider (Go, Zen, OpenRouter, etc.).

use crate::{CompletionRequest, MsgRole, ProviderEvent, ToolCall, Usage};
use serde_json::{json, Value};

/// Build the OpenAI Chat Completions `/v1/chat/completions` request body.
/// System prompt is a leading `{"role":"system"}` message. Tool-call
/// `arguments` are serialized as a JSON-encoded **string** (OpenAI's shape,
/// the inverse of Ollama's object shape). Tool results carry `tool_call_id`.
pub fn request_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    for m in &req.messages {
        match m.role {
            MsgRole::User => messages.push(json!({ "role": "user", "content": m.content })),
            MsgRole::Assistant => {
                let mut obj = json!({ "role": "assistant", "content": m.content });
                if !m.tool_calls.is_empty() {
                    obj["tool_calls"] = Value::Array(
                        m.tool_calls.iter().map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".into()),
                                }
                            })
                        }).collect(),
                    );
                }
                messages.push(obj);
            }
            MsgRole::Tool => messages.push(json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": m.content,
            })),
        }
    }
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "stream_options": { "include_usage": true },
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools.iter().map(|t| json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
            })).collect(),
        );
    }
    body
}

/// Accumulates OpenAI tool-call fragments (which arrive piecewise across SSE
/// chunks) by `index`, flushing complete `ToolCall`s when `take()` is called
/// (at `data: [DONE]` or stream end). id + name lock on first sighting;
/// arguments strings concatenate across chunks and are re-parsed to an object.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    by_index: std::collections::BTreeMap<u32, ToolCallAccum>,
}

#[derive(Debug, Clone)]
struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    pub fn new() -> Self { Self::default() }

    /// Feed one chunk's `delta.tool_calls[]` entry.
    fn feed(&mut self, call: &Value) {
        let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
        let entry = self.by_index.entry(index).or_insert_with(|| ToolCallAccum {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        });
        if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
            entry.id = id.to_string();
        }
        if let Some(func) = call.get("function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                entry.name = name.to_string();
            }
            if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                entry.arguments.push_str(args);
            }
        }
    }

    /// Drain the accumulated tool calls as `ToolCall` events, in index order.
    /// Each `arguments` JSON string is re-parsed to a `Value::Object` (matching
    /// `coerce_tool_args`'s contract in `ollama.rs`); invalid JSON → `{}`.
    pub fn take(&mut self) -> Vec<ProviderEvent> {
        std::mem::take(&mut self.by_index)
            .into_values()
            .map(|a| ProviderEvent::ToolCall(ToolCall {
                id: a.id,
                name: a.name,
                args: serde_json::from_str(&a.arguments)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({})),
            }))
            .collect()
    }
}

/// Parse one OpenAI Chat Completions SSE `data:` payload (the JSON object after
/// `data: `) into zero-or-more `ProviderEvent`s. `acc` accumulates tool-call
/// fragments across calls. Never panics. The caller handles `data: [DONE]`
/// separately (flushing `acc.take()` then emitting `Done`).
pub fn parse_chunk(data: &str, acc: &mut ToolCallAccumulator) -> Vec<ProviderEvent> {
    let data = data.trim();
    if data.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Some(err) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return vec![ProviderEvent::Error(err.to_string())];
    }
    let mut out = Vec::new();
    if let Some(content) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")).and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
        if !content.is_empty() {
            out.push(ProviderEvent::TextDelta(content.to_string()));
        }
    }
    if let Some(calls) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")).and_then(|d| d.get("tool_calls")).and_then(|t| t.as_array()) {
        for call in calls {
            acc.feed(call);
        }
    }
    if let Some(reason) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("finish_reason")).and_then(|f| f.as_str()) {
        if reason == "length" {
            out.push(ProviderEvent::Truncated);
        }
    }
    if let Some(usage) = v.get("usage") {
        let input = usage.get("prompt_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        let output = usage.get("completion_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
        let cached = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        out.push(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: output, cached }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolSpec};
    use serde_json::json;

    #[test]
    fn body_has_stream_options_and_system_leading_message() {
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(body["model"], "glm-5.2");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
            ])
        );
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        assert_eq!(
            request_body(&req)["messages"],
            json!([{ "role": "user", "content": "x" }])
        );
    }

    #[test]
    fn assistant_with_tool_calls_emits_arguments_as_json_string() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message {
                role: MsgRole::Assistant,
                content: "".into(),
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    args: json!({"path": "a.txt"}),
                }],
                tool_name: None,
                tool_call_id: None,
            }],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        let tc = &body["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "call-1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        assert_eq!(tc["function"]["arguments"], json!(r#"{"path":"a.txt"}"#));
        assert!(tc["function"]["arguments"].is_string(), "arguments must be a string");
    }

    #[test]
    fn tool_message_emits_tool_call_id() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::tool_with_call_id("read_file", "call-1", "body")],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "tool", "tool_call_id": "call-1", "content": "body" })
        );
    }

    #[test]
    fn body_includes_tools_array_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
        };
        let body = request_body(&req);
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": { "name": "read_file", "description": "read a file", "parameters": {"type": "object"} }
            }])
        );
    }

    #[test]
    fn body_without_tools_omits_tools_key() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        assert!(request_body(&req).get("tools").is_none());
    }

    #[test]
    fn parse_chunk_content_delta_yields_textdelta() {
        let data = r#"{"choices":[{"delta":{"content":"Hel"}}]}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn parse_chunk_empty_content_yields_nothing() {
        let data = r#"{"choices":[{"delta":{"content":""}}]}"#;
        assert!(parse_chunk(data, &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn parse_chunk_finish_reason_length_yields_truncated() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Truncated]
        );
    }

    #[test]
    fn parse_chunk_finish_reason_stop_yields_nothing() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        assert!(parse_chunk(data, &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn parse_chunk_usage_yields_usage_with_cached_tokens() {
        let data = r#"{"usage":{"prompt_tokens":120,"completion_tokens":40,"prompt_tokens_details":{"cached_tokens":30}}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Usage(Usage { input_tokens: 120, output_tokens: 40, cached: 30 })]
        );
    }

    #[test]
    fn parse_chunk_usage_without_cached_tokens_defaults_to_zero() {
        let data = r#"{"usage":{"prompt_tokens":120,"completion_tokens":40}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Usage(Usage { input_tokens: 120, output_tokens: 40, cached: 0 })]
        );
    }

    #[test]
    fn parse_chunk_error_yields_error() {
        let data = r#"{"error":{"message":"Unauthorized"}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Error("Unauthorized".into())]
        );
    }

    #[test]
    fn parse_chunk_malformed_yields_nothing() {
        assert!(parse_chunk("not json", &mut ToolCallAccumulator::new()).is_empty());
        assert!(parse_chunk("", &mut ToolCallAccumulator::new()).is_empty());
    }

    #[test]
    fn tool_call_accumulator_single_chunk_flushes_at_take() {
        let mut acc = ToolCallAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}}]}"#;
        let _ = parse_chunk(data, &mut acc);
        assert_eq!(
            acc.take(),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn tool_call_accumulator_two_chunks_concatenates_arguments() {
        let mut acc = ToolCallAccumulator::new();
        let c1 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#;
        let c2 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]}}]}"#;
        let _ = parse_chunk(c1, &mut acc);
        let _ = parse_chunk(c2, &mut acc);
        assert_eq!(
            acc.take(),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn tool_call_accumulator_two_distinct_calls_in_index_order() {
        let mut acc = ToolCallAccumulator::new();
        let c1 = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","function":{"name":"read_file","arguments":"{}"}}]}}]}"#;
        let c2 = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call-b","function":{"name":"list_dir","arguments":"{}"}}]}}]}"#;
        let _ = parse_chunk(c1, &mut acc);
        let _ = parse_chunk(c2, &mut acc);
        let out = acc.take();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ProviderEvent::ToolCall(ToolCall { id: "call-a".into(), name: "read_file".into(), args: json!({}) }));
        assert_eq!(out[1], ProviderEvent::ToolCall(ToolCall { id: "call-b".into(), name: "list_dir".into(), args: json!({}) }));
    }

    #[test]
    fn tool_call_accumulator_take_drains_so_second_take_is_empty() {
        let mut acc = ToolCallAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{}"}}]}}]}"#;
        let _ = parse_chunk(data, &mut acc);
        let first = acc.take();
        assert_eq!(first.len(), 1);
        let second = acc.take();
        assert!(second.is_empty(), "take() must drain — second call should be empty");
    }
}