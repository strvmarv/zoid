//! The LLM provider seam. P0 ships only the trait + a deterministic fake;
//! the real streaming Anthropic provider arrives in P1.

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
    Done,
}

#[async_trait]
pub trait Provider {
    /// Produce the assistant's response to `prompt` as ordered events.
    /// P0 returns the full list; P1 swaps this for a streamed SSE response.
    async fn stream(&self, prompt: &str) -> Vec<ProviderEvent>;
}

pub struct FakeProvider {
    pub scripted: Vec<ProviderEvent>,
}

impl FakeProvider {
    pub fn new(scripted: Vec<ProviderEvent>) -> Self {
        FakeProvider { scripted }
    }
}

#[async_trait]
impl Provider for FakeProvider {
    async fn stream(&self, _prompt: &str) -> Vec<ProviderEvent> {
        self.scripted.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_replays_scripted_events_in_order() {
        let script = vec![
            ProviderEvent::TextDelta("hel".into()),
            ProviderEvent::TextDelta("lo".into()),
            ProviderEvent::Done,
        ];
        let provider = FakeProvider::new(script.clone());
        let out = provider.stream("ignored prompt").await;
        assert_eq!(out, script);
    }
}
