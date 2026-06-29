//! The Ollama Cloud provider via the native Chat API
//! (`POST {base}/api/chat`, NDJSON streaming, `"done":true` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Default model when `$ZOID_MODEL` is unset (GLM on Ollama Cloud).
pub const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2:cloud";

/// Build the native Ollama `/api/chat` request body. System prompt is a leading
/// `{"role":"system"}` message. Only `model`/`messages`/`stream` are sent — the
/// native API does not take OpenAI's `max_tokens`/`stream_options`.
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
        "messages": messages,
    })
}

/// Parse one native NDJSON line into a `ProviderEvent`.
/// `{"done":true,...}` → `Done`; `{"error":"..."}` → `Error`;
/// `message.content` non-empty → `TextDelta`; everything else
/// (thinking-only/empty/blank/malformed) → `None`. Never panics.
pub fn parse_line(line: &str) -> Option<ProviderEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Some(ProviderEvent::Error(err.to_string()));
    }
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        return Some(ProviderEvent::Done);
    }
    if let Some(text) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
        if !text.is_empty() {
            return Some(ProviderEvent::TextDelta(text.to_string()));
        }
    }
    None
}

/// Streaming Ollama Cloud provider (native Chat API).
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
            .post(format!("{}/api/chat", self.base_url))
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

        // Native /api/chat streams newline-delimited JSON. Buffer raw bytes and
        // split on b'\n' (safe: newline never appears inside a multibyte UTF-8
        // sequence), decoding only complete lines.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        if let Some(pe) = parse_line(&line) {
                            let is_done = matches!(pe, ProviderEvent::Done);
                            if sink.send(pe).await.is_err() {
                                return Ok(());
                            }
                            if is_done {
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    return Ok(());
                }
            }
        }

        // Flush any trailing line without a final newline.
        if !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf);
            if let Some(pe) = parse_line(&line) {
                let _ = sink.send(pe).await;
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
    fn native_body_has_stream_and_system_leading_message_no_openai_fields() {
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
            "messages": [
                { "role": "system", "content": "be terse" },
                { "role": "user", "content": "hi" },
                { "role": "assistant", "content": "hello" },
            ],
        }));
        // native body must NOT carry OpenAI-only fields
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn body_without_system_has_no_system_message() {
        let req = CompletionRequest {
            model: "m".into(), system: None,
            messages: vec![Message { role: MsgRole::User, content: "x".into() }],
            max_tokens: 8,
        };
        assert_eq!(request_body(&req)["messages"], json!([{ "role": "user", "content": "x" }]));
    }

    #[test]
    fn parses_content_delta_line() {
        let line = r#"{"model":"glm-5.2:cloud","message":{"role":"assistant","content":"Hel"},"done":false}"#;
        assert_eq!(parse_line(line), Some(ProviderEvent::TextDelta("Hel".into())));
    }

    #[test]
    fn thinking_only_line_yields_none() {
        let line = r#"{"message":{"role":"assistant","content":"","thinking":"reasoning"},"done":false}"#;
        assert_eq!(parse_line(line), None);
    }

    #[test]
    fn done_line_yields_done() {
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":58}"#;
        assert_eq!(parse_line(line), Some(ProviderEvent::Done));
    }

    #[test]
    fn error_line_yields_error() {
        assert_eq!(parse_line(r#"{"error":"Unauthorized"}"#),
            Some(ProviderEvent::Error("Unauthorized".into())));
    }

    #[test]
    fn empty_and_malformed_lines_yield_none() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
        assert_eq!(parse_line("not json"), None);
    }
}
