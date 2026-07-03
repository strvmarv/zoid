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
            // NOTE: Anthropic is text-only this phase (P1b). This match exists
            // only to stay exhaustive after `MsgRole` gained `Tool`; the
            // Anthropic Messages API has NO "tool" role — tool results are a
            // `user` message carrying a `tool_result` content block. We map
            // `Tool` to a *valid* role ("user") rather than an invalid "tool"
            // so no bogus wire output can leak; real Anthropic tool-calling
            // (the proper `tool_result` mapping) is a deferred follow-up (P1b.1).
            let role = match m.role {
                MsgRole::User | MsgRole::Tool => "user",
                MsgRole::Assistant => "assistant",
            };
            json!({
                "role": role,
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
            let input = v
                .get("message")?
                .get("usage")?
                .get("input_tokens")?
                .as_u64()?;
            Some(ProviderEvent::Usage(Usage {
                input_tokens: input,
                output_tokens: 0,
            }))
        }
        "message_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let output = v.get("usage")?.get("output_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: output,
            }))
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

/// Extract model ids from an Anthropic `/v1/models` response body. Lenient.
pub fn parse_anthropic_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

use crate::Provider;
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio::sync::mpsc;

/// Streaming Anthropic Messages API provider.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("anthropic-api")
                .unwrap_or("https://api.anthropic.com")
                .to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Override the default base URL (config `base_url`). An empty/whitespace
    /// value is ignored (keeps the built-in default), and a trailing slash is
    /// trimmed so the `{base}/v1/messages` join never produces a double slash.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        req: &crate::CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body(req))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let _ = sink
                .send(ProviderEvent::Error(format!("HTTP {status}: {text}")))
                .await;
            return Ok(());
        }

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if let Some(pe) = parse_event(&event.event, &event.data) {
                        let is_done = matches!(pe, ProviderEvent::Done);
                        if sink.send(pe).await.is_err() {
                            break; // receiver gone
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

    async fn list_models(&self) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;
        Ok(parse_anthropic_models(&resp.text().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use serde_json::json;

    #[test]
    fn new_uses_default_base_url() {
        assert_eq!(
            AnthropicProvider::new("k".into()).base_url,
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn with_base_url_overrides_and_trims_trailing_slash() {
        let p =
            AnthropicProvider::new("k".into()).with_base_url("https://proxy.internal/anthropic/");
        assert_eq!(p.base_url, "https://proxy.internal/anthropic");
    }

    #[test]
    fn with_base_url_ignores_empty_or_blank() {
        assert_eq!(
            AnthropicProvider::new("k".into())
                .with_base_url("")
                .base_url,
            "https://api.anthropic.com"
        );
        assert_eq!(
            AnthropicProvider::new("k".into())
                .with_base_url("   ")
                .base_url,
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn builds_messages_body_with_stream_flag() {
        let req = CompletionRequest {
            model: "claude-sonnet-4-6".into(),
            system: None,
            messages: vec![Message::user("hi"), Message::assistant("hello")],
            max_tokens: 1024,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(
            body,
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 1024,
                "stream": true,
                "messages": [
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": "hello" },
                ],
            })
        );
    }

    #[test]
    fn includes_system_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(body["system"], json!("be terse"));
    }

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(
            parse_event("content_block_delta", data),
            Some(ProviderEvent::TextDelta("Hello".into()))
        );
    }

    #[test]
    fn parses_message_stop_as_done() {
        assert_eq!(
            parse_event("message_stop", r#"{"type":"message_stop"}"#),
            Some(ProviderEvent::Done)
        );
    }

    #[test]
    fn parses_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        assert_eq!(
            parse_event("message_delta", data),
            Some(ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: 12
            }))
        );
    }

    #[test]
    fn parses_message_start_input_usage() {
        let data =
            r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#;
        assert_eq!(
            parse_event("message_start", data),
            Some(ProviderEvent::Usage(Usage {
                input_tokens: 7,
                output_tokens: 0
            }))
        );
    }

    #[test]
    fn parses_error() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            parse_event("error", data),
            Some(ProviderEvent::Error("Overloaded".into()))
        );
    }

    #[test]
    fn ignores_unhandled_frames() {
        assert_eq!(parse_event("ping", "{}"), None);
        assert_eq!(
            parse_event("content_block_start", r#"{"type":"content_block_start"}"#),
            None
        );
        assert_eq!(
            parse_event("content_block_stop", r#"{"type":"content_block_stop"}"#),
            None
        );
    }

    #[test]
    fn malformed_data_yields_none_not_panic() {
        assert_eq!(parse_event("content_block_delta", "not json"), None);
    }

    #[test]
    fn parses_anthropic_model_ids() {
        let body = r#"{"data":[{"id":"claude-opus-4-8","type":"model"},{"id":"claude-sonnet-4-6"}]}"#;
        assert_eq!(parse_anthropic_models(body), vec!["claude-opus-4-8", "claude-sonnet-4-6"]);
    }
    #[test]
    fn anthropic_models_bad_is_empty() {
        assert!(parse_anthropic_models("nope").is_empty());
    }
}
