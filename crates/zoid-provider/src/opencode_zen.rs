//! The dedicated OpenCode Zen provider: holds a static per-model wire-shape map
//! and delegates `stream()`/`list_models()` to one of four sub-clients
//! (OpenAICompatProvider, AnthropicProvider, OpenAIResponsesProvider,
//! GoogleGeminiProvider) based on the active model id.

use crate::anthropic::AnthropicProvider;
use crate::google_gemini::GoogleGeminiProvider;
use crate::openai_compat::OpenAICompatProvider;
use crate::openai_responses::OpenAIResponsesProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZenWireShape {
    OpenAIChat,
    AnthropicMessages,
    OpenAIResponses,
    GoogleGemini,
}

const ZEN_MODELS: &[(&str, ZenWireShape)] = &[
    // --- Anthropic Messages (13) ---
    ("claude-sonnet-4-5", ZenWireShape::AnthropicMessages),
    ("claude-fable-5", ZenWireShape::AnthropicMessages),
    ("claude-opus-4-8", ZenWireShape::AnthropicMessages),
    ("claude-opus-4-7", ZenWireShape::AnthropicMessages),
    ("claude-opus-4-6", ZenWireShape::AnthropicMessages),
    ("claude-opus-4-5", ZenWireShape::AnthropicMessages),
    ("claude-sonnet-5", ZenWireShape::AnthropicMessages),
    ("claude-sonnet-4-6", ZenWireShape::AnthropicMessages),
    ("claude-haiku-4-5", ZenWireShape::AnthropicMessages),
    ("qwen3.7-max", ZenWireShape::AnthropicMessages),
    ("qwen3.7-plus", ZenWireShape::AnthropicMessages),
    ("qwen3.6-plus", ZenWireShape::AnthropicMessages),
    ("qwen3.5-plus", ZenWireShape::AnthropicMessages),
    // --- OpenAI Responses (17) ---
    ("gpt-5.5", ZenWireShape::OpenAIResponses),
    ("gpt-5.5-pro", ZenWireShape::OpenAIResponses),
    ("gpt-5.4", ZenWireShape::OpenAIResponses),
    ("gpt-5.4-pro", ZenWireShape::OpenAIResponses),
    ("gpt-5.4-mini", ZenWireShape::OpenAIResponses),
    ("gpt-5.4-nano", ZenWireShape::OpenAIResponses),
    ("gpt-5.3-codex", ZenWireShape::OpenAIResponses),
    ("gpt-5.3-codex-spark", ZenWireShape::OpenAIResponses),
    ("gpt-5.2", ZenWireShape::OpenAIResponses),
    ("gpt-5.2-codex", ZenWireShape::OpenAIResponses),
    ("gpt-5.1", ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex-max", ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex", ZenWireShape::OpenAIResponses),
    ("gpt-5.1-codex-mini", ZenWireShape::OpenAIResponses),
    ("gpt-5", ZenWireShape::OpenAIResponses),
    ("gpt-5-codex", ZenWireShape::OpenAIResponses),
    ("gpt-5-nano", ZenWireShape::OpenAIResponses),
    // --- OpenAI Chat Completions (19) ---
    ("deepseek-v4-pro", ZenWireShape::OpenAIChat),
    ("deepseek-v4-flash", ZenWireShape::OpenAIChat),
    ("deepseek-v4-flash-free", ZenWireShape::OpenAIChat),
    ("glm-5.2", ZenWireShape::OpenAIChat),
    ("glm-5.1", ZenWireShape::OpenAIChat),
    ("glm-5", ZenWireShape::OpenAIChat),
    ("grok-4.5", ZenWireShape::OpenAIChat),
    ("grok-build-0.1", ZenWireShape::OpenAIChat),
    ("kimi-k2.5", ZenWireShape::OpenAIChat),
    ("kimi-k2.6", ZenWireShape::OpenAIChat),
    ("kimi-k2.7-code", ZenWireShape::OpenAIChat),
    ("minimax-m3", ZenWireShape::OpenAIChat),
    ("minimax-m2.7", ZenWireShape::OpenAIChat),
    ("minimax-m2.5", ZenWireShape::OpenAIChat),
    ("big-pickle", ZenWireShape::OpenAIChat),
    ("hy3-free", ZenWireShape::OpenAIChat),
    ("mimo-v2.5-free", ZenWireShape::OpenAIChat),
    ("north-mini-code-free", ZenWireShape::OpenAIChat),
    ("nemotron-3-ultra-free", ZenWireShape::OpenAIChat),
    // --- Google Gemini (3) ---
    ("gemini-3.5-flash", ZenWireShape::GoogleGemini),
    ("gemini-3.1-pro", ZenWireShape::GoogleGemini),
    ("gemini-3-flash", ZenWireShape::GoogleGemini),
];

pub struct OpenCodeZenProvider {
    api_key: String,
    base_url: String,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeZenProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("opencode-zen")
                .unwrap_or("https://opencode.ai/zen")
                .to_string(),
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

    fn wire_shape_for(&self, model: &str) -> ZenWireShape {
        match ZEN_MODELS.iter().find(|(id, _)| *id == model) {
            Some((_, shape)) => *shape,
            None => {
                tracing::warn!(
                    model = %model,
                    "opencode-zen: model not in wire-shape map; defaulting to OpenAIChat"
                );
                ZenWireShape::OpenAIChat
            }
        }
    }
}

#[async_trait]
impl Provider for OpenCodeZenProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        match self.wire_shape_for(&req.model) {
            ZenWireShape::OpenAIChat => {
                OpenAICompatProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::AnthropicMessages => {
                AnthropicProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::OpenAIResponses => {
                OpenAIResponsesProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            ZenWireShape::GoogleGemini => {
                GoogleGeminiProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Zen's gateway normalizes /v1/models to the OpenAI {data:[{id}]} shape.
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_idle_timeout(self.idle_timeout)
            .list_models()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn wire_shape_for_known_models_matches_table() {
        for (id, shape) in ZEN_MODELS {
            let p = OpenCodeZenProvider::new("k".into());
            assert_eq!(p.wire_shape_for(id), *shape, "mismatch for {id}");
        }
    }

    #[test]
    fn wire_shape_for_unknown_defaults_to_openai_chat() {
        let p = OpenCodeZenProvider::new("k".into());
        assert_eq!(p.wire_shape_for("unknown-model"), ZenWireShape::OpenAIChat);
    }

    #[test]
    fn with_base_url_propagates_to_subclient() {
        let p = OpenCodeZenProvider::new("k".into()).with_base_url("https://example.test/zen/");
        assert_eq!(p.base_url, "https://example.test/zen");
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn spawn_recording_server() -> (
        std::net::SocketAddr,
        std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let recorded_clone = recorded.clone();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_text.lines().next().unwrap_or("").to_string();
                *recorded_clone.lock().await = Some(first_line);
                let body = "data: [DONE]\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (addr, recorded)
    }

    fn zen_req(model: &str) -> CompletionRequest {
        CompletionRequest {
            model: model.into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        }
    }

    #[tokio::test]
    async fn chat_model_routes_to_chat_completions() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("glm-5.2"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/chat/completions"),
            "expected /v1/chat/completions, got: {first}"
        );
    }

    #[tokio::test]
    async fn anthropic_model_routes_to_messages() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("claude-sonnet-4-5"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/messages"),
            "expected /v1/messages, got: {first}"
        );
    }

    #[tokio::test]
    async fn responses_model_routes_to_responses() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("gpt-5.4"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/responses"),
            "expected /v1/responses, got: {first}"
        );
    }

    #[tokio::test]
    async fn gemini_model_routes_to_stream_generate_content() {
        let (addr, recorded) = spawn_recording_server().await;
        let provider = OpenCodeZenProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&zen_req("gemini-3-flash"), tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("streamGenerateContent"),
            "expected streamGenerateContent, got: {first}"
        );
    }
}
