//! The ZAI Coding Plan provider: delegates to OpenAICompatProvider with
//! path_prefix="" (ZAI's endpoint is {base}/chat/completions, no /v1/ segment).

use crate::openai_compat::OpenAICompatProvider;
use crate::{CompletionRequest, Provider, ProviderEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct ZaiProvider {
    api_key: String,
    base_url: String,
    #[allow(dead_code)]
    client: reqwest::Client,
    idle_timeout: Duration,
}

impl ZaiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: crate::model::default_base_url("zai-coding-plan")
                .unwrap_or("https://api.z.ai/api/coding/paas/v4")
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
}

#[async_trait]
impl Provider for ZaiProvider {
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")
            .with_idle_timeout(self.idle_timeout)
            .stream(req, sink)
            .await
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        OpenAICompatProvider::new(self.api_key.clone())
            .with_base_url(&self.base_url)
            .with_path_prefix("")
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
    fn new_uses_default_base_url() {
        let p = ZaiProvider::new("k".into());
        assert_eq!(p.base_url, "https://api.z.ai/api/coding/paas/v4");
    }

    #[test]
    fn with_base_url_overrides_and_trims_trailing_slash() {
        let p = ZaiProvider::new("k".into()).with_base_url("https://proxy.test/zai/");
        assert_eq!(p.base_url, "https://proxy.test/zai");
    }

    #[tokio::test]
    async fn zai_list_models_hits_models_without_v1_prefix() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
                let body = r#"{"data":[{"id":"glm-5.2"}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = ZaiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let models = provider.list_models().await.unwrap();
        assert_eq!(models, vec!["glm-5.2"]);
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/models") && !first.contains("/v1/models"),
            "ZAI list_models must hit /models (no /v1/), got: {first}"
        );
    }

    #[tokio::test]
    async fn zai_stream_hits_chat_completions_without_v1_prefix() {
        // Recording server: capture the request line, then respond with [DONE].
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let provider = ZaiProvider::new("k".into())
            .with_base_url(format!("http://{addr}"))
            .with_idle_timeout(Duration::from_secs(2));
        let req = CompletionRequest {
            model: "glm-5.2".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: crate::ThinkingMode::Off,
        };
        let (tx, _rx) = mpsc::channel::<ProviderEvent>(16);
        let _ = provider.stream(&req, tx).await;
        let first = recorded.lock().await.clone().unwrap_or_default();
        assert!(
            first.contains("/chat/completions") && !first.contains("/v1/chat/completions"),
            "ZAI must hit /chat/completions (no /v1/), got: {first}"
        );
    }
}
