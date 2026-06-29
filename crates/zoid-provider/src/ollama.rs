//! The Ollama Cloud provider via the OpenAI-compatible Chat Completions API
//! (`POST {base}/v1/chat/completions`, SSE streaming, `data: [DONE]` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.
//! Task 1: request body + chunk parser. Task 2: the provider + wiring.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Default model when `$ZOID_MODEL` is unset (GLM on Ollama Cloud).
pub const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2:cloud";

/// Build the OpenAI-compatible request body. The system prompt becomes a
/// leading `{"role":"system"}` message (OpenAI puts system inside `messages`).
/// `stream_options` is intentionally omitted (some servers reject unknown
/// fields; usage is not needed in P1a).
pub fn request_body(req: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    for m in &req.messages {
        messages.push(json!({
            "role": match m.role { MsgRole::User => "user", MsgRole::Assistant => "assistant" },
            "content": m.content,
        }));
    }
    json!({
        "model": req.model,
        "stream": true,
        "max_tokens": req.max_tokens,
        "messages": messages,
    })
}

/// Parse one OpenAI-compatible SSE `data:` payload into a `ProviderEvent`.
/// The terminator is the literal `[DONE]`. Content lives at
/// `choices[0].delta.content`; a usage-bearing chunk maps to `Usage`.
/// Empty/role-only/finish chunks and malformed JSON return `None` (never panics).
pub fn parse_chunk(data: &str) -> Option<ProviderEvent> {
    let data = data.trim();
    if data == "[DONE]" {
        return Some(ProviderEvent::Done);
    }
    let v: Value = serde_json::from_str(data).ok()?;

    if let Some(text) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|t| t.as_str())
    {
        if !text.is_empty() {
            return Some(ProviderEvent::TextDelta(text.to_string()));
        }
    }

    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        let input = usage.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        let output = usage.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        return Some(ProviderEvent::Usage(Usage { input_tokens: input, output_tokens: output }));
    }

    None
}

/// Streaming Ollama Cloud provider (OpenAI-compatible Chat Completions API).
pub struct OllamaProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://ollama.com".to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&request_body(req))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let _ = sink.send(ProviderEvent::Error(format!("HTTP {status}: {text}"))).await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if let Some(pe) = parse_chunk(&event.data) {
                        let is_done = matches!(pe, ProviderEvent::Done);
                        if sink.send(pe).await.is_err() {
                            break;
                        }
                        if is_done {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use serde_json::json;

    #[test]
    fn body_maps_system_to_leading_message_and_sets_stream() {
        let req = CompletionRequest {
            model: "glm-5.2:cloud".into(),
            system: Some("be terse".into()),
            messages: vec![
                Message { role: MsgRole::User, content: "hi".into() },
                Message { role: MsgRole::Assistant, content: "hello".into() },
            ],
            max_tokens: 1024,
        };
        let body = request_body(&req);
        assert_eq!(body, json!({
            "model": "glm-5.2:cloud",
            "stream": true,
            "max_tokens": 1024,
            "messages": [
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        let body = request_body(&req);
        assert_eq!(body["messages"], json!([{ "role": "user", "content": "x" }]));
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn parses_content_delta() {
        let data = r#"{"choices":[{"index":0,"delta":{"content":"Hel"}}]}"#;
        assert_eq!(parse_chunk(data), Some(ProviderEvent::TextDelta("Hel".into())));
    }

    #[test]
    fn parses_done_sentinel() {
        assert_eq!(parse_chunk("[DONE]"), Some(ProviderEvent::Done));
    }

    #[test]
    fn parses_usage_chunk() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":12}}"#;
        assert_eq!(parse_chunk(data),
            Some(ProviderEvent::Usage(Usage { input_tokens: 7, output_tokens: 12 })));
    }

    #[test]
    fn empty_content_and_role_only_chunks_yield_none() {
        // first chunk often carries role but no content
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#), None);
        // explicit empty content
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{"content":""}}]}"#), None);
        // finish chunk with no content/usage
        assert_eq!(parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#), None);
    }

    #[test]
    fn malformed_data_yields_none_not_panic() {
        assert_eq!(parse_chunk("not json"), None);
        assert_eq!(parse_chunk(""), None);
    }
}
