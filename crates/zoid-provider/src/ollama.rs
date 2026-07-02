//! The Ollama Cloud provider via the native Chat API
//! (`POST {base}/api/chat`, NDJSON streaming, `"done":true` terminator).
//! Self-contained like the `anthropic` module; uses the crate's `Provider` seam.

use crate::{CompletionRequest, MsgRole, Provider, ProviderEvent, ToolCall};
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
    if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
        out.push(ProviderEvent::Done);
    }
    out
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
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
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
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        for pe in parse_line(&line) {
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
            for pe in parse_line(&line) {
                if sink.send(pe).await.is_err() {
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
    use crate::{Message, ToolCall, ToolSpec};
    use serde_json::json;

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
            })
        );
        // native body must NOT carry OpenAI-only fields
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("stream_options").is_none());
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
    fn done_line_yields_done() {
        let line = r#"{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":58}"#;
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
}
