//! The dedicated OpenCode Go provider: holds an `Arc<Registry>` and delegates
//! `stream()`/`list_models()` to either `OpenAICompatProvider`
//! (POST {base}/v1/chat/completions) or `AnthropicProvider`
//! (POST {base}/v1/messages) based on the (provider, model) wire shape resolved
//! from the registry via `Registry::wire_shape("opencode-go", model)`.

use crate::anthropic::AnthropicProvider;
use crate::openai_compat::OpenAICompatProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use zoid_model::{Registry, WireShape};

pub struct OpenCodeGoProvider {
    api_key: String,
    base_url: String,
    reg: Arc<Registry>,
    // Reserved for a future shared-client optimization (v1 constructs a fresh
    // sub-client per `stream()` call). Kept on the struct so the field's
    // lifetime tracks the provider; clippy is silenced via `#[allow(dead_code)]`.
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl OpenCodeGoProvider {
    pub fn new(api_key: String, reg: Arc<Registry>) -> Self {
        Self {
            api_key,
            base_url: reg
                .default_base_url("opencode-go")
                .unwrap_or("https://opencode.ai/zen/go")
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
            .wire_shape("opencode-go", model)
            .unwrap_or_else(|| {
                tracing::warn!(
                    model = %model,
                    "opencode-go: model not in registry; defaulting to OpenAIChat"
                );
                WireShape::OpenAIChat
            })
    }
}

#[async_trait]
impl Provider for OpenCodeGoProvider {
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
            other => {
                tracing::warn!(
                    shape = ?other,
                    "opencode-go: unexpected wire shape; defaulting to OpenAIChat"
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
        // Both sub-clients' /v1/models share the {data:[{id}]} shape; reuse
        // the OpenAI-compat client's list_models (it hits {base}/v1/models).
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

    /// Build an in-memory `Registry` whose `opencode-go` provider maps the given
    /// `(model id, WireShape)` rows. The composite provider routes `stream()`
    /// off these rows; tests assert the recorded request line.
    fn test_reg(rows: &[(&str, WireShape)]) -> Arc<Registry> {
        Arc::new(Registry {
            providers: vec![ProviderEntry {
                id: "opencode-go".to_string(),
                display: "go".into(),
                family: "opencode-go".into(),
                transport: Transport::Http {
                    default_base_url: "https://opencode.ai/zen/go".into(),
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
        let reg = test_reg(&[("minimax-m3", WireShape::AnthropicMessages)]);
        let p = OpenCodeGoProvider::new("k".into(), reg);
        assert_eq!(p.wire_shape_for("minimax-m3"), WireShape::AnthropicMessages);
        assert_eq!(p.wire_shape_for("unknown"), WireShape::OpenAIChat);
    }

    #[test]
    fn wire_shape_for_unknown_defaults_to_openai_chat() {
        let reg = test_reg(&[]);
        let p = OpenCodeGoProvider::new("k".into(), reg);
        assert_eq!(p.wire_shape_for("unknown-model"), WireShape::OpenAIChat);
    }

    #[test]
    fn with_base_url_propagates_to_subclient() {
        let reg = test_reg(&[]);
        let p = OpenCodeGoProvider::new("k".into(), reg).with_base_url("https://example.test/go/");
        assert_eq!(p.base_url, "https://example.test/go");
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Server that records the request line of the first request, then writes
    /// a minimal SSE `data: [DONE]` so the stream terminates cleanly.
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
                // Respond with a minimal SSE stream: one [DONE] so stream()
                // returns. Uses Content-Length to avoid brittle chunk sizes.
                let body = "data: [DONE]\r\n\r\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (addr, recorded)
    }

    #[tokio::test]
    async fn openai_compat_model_routes_to_chat_completions() {
        let (addr, recorded) = spawn_recording_server().await;
        let reg = test_reg(&[("glm-5.2", WireShape::OpenAIChat)]);
        let provider = OpenCodeGoProvider::new("k".into(), reg)
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(std::time::Duration::from_secs(2));
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            model_info: crate::model::model_info("glm-5.2"),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/chat/completions"),
            "expected /v1/chat/completions, got: {first}"
        );
    }

    #[tokio::test]
    async fn anthropic_model_routes_to_messages() {
        let (addr, recorded) = spawn_recording_server().await;
        let reg = test_reg(&[("minimax-m3", WireShape::AnthropicMessages)]);
        let provider = OpenCodeGoProvider::new("k".into(), reg)
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(std::time::Duration::from_secs(2));
        let req = CompletionRequest {
            model: "minimax-m3".into(),
            model_info: crate::model::model_info("minimax-m3"),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/v1/messages"),
            "expected /v1/messages, got: {first}"
        );
    }
}
