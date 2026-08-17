//! The dedicated OpenCode Zen provider: holds an `Arc<Registry>` and delegates
//! `stream()`/`list_models()` to one of four sub-clients
//! (OpenAICompatProvider, AnthropicProvider, OpenAIResponsesProvider,
//! GoogleGeminiProvider) based on the (provider, model) wire shape resolved
//! from the registry via `Registry::wire_shape("opencode-zen", model)`.

use crate::anthropic::AnthropicProvider;
use crate::google_gemini::GoogleGeminiProvider;
use crate::openai_compat::OpenAICompatProvider;
use crate::openai_responses::OpenAIResponsesProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use zoid_model::{Registry, WireShape};

pub struct OpenCodeZenProvider {
    api_key: String,
    base_url: String,
    reg: Arc<Registry>,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeZenProvider {
    pub fn new(api_key: String, reg: Arc<Registry>) -> Self {
        Self {
            api_key,
            base_url: reg
                .default_base_url("opencode-zen")
                .unwrap_or("https://opencode.ai/zen")
                .to_string(),
            reg,
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

    fn wire_shape_for(&self, model: &str) -> WireShape {
        self.reg
            .wire_shape("opencode-zen", model)
            .unwrap_or_else(|| {
                tracing::warn!(
                    model = %model,
                    "opencode-zen: model not in registry; defaulting to OpenAIChat"
                );
                WireShape::OpenAIChat
            })
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
            WireShape::OpenAIChat => {
                OpenAICompatProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            WireShape::AnthropicMessages => {
                AnthropicProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            WireShape::OpenAIResponses => {
                OpenAIResponsesProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            WireShape::GoogleGemini => {
                GoogleGeminiProvider::new(self.api_key.clone())
                    .with_base_url(&self.base_url)
                    .with_idle_timeout(self.idle_timeout)
                    .stream(req, sink)
                    .await
            }
            other => {
                tracing::warn!(
                    shape = ?other,
                    "opencode-zen: unexpected wire shape; defaulting to OpenAIChat"
                );
                OpenAICompatProvider::new(self.api_key.clone())
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
    use zoid_model::{
        ModelEntry, ModelInfo, ProviderEntry, Source, Status, ThinkingSupport, ThinkingWireShape,
        Transport,
    };

    /// Build an in-memory `Registry` whose `opencode-zen` provider maps the
    /// given `(model id, WireShape)` rows. The composite provider routes
    /// `stream()` off these rows; tests assert the recorded request line.
    fn test_reg(rows: &[(&str, WireShape)]) -> Arc<Registry> {
        Arc::new(Registry {
            providers: vec![ProviderEntry {
                id: "opencode-zen".to_string(),
                display: "zen".into(),
                family: "opencode-zen".into(),
                transport: Transport::Http {
                    default_base_url: "https://opencode.ai/zen".into(),
                },
                status: Status::Available,
                key_url: Some("https://x".into()),
                key_env: Some("OPENCODE_GO_API_KEY".into()),
                models: rows
                    .iter()
                    .map(|(id, shape)| ModelEntry {
                        id: id.to_string(),
                        display: None,
                        wire_shape: *shape,
                        source: Source::Static,
                        default: false,
                        hidden: false,
                        info: ModelInfo {
                            context_window: 200_000,
                            max_output: 0,
                            tools: false,
                            prompt_cache: true,
                            thinking: ThinkingSupport::None,
                            thinking_wire: ThinkingWireShape::None,
                        },
                        runtime: None,
                        download_source: None,
                        quant: None,
                        modelfile: None,
                        num_ctx: None,
                        vram_curve: None,
                    })
                    .collect(),
            }],
        })
    }

    #[test]
    fn routes_via_registry_wire_shape() {
        let reg = test_reg(&[
            ("glm-5.2", WireShape::OpenAIChat),
            ("claude-sonnet-4-5", WireShape::AnthropicMessages),
            ("gpt-5.4", WireShape::OpenAIResponses),
            ("gemini-3-flash", WireShape::GoogleGemini),
        ]);
        let p = OpenCodeZenProvider::new("k".into(), reg);
        assert_eq!(p.wire_shape_for("glm-5.2"), WireShape::OpenAIChat);
        assert_eq!(
            p.wire_shape_for("claude-sonnet-4-5"),
            WireShape::AnthropicMessages
        );
        assert_eq!(p.wire_shape_for("gpt-5.4"), WireShape::OpenAIResponses);
        assert_eq!(p.wire_shape_for("gemini-3-flash"), WireShape::GoogleGemini);
        assert_eq!(p.wire_shape_for("unknown"), WireShape::OpenAIChat);
    }

    #[test]
    fn wire_shape_for_unknown_defaults_to_openai_chat() {
        let reg = test_reg(&[]);
        let p = OpenCodeZenProvider::new("k".into(), reg);
        assert_eq!(p.wire_shape_for("unknown-model"), WireShape::OpenAIChat);
    }

    #[test]
    fn with_base_url_propagates_to_subclient() {
        let reg = test_reg(&[]);
        let p = OpenCodeZenProvider::new("k".into(), reg).with_base_url("https://example.test/zen/");
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
            model_info: crate::test_model_info(),
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
        let reg = test_reg(&[("glm-5.2", WireShape::OpenAIChat)]);
        let provider = OpenCodeZenProvider::new("k".into(), reg)
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
        let reg = test_reg(&[("claude-sonnet-4-5", WireShape::AnthropicMessages)]);
        let provider = OpenCodeZenProvider::new("k".into(), reg)
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
        let reg = test_reg(&[("gpt-5.4", WireShape::OpenAIResponses)]);
        let provider = OpenCodeZenProvider::new("k".into(), reg)
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
        let reg = test_reg(&[("gemini-3-flash", WireShape::GoogleGemini)]);
        let provider = OpenCodeZenProvider::new("k".into(), reg)
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