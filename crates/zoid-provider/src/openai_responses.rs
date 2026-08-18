//! The generic OpenAI Responses client (POST {base}/v1/responses, SSE streaming,
//! response.* events, function-call via response.function_call_arguments.delta/.done,
//! reasoning summaries, usage on response.completed). Self-contained like the
//! `openai_compat`/`anthropic` modules; uses the crate's `Provider` seam. No
//! opencode-zen-specifics — a generic leaf reusable by direct-OpenAI, OpenRouter, etc.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// Map `ThinkingMode` to the Responses `reasoning.effort` object, or `None` to
/// omit the field entirely (Off). zoid's EffortLevel::Max → OpenAI "xhigh".
/// Only emits when the model supports OpenAI-style reasoning (gated by
/// `model_info(model).thinking_wire == OpenAI`), matching `openai_compat.rs`'s
/// `thinking_params()` pattern. Non-reasoning models get no `reasoning` field.
fn reasoning_params(req: &CompletionRequest) -> Option<Value> {
    let info = req.model_info;
    if info.thinking_wire != crate::model::ThinkingWireShape::OpenAI {
        return None;
    }
    let effort = match req.thinking {
        crate::ThinkingMode::Off => return None,
        crate::ThinkingMode::Auto => "medium",
        crate::ThinkingMode::Effort(crate::EffortLevel::Low) => "low",
        crate::ThinkingMode::Effort(crate::EffortLevel::Medium) => "medium",
        crate::ThinkingMode::Effort(crate::EffortLevel::High) => "high",
        crate::ThinkingMode::Effort(crate::EffortLevel::Max) => "xhigh",
    };
    Some(json!({ "effort": effort }))
}

/// Build the OpenAI Responses `/v1/responses` request body. System prompt maps
/// to the top-level `instructions` field. A single user text message with no
/// tool messages uses the `input: <string>` shorthand; otherwise `input` is an
/// array of items (message items + function_call_output items for tool results).
pub fn request_body(req: &CompletionRequest) -> Value {
    let has_tool_messages = req.messages.iter().any(|m| m.role == MsgRole::Tool);
    let single_user_string = req.messages.len() == 1
        && req.messages[0].role == MsgRole::User
        && req.messages[0].tool_calls.is_empty()
        && !has_tool_messages;

    let mut body = json!({
        "model": req.model,
        "stream": true,
        "max_output_tokens": req.max_tokens,
        "tool_choice": "auto",
    });

    if single_user_string {
        body["input"] = json!(req.messages[0].content.clone());
    } else {
        let mut input: Vec<Value> = Vec::new();
        for m in &req.messages {
            match m.role {
                MsgRole::User => input.push(json!({
                    "role": "user",
                    "content": [{ "type": "input_text", "text": m.content }],
                })),
                MsgRole::Assistant => {
                    let mut parts = vec![json!({ "type": "output_text", "text": m.content })];
                    for tc in &m.tool_calls {
                        parts.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".into()),
                        }));
                    }
                    input.push(json!({
                        "role": "assistant",
                        "content": parts,
                    }));
                }
                MsgRole::Tool => input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content,
                })),
            }
        }
        body["input"] = Value::Array(input);
    }

    if let Some(sys) = &req.system {
        body["instructions"] = json!(sys);
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                        "strict": false,
                    })
                })
                .collect(),
        );
    }
    if let Some(r) = reasoning_params(req) {
        body["reasoning"] = r;
    }
    body
}

/// Accumulates OpenAI Responses function-call argument fragments by `item_id`,
/// flushing a complete `ToolCall` on `response.function_call_arguments.done`.
///
/// `call_id` is learned from `response.output_item.added` (which carries the
/// full item including `call_id`), keyed by `item_id`. The `.delta`/`.done`
/// events carry only `item_id` — NOT `call_id` (confirmed via 2026-07-11 Zen
/// capture; see spike `## Tool-call captures`). The two values are distinct;
/// the next turn's `function_call_output` MUST use `call_id`.
#[derive(Debug, Default)]
pub struct ResponsesToolAccum {
    /// item_id → accumulated argument fragments.
    by_item: std::collections::BTreeMap<String, String>,
    /// item_id → call_id (learned from `response.output_item.added`).
    call_ids: std::collections::BTreeMap<String, String>,
}

impl ResponsesToolAccum {
    pub fn new() -> Self {
        Self::default()
    }

    fn feed(&mut self, item_id: &str, delta: &str) {
        self.by_item
            .entry(item_id.to_string())
            .or_default()
            .push_str(delta);
    }

    /// Record the `call_id` for an `item_id`, learned from
    /// `response.output_item.added`.
    fn note_call_id(&mut self, item_id: &str, call_id: &str) {
        self.call_ids
            .insert(item_id.to_string(), call_id.to_string());
    }

    fn flush(&mut self, item_id: &str, name: &str, arguments: &str) -> Option<ProviderEvent> {
        self.by_item.remove(item_id);
        let call_id = self.call_ids.remove(item_id).unwrap_or_default();
        let args: Value = serde_json::from_str(arguments)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        Some(ProviderEvent::ToolCall(ToolCall {
            id: call_id,
            name: name.to_string(),
            args,
        }))
    }
}

/// Parse one OpenAI Responses SSE `data:` payload (the JSON object after
/// `data: `) into zero-or-more `ProviderEvent`s. `acc` accumulates
/// function-call argument fragments. Never panics. The caller handles the
/// stream end separately.
pub fn parse_event(data: &str, acc: &mut ResponsesToolAccum) -> Vec<ProviderEvent> {
    let data = data.trim();
    if data.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut out = Vec::new();
    match ty {
        "response.output_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    out.push(ProviderEvent::TextDelta(delta.to_string()));
                }
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                if !delta.is_empty() {
                    out.push(ProviderEvent::ThinkingDelta(delta.to_string()));
                }
            }
        }
        "response.output_item.added" => {
            // Learn call_id from the full item (the .delta/.done events carry
            // only item_id, NOT call_id — confirmed via 2026-07-11 capture).
            if let Some(item) = v.get("item") {
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    let item_id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let call_id = item.get("call_id").and_then(|c| c.as_str()).unwrap_or("");
                    if !item_id.is_empty() {
                        acc.note_call_id(item_id, call_id);
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let item_id = v.get("item_id").and_then(|i| i.as_str()).unwrap_or("");
            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                acc.feed(item_id, delta);
            }
        }
        "response.function_call_arguments.done" => {
            let item_id = v.get("item_id").and_then(|i| i.as_str()).unwrap_or("");
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = v.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
            if let Some(ev) = acc.flush(item_id, name, arguments) {
                out.push(ev);
            }
        }
        "response.completed" => {
            if let Some(usage) = v.get("response").and_then(|r| r.get("usage")) {
                let input = usage
                    .get("input_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let output = usage
                    .get("output_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let cached = usage
                    .get("input_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let thinking = usage
                    .get("output_tokens_details")
                    .and_then(|d| d.get("reasoning_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                out.push(ProviderEvent::Usage(Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cached,
                    thinking_tokens: thinking,
                }));
            }
            out.push(ProviderEvent::Done);
        }
        "response.incomplete" => {
            out.push(ProviderEvent::Truncated);
            out.push(ProviderEvent::Done);
        }
        "response.failed" => {
            let msg = v
                .get("response")
                .and_then(|r| r.get("error"))
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("response failed");
            out.push(ProviderEvent::Error(msg.to_string()));
        }
        _ => {
            tracing::trace!(event = %ty, "openai-responses: ignoring event");
        }
    }
    out
}

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Streaming OpenAI Responses provider.
pub struct OpenAIResponsesProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenAIResponsesProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
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
}

#[async_trait]
impl Provider for OpenAIResponsesProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let mut acc = ResponsesToolAccum::new();
        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/v1/responses", self.base_url))
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
            if item.data.is_empty() {
                continue;
            }
            let mut got_done = false;
            for pe in parse_event(&item.data, &mut acc) {
                if matches!(pe, ProviderEvent::Done) {
                    got_done = true;
                }
                if sink.send(pe).await.is_err() {
                    ended_early = true;
                    break;
                }
            }
            if got_done || ended_early {
                break;
            }
        }
        if !ended_early {
            let _ = sink.send(ProviderEvent::Done).await;
        }
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
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
    fn body_has_model_input_instructions_stream() {
        let req = CompletionRequest {
            model: "gpt-5.4".into(),
            model_info: crate::model::model_info("gpt-5.4"),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req);
        assert_eq!(body["model"], "gpt-5.4");
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "be terse");
        assert_eq!(body["input"], "hi");
        assert_eq!(body["max_output_tokens"], 1024);
        assert!(body.get("reasoning").is_none(), "Off must omit reasoning");
    }

    #[test]
    fn body_with_tool_message_uses_input_array_with_function_call_output() {
        let req = CompletionRequest {
            model: "m".into(),
            model_info: crate::model::model_info("m"),
            system: None,
            messages: vec![
                Message::user("call the tool"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "a.txt"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                Message::tool_with_call_id("read_file", "call-1", "file body"),
            ],
            max_tokens: 64,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req);
        assert!(
            body["input"].is_array(),
            "multi-message input must be an array"
        );
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call-1");
        assert_eq!(input[2]["output"], "file body");
    }

    #[test]
    fn body_includes_tools_array_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            model_info: crate::model::model_info("m"),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["strict"], false);
    }

    #[test]
    fn body_emits_reasoning_effort_for_auto() {
        let req = CompletionRequest {
            model: "gpt-5.4".into(),
            model_info: crate::model::model_info("gpt-5.4"),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
            reassert: None,
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn body_emits_xhigh_for_max_effort() {
        let req = CompletionRequest {
            model: "gpt-5.4".into(),
            model_info: crate::model::model_info("gpt-5.4"),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Effort(crate::EffortLevel::Max),
            reassert: None,
        };
        let body = request_body(&req);
        assert_eq!(body["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn body_omits_reasoning_for_non_reasoning_model() {
        // A model whose thinking_wire != OpenAI must NOT get a reasoning field,
        // even when thinking is Auto. Guards against the generic leaf spuriously
        // enabling reasoning for non-reasoning models (review B1).
        let req = CompletionRequest {
            model: "ollama-model".into(),
            model_info: crate::model::model_info("ollama-model"),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
            reassert: None,
        };
        let body = request_body(&req);
        assert!(
            body.get("reasoning").is_none(),
            "non-reasoning model must not get reasoning field: got {body}"
        );
    }

    #[test]
    fn parse_output_text_delta_yields_textdelta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hel"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn parse_reasoning_summary_delta_yields_thinking_delta() {
        let data = r#"{"type":"response.reasoning_summary_text.delta","delta":"pondering"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::ThinkingDelta("pondering".into())]
        );
    }

    #[test]
    fn parse_function_call_arguments_done_emits_toolcall() {
        let mut acc = ResponsesToolAccum::new();
        // output_item.added carries the full item INCLUDING call_id (confirmed
        // via 2026-07-11 capture). The .delta/.done events carry only item_id.
        let added = r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"fc_1","type":"function_call","status":"in_progress","name":"read_file","call_id":"call_xyz","arguments":""}}"#;
        let d1 = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":"}"#;
        let d2 = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"\"a\"}"}"#;
        let done = r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","name":"read_file","output_index":0,"arguments":"{\"path\":\"a\"}"}"#;
        let _ = parse_event(added, &mut acc);
        let _ = parse_event(d1, &mut acc);
        let _ = parse_event(d2, &mut acc);
        let out = parse_event(done, &mut acc);
        // The emitted ToolCall uses call_id ("call_xyz"), NOT item_id ("fc_1").
        assert_eq!(
            out,
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "call_xyz".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn parse_completed_emits_usage_then_done() {
        let data = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":20,"input_tokens_details":{"cached_tokens":3},"output_tokens_details":{"reasoning_tokens":5},"total_tokens":33}}}"#;
        let out = parse_event(data, &mut ResponsesToolAccum::new());
        assert_eq!(
            out,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cached: 3,
                    thinking_tokens: 5,
                }),
                ProviderEvent::Done,
            ]
        );
    }

    #[test]
    fn parse_incomplete_emits_truncated_then_done() {
        let data = r#"{"type":"response.incomplete"}"#;
        assert_eq!(
            parse_event(data, &mut ResponsesToolAccum::new()),
            vec![ProviderEvent::Truncated, ProviderEvent::Done]
        );
    }

    #[test]
    fn parse_failed_emits_error() {
        let data = r#"{"type":"response.failed","response":{"error":{"message":"rate limited"}}}"#;
        let out = parse_event(data, &mut ResponsesToolAccum::new());
        assert!(matches!(out.last(), Some(ProviderEvent::Error(e)) if e.contains("rate limited")));
    }

    #[test]
    fn parse_unknown_event_yields_nothing() {
        let data = r#"{"type":"response.created"}"#;
        assert!(parse_event(data, &mut ResponsesToolAccum::new()).is_empty());
    }

    #[test]
    fn parse_malformed_yields_nothing() {
        assert!(parse_event("not json", &mut ResponsesToolAccum::new()).is_empty());
        assert!(parse_event("", &mut ResponsesToolAccum::new()).is_empty());
    }
}
