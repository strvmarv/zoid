//! The LLM provider seam: a streaming, tool-agnostic interface plus a
//! deterministic `FakeProvider` for offline tests. The real `AnthropicProvider`
//! lives in the `anthropic` submodule. The seam is intentionally self-contained
//! (no dependency on `zoid-core`) so the provider/plugin surface stays decoupled.

pub mod anthropic;
pub mod ollama;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: MsgRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
    Usage(Usage),
    Done,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream the assistant's response to `req` by sending ordered
    /// `ProviderEvent`s into `sink`. Returns when the stream ends (the sink is
    /// dropped on return). Transport errors are reported as a final
    /// `ProviderEvent::Error` rather than an `Err` where possible.
    async fn stream(&self, req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()>;
}

/// A deterministic, offline provider that replays a scripted event list.
pub struct FakeProvider {
    pub scripted: Vec<ProviderEvent>,
}

impl FakeProvider {
    pub fn new(scripted: Vec<ProviderEvent>) -> Self {
        Self { scripted }
    }
}

#[async_trait]
impl Provider for FakeProvider {
    async fn stream(&self, _req: &CompletionRequest, sink: mpsc::Sender<ProviderEvent>) -> Result<()> {
        for ev in &self.scripted {
            if sink.send(ev.clone()).await.is_err() {
                break; // receiver gone
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_streams_scripted_events_in_order() {
        let script = vec![
            ProviderEvent::TextDelta("hel".into()),
            ProviderEvent::TextDelta("lo".into()),
            ProviderEvent::Usage(Usage { input_tokens: 3, output_tokens: 2 }),
            ProviderEvent::Done,
        ];
        let provider = FakeProvider::new(script.clone());
        let req = CompletionRequest {
            model: "fake".into(),
            system: None,
            messages: vec![Message { role: MsgRole::User, content: "hi".into() }],
            max_tokens: 64,
        };
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&req, tx).await.unwrap();

        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert_eq!(got, script);
    }
}
