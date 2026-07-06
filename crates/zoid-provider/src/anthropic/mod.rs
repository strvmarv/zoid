//! The real streaming Anthropic provider (reqwest + SSE).
//! Task 6: request body. Task 7: SSE parsing. Task 8: the provider + selection.

use crate::{CompletionRequest, MsgRole, ProviderEvent, Usage};
use serde_json::{json, Value};
use std::time::Duration;

pub mod cache;
pub mod request;
pub mod types;

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

    // Prompt caching (Anthropic ephemeral cache): place a cache breakpoint on the
    // system block and on the last message. Anthropic caches the longest matching
    // prefix (tools → system → messages), so the previous turn's breakpoint —
    // now an interior message — still serves as a cache *read*, while the new
    // breakpoint extends the cached prefix for the next turn. Only the newly
    // appended delta pays the cache-creation surcharge. Prompts below the model's
    // minimum cacheable size are simply not cached (no error, `cached` stays 0).
    let mut messages = messages;
    if let Some(last) = messages.last_mut() {
        let text = last["content"].take();
        last["content"] =
            json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }]);
    }

    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "messages": messages,
    });
    if let Some(sys) = &req.system {
        body["system"] =
            json!([{ "type": "text", "text": sys, "cache_control": { "type": "ephemeral" } }]);
    }
    body
}

/// Map one Anthropic SSE frame to zero-or-more `ProviderEvent`s. Wraps
/// `parse_one` and appends a `Truncated` marker when a `message_delta` reports
/// `stop_reason:"max_tokens"`, so that single frame yields both its `Usage` and
/// the truncation signal. Never panics.
pub fn parse_event(event_type: &str, data: &str) -> Vec<ProviderEvent> {
    let mut out: Vec<ProviderEvent> = parse_one(event_type, data).into_iter().collect();
    if event_type == "message_delta" {
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            if v.get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|s| s.as_str())
                == Some("max_tokens")
            {
                out.push(ProviderEvent::Truncated);
            }
        }
    }
    out
}

/// Map one Anthropic SSE frame to a single `ProviderEvent`. Unhandled or
/// malformed frames return `None` (the caller skips them). Never panics.
fn parse_one(event_type: &str, data: &str) -> Option<ProviderEvent> {
    match event_type {
        "content_block_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let text = v.get("delta")?.get("text")?.as_str()?;
            Some(ProviderEvent::TextDelta(text.to_string()))
        }
        "message_start" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let usage = v.get("message")?.get("usage")?;
            let input = usage.get("input_tokens")?.as_u64()?;
            // Anthropic reports cache-read and cache-creation tokens on separate
            // lines; `input_tokens` counts neither. Fold both back in so the
            // economy total reflects the true prompt size, and expose the
            // cache-read subset as `cached`.
            let read = usage
                .get("cache_read_input_tokens")
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            let creation = usage
                .get("cache_creation_input_tokens")
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            Some(ProviderEvent::Usage(Usage {
                input_tokens: input + read + creation,
                output_tokens: 0,
                cached: read,
            }))
        }
        "message_delta" => {
            let v: Value = serde_json::from_str(data).ok()?;
            let output = v.get("usage")?.get("output_tokens")?.as_u64()?;
            Some(ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: output,
                cached: 0,
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
    crate::parse_data_id_models(body)
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
    /// Idle deadline for the initial response and between streamed SSE events;
    /// see `crate::stream_idle_timeout`.
    idle_timeout: Duration,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("anthropic-api")
                .unwrap_or("https://api.anthropic.com")
                .to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
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

    /// Override the stream idle/response timeout. Primarily for tests (drive a
    /// stalled server without a 120s wait); operators use `ZOID_HTTP_IDLE_SECS`.
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
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
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;

        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&request_body(req))
                .send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                // Surface send-phase transport errors (including the connect
                // timeout) as a stream Error: a bare `return Err` is swallowed
                // by the agent's `let _ = provider.stream(...)`, leaving the
                // user with a silent, unexplained empty turn.
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
            // Bound the error-body read too: a non-2xx header followed by a
            // stalled body would otherwise hang here forever, defeating the fix.
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
        loop {
            let item = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(item)) => item,
                Ok(None) => break, // transport closed normally
                Err(_) => {
                    let _ = sink
                        .send(ProviderEvent::Error(format!(
                            "provider idle timeout: no data for {}s",
                            self.idle_timeout.as_secs()
                        )))
                        .await;
                    break;
                }
            };
            match item {
                Ok(event) => {
                    let mut stop = false;
                    for pe in parse_event(&event.event, &event.data) {
                        if ttft.is_none() {
                            ttft = Some(start.elapsed().as_millis() as u64);
                        }
                        let is_done = matches!(pe, ProviderEvent::Done);
                        if sink.send(pe).await.is_err() {
                            stop = true; // receiver gone
                            break;
                        }
                        if is_done {
                            stop = true;
                            break;
                        }
                    }
                    if stop {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }

        tracing::info!(
            kind = "provider",
            provider = "anthropic",
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
                    // Interior messages stay plain strings; only the last carries
                    // the rolling cache breakpoint.
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": [
                        { "type": "text", "text": "hello", "cache_control": { "type": "ephemeral" } }
                    ] },
                ],
            })
        );
    }

    #[test]
    fn includes_system_as_cacheable_block_when_present() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
        };
        let body = request_body(&req);
        assert_eq!(
            body["system"],
            json!([{ "type": "text", "text": "be terse", "cache_control": { "type": "ephemeral" } }])
        );
    }

    #[test]
    fn caches_only_the_last_message() {
        // A cache breakpoint on the last message caches the whole conversation
        // prefix; interior messages must not each carry one (max 4 breakpoints).
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
            ],
            max_tokens: 8,
            tools: vec![],
        };
        let msgs = request_body(&req)["messages"].clone();
        assert_eq!(msgs[0], json!({ "role": "user", "content": "a" }));
        assert_eq!(msgs[1], json!({ "role": "assistant", "content": "b" }));
        assert_eq!(
            msgs[2],
            json!({ "role": "user", "content": [
                { "type": "text", "text": "c", "cache_control": { "type": "ephemeral" } }
            ] })
        );
    }

    #[test]
    fn parses_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        assert_eq!(
            parse_event("content_block_delta", data),
            vec![ProviderEvent::TextDelta("Hello".into())]
        );
    }

    #[test]
    fn parses_message_stop_as_done() {
        assert_eq!(
            parse_event("message_stop", r#"{"type":"message_stop"}"#),
            vec![ProviderEvent::Done]
        );
    }

    #[test]
    fn parses_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        assert_eq!(
            parse_event("message_delta", data),
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: 12,
                cached: 0
            })]
        );
    }

    #[test]
    fn parses_message_start_input_usage() {
        let data =
            r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}"#;
        assert_eq!(
            parse_event("message_start", data),
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 7,
                output_tokens: 0,
                cached: 0
            })]
        );
    }

    #[test]
    fn message_start_folds_cache_tokens_into_input_and_reports_cached() {
        // input_tokens excludes cache lines; total prompt = 7 + 40 read + 3 creation.
        let data = r#"{"type":"message_start","message":{"usage":{"input_tokens":7,"cache_read_input_tokens":40,"cache_creation_input_tokens":3}}}"#;
        assert_eq!(
            parse_event("message_start", data),
            vec![ProviderEvent::Usage(Usage {
                input_tokens: 50,
                output_tokens: 0,
                cached: 40,
            })]
        );
    }

    #[test]
    fn parses_error() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            parse_event("error", data),
            vec![ProviderEvent::Error("Overloaded".into())]
        );
    }

    #[test]
    fn ignores_unhandled_frames() {
        assert!(parse_event("ping", "{}").is_empty());
        assert!(parse_event("content_block_start", r#"{"type":"content_block_start"}"#).is_empty());
        assert!(parse_event("content_block_stop", r#"{"type":"content_block_stop"}"#).is_empty());
    }

    #[test]
    fn malformed_data_yields_empty_not_panic() {
        assert!(parse_event("content_block_delta", "not json").is_empty());
    }

    #[test]
    fn message_delta_with_max_tokens_stop_yields_usage_then_truncated() {
        // A length-capped turn: the message_delta carries both the output usage
        // and stop_reason:"max_tokens" → Usage then Truncated, in that order.
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":4096}}"#;
        assert_eq!(
            parse_event("message_delta", data),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 0,
                    output_tokens: 4096,
                    cached: 0
                }),
                ProviderEvent::Truncated
            ]
        );
    }

    #[test]
    fn parses_anthropic_model_ids() {
        let body =
            r#"{"data":[{"id":"claude-opus-4-8","type":"model"},{"id":"claude-sonnet-4-6"}]}"#;
        assert_eq!(
            parse_anthropic_models(body),
            vec!["claude-opus-4-8", "claude-sonnet-4-6"]
        );
    }
    #[test]
    fn anthropic_models_bad_is_empty() {
        assert!(parse_anthropic_models("nope").is_empty());
    }

    /// Spawn a throwaway server that accepts one connection, optionally writes
    /// `headers`, then stalls (holds the socket open, sending nothing further),
    /// simulating a hung provider. `None` = never reply; `Some(hdr)` = write
    /// those response headers then go silent. Returns the bound address.
    async fn spawn_stalling_server(headers: Option<&'static [u8]>) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await; // consume the request
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
        }
    }

    #[tokio::test]
    async fn idle_timeout_emits_error_when_stream_stalls() {
        // SSE headers arrive, then the server goes silent: the read loop must
        // give up on the idle deadline instead of awaiting an event forever.
        let addr = spawn_stalling_server(Some(OK_SSE_HEADERS)).await;
        let provider = AnthropicProvider::new("k".into())
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
            "expected a trailing idle-timeout Error, got {got:?}"
        );
    }

    #[tokio::test]
    async fn request_timeout_emits_error_when_no_response() {
        // Server accepts but never sends headers: the initial send() must time
        // out rather than block the turn forever.
        let addr = spawn_stalling_server(None).await;
        let provider = AnthropicProvider::new("k".into())
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
    async fn error_body_timeout_emits_error_when_body_stalls() {
        // Non-2xx headers arrive, then the body stalls (Content-Length promises
        // 100 bytes that never come): the error-path resp.text() read must time
        // out rather than hang. The HTTP status is known from headers, so the
        // surfaced Error still names it.
        let addr = spawn_stalling_server(Some(
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 100\r\n\r\n",
        ))
        .await;
        let provider = AnthropicProvider::new("k".into())
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
}
