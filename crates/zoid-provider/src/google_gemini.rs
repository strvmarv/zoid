//! The generic Google Gemini client (POST {base}/v1/models/<model>:streamGenerateContent
//! ?alt=sse, candidates[].content.parts[] with text/functionCall/thought, usageMetadata).
//! Self-contained like the other leaves; uses the crate's `Provider` seam. No
//! opencode-zen-specifics — a generic leaf reusable by direct-Gemini etc.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall, Usage};
use anyhow::Result;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// Build the Gemini generate request: returns (path_suffix, body). The model
/// lives in the path, not the body. `contents` carries the conversation;
/// `systemInstruction` carries the system prompt; `tools` carries function
/// declarations; `generationConfig` carries maxOutputTokens + thinkingConfig.
pub fn request_body(req: &CompletionRequest, model: &str) -> (String, Value) {
    let path = format!("v1/models/{model}:streamGenerateContent");

    let mut contents: Vec<Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            MsgRole::User => contents.push(json!({
                "role": "user",
                "parts": [{ "text": m.content }],
            })),
            MsgRole::Assistant => {
                let mut parts: Vec<Value> = if m.content.is_empty() {
                    Vec::new()
                } else {
                    vec![json!({ "text": m.content })]
                };
                for tc in &m.tool_calls {
                    parts.push(json!({
                        "functionCall": {
                            "id": tc.id,
                            "name": tc.name,
                            "args": tc.args,
                        }
                    }));
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            MsgRole::Tool => contents.push(json!({
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "name": m.tool_name.clone().unwrap_or_default(),
                        "response": { "content": m.content },
                    }
                }],
            })),
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": { "maxOutputTokens": req.max_tokens },
    });

    if let Some(sys) = &req.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
    }
    if !req.tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": req.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })).collect::<Vec<_>>()
        }]);
    }
    // thinking → thinkingConfig.includeThoughts (Gemini surfaces thought parts
    // separately). Off omits the config entirely. Leaf-local rendering:
    // MODEL_CAPS sets thinking_wire: None for Gemini; the leaf consults
    // req.thinking directly (mirrors ollama.rs's leaf-local approach).
    if !matches!(req.thinking, crate::ThinkingMode::Off) {
        body["generationConfig"]["thinkingConfig"] = json!({ "includeThoughts": true });
    }
    (path, body)
}

/// Parse one Gemini `GenerateContentResponse` chunk (a parsed JSON object) into
/// zero-or-more `ProviderEvent`s. `usageMetadata` (final chunk) emits `Usage`
/// then `Done`; `finishReason: MAX_TOKENS` emits `Truncated`. Never panics.
pub fn parse_chunk(obj: &Value) -> Vec<ProviderEvent> {
    let mut out = Vec::new();
    if let Some(cands) = obj.get("candidates").and_then(|c| c.as_array()) {
        for cand in cands {
            if let Some(parts) = cand
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    // thought part (only present when includeThoughts true)
                    let is_thought = part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false);
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            out.push(if is_thought {
                                ProviderEvent::ThinkingDelta(text.to_string())
                            } else {
                                ProviderEvent::TextDelta(text.to_string())
                            });
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let id = fc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args = fc.get("args").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
                        out.push(ProviderEvent::ToolCall(ToolCall { id, name, args }));
                    }
                }
            }
            if let Some(reason) = cand.get("finishReason").and_then(|f| f.as_str()) {
                if reason == "MAX_TOKENS" {
                    out.push(ProviderEvent::Truncated);
                }
            }
        }
    }
    // promptFeedback.blockReason → Error (spec §4.4/§7).
    if let Some(reason) = obj
        .get("promptFeedback")
        .and_then(|pf| pf.get("blockReason"))
        .and_then(|b| b.as_str())
    {
        if !reason.is_empty() {
            out.push(ProviderEvent::Error(format!("gemini blocked: {reason}")));
        }
    }
    // Zen's Gemini stream emits `usageMetadata` on EVERY chunk — intermediate
    // chunks carry `"usageMetadata":{}` (empty object). An empty object is
    // `Some`, not `None`, so a naive `if let Some(usage) = obj.get(...)` check
    // matches every chunk and emits premature `Usage{0,0,0,0}` + `Done`, killing
    // the stream after the first text delta. Gate on `promptTokenCount` being
    // present (only the final frame carries real counts). Confirmed Zen behavior
    // (see spike research, 2026-07-10/2026-07-11).
    if let Some(usage) = obj.get("usageMetadata") {
        if let Some(prompt_tokens) = usage.get("promptTokenCount").and_then(|n| n.as_u64()) {
            let output = usage.get("candidatesTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
            let cached = usage.get("cachedContentTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
            let thinking = usage.get("thoughtsTokenCount").and_then(|n| n.as_u64()).unwrap_or(0);
            out.push(ProviderEvent::Usage(Usage {
                input_tokens: prompt_tokens,
                output_tokens: output,
                cached,
                thinking_tokens: thinking,
            }));
            out.push(ProviderEvent::Done);
        }
    }
    out
}

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Streaming Google Gemini provider.
pub struct GoogleGeminiProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl GoogleGeminiProvider {
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
impl Provider for GoogleGeminiProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        let model = req.model.clone();
        let (path, body) = request_body(req, &model);
        let resp = match tokio::time::timeout(
            self.idle_timeout,
            self.client
                .post(format!("{}/{}?alt=sse", self.base_url, path))
                .header("x-goog-api-key", &self.api_key)
                .header("content-type", "application/json")
                .json(&body)
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
            let v: Value = match serde_json::from_str(&item.data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut got_done = false;
            for pe in parse_chunk(&v) {
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
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await?;
        let body = resp.text().await?;
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        Ok(v
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("name")
                            .and_then(|n| n.as_str())
                            .and_then(|s| s.strip_prefix("models/"))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, ToolSpec};
    use serde_json::json;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn body_path_includes_model_and_endpoint() {
        let req = CompletionRequest {
            model: "gemini-3-flash".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 128,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (path, body) = request_body(&req, "gemini-3-flash");
        assert_eq!(path, "v1/models/gemini-3-flash:streamGenerateContent");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 128);
        assert!(body.get("systemInstruction").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("thinkingConfig").is_none());
    }

    #[test]
    fn body_with_system_prompt_emits_system_instruction() {
        let req = CompletionRequest {
            model: "m".into(),
            system: Some("be terse".into()),
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be terse");
    }

    #[test]
    fn body_with_tools_emits_function_declarations() {
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
            reassert: None,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "read_file");
    }

    #[test]
    fn body_tool_message_emits_function_response() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![
                Message::user("call it"),
                Message {
                    role: MsgRole::Assistant,
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "fc_1".into(),
                        name: "read_file".into(),
                        args: json!({"path": "a"}),
                    }],
                    tool_name: None,
                    tool_call_id: None,
                },
                Message::tool("read_file", "file body"),
            ],
            max_tokens: 64,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (_, body) = request_body(&req, "m");
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read_file");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "read_file");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["response"]["content"], "file body");
    }

    #[test]
    fn body_thinking_on_emits_thinking_config() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Auto,
            reassert: None,
        };
        let (_, body) = request_body(&req, "m");
        assert_eq!(body["generationConfig"]["thinkingConfig"]["includeThoughts"], true);
    }

    #[test]
    fn parse_text_part_yields_textdelta() {
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "Hel" }] }
            }]
        });
        assert_eq!(parse_chunk(&chunk), vec![ProviderEvent::TextDelta("Hel".into())]);
    }

    #[test]
    fn parse_function_call_part_yields_toolcall() {
        // Zen's Gemini does NOT populate functionCall.id (confirmed via
        // 2026-07-11 capture) — falls back to empty string, matching Ollama.
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{
                    "functionCall": { "name": "read_file", "args": { "path": "a" } }
                }] }
            }]
        });
        assert_eq!(
            parse_chunk(&chunk),
            vec![ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "read_file".into(),
                args: json!({"path": "a"}),
            })]
        );
    }

    #[test]
    fn parse_thought_part_yields_thinking_delta() {
        // This is the Google-standard schema shape ({thought:true, text}).
        // Zen's Gemini does NOT emit this — it uses opaque thoughtSignature
        // blobs instead (see parse_thought_signature_is_ignored below).
        // Keep this test for direct-Gemini correctness.
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "thought": true, "text": "pondering" }] }
            }]
        });
        assert_eq!(
            parse_chunk(&chunk),
            vec![ProviderEvent::ThinkingDelta("pondering".into())]
        );
    }

    #[test]
    fn parse_thought_signature_is_ignored() {
        // Zen's Gemini emits opaque thoughtSignature blobs on empty-text parts
        // (confirmed via 2026-07-11 capture). Must not crash or emit a delta.
        let chunk = json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "", "thoughtSignature": "AY89a1+cmbdq5mYb..." }] }
            }]
        });
        let out = parse_chunk(&chunk);
        assert!(out.is_empty(), "empty-text thoughtSignature part yields nothing: got {out:?}");
    }

    #[test]
    fn parse_ping_event_is_ignored() {
        // Zen injects {type:ping} SSE events (cost/keepalive) between content
        // chunks. No candidates, no usageMetadata — must be silently ignored.
        let chunk = json!({"type": "ping", "cost": "0.00000900"});
        let out = parse_chunk(&chunk);
        assert!(out.is_empty(), "ping event must yield nothing: got {out:?}");
    }

    #[test]
    fn parse_prompt_feedback_block_reason_yields_error() {
        // Spec §4.4/§7: promptFeedback.blockReason → Error.
        let chunk = json!({
            "promptFeedback": { "blockReason": "SAFETY" }
        });
        let out = parse_chunk(&chunk);
        assert!(
            out.iter().any(|e| matches!(e, ProviderEvent::Error(msg) if msg.contains("SAFETY"))),
            "blocked request must yield Error: got {out:?}"
        );
    }

    #[test]
    fn parse_finish_reason_max_tokens_yields_truncated() {
        let chunk = json!({
            "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": "MAX_TOKENS" }]
        });
        assert_eq!(parse_chunk(&chunk), vec![ProviderEvent::Truncated]);
    }

    #[test]
    fn parse_usage_metadata_yields_usage_then_done() {
        let chunk = json!({
            "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": "STOP" }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 5,
                "totalTokenCount": 35
            }
        });
        assert_eq!(
            parse_chunk(&chunk),
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
    fn parse_empty_usage_metadata_yields_nothing() {
        // Zen emits `{"usageMetadata":{}}` on intermediate chunks — must NOT
        // emit Usage or Done (H1 bug guard: premature Done kills the stream).
        let chunk = json!({
            "candidates": [{ "content": { "role": "model", "parts": [{ "text": "hi" }] } }],
            "usageMetadata": {}
        });
        let out = parse_chunk(&chunk);
        assert_eq!(out, vec![ProviderEvent::TextDelta("hi".into())]);
    }

    #[test]
    fn parse_empty_candidates_yields_nothing() {
        let chunk = json!({ "candidates": [] });
        assert!(parse_chunk(&chunk).is_empty());
    }

    #[test]
    fn parse_malformed_yields_nothing() {
        assert!(parse_chunk(&json!(42)).is_empty());
    }

    fn probe_req() -> CompletionRequest {
        CompletionRequest {
            model: "gemini-3-flash".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        }
    }

    #[tokio::test]
    async fn gemini_routes_to_stream_generate_content() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None::<String>));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}],\"usageMetadata\":{}}\r\n\r\n\
                            data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":1}}\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = GoogleGeminiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&probe_req(), tx).await.unwrap();
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("streamGenerateContent"),
            "expected streamGenerateContent, got: {first}"
        );
        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::TextDelta(t) if t == "hi")));
        assert!(got.iter().any(|e| matches!(e, ProviderEvent::Done)));
    }
}
