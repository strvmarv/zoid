//! The LLM provider seam: a streaming, tool-agnostic interface plus a
//! deterministic `FakeProvider` for offline tests. The real `AnthropicProvider`
//! lives in the `anthropic` submodule. The seam is intentionally self-contained
//! (no dependency on `zoid-core`) so the provider/plugin surface stays decoupled.

pub mod anthropic;
pub mod model;
pub mod ollama;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: MsgRole,
    pub content: String,
    /// Populated only on assistant messages that requested tools.
    pub tool_calls: Vec<ToolCall>,
    /// Populated only on `MsgRole::Tool` messages: the tool whose result this is.
    pub tool_name: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }
    }
    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: Some(name.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Total prompt tokens for the request, *including* any cache-read and
    /// cache-creation tokens (so the economy total stays honest even when a
    /// provider bills cached input on a separate line).
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache-read tokens: the subset of `input_tokens` served from the
    /// provider's prompt cache (Anthropic `cache_read_input_tokens`). 0 for
    /// providers without a token-level prompt cache (e.g. Ollama). Powers the
    /// context drawer's per-turn cache sparkline.
    pub cached: u64,
}

/// A tool the model may call (OpenAI/Ollama function shape). `parameters` is a
/// JSON Schema object describing the tool's arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// A tool invocation requested by the model. `id` is empty for providers (Ollama
/// native) that don't issue call ids; `args` is the parsed arguments object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCall(ToolCall),
    Usage(Usage),
    Done,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stream the assistant's response to `req` by sending ordered
    /// `ProviderEvent`s into `sink`. Returns when the stream ends (the sink is
    /// dropped on return). Transport errors are reported as a final
    /// `ProviderEvent::Error` rather than an `Err` where possible.
    async fn stream(
        &self,
        req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()>;
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
    async fn stream(
        &self,
        _req: &CompletionRequest,
        sink: mpsc::Sender<ProviderEvent>,
    ) -> Result<()> {
        for ev in &self.scripted {
            if sink.send(ev.clone()).await.is_err() {
                break; // receiver gone
            }
        }
        Ok(())
    }
}

/// Select the provider from the environment:
/// `OLLAMA_API_KEY` → Ollama Cloud; else `ANTHROPIC_API_KEY` → Anthropic;
/// else an offline `FakeProvider` (so the binary always runs).
pub fn default_provider() -> Arc<dyn Provider> {
    if let Ok(key) = std::env::var("OLLAMA_API_KEY") {
        if !key.is_empty() {
            return Arc::new(ollama::OllamaProvider::new(key));
        }
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Arc::new(anthropic::AnthropicProvider::new(key));
        }
    }
    Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("(no OLLAMA_API_KEY / ANTHROPIC_API_KEY — offline echo) ".into()),
        ProviderEvent::TextDelta("hello from zoid's fake provider.".into()),
        ProviderEvent::Done,
    ]))
}

/// The default model id matching the selected provider (overridden by
/// `$ZOID_MODEL` in the binary).
pub fn default_model() -> &'static str {
    if std::env::var("OLLAMA_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        ollama::DEFAULT_OLLAMA_MODEL
    } else {
        anthropic::DEFAULT_MODEL
    }
}

/// The context-window ceiling (tokens) for `model` — the economy ⑤ denominator.
/// `ZOID_CONTEXT_CEILING` (a positive integer) overrides the registry.
pub fn context_ceiling(model: &str) -> u64 {
    if let Ok(v) = std::env::var("ZOID_CONTEXT_CEILING") {
        if let Ok(n) = v.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    model::model_info(model).context_window
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn default_model_constants_are_wired() {
        // The two provider defaults are distinct and non-empty; default_model()
        // returns one of them. (Env-based branch selection is exercised at
        // runtime / manual smoke — env vars are process-global and unsafe to
        // mutate in parallel tests.)
        assert_eq!(anthropic::DEFAULT_MODEL, "claude-sonnet-4-6");
        assert_eq!(ollama::DEFAULT_OLLAMA_MODEL, "glm-5.2:cloud");
        let m = default_model();
        assert!(m == anthropic::DEFAULT_MODEL || m == ollama::DEFAULT_OLLAMA_MODEL);
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
            ProviderEvent::Usage(Usage {
                input_tokens: 3,
                output_tokens: 2,
                cached: 0,
            }),
            ProviderEvent::Done,
        ];
        let provider = FakeProvider::new(script.clone());
        let req = CompletionRequest {
            model: "fake".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 64,
            tools: vec![],
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

#[cfg(test)]
mod tool_types_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_constructors_set_role_and_fields() {
        let u = Message::user("hi");
        assert_eq!(u.role, MsgRole::User);
        assert_eq!(u.content, "hi");
        assert!(u.tool_calls.is_empty());
        assert_eq!(u.tool_name, None);

        let t = Message::tool("read_file", "file contents");
        assert_eq!(t.role, MsgRole::Tool);
        assert_eq!(t.content, "file contents");
        assert_eq!(t.tool_name.as_deref(), Some("read_file"));
    }

    #[test]
    fn request_carries_tools_and_event_carries_tool_call() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("x")],
            max_tokens: 8,
            tools: vec![spec.clone()],
        };
        assert_eq!(req.tools, vec![spec]);

        let ev = ProviderEvent::ToolCall(ToolCall {
            id: "".into(),
            name: "read_file".into(),
            args: json!({"path": "a.txt"}),
        });
        assert_eq!(
            ev,
            ProviderEvent::ToolCall(ToolCall {
                id: "".into(),
                name: "read_file".into(),
                args: json!({"path": "a.txt"})
            })
        );
    }
}
