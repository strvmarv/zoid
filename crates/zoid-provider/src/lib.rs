//! The LLM provider seam: a streaming, tool-agnostic interface plus a
//! deterministic `FakeProvider` for offline tests. The real `AnthropicProvider`
//! lives in the `anthropic` submodule. The seam is intentionally self-contained
//! (no dependency on `zoid-core`) so the provider/plugin surface stays decoupled.

pub mod anthropic;
pub mod ollama;
pub mod openai_compat;
pub mod opencode_go;
pub mod openai_responses;
pub mod google_gemini;
pub mod opencode_zen;
pub mod zai;

/// The shared model/provider catalog lives in the dependency-free `zoid-model`
/// crate; re-exported here so `zoid_provider::model::…` keeps resolving for the
/// provider internals and the bin.
pub use zoid_model as model;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Connect-phase timeout (TCP + TLS handshake) for provider HTTP clients.
const CONNECT_TIMEOUT_SECS: u64 = 20;

/// Default stream idle timeout: the maximum gap with no bytes from the provider
/// — applied both to the initial response and between streamed chunks — before
/// the stream is abandoned with a `ProviderEvent::Error`. A silent mid-stream
/// stall (dropped TCP, hung cloud worker) would otherwise block the turn
/// forever, and the only recovery would be killing the process. Overridable via
/// `ZOID_HTTP_IDLE_SECS`. This is an *idle* deadline, not a total-request cap,
/// so a long-but-live generation is never cut off — the deadline re-arms on
/// every chunk. It also gates the wait for the *first* byte after `200 OK`, so
/// a model whose cold weight-load exceeds this between the response headers and
/// its first token needs a higher `ZOID_HTTP_IDLE_SECS`.
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;

/// The configured stream idle timeout: `ZOID_HTTP_IDLE_SECS` (a positive
/// integer, seconds) or the 900s default.
pub(crate) fn stream_idle_timeout() -> Duration {
    std::env::var("ZOID_HTTP_IDLE_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS))
}

/// Build a provider HTTP client with a bounded connect timeout. Falls back to
/// the default client if the builder fails (it won't in practice).
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

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
    /// Populated only on `MsgRole::Tool` messages: the originating tool-call id.
    /// OpenAI Chat Completions identifies tool results by `tool_call_id`;
    /// Ollama's native API uses `tool_name` instead (its request-body writer
    /// ignores this field). Anthropic (text-only P1b) also ignores it.
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
            tool_call_id: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: None,
            tool_call_id: None,
        }
    }
    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MsgRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: Some(name.into()),
            tool_call_id: None,
        }
    }
    /// Like `Message::tool` but with the originating tool-call id. The agent
    /// loop uses this when dispatching a tool result so the OpenAI-compat
    /// request body can emit `tool_call_id`. Existing providers ignore the id.
    pub fn tool_with_call_id(
        name: impl Into<String>,
        call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: MsgRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_name: Some(name.into()),
            tool_call_id: Some(call_id.into()),
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
    /// Reasoning/thinking token count (Anthropic only, if reported separately).
    /// 0 for providers that don't break out thinking tokens (DeepSeek bundles
    /// them into `output_tokens`; Ollama has no token-level breakdown).
    pub thinking_tokens: u64,
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
    /// Reasoning/thinking text from the model (Anthropic thinking blocks,
    /// DeepSeek `reasoning_content`, Ollama `message.thinking`). Accumulated
    /// by the agent loop and rendered as a collapsible "▶ Thinking…" marker.
    ThinkingDelta(String),
    /// Anthropic thinking-block signature (for future replay). Emitted at the
    /// end of each thinking block. Other providers never emit this.
    ThinkingSignature(String),
    ToolCall(ToolCall),
    /// An **additive** usage delta. The agent loop sums every `Usage` event in a
    /// sub-turn, so a provider must emit each token dimension exactly once, or as
    /// disjoint deltas — never a running cumulative total on every chunk, which
    /// would double-count under summation. Anthropic reports input on
    /// `message_start` and output on `message_delta` (two disjoint events);
    /// Ollama emits a single cumulative snapshot on its final frame only.
    Usage(Usage),
    /// The model stopped because it hit the output token cap (Anthropic
    /// `stop_reason:"max_tokens"` / Ollama `done_reason:"length"`), so its reply
    /// is incomplete. Emitted just before `Done`; the agent surfaces a warning
    /// but still treats the following `Done` as the terminal event.
    Truncated,
    Done,
    Error(String),
}

/// Reasoning effort level for models that support granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

/// Controls whether and how the model reasons (thinks) before answering.
/// Phase 1: reasoning content is consumed and discarded by each provider's
/// parse layer — never surfaced to the agent loop or UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingMode {
    /// Thinking disabled (today's behavior — the default).
    #[default]
    Off,
    /// Thinking enabled; derive budget/effort from model capabilities + context.
    Auto,
    /// Thinking enabled at a specific effort level.
    Effort(EffortLevel),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub tools: Vec<ToolSpec>,
    pub thinking: ThinkingMode,
    /// Live-edge re-assertion text (spec: re-floor). `None` = no reminder this
    /// request (body byte-identical to pre-feature). `Some` = adapters render it
    /// at the tail (per-adapter placement).
    pub reassert: Option<String>,
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

    /// Fetch the provider's available model ids. Default: none (offline / seam).
    async fn list_models(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Fetch capabilities for a specific model. Returns `None` when the
    /// provider doesn't support capability introspection — the static
    /// `MODEL_CAPS` registry is the fallback. Default: `None`.
    async fn fetch_model_info(&self, _model: &str) -> Result<Option<model::ModelInfo>> {
        Ok(None)
    }
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

/// Whether the provider/model reports a token-level prompt cache (cache-read
/// tokens). Anthropic does (ephemeral cache breakpoints); Ollama's native
/// `/api/chat` does not (`cached` is always 0 — `keep_alive` is infra-level,
/// not token-level). Powers the "n/a" display for the session drawer's `cac`
/// line and the context drawer's cache sparkline dimming.
pub fn has_prompt_cache(model: &str) -> bool {
    model::model_info(model).prompt_cache
}

/// Heuristic: does a provider error string indicate the request exceeded the
/// model's context window? Both Anthropic ("prompt is too long", "maximum
/// context length") and Ollama/OpenAI-shape ("context length", "context window")
/// surface these in the error body. Used by the agent's bounded capacity-error
/// retry (the hard-bound backstop for a fallible pre-flight estimate).
pub fn is_context_length_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("too long")
        || m.contains("context length")
        || m.contains("context window")
        || m.contains("maximum context")
        || (m.contains("context") && m.contains("exceed"))
}

/// Parse a `{"data":[{"id":...}]}` model-list response body (the shape used by
/// both the Anthropic `/v1/models` and OpenAI-compat `/v1/models` endpoints).
/// Lenient: unknown/!json → empty.
pub fn parse_data_id_models(body: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("data").and_then(|d| d.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
                thinking_tokens: 0,
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
            thinking: crate::ThinkingMode::Off,
            reassert: None,
        };
        let (tx, mut rx) = mpsc::channel(16);
        provider.stream(&req, tx).await.unwrap();

        let mut got = Vec::new();
        while let Some(ev) = rx.recv().await {
            got.push(ev);
        }
        assert_eq!(got, script);
    }

    #[test]
    fn detects_context_length_errors() {
        assert!(is_context_length_error(
            "prompt is too long: 1050000 tokens > 1000000 maximum"
        ));
        assert!(is_context_length_error(
            "This model's maximum context length is 200000 tokens"
        ));
        assert!(is_context_length_error(
            "input length exceeds context window"
        ));
        assert!(!is_context_length_error("rate limit exceeded"));
        assert!(!is_context_length_error("connection reset"));
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
            thinking: crate::ThinkingMode::Off,
            reassert: None,
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

#[cfg(test)]
mod tool_call_id_tests {
    use super::*;

    #[test]
    fn existing_constructors_default_tool_call_id_to_none() {
        assert_eq!(Message::user("hi").tool_call_id, None);
        assert_eq!(Message::assistant("hi").tool_call_id, None);
        assert_eq!(Message::tool("read_file", "body").tool_call_id, None);
    }

    #[test]
    fn tool_with_call_id_sets_the_field() {
        let m = Message::tool_with_call_id("read_file", "call-42", "body");
        assert_eq!(m.role, MsgRole::Tool);
        assert_eq!(m.content, "body");
        assert_eq!(m.tool_name.as_deref(), Some("read_file"));
        assert_eq!(m.tool_call_id.as_deref(), Some("call-42"));
    }
}

#[cfg(test)]
mod parse_data_id_models_tests {
    use super::parse_data_id_models;

    #[test]
    fn parses_data_id_array() {
        let body = r#"{"data":[{"id":"glm-5.2"},{"id":"kimi-k2.6"}]}"#;
        assert_eq!(parse_data_id_models(body), vec!["glm-5.2", "kimi-k2.6"]);
    }

    #[test]
    fn empty_or_bad_body_is_empty() {
        assert!(parse_data_id_models("{}").is_empty());
        assert!(parse_data_id_models("not json").is_empty());
        assert!(parse_data_id_models(r#"{"data":[]}"#).is_empty());
    }
}

#[cfg(test)]
mod thinking_mode_tests {
    use super::*;

    #[test]
    fn thinking_mode_off_is_default() {
        let req = CompletionRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
            max_tokens: 8,
            tools: vec![],
            thinking: ThinkingMode::Off,
            reassert: None,
        };
        assert_eq!(req.thinking, ThinkingMode::Off);
    }

    #[test]
    fn effort_level_variants_exist() {
        assert_ne!(EffortLevel::Low, EffortLevel::High);
        assert_ne!(EffortLevel::Medium, EffortLevel::Max);
    }
}
