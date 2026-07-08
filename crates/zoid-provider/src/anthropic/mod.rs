//! The real streaming Anthropic provider (reqwest + SSE).

pub mod cache;
pub mod parse;
pub mod request;
pub mod types;

use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use parse::ToolUseAccumulator;
use std::time::Duration;
use tokio::sync::mpsc;

/// Default model when `$ZOID_MODEL` is unset (latest Claude Sonnet).
pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

const MAX_RETRIES: u32 = 3;
const BASE_BACKOFF: Duration = Duration::from_millis(500);

/// Extract model ids from an Anthropic `/v1/models` response body. Lenient.
/// Lives here in `mod.rs` (NOT `parse.rs`) because it's about the HTTP models
/// endpoint, not SSE streaming.
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

/// Streaming Anthropic Messages API provider.
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
    /// Beta feature flags sent as the `anthropic-beta` header (comma-joined).
    /// Populated from config or `ZOID_ANTHROPIC_BETAS`. Empty = no header.
    betas: Vec<String>,
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
            betas: Vec::new(),
        }
    }

    /// Override the default base URL. Empty/whitespace ignored; trailing slash trimmed.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }

    /// Override the stream idle/response timeout. Primarily for tests.
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }

    /// Set the `anthropic-beta` header flags. Empty clears them.
    pub fn with_betas(mut self, betas: Vec<String>) -> Self {
        self.betas = betas;
        self
    }

    fn beta_header_value(&self) -> Option<String> {
        if self.betas.is_empty() {
            None
        } else {
            Some(self.betas.join(","))
        }
    }

    /// Build the request headers (x-api-key, anthropic-version, optional beta).
    /// All inserts are fallible `if let Ok` — never panics on a malformed api
    /// key or beta value (the header is simply skipped, matching the "never
    /// panic on malformed input" constraint).
    fn request_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = self.api_key.as_str().parse() {
            headers.insert("x-api-key", v);
        }
        if let Ok(v) = "2023-06-01".parse() {
            headers.insert("anthropic-version", v);
        }
        if let Ok(v) = "application/json".parse() {
            headers.insert("content-type", v);
        }
        if let Some(beta) = self.beta_header_value() {
            if let Ok(v) = beta.parse() {
                headers.insert("anthropic-beta", v);
            }
        }
        headers
    }

    /// Build request headers, merging per-request thinking betas with the
    /// provider's static betas. Used when thinking is enabled on Budget or
    /// Adaptive models that need the `extended-thinking` beta header.
    fn request_headers_with_thinking(&self, req: &CompletionRequest) -> reqwest::header::HeaderMap {
        let mut headers = self.request_headers();
        let thinking_betas = request::thinking_betas(req);
        if !thinking_betas.is_empty() {
            let mut all_betas = self.betas.clone();
            for b in &thinking_betas {
                if !all_betas.contains(b) {
                    all_betas.push(b.clone());
                }
            }
            if let Ok(v) = all_betas.join(",").parse() {
                headers.insert("anthropic-beta", v);
            }
        }
        headers
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        // `fetch_model_info` is NOT overridden — inherits the trait default
        // `Ok(None)` (spec §7.4). Only `stream` and `list_models` are impl'd.
        self.stream_with_retries(req, &sink, 0).await
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

impl AnthropicProvider {
    /// Connect-phase send with bounded 429 retry. `attempt` is the zero-based
    /// retry index (0 = first try). On 429 with `attempt < MAX_RETRIES`, sleep
    /// `retry-after` (or exponential backoff) + jitter, then recurse with
    /// `attempt + 1`. The recursion is bounded by the `attempt < MAX_RETRIES`
    /// check *before* recursing, so `attempt` strictly grows — unlike a naive
    /// `stream()` re-entry that would reset the counter. The recursive call is
    /// `Box::pin`'d (rustc requires indirection for recursive async fns, E0733);
    /// depth is bounded by `MAX_RETRIES` so it does not grow unboundedly.
    async fn stream_with_retries(
        &self,
        req: &CompletionRequest,
        sink: &mpsc::Sender<ProviderEvent>,
        attempt: u32,
    ) -> Result<()> {
        let body = request::build(req);
        let url = format!("{}/v1/messages", self.base_url);

        let send = self
            .client
            .post(&url)
            .headers(self.request_headers_with_thinking(req))
            .json(&body)
            .send();
        let resp = match tokio::time::timeout(self.idle_timeout, send).await {
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

        // 429 retry (connect-phase only). Mid-stream overload is terminal.
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            if attempt < MAX_RETRIES {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| BASE_BACKOFF.saturating_mul(2u32.pow(attempt)));
                let jitter = Duration::from_millis(rand_jitter_ms());
                tracing::warn!(attempt, "anthropic 429; retrying after backoff");
                tokio::time::sleep(retry_after + jitter).await;
                // Box::pin the recursion: rustc requires indirection for
                // recursive async fns (E0733). Depth is still bounded by
                // `attempt < MAX_RETRIES`, so this does not grow unboundedly.
                return Box::pin(self.stream_with_retries(req, sink, attempt + 1)).await;
            }
            // exhausted: surface the 429 as an Error
            let _ = sink
                .send(ProviderEvent::Error(format!(
                    "HTTP 429: retried {MAX_RETRIES} times"
                )))
                .await;
            return Ok(());
        }

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

        self.stream_sse(resp, sink, &req.model).await
    }

    /// Drive the SSE stream after a successful 200 response. Owns the
    /// `ToolUseAccumulator` and maps each frame via `parse::event`.
    async fn stream_sse(
        &self,
        resp: reqwest::Response,
        sink: &mpsc::Sender<ProviderEvent>,
        model: &str,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;
        let mut acc = ToolUseAccumulator::default();
        let mut stream = resp.bytes_stream().eventsource();
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
                    break;
                }
            };
            match item {
                Ok(event) => {
                    let mut stop = false;
                    // Deserialize the SSE data as a typed StreamEvent; unknown
                    // types fall through to None (no panic).
                    let frame: Option<types::StreamEvent> = serde_json::from_str(&event.data).ok();
                    if let Some(frame) = frame {
                        for pe in parse::event(frame, &mut acc) {
                            if ttft.is_none() {
                                ttft = Some(start.elapsed().as_millis() as u64);
                            }
                            let is_done = matches!(pe, ProviderEvent::Done);
                            if sink.send(pe).await.is_err() {
                                stop = true;
                                break;
                            }
                            if is_done {
                                stop = true;
                                break;
                            }
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
            model = model,
            ttft_ms = ttft.unwrap_or(0),
            total_ms = start.elapsed().as_millis() as u64,
            "provider stream complete"
        );
        Ok(())
    }
}

/// Non-cryptographic jitter from wall-clock nanos; sufficient for retry
/// spacing (avoids pulling the `rand` workspace dep into this crate).
fn rand_jitter_ms() -> u64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % 250
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ProviderEvent};
    use std::time::Duration;
    use tokio::sync::mpsc;

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
    fn with_betas_sets_header_value() {
        let p = AnthropicProvider::new("k".into())
            .with_betas(vec!["extended-thinking-2025-05-14".into()]);
        assert_eq!(
            p.beta_header_value().as_deref(),
            Some("extended-thinking-2025-05-14")
        );
    }

    #[test]
    fn empty_betas_omits_header() {
        let p = AnthropicProvider::new("k".into());
        assert!(p.beta_header_value().is_none());
    }

    #[test]
    fn multiple_betas_are_comma_joined() {
        let p = AnthropicProvider::new("k".into()).with_betas(vec![
            "extended-thinking-2025-05-14".into(),
            "fine-grained-tool-streaming-2025-05-14".into(),
        ]);
        assert_eq!(
            p.beta_header_value().as_deref(),
            Some("extended-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14")
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
            thinking: crate::ThinkingMode::Off,
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
        // surfaced Error still names it. (Uses 500, not 429, because 429 now
        // goes through the connect-phase retry path rather than this body-read
        // timeout path.)
        let addr = spawn_stalling_server(Some(
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100\r\n\r\n",
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
            matches!(got.last(), Some(ProviderEvent::Error(e)) if e.contains("500")),
            "expected an HTTP 500 Error, got {got:?}"
        );
    }

    /// Spawn a server that responds 429 once (with retry-after: 0) then 200
    /// with a minimal SSE stream. Returns the bound address. Two accepts on
    /// one listener: first gets the 429, second gets the 200 + SSE.
    async fn spawn_429_then_ok_server() -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // first connection: 429
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 0\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.flush().await;
            drop(sock);
            // second connection: 200 + minimal SSE (a single message_stop)
            let (mut sock2, _) = listener.accept().await.unwrap();
            let mut buf2 = [0u8; 4096];
            let _ = sock2.read(&mut buf2).await;
            let _ = sock2
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
                .await;
            let _ = sock2.flush().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        addr
    }

    #[tokio::test]
    async fn retry_on_429_then_succeeds() {
        let addr = spawn_429_then_ok_server().await;
        let provider = AnthropicProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(5));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        // after retry, the stream emits Done (from the message_stop frame)
        assert!(
            got.iter().any(|e| matches!(e, ProviderEvent::Done)),
            "expected a Done after retry, got {got:?}"
        );
    }

    /// A server that always returns 429 for up to MAX_RETRIES+2 connections.
    /// After the retry loop exhausts (MAX_RETRIES retries), the provider must
    /// surface an Error mentioning 429.
    async fn spawn_always_429_server() -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // MAX_RETRIES + 2 = 5 connections; each returns 429.
            for _ in 0..5 {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    let _ = sock.flush().await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn retry_exhausted_surfaces_error() {
        let addr = spawn_always_429_server().await;
        let provider = AnthropicProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(5));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(
            matches!(got.last(), Some(ProviderEvent::Error(e)) if e.contains("429")),
            "expected a trailing 429 Error, got {got:?}"
        );
    }
}
