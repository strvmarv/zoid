//! The Ollama Cloud provider via the native Chat API
//! (`POST {base}/api/chat`, NDJSON streaming, `"done":true` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// Default model when `$ZOID_MODEL` is unset (GLM on Ollama Cloud).
pub const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2:cloud";

/// Default context window requested from a **local** Ollama daemon when
/// `ZOID_NUM_CTX` is unset. A local daemon applies its own (small) default and
/// then silently truncates an over-long prompt rather than erroring, so the
/// client must ask. zoid's fixed per-turn overhead is ~1,850 tokens (13 tool
/// schemas ≈ 1,700 tokens plus the system prompt), which 32K leaves ample room
/// around. Never sent to Ollama Cloud, which sizes context server-side.
pub const DEFAULT_LOCAL_NUM_CTX: u32 = 32768;

/// Parse a `ZOID_NUM_CTX` value. Mirrors the contract of
/// `crate::stream_idle_timeout` (lib.rs:44): a positive integer wins, and
/// anything else — absent, empty, zero, negative, non-numeric, or beyond u32 —
/// falls back to `DEFAULT_LOCAL_NUM_CTX`.
pub fn parse_num_ctx(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_LOCAL_NUM_CTX)
}

/// The configured local context window. Precedence: `ZOID_NUM_CTX` env var
/// (back-compat) > `[economy] num_ctx` in config.toml > `DEFAULT_LOCAL_NUM_CTX`.
/// Read at provider-construction time by the bin's `ollama-local` branch.
pub fn configured_num_ctx(config_num_ctx: Option<u32>) -> u32 {
    if let Some(raw) = std::env::var("ZOID_NUM_CTX").ok() {
        // Env var present: it wins (back-compat). Invalid value → default.
        return parse_num_ctx(Some(&raw));
    }
    config_num_ctx.unwrap_or(DEFAULT_LOCAL_NUM_CTX)
}

/// Build the native Ollama `/api/chat` request body. System prompt is a leading
/// `{"role":"system"}` message. Only `model`/`messages`/`stream` are sent — the
/// native API does not take OpenAI's `max_tokens`/`stream_options`.
///
/// `num_ctx` is `Some` only for **local** daemons (`ollama-local`), which
/// otherwise apply their own default and silently truncate. It is `None` for
/// Ollama Cloud, which sizes context server-side — and when `None` the emitted
/// body is byte-identical to the pre-`num_ctx` body.
pub fn request_body(req: &CompletionRequest, num_ctx: Option<u32>) -> Value {
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
                        m.tool_calls.iter()
                            .map(|tc| json!({ "function": { "name": tc.name, "arguments": tc.args } }))
                            .collect(),
                    );
                }
                messages.push(obj);
            }
            MsgRole::Tool => messages.push(json!({
                "role": "tool",
                "content": m.content,
                "tool_name": m.tool_name.clone().unwrap_or_default(),
            })),
        }
    }
    if let Some(text) = &req.reassert {
        messages.push(json!({ "role": "system", "content": text }));
    }
    // Only emit `think` for models that support thinking. The capability gate
    // in resolve_thinking should have caught unsupported models, but this is
    // defensive — never send `think: true` to a model that might not handle it.
    let info = crate::model::model_info(&req.model);
    let think = match req.thinking {
        crate::ThinkingMode::Off => false,
        crate::ThinkingMode::Auto | crate::ThinkingMode::Effort(_) => {
            info.thinking != crate::model::ThinkingSupport::None
        }
    };
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "messages": messages,
        // Keep the model warm between turns (Ollama's analog to prompt caching):
        // hold it loaded for 30m after a response so a coding session's rapid
        // follow-up turns skip the cold reload. The native API has no token-level
        // prompt cache, so `cached` stays 0 for this provider.
        "keep_alive": "30m",
        "think": think,
    });
    if let Some(n) = num_ctx {
        body["options"] = json!({ "num_ctx": n });
    }
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools.iter()
                .map(|t| json!({
                    "type": "function",
                    "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
                }))
                .collect(),
        );
    }
    body
}

/// Parse one native NDJSON line into zero or more `ProviderEvent`s, in order:
/// `error` short-circuits to `[Error]`; otherwise non-empty `message.content`
/// → `TextDelta`, each `message.tool_calls[]` → `ToolCall`, then `done:true`
/// → `Done`. Empty/thinking-only/blank/malformed lines → `[]`. Never panics.
pub fn parse_line(
    line: &str,
    last_prompt_eval: &std::sync::atomic::AtomicU64,
) -> Vec<ProviderEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return vec![ProviderEvent::Error(err.to_string())];
    }

    let mut out = Vec::new();
    if let Some(text) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        if !text.is_empty() {
            out.push(ProviderEvent::TextDelta(text.to_string()));
        }
    }
    if let Some(thinking) = v
        .get("message")
        .and_then(|m| m.get("thinking"))
        .and_then(|t| t.as_str())
    {
        if !thinking.is_empty() {
            out.push(ProviderEvent::ThinkingDelta(thinking.to_string()));
        }
    }
    if let Some(calls) = v
        .get("message")
        .and_then(|m| m.get("tool_calls"))
        .and_then(|c| c.as_array())
    {
        for call in calls {
            if let Some(func) = call.get("function") {
                let name = func
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let args = coerce_tool_args(func.get("arguments"));
                let id = call
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or_default()
                    .to_string();
                out.push(ProviderEvent::ToolCall(ToolCall { id, name, args }));
            }
        }
    }
    // Token accounting: the native /api/chat final frame carries
    // `prompt_eval_count` (input tokens) and `eval_count` (output tokens) as a
    // single cumulative snapshot. Emit Usage ONLY on that final (`done`) frame:
    // `ProviderEvent::Usage` is an additive delta the agent sums, so emitting it
    // only once here keeps the economy ledger from double-counting. Ordered
    // before Done: the agent accumulates Usage during the stream and records it
    // when the turn ends.
    let is_done = v.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    if is_done {
        let input = v.get("prompt_eval_count").and_then(|n| n.as_u64());
        let output = v.get("eval_count").and_then(|n| n.as_u64());
        if input.is_some() || output.is_some() {
            let curr = input.unwrap_or(0);
            // Approximate the prompt-cache hit: Ollama's `keep_alive` holds the
            // model's KV cache warm for 30m, so the overlap between this prompt
            // and the previous sub-turn's prompt is served from the warm cache.
            // The native `/api/chat` `done` frame reports the whole prompt as
            // `prompt_eval_count` with no cache-read breakdown, so we derive it:
            // `cached = min(curr, prev)`. The first sub-turn (prev=0) yields
            // cached=0 — correct, nothing was warm yet. Store curr for next time.
            use std::sync::atomic::Ordering;
            let prev = last_prompt_eval.swap(curr, Ordering::Relaxed);
            let cached_approx = curr.min(prev);
            out.push(ProviderEvent::Usage(Usage {
                input_tokens: curr,
                output_tokens: output.unwrap_or(0),
                cached: cached_approx,
                thinking_tokens: 0,
            }));
        }
        // `done_reason:"length"` = the model hit the output cap; its reply is
        // truncated. Surface it (before Done) so the turn isn't treated as a
        // clean stop.
        if v.get("done_reason").and_then(|r| r.as_str()) == Some("length") {
            out.push(ProviderEvent::Truncated);
        }
        out.push(ProviderEvent::Done);
    }
    out
}

/// Coerce a tool-call `arguments` field into a usable arguments value. Ollama
/// family models are inconsistent: some send a JSON object, some a JSON-encoded
/// string, some omit it entirely. A string is parsed as JSON (falling back to
/// `{}` when it isn't valid JSON); an object is kept as-is; anything else
/// (absent, `null`, or a bare scalar/array) becomes `{}` — tool dispatch always
/// expects an object, so a missing/garbage payload is safer as empty than as
/// `Null` (which downstream would have to special-case).
fn coerce_tool_args(raw: Option<&Value>) -> Value {
    match raw {
        // A JSON-encoded string must decode to an object to be usable as args;
        // anything else (invalid JSON, or valid JSON that isn't an object like
        // "[1,2]"/"42") falls back to {}.
        Some(Value::String(s)) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        Some(v @ Value::Object(_)) => v.clone(),
        _ => json!({}),
    }
}

/// Extract model names from an Ollama `/api/tags` response body. Lenient:
/// unknown/!json → empty (the caller falls back to the registry list).
pub fn parse_ollama_tags(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("models").and_then(|m| m.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Extract context window from an Ollama `/api/show` response body. The
/// `model_info` map carries family-specific keys like `glm.context_length`,
/// `llama.context_length`, etc. — we try known keys and fall back to any
/// key ending in `.context_length`. Returns `None` when unparseable.
pub fn parse_ollama_context_window(body: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let info = v.get("model_info")?;
    // Try known family keys first, then any `*.context_length` key.
    for key in &[
        "glm.context_length",
        "llama.context_length",
        "deepseek.context_length",
        "qwen.context_length",
        "mistral.context_length",
    ] {
        if let Some(n) = info.get(key).and_then(|v| v.as_f64()) {
            return Some(n as u64);
        }
    }
    // Fallback: scan for any key ending in `.context_length`.
    if let Some(obj) = info.as_object() {
        for (k, v) in obj {
            if k.ends_with(".context_length") {
                if let Some(n) = v.as_f64() {
                    return Some(n as u64);
                }
            }
        }
    }
    None
}

/// Streaming Ollama Cloud provider (native Chat API).
pub struct OllamaProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    /// Idle deadline for the initial response and between streamed chunks; see
    /// `crate::stream_idle_timeout`.
    idle_timeout: Duration,
    /// The previous sub-turn's `prompt_eval_count` (full prompt size). Ollama's
    /// `keep_alive` holds the model's KV cache warm for 30m, so the bulk of each
    /// new prompt is a re-evaluation of a warm prefix — but the native `/api/chat`
    /// `done` frame reports it all as `prompt_eval_count` with no cache-read
    /// breakdown. We approximate: the overlap with the previous prompt is
    /// "cached" (warm in KV). Cross-stream state so `parse_line`'s `done` frame
    /// can read it. Ollama implicit-cache approximation.
    last_prompt_eval: std::sync::atomic::AtomicU64,
    /// Explicit context window for `options.num_ctx`. `Some` only for
    /// `ollama-local`; `None` for Ollama Cloud, which sizes context
    /// server-side and whose request body must stay byte-identical.
    num_ctx: Option<u32>,
}

impl OllamaProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("ollama-cloud")
                .unwrap_or("https://ollama.com")
                .to_string(),
            client: crate::http_client(),
            idle_timeout: crate::stream_idle_timeout(),
            last_prompt_eval: std::sync::atomic::AtomicU64::new(0),
            num_ctx: None,
        }
    }

    /// Override the default base URL (config `base_url`). An empty/whitespace
    /// value is ignored (keeps the built-in default), and a trailing slash is
    /// trimmed so the `{base}/api/chat` join never produces a double slash.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let b = base_url.into();
        let b = b.trim().trim_end_matches('/');
        if !b.is_empty() {
            self.base_url = b.to_string();
        }
        self
    }

    /// Override the stream idle/response timeout. Primarily for tests (drive a
    /// stalled server without a 300s wait); operators use `ZOID_HTTP_IDLE_SECS`.
    pub fn with_idle_timeout(mut self, idle: Duration) -> Self {
        self.idle_timeout = idle;
        self
    }

    /// Request an explicit context window (`options.num_ctx`). Set only for
    /// `ollama-local`: a local daemon otherwise applies its own default and
    /// silently truncates the prompt — evicting the system prompt and tool
    /// schemas — without ever returning an error.
    pub fn with_num_ctx(mut self, num_ctx: u32) -> Self {
        self.num_ctx = Some(num_ctx);
        self
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let mut ttft: Option<u64> = None;

        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/api/chat", self.base_url))
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&request_body(req, self.num_ctx))
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

        // Native /api/chat streams newline-delimited JSON. Buffer raw bytes and
        // split on b'\n' (safe: newline never appears inside a multibyte UTF-8
        // sequence), decoding only complete lines.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        // Tracks whether the read loop ended via an explicit Done/send-failure/
        // transport-error exit (in which case the trailing-line flush below must
        // be skipped, matching the original early-`return Ok(())` behavior) as
        // opposed to falling out of the loop because the transport simply closed.
        let mut ended_early = false;
        'read: loop {
            let chunk = match tokio::time::timeout(self.idle_timeout, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break 'read, // transport closed normally
                Err(_) => {
                    let _ = sink
                        .send(ProviderEvent::Error(format!(
                            "provider idle timeout: no data for {}s",
                            self.idle_timeout.as_secs()
                        )))
                        .await;
                    ended_early = true;
                    break 'read;
                }
            };
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        for pe in parse_line(&line, &self.last_prompt_eval) {
                            if ttft.is_none() {
                                ttft = Some(start.elapsed().as_millis() as u64);
                            }
                            let is_done = matches!(pe, ProviderEvent::Done);
                            if sink.send(pe).await.is_err() {
                                ended_early = true;
                                break 'read;
                            }
                            if is_done {
                                ended_early = true;
                                break 'read;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = sink.send(ProviderEvent::Error(e.to_string())).await;
                    ended_early = true;
                    break 'read;
                }
            }
        }

        // Flush any trailing line without a final newline.
        if !ended_early && !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf);
            for pe in parse_line(&line, &self.last_prompt_eval) {
                if ttft.is_none() {
                    ttft = Some(start.elapsed().as_millis() as u64);
                }
                if sink.send(pe).await.is_err() {
                    break;
                }
            }
        }

        tracing::info!(
            kind = "provider",
            provider = "ollama",
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
            .get(format!("{}/api/tags", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Ok(parse_ollama_tags(&resp.text().await?))
    }

    async fn fetch_model_info(&self, model: &str) -> Result<Option<crate::model::ModelInfo>> {
        let resp = self
            .client
            .post(format!("{}/api/show", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await?;
        let body = resp.text().await?;
        let window = parse_ollama_context_window(&body);
        Ok(window.map(|w| crate::model::ModelInfo {
            // A local daemon silently truncates past its actual context window.
            // If we requested `num_ctx`, clamp the reported window to that value
            // so the preflight gate and the economy view reflect the real limit
            // — not the model's theoretical max that the daemon won't honor.
            context_window: self.num_ctx
                .filter(|&n| (n as u64) < w)
                .map(|n| n as u64)
                .unwrap_or(w),
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: crate::model::ThinkingSupport::None,
            thinking_wire: crate::model::ThinkingWireShape::None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolCall, ToolSpec};
    use serde_json::json;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn parse_num_ctx_accepts_positive_integers() {
        assert_eq!(parse_num_ctx(Some("32768")), 32768);
        assert_eq!(parse_num_ctx(Some("  8192  ")), 8192);
        assert_eq!(parse_num_ctx(Some("1")), 1);
    }

    #[test]
    fn parse_num_ctx_falls_back_on_invalid_input() {
        assert_eq!(parse_num_ctx(None), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("")), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("0")), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("-4096")), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("lots")), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("32768.5")), DEFAULT_LOCAL_NUM_CTX);
        assert_eq!(parse_num_ctx(Some("99999999999")), DEFAULT_LOCAL_NUM_CTX);
    }

    #[test]
    fn default_local_num_ctx_clears_zoid_fixed_overhead() {
        assert!(DEFAULT_LOCAL_NUM_CTX >= 32768);
    }

    fn ctx_req() -> CompletionRequest {
        CompletionRequest {
            model: "ornith:9b".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi")],
            max_tokens: 1024,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        }
    }

    #[test]
    fn local_body_carries_options_num_ctx() {
        let body = request_body(&ctx_req(), Some(32768));
        assert_eq!(body["options"]["num_ctx"], json!(32768));
    }

    #[test]
    fn cloud_body_omits_options_entirely() {
        let body = request_body(&ctx_req(), None);
        assert!(body.get("options").is_none());
    }

    #[test]
    fn num_ctx_does_not_disturb_other_body_fields() {
        let with = request_body(&ctx_req(), Some(8192));
        let without = request_body(&ctx_req(), None);
        for key in ["model", "stream", "messages", "keep_alive", "think"] {
            assert_eq!(with[key], without[key], "field `{key}` changed");
        }
    }

    #[test]
    fn new_defaults_num_ctx_to_none_for_cloud() {
        assert_eq!(OllamaProvider::new("k".into()).num_ctx, None);
    }

    #[test]
    fn with_num_ctx_sets_the_field() {
        let p = OllamaProvider::new(String::new())
            .with_base_url("http://localhost:11434")
            .with_num_ctx(16384);
        assert_eq!(p.num_ctx, Some(16384));
    }

    /// Call `parse_line` with a fresh (zero) `last_prompt_eval`. Used by tests
    /// that don't exercise the implicit-cache approximation (the first sub-turn:
    /// prev=0, so cached=0, matching the old behavior).
    fn parse_first(line: &str) -> Vec<ProviderEvent> {
        parse_line(line, &AtomicU64::new(0))
    }

    /// Call `parse_line` with a shared `last_prompt_eval` across multiple
    /// sub-turns, so the implicit-cache approximation sees a growing prefix.
    fn parse_seq(lines: &[&str]) -> Vec<Vec<ProviderEvent>> {
        let le = AtomicU64::new(0);
        lines.iter().map(|l| parse_line(l, &le)).collect()
    }

    #[test]
    fn new_uses_default_base_url() {
        assert_eq!(
            OllamaProvider::new("k".into()).base_url,
            "https://ollama.com"
        );
    }

    #[test]
    fn with_base_url_overrides_and_trims_trailing_slash() {
        let p = OllamaProvider::new("k".into()).with_base_url("http://localhost:11434/");
        assert_eq!(p.base_url, "http://localhost:11434");
    }

    #[test]
    fn with_base_url_ignores_empty_or_blank() {
        assert_eq!(
            OllamaProvider::new("k".into()).with_base_url("").base_url,
            "https://ollama.com"
        );
        assert_eq!(
            OllamaProvider::new("k".into())
                .with_base_url("   ")
                .base_url,
            "https://ollama.com"
        );
    }

    #[test]
    fn native_body_has_stream_and_system_leading_message_no_openai_fields() {
        let req = CompletionRequest {
            model: "glm-5.2:cloud".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("hi"), Message::assistant("hello")],
            max_tokens: 1024,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req, None);
        assert_eq!(
            body,
            json!({
                "model": "glm-5.2:cloud",
                "stream": true,
                "messages": [
                    { "role": "system", "content": "be terse" },
                    { "role": "user", "content": "hi" },
                    { "role": "assistant", "content": "hello" },
                ],
                "keep_alive": "30m",
                "think": false,
            })
        );
    // native body must NOT carry OpenAI-only fields
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("stream_options").is_none());
        // keeps the model warm between turns (Ollama's caching analog)
        assert_eq!(body["keep_alive"], json!("30m"));
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
            reassert: None,
        };
        assert_eq!(
            request_body(&req, None)["messages"],
            json!([{ "role": "user", "content": "x" }])
        );
    }

    #[test]
    fn body_includes_tools_and_tool_messages() {
        let req = CompletionRequest {
            model: "glm-5.2:cloud".into(),
            system: None,
            messages: vec![
                Message::user("read foo"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "".into(),
                        name: "read_file".into(),
                        args: json!({"path": "foo"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                Message::tool("read_file", "bar"),
            ],
            max_tokens: 8,
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "read a file".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req, None);
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": { "name": "read_file", "description": "read a file", "parameters": {"type": "object"} }
            }])
        );
        assert_eq!(
            body["messages"],
            json!([
                { "role": "user", "content": "read foo" },
                { "role": "assistant", "content": "", "tool_calls": [ { "function": { "name": "read_file", "arguments": {"path": "foo"} } } ] },
                { "role": "tool", "content": "bar", "tool_name": "read_file" },
            ])
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
            reassert: None,
        };
        assert!(request_body(&req, None).get("tools").is_none());
    }


    #[test]
    fn body_emits_think_false_when_thinking_auto_for_unknown_model() {
        // "m" is an unknown model → ThinkingSupport::None → think=false (defensive)
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
            reassert: None,
        };
        let body = request_body(&req, None);
        assert_eq!(body["think"], json!(false), "unknown model with ThinkingSupport::None must get think=false");
    }

    #[test]
    fn body_emits_think_false_when_thinking_off() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let body = request_body(&req, None);
        assert_eq!(body["think"], json!(false));
    }

    #[test]
    fn body_emits_think_false_for_non_thinking_model_even_when_auto() {
        // glm-5.2:cloud has ThinkingSupport::None — think must be false
        let req = CompletionRequest {
            model: "glm-5.2:cloud".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
            reassert: None,
        };
        let body = request_body(&req, None);
        assert_eq!(body["think"], json!(false), "non-thinking model must get think=false even when ThinkingMode::Auto");
    }

    #[test]
    fn reassert_pushes_trailing_system_message_ollama() {
        let mut req = CompletionRequest {
            model: "m".into(), system: None, messages: vec![Message::user("hi")],
            max_tokens: 16, tools: vec![], thinking: crate::ThinkingMode::Off, reassert: None,
        };
        req.reassert = Some("STANDING REMINDER".into());
        let body = request_body(&req, None);
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "system");
        assert_eq!(last["content"], "STANDING REMINDER");
    }

    #[test]
    fn parse_line_thinking_field_emits_thinking_delta() {
        let line = r#"{"message":{"role":"assistant","content":"answer","thinking":"reasoning"},"done":false}"#;
        let atomic = std::sync::atomic::AtomicU64::new(0);
        let events = parse_line(line, &atomic);
        assert!(events.contains(&ProviderEvent::ThinkingDelta("reasoning".into())),
            "thinking field must emit ThinkingDelta, got: {:?}", events);
        assert!(events.contains(&ProviderEvent::TextDelta("answer".into())),
            "content must still emit TextDelta, got: {:?}", events);
    }

    #[test]
    fn parses_content_delta_line() {
        let line = r#"{"model":"glm-5.2:cloud","message":{"role":"assistant","content":"Hel"},"done":false}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn thinking_only_line_yields_thinking_delta() {
        let line =
            r#"{"message":{"role":"assistant","content":"","thinking":"reasoning"},"done":false}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::ThinkingDelta("reasoning".into())]
        );
    }

    #[test]
    fn done_line_with_counts_yields_usage_then_done() {
        // The final frame carries prompt_eval_count (input) + eval_count (output);
        // both surface as a Usage event ahead of Done.
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":124,"eval_count":58}"#;
        assert_eq!(
            parse_first(line),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 124,
                    output_tokens: 58,
                    cached: 0,
                    thinking_tokens: 0,
                }),
                ProviderEvent::Done
            ]
        );
    }

    #[test]
    fn partial_counts_default_missing_side_to_zero() {
        // Only eval_count present → input defaults to 0, still emits Usage.
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"eval_count":58}"#;
        assert_eq!(
            parse_first(line),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 0,
                    output_tokens: 58,
                    cached: 0,
                    thinking_tokens: 0,
                }),
                ProviderEvent::Done
            ]
        );
    }

    #[test]
    fn done_line_without_counts_yields_only_done() {
        let line =
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        assert_eq!(parse_first(line), vec![ProviderEvent::Done]);
    }

    #[test]
    fn done_reason_length_yields_usage_then_truncated_then_done() {
        // A length-capped final frame: Usage, then Truncated, then Done — in
        // that order (the agent accumulates Usage, warns on Truncated, breaks on
        // Done).
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"length","prompt_eval_count":124,"eval_count":4096}"#;
        assert_eq!(
            parse_first(line),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 124,
                    output_tokens: 4096,
                    cached: 0,
                    thinking_tokens: 0,
                }),
                ProviderEvent::Truncated,
                ProviderEvent::Done
            ]
        );
    }

    #[test]
    fn counts_on_a_non_final_frame_do_not_emit_usage() {
        // Only the terminal `done` frame carries token counts. If a non-final
        // frame ever includes them they must NOT emit Usage — it's summed by the
        // agent, so a stray emission would double-count. Content still surfaces.
        let line = r#"{"message":{"role":"assistant","content":"hi"},"done":false,"prompt_eval_count":100,"eval_count":50}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::TextDelta("hi".into())]
        );
    }

    #[test]
    fn error_line_yields_error() {
        assert_eq!(
            parse_first(r#"{"error":"Unauthorized"}"#),
            vec![ProviderEvent::Error("Unauthorized".into())]
        );
    }

    #[test]
    fn empty_and_malformed_lines_yield_none() {
        assert!(parse_first("").is_empty());
        assert!(parse_first("   ").is_empty());
        assert!(parse_first("not json").is_empty());
    }

    #[test]
    fn parses_tool_call_line() {
        let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"a.txt"}}}]},"done":false}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "read_file".into(),
                args: json!({"path": "a.txt"})
            })]
        );
    }

    #[test]
    fn tool_call_arguments_encoded_as_string_are_parsed() {
        // Some models emit `arguments` as a JSON-encoded string rather than an
        // object; it must be decoded to the object so dispatch sees real args.
        let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"done":false}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "read_file".into(),
                args: json!({"path": "a.txt"})
            })]
        );
    }

    #[test]
    fn tool_call_missing_or_garbage_arguments_default_to_empty_object() {
        // Missing arguments → {}.
        let missing = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_dir"}}]},"done":false}"#;
        assert_eq!(
            parse_first(missing),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "list_dir".into(),
                args: json!({})
            })]
        );
        // A non-JSON string → {} (rather than an unusable raw string).
        let garbage = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_dir","arguments":"not json"}}]},"done":false}"#;
        assert_eq!(
            parse_first(garbage),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "list_dir".into(),
                args: json!({})
            })]
        );
        // A string that decodes to valid-but-non-object JSON → {} (tools take
        // object args; an array/scalar is unusable).
        let non_object = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_dir","arguments":"[1,2]"}}]},"done":false}"#;
        assert_eq!(
            parse_first(non_object),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "list_dir".into(),
                args: json!({})
            })]
        );
    }

    #[test]
    fn parses_text_then_done_as_two_events() {
        let line = r#"{"message":{"role":"assistant","content":"hi"},"done":true}"#;
        assert_eq!(
            parse_first(line),
            vec![ProviderEvent::TextDelta("hi".into()), ProviderEvent::Done]
        );
    }

    #[test]
    fn parses_ollama_tags_names() {
        let body = r#"{"models":[{"name":"glm-5.2:cloud"},{"name":"llama3.1:70b"}]}"#;
        assert_eq!(
            parse_ollama_tags(body),
            vec!["glm-5.2:cloud", "llama3.1:70b"]
        );
    }
    #[test]
    fn ollama_tags_empty_or_bad_is_empty() {
        assert!(parse_ollama_tags("{}").is_empty());
        assert!(parse_ollama_tags("not json").is_empty());
    }

    #[test]
    fn parse_context_window_from_show_response() {
        let body = r#"{"model_info":{"glm.context_length":256000.0}}"#;
        assert_eq!(parse_ollama_context_window(body), Some(256_000));
    }

    #[test]
    fn parse_context_window_fallback_to_any_dot_context_length() {
        let body = r#"{"model_info":{"some.family.context_length":128000.0}}"#;
        assert_eq!(parse_ollama_context_window(body), Some(128_000));
    }

    #[test]
    fn parse_context_window_returns_none_when_missing() {
        assert_eq!(parse_ollama_context_window(r#"{"model_info":{}}"#), None);
        assert_eq!(parse_ollama_context_window("{}"), None);
        assert_eq!(parse_ollama_context_window("not json"), None);
    }

    /// Spawn a throwaway server that accepts one connection, optionally writes
    /// `headers`, then stalls (holds the socket open, sending nothing further),
    /// simulating a hung provider. `None` = never reply (initial send() has
    /// nothing); `Some(hdr)` = write those response headers then go silent.
    /// Returns the bound address.
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

    const OK_NDJSON_HEADERS: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n";

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        }
    }

    #[tokio::test]
    async fn idle_timeout_emits_error_when_stream_stalls() {
        // Headers arrive, then the server goes silent: the read loop must give
        // up on the idle deadline instead of awaiting a chunk forever.
        let addr = spawn_stalling_server(Some(OK_NDJSON_HEADERS)).await;
        let provider = OllamaProvider::new("k".into())
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
        let provider = OllamaProvider::new("k".into())
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
        let provider = OllamaProvider::new("k".into())
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

    #[test]
    fn implicit_cache_approx_first_subturn_has_zero_cached() {
        // First sub-turn: prev=0, so cached=min(curr,0)=0 (nothing warm yet).
        let out = parse_first(
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":12000,"eval_count":40}"#,
        );
        assert_eq!(
            out,
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 12000,
                    output_tokens: 40,
                    cached: 0,
                    thinking_tokens: 0,
                }),
                ProviderEvent::Done
            ]
        );
    }

    #[test]
    fn implicit_cache_approx_second_subturn_credits_overlap() {
        // Two sub-turns: 12k then 13k tokens. The second has curr=13000 >= prev=12000,
        // so it's a cache miss (full eval): input=13000 (the full prompt), cached=0.
        // The old min(curr, prev) synthetic cached no longer applies.
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":12000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":13000,"eval_count":10}"#,
        ]);
        // First sub-turn: cached 0 (prev 0).
        assert!(matches!(
            out[0][0],
            ProviderEvent::Usage(Usage {
                cached: 0,
                thinking_tokens: 0,
                input_tokens: 12000,
                output_tokens: 40
            })
        ));
        // Second sub-turn: cache miss (curr >= prev), input=13000, cached=0.
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                cached: 0,
                thinking_tokens: 0,
                input_tokens: 13000,
                output_tokens: 10
            })
        ));
    }

    #[test]
    fn implicit_cache_approx_shrinking_prompt_credits_smaller_overlap() {
        // A turn whose prompt is SMALLER than the previous (e.g. after eviction)
        // triggers the cache-hit reconstruction: input=prev (50000), cached=prev-curr
        // (20000). This is a false positive after eviction (the real prompt is
        // 30000), but it's bounded and self-corrects on the next cache-miss turn.
        let out = parse_seq(&[
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":50000,"eval_count":40}"#,
            r#"{"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":30000,"eval_count":10}"#,
        ]);
        assert!(matches!(
            out[1][0],
            ProviderEvent::Usage(Usage {
                cached: 20000,
                thinking_tokens: 0,
                input_tokens: 50000,
                output_tokens: 10
            })
        ));
    }
}
