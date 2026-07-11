//! The generic OpenAI Chat Completions client (POST {base}/v1/chat/completions,
//! SSE streaming, tool-calling with fragment accumulation). Self-contained
//! like the `anthropic`/`ollama` modules; uses the crate's `Provider` seam.
//! No opencode-go-specifics — a generic leaf reusable by any OpenAI-compat
//! provider (Go, Zen, OpenRouter, etc.).

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

/// Map `ThinkingMode` + `ThinkingWireShape` to the OpenAI-compat thinking
/// params. Returns `None` for models without thinking support (defensive —
/// the capability gate should have caught this earlier).
fn thinking_params(req: &CompletionRequest) -> Option<Vec<(&'static str, Value)>> {
    let info = crate::model::model_info(&req.model);
    let wire = info.thinking_wire;
    match wire {
        crate::model::ThinkingWireShape::DeepSeek => {
            // deepseek-v4-pro is thinking-only: Off silently becomes Auto.
            let is_thinking_only = info.thinking == crate::model::ThinkingSupport::ToggleWithEffort
                && req.model == "deepseek-v4-pro";
            let effective = if is_thinking_only && matches!(req.thinking, crate::ThinkingMode::Off)
            {
                tracing::warn!(model = %req.model, "thinking-only model: Off silently becomes Auto");
                crate::ThinkingMode::Auto
            } else {
                req.thinking
            };
            let mut params = Vec::new();
            let (thinking_type, has_effort) = match effective {
                crate::ThinkingMode::Off => ("disabled", false),
                crate::ThinkingMode::Auto => ("enabled", true),
                crate::ThinkingMode::Effort(_) => ("enabled", true),
            };
            params.push(("thinking", json!({ "type": thinking_type })));
            if has_effort {
                let effort = match effective {
                    crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "max",
                    _ => "high",
                };
                params.push(("reasoning_effort", json!(effort)));
            }
            Some(params)
        }
        crate::model::ThinkingWireShape::OpenAI => {
            let effort = match req.thinking {
                crate::ThinkingMode::Off => return Some(vec![("reasoning_effort", json!("none"))]),
                crate::ThinkingMode::Auto => "medium",
                crate::ThinkingMode::Effort(crate::EffortLevel::Low) => "low",
                crate::ThinkingMode::Effort(crate::EffortLevel::Medium) => "medium",
                crate::ThinkingMode::Effort(crate::EffortLevel::High) => "high",
                crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "xhigh",
            };
            Some(vec![("reasoning_effort", json!(effort))])
        }
        _ => None,
    }
}

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
    if let Some(params) = thinking_params(req) {
        for (key, val) in params {
            body[key] = val;
        }
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
    pub fn new() -> Self {
        Self::default()
    }

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
            .map(|a| {
                ProviderEvent::ToolCall(ToolCall {
                    id: a.id,
                    name: a.name,
                    args: serde_json::from_str(&a.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({})),
                })
            })
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
    if let Some(err) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return vec![ProviderEvent::Error(err.to_string())];
    }
    let mut out = Vec::new();
    if let Some(content) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
    {
        if !content.is_empty() {
            out.push(ProviderEvent::TextDelta(content.to_string()));
        }
    }
    if let Some(reasoning) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("reasoning_content"))
        .and_then(|c| c.as_str())
    {
        if !reasoning.is_empty() {
            out.push(ProviderEvent::ThinkingDelta(reasoning.to_string()));
        }
    }
    if let Some(calls) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for call in calls {
            acc.feed(call);
        }
    }
    if let Some(reason) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
    {
        if reason == "length" {
            out.push(ProviderEvent::Truncated);
        }
    }
    if let Some(usage) = v.get("usage") {
        let input = usage
            .get("prompt_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("completion_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        out.push(ProviderEvent::Usage(Usage {
            input_tokens: input,
            output_tokens: output,
            cached,
            thinking_tokens: 0,
        }));
    }
    out
}

/// Default base URL when none is configured. Callers override via
/// `with_base_url`; the OpenAI-compat leaf has no single canonical host
/// (OpenAI, OpenRouter, OpenCode Go/Zen all differ), so this is a placeholder
/// that real callers always override.
const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Streaming OpenAI Chat Completions provider.
pub struct OpenAICompatProvider {
    api_key: String,
    base_url: String,
    path_prefix: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenAICompatProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            path_prefix: "/v1".to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
        }
    }
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = prefix.into();
        self
    }
}

#[async_trait]
impl Provider for OpenAICompatProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;
        let mut acc = ToolCallAccumulator::new();

        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!(
                    "{}{}/chat/completions",
                    self.base_url, self.path_prefix
                ))
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&request_body(req))
                .send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                return Ok(());
            }
            Err(_) => {
                let _ = sink
                    .send(ProviderEvent::Error(format!(
                        "provider request timed out after {}s (no response)",
                        self.idle_timeout.as_secs()
                    )))
                    .await;
                return Ok(());
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = match tokio::time::timeout(self.idle_timeout, resp.text()).await {
                Ok(Ok(t)) => t,
                _ => String::new(),
            };
            let _ = sink
                .send(ProviderEvent::Error(format!("HTTP {status}: {text}")))
                .await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        let mut ended_early = false;
        loop {
            let item = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    let _ = sink
                        .send(ProviderEvent::Error(format!(
                            "provider idle timeout: no data for {}s",
                            self.idle_timeout.as_secs()
                        )))
                        .await;
                    ended_early = true;
                    break;
                }
            };
            let item = match item {
                Ok(ev) => ev,
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    ended_early = true;
                    break;
                }
            };
            // OpenAI uses `data: [DONE]` as the terminator; eventsource may
            // surface it as an event with data "[DONE]" (no event type).
            if item.data == "[DONE]" {
                for tc in acc.take() {
                    if ttft.is_none() {
                        ttft = Some(start.elapsed().as_millis() as u64);
                    }
                    if sink.send(tc).await.is_err() {
                        ended_early = true;
                        break;
                    }
                }
                if !ended_early {
                    let _ = sink.send(ProviderEvent::Done).await;
                }
                ended_early = true;
                break;
            }
            for pe in parse_chunk(&item.data, &mut acc) {
                if ttft.is_none() {
                    ttft = Some(start.elapsed().as_millis() as u64);
                }
                let is_done = matches!(pe, ProviderEvent::Done);
                if sink.send(pe).await.is_err() {
                    ended_early = true;
                    break;
                }
                if is_done {
                    break;
                }
            }
            if ended_early {
                break;
            }
        }
        // If the transport closed without an explicit [DONE], flush + Done
        // (matches the ollama.rs trailing-line flush philosophy).
        if !ended_early {
            for tc in acc.take() {
                if ttft.is_none() {
                    ttft = Some(start.elapsed().as_millis() as u64);
                }
                if sink.send(tc).await.is_err() {
                    break;
                }
            }
            let _ = sink.send(ProviderEvent::Done).await;
        }
        tracing::info!(
            kind = "provider",
            provider = "openai-compat",
            model = %req.model,
            ttft_ms = ttft.unwrap_or(0),
            total_ms = start.elapsed().as_millis() as u64,
            "provider stream complete"
        );
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}{}/models", self.base_url, self.path_prefix))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Ok(crate::parse_data_id_models(&resp.text().await?))
    }
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
            thinking: crate::ThinkingMode::Off,
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
            thinking: crate::ThinkingMode::Off,
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
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        let tc = &body["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "call-1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "read_file");
        assert_eq!(tc["function"]["arguments"], json!(r#"{"path":"a.txt"}"#));
        assert!(
            tc["function"]["arguments"].is_string(),
            "arguments must be a string"
        );
    }

    #[test]
    fn tool_message_emits_tool_call_id() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::tool_with_call_id("read_file", "call-1", "body")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
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
            thinking: crate::ThinkingMode::Off,
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
            thinking: crate::ThinkingMode::Off,
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
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 120,
                output_tokens: 40,
                cached: 30,
                thinking_tokens: 0,
            })]
        );
    }

    #[test]
    fn parse_chunk_usage_without_cached_tokens_defaults_to_zero() {
        let data = r#"{"usage":{"prompt_tokens":120,"completion_tokens":40}}"#;
        assert_eq!(
            parse_chunk(data, &mut ToolCallAccumulator::new()),
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 120,
                output_tokens: 40,
                cached: 0,
                thinking_tokens: 0,
            })]
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
        assert_eq!(
            out[0],
            ProviderEvent::ToolCall(ToolCall {
                id: "call-a".into(),
                name: "read_file".into(),
                args: json!({})
            })
        );
        assert_eq!(
            out[1],
            ProviderEvent::ToolCall(ToolCall {
                id: "call-b".into(),
                name: "list_dir".into(),
                args: json!({})
            })
        );
    }

    #[test]
    fn tool_call_accumulator_take_drains_so_second_take_is_empty() {
        let mut acc = ToolCallAccumulator::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":"{}"}}]}}]}"#;
        let _ = parse_chunk(data, &mut acc);
        let first = acc.take();
        assert_eq!(first.len(), 1);
        let second = acc.take();
        assert!(
            second.is_empty(),
            "take() must drain — second call should be empty"
        );
    }

    #[test]
    fn deepseek_body_emits_thinking_and_effort_when_auto() {
        let req = CompletionRequest {
            model: "deepseek-v4-pro".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_5_2_thinking_off_emits_disabled_no_effort() {
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn glm_5_2_thinking_auto_emits_enabled_high() {
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_5_2_thinking_max_emits_enabled_max() {
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn glm_5_turbo_thinking_off_emits_disabled_no_effort() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn glm_5_turbo_thinking_auto_emits_enabled_high() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_5_turbo_thinking_max_emits_enabled_max() {
        let req = CompletionRequest {
            model: "glm-5-turbo".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn glm_4_7_thinking_off_emits_disabled_no_effort() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn glm_4_7_thinking_auto_emits_enabled_high() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn glm_4_7_thinking_max_emits_enabled_max() {
        let req = CompletionRequest {
            model: "glm-4.7".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 16,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn deepseek_body_emits_disabled_when_off() {
        let req = CompletionRequest {
            model: "deepseek-v4-flash".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_body_emits_max_effort() {
        let req = CompletionRequest {
            model: "deepseek-v4-pro".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("max"));
    }

    #[test]
    fn deepseek_body_low_effort_maps_to_high() {
        let req = CompletionRequest {
            model: "deepseek-v4-pro".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Low),
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn deepseek_v4_pro_off_silently_becomes_auto() {
        let req = CompletionRequest {
            model: "deepseek-v4-pro".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        // v4-pro is thinking-only: Off → Auto → enabled + high
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn openai_body_emits_reasoning_effort_when_auto() {
        let req = CompletionRequest {
            model: "o3".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning_effort"], json!("medium"));
        assert!(
            body.get("thinking").is_none(),
            "OpenAI shape must NOT emit a thinking key"
        );
    }

    #[test]
    fn openai_body_emits_xhigh_for_max() {
        let req = CompletionRequest {
            model: "o3".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning_effort"], json!("xhigh"));
    }

    #[test]
    fn non_thinking_model_emits_nothing_when_off() {
        let req = CompletionRequest {
            model: "mimo-v2.5".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let body = request_body(&req);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn non_thinking_model_emits_nothing_even_when_thinking_on() {
        let req = CompletionRequest {
            model: "mimo-v2.5".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 4096,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
        };
        let body = request_body(&req);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn parse_chunk_reasoning_content_emits_thinking_delta() {
        let data =
            r#"{"choices":[{"delta":{"content":"answer","reasoning_content":"thinking..."}}]}"#;
        let events = parse_chunk(data, &mut ToolCallAccumulator::new());
        assert!(
            events.contains(&ProviderEvent::ThinkingDelta("thinking...".into())),
            "reasoning_content must emit ThinkingDelta, got: {:?}",
            events
        );
        assert!(
            events.contains(&ProviderEvent::TextDelta("answer".into())),
            "content must still emit TextDelta, got: {:?}",
            events
        );
    }

    #[test]
    fn parse_chunk_reasoning_content_alone_emits_thinking_delta_only() {
        let data = r#"{"choices":[{"delta":{"reasoning_content":"deep thoughts"}}]}"#;
        let events = parse_chunk(data, &mut ToolCallAccumulator::new());
        assert_eq!(
            events,
            vec![ProviderEvent::ThinkingDelta("deep thoughts".into())],
            "reasoning-only delta must produce only ThinkingDelta"
        );
    }

    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Throwaway server that accepts one connection, optionally writes
    /// `headers`, then stalls. Mirrors ollama.rs:773-789.
    async fn spawn_stalling_server(headers: Option<&'static [u8]>) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                if let Some(hdr) = headers {
                    let _ = sock.write_all(hdr).await;
                    let _ = sock.flush().await;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        addr
    }

    const OK_SSE_HEADERS: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        }
    }

    #[tokio::test]
    async fn idle_timeout_emits_error_when_stream_stalls() {
        let addr = spawn_stalling_server(Some(OK_SSE_HEADERS)).await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done =
            tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung — idle timeout not enforced");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(
            matches!(got.last(), Some(ProviderEvent::Error(_))),
            "expected trailing idle-timeout Error, got {got:?}"
        );
    }

    #[tokio::test]
    async fn request_timeout_emits_error_when_no_response() {
        let addr = spawn_stalling_server(None).await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done =
            tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung waiting for response headers");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(
            matches!(got.last(), Some(ProviderEvent::Error(_))),
            "expected a request-timeout Error, got {got:?}"
        );
    }

    #[tokio::test]
    async fn error_body_timeout_emits_error_with_status() {
        let addr = spawn_stalling_server(Some(
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 100\r\n\r\n",
        ))
        .await;
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_millis(150));
        let (tx, mut rx) = mpsc::channel(16);
        let done =
            tokio::time::timeout(Duration::from_secs(5), provider.stream(&probe_req(), tx)).await;
        assert!(done.is_ok(), "stream() hung reading a stalled error body");
        done.unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(
            matches!(got.last(), Some(ProviderEvent::Error(e)) if e.contains("429")),
            "expected an HTTP 429 Error, got {got:?}"
        );
    }

    #[tokio::test]
    async fn done_terminator_emits_exactly_one_done() {
        // Server writes a minimal SSE stream: one content delta, then [DONE].
        // Regression: the [DONE] branch must not let the trailing-flush block
        // re-emit Done (a double Done was emitted before the fix set
        // `ended_early = true` before break). Uses Content-Length (not chunked)
        // to avoid brittle hand-computed chunk sizes.
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\r\n\r\n\
                    data: [DONE]\r\n\r\n";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(body).await;
            }
        });
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        let done_count = got
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Done))
            .count();
        assert_eq!(
            done_count, 1,
            "expected exactly one Done, got {done_count} in {got:?}"
        );
        assert!(
            got.iter()
                .any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "hi")),
            "expected the content delta, got {got:?}"
        );
    }

    #[test]
    fn default_path_prefix_is_v1() {
        let p = OpenAICompatProvider::new("k".into());
        assert_eq!(p.path_prefix, "/v1");
    }

    #[test]
    fn with_path_prefix_overrides_default() {
        let p = OpenAICompatProvider::new("k".into()).with_path_prefix("");
        assert_eq!(p.path_prefix, "");
    }

    #[tokio::test]
    async fn default_path_prefix_emits_v1_chat_completions() {
        // Regression: default prefix must still emit /v1/chat/completions.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: [DONE]\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = OpenAICompatProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(std::time::Duration::from_secs(2));
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![crate::Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/chat/completions"),
            "default prefix must emit /v1/chat/completions, got: {first}"
        );
    }
}
