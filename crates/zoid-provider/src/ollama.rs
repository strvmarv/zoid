//! The Ollama Cloud provider via the native Chat API
//! (`POST {base}/api/chat`, NDJSON streaming, `"done":true` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
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
    let mut body = json!({
        "model": req.model,
        "stream": true,
        "messages": messages,
        // Keep the model warm between turns (Ollama's analog to prompt caching):
        // hold it loaded for 30m after a response so a coding session's rapid
        // follow-up turns skip the cold reload. The native API has no token-level
        // prompt cache, so `cached` stays 0 for this provider.
        "keep_alive": "30m",
    });
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
pub fn parse_line(line: &str) -> Vec<ProviderEvent> {
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
                let args = func.get("arguments").cloned().unwrap_or(Value::Null);
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
    // `prompt_eval_count` (input tokens) and `eval_count` (output tokens). Emit
    // a Usage event whenever either is present so the economy ledger reflects
    // real spend — this is the Ollama counterpart to the Anthropic provider's
    // `usage` parsing, and the only reason the session "tok" line is non-zero
    // on Ollama. Ordered before Done: the agent loop accumulates Usage during
    // the stream and records it when the turn ends.
    let input = v.get("prompt_eval_count").and_then(|n| n.as_u64());
    let output = v.get("eval_count").and_then(|n| n.as_u64());
    if input.is_some() || output.is_some() {
        out.push(ProviderEvent::Usage(Usage {
            input_tokens: input.unwrap_or(0),
            output_tokens: output.unwrap_or(0),
            cached: 0, // native /api/chat has no token-level prompt cache
        }));
    }
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        out.push(ProviderEvent::Done);
    }
    out
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
}

impl OllamaProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("ollama-cloud")
                .unwrap_or("https://ollama.com")
                .to_string(),
            client: reqwest::Client::new(),
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
        'read: while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        for pe in parse_line(&line) {
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
            for pe in parse_line(&line) {
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
            context_window: w,
            max_output: 0,
            tools: true,
            prompt_cache: false, // Ollama has no token-level prompt cache
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolCall, ToolSpec};
    use serde_json::json;

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
        };
        let body = request_body(&req);
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
        };
        assert_eq!(
            request_body(&req)["messages"],
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
                },
                Message::tool("read_file", "bar"),
            ],
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
        };
        assert!(request_body(&req).get("tools").is_none());
    }

    #[test]
    fn parses_content_delta_line() {
        let line = r#"{"model":"glm-5.2:cloud","message":{"role":"assistant","content":"Hel"},"done":false}"#;
        assert_eq!(
            parse_line(line),
            vec![ProviderEvent::TextDelta("Hel".into())]
        );
    }

    #[test]
    fn thinking_only_line_yields_none() {
        let line =
            r#"{"message":{"role":"assistant","content":"","thinking":"reasoning"},"done":false}"#;
        assert!(parse_line(line).is_empty());
    }

    #[test]
    fn done_line_with_counts_yields_usage_then_done() {
        // The final frame carries prompt_eval_count (input) + eval_count (output);
        // both surface as a Usage event ahead of Done.
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":124,"eval_count":58}"#;
        assert_eq!(
            parse_line(line),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 124,
                    output_tokens: 58,
                    cached: 0
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
            parse_line(line),
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: 0,
                    output_tokens: 58,
                    cached: 0
                }),
                ProviderEvent::Done
            ]
        );
    }

    #[test]
    fn done_line_without_counts_yields_only_done() {
        let line =
            r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}"#;
        assert_eq!(parse_line(line), vec![ProviderEvent::Done]);
    }

    #[test]
    fn error_line_yields_error() {
        assert_eq!(
            parse_line(r#"{"error":"Unauthorized"}"#),
            vec![ProviderEvent::Error("Unauthorized".into())]
        );
    }

    #[test]
    fn empty_and_malformed_lines_yield_none() {
        assert!(parse_line("").is_empty());
        assert!(parse_line("   ").is_empty());
        assert!(parse_line("not json").is_empty());
    }

    #[test]
    fn parses_tool_call_line() {
        let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"a.txt"}}}]},"done":false}"#;
        assert_eq!(
            parse_line(line),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "read_file".into(),
                args: json!({"path": "a.txt"})
            })]
        );
    }

    #[test]
    fn parses_text_then_done_as_two_events() {
        let line = r#"{"message":{"role":"assistant","content":"hi"},"done":true}"#;
        assert_eq!(
            parse_line(line),
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
}
