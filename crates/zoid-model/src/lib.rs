//! Basic, caps-only model registry (spec 2026-07-01-model-registry.md): one
//! source of truth for known providers/models and per-model capabilities.
//! No cost/pricing (economy is tokens-only). Wire-derived caps (Ollama
//! /api/show) are a future refinement.
//!
//! This lives in its own leaf crate (no dependencies) so both `zoid-provider`
//! and `zoid-tui` can share the catalog without the TUI reaching into the
//! provider implementation crate, and without coupling `zoid-provider` to
//! `zoid-core`. `zoid-provider` re-exports it as `zoid_provider::model`.

/// Stable, model-agnostic capabilities of a model. No cost fields by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output: u64, // 0 = "use provider default"
    pub tools: bool,
    /// Whether this model's provider reports a token-level prompt cache
    /// (e.g. Anthropic's `cache_read_input_tokens`). When false, the session
    /// drawer shows "n/a" for the `cac` line and the context drawer dims its
    /// cache sparkline.
    pub prompt_cache: bool,
    pub thinking: ThinkingSupport,
    pub thinking_wire: ThinkingWireShape,
}

/// Whether and how a model supports reasoning/thinking modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingSupport {
    /// Model doesn't support thinking.
    None,
    /// On/off only (Ollama).
    Toggle,
    /// On/off + effort levels (DeepSeek, OpenAI).
    ToggleWithEffort,
    /// On/off + token budget (Anthropic older models — 4.5, earlier).
    Budget,
    /// Always-on adaptive; effort controls depth (Anthropic newest).
    Adaptive,
}

/// Which native param shape the provider emits for thinking.
/// Drives the OpenAI-compat builder to distinguish DeepSeek from OpenAI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingWireShape {
    /// No thinking params on the wire.
    None,
    /// Anthropic: thinking: {type, budget_tokens?, effort?}
    Anthropic,
    /// DeepSeek: thinking: {type} + reasoning_effort
    DeepSeek,
    /// OpenAI: reasoning_effort
    OpenAI,
    /// Ollama: think: bool
    Ollama,
}

/// How a provider entry is reached. Http/Cli carry their default connection
/// value; Sdk has none (ambient auth). This is the growth seam for new
/// transports (spec 2026-07-03-settings-redesign).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Http { default_base_url: &'static str },
    Cli { default_command: &'static str },
    Sdk,
}

/// Whether an entry is implemented (selectable) or a visible-but-inert seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned,
}

/// One provider flavor. `id` is a stable hyphenated `family-variant` key;
/// code reads these fields, never substring-parses `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub display: &'static str,
    pub family: &'static str,
    pub transport: Transport,
    pub models: &'static [&'static str],
    pub status: Status,
}

/// The provider registry. Order is the picker display order.
pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "ollama-local",
        display: "ollama · local",
        family: "ollama",
        transport: Transport::Http {
            default_base_url: "http://localhost:11434",
        },
        models: &[], // local tags are arbitrary; free-text entry
        status: Status::Available,
    },
    ProviderEntry {
        id: "ollama-cloud",
        display: "ollama · cloud",
        family: "ollama",
        transport: Transport::Http {
            default_base_url: "https://ollama.com",
        },
        models: &["glm-5.2:cloud"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "opencode-go",
        display: "opencode · go",
        family: "opencode-go",
        transport: Transport::Http {
            default_base_url: "https://opencode.ai/zen/go",
        },
        models: &[
            "glm-5.2",
            "glm-5.1",
            "kimi-k2.7-code",
            "kimi-k2.6",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "mimo-v2.5",
            "mimo-v2.5-pro",
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.7-max",
            "qwen3.7-plus",
        ],
        status: Status::Available,
    },
    ProviderEntry {
        id: "anthropic-api",
        display: "anthropic · api key",
        family: "anthropic",
        transport: Transport::Http {
            default_base_url: "https://api.anthropic.com",
        },
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "zai-coding-plan",
        display: "zai · coding plan",
        family: "zai",
        transport: Transport::Http {
            default_base_url: "https://api.z.ai/api/coding/paas/v4",
        },
        models: &["glm-5.2", "glm-5-turbo", "glm-4.7"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "opencode-zen",
        display: "opencode · zen",
        family: "opencode-zen",
        transport: Transport::Http {
            default_base_url: "https://opencode.ai/zen",
        },
        models: ZEN_MODEL_IDS,
        status: Status::Available,
    },
];

/// All 52 Zen model ids, grouped by wire shape. First entry = default model.
/// Wire-shape routing lives in `opencode_zen.rs::ZEN_MODELS`; this list is the
/// registry's model picker source.
pub static ZEN_MODEL_IDS: &[&str] = &[
    // --- Anthropic Messages (13) ---
    "claude-sonnet-4-5", // default model
    "claude-fable-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-plus",
    "qwen3.5-plus",
    // --- OpenAI Responses (17) ---
    "gpt-5.5",
    "gpt-5.5-pro",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2",
    "gpt-5.2-codex",
    "gpt-5.1",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
    "gpt-5.1-codex-mini",
    "gpt-5",
    "gpt-5-codex",
    "gpt-5-nano",
    // --- OpenAI Chat Completions (19) ---
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-v4-flash-free",
    "glm-5.2",
    "glm-5.1",
    "glm-5",
    "grok-4.5",
    "grok-build-0.1",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "big-pickle",
    "hy3-free",
    "mimo-v2.5-free",
    "north-mini-code-free",
    "nemotron-3-ultra-free",
    // --- Google Gemini (3) ---
    "gemini-3.5-flash",
    "gemini-3.1-pro",
    "gemini-3-flash",
];

/// Per-model capabilities. One entry per known model id (case-insensitive
/// lookup). Unknown models get a conservative default (32k, no prompt cache).
const MODEL_CAPS: &[(&str, ModelInfo)] = &[
    (
        "claude-sonnet-4-6",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 0,
            // Tool-use wired via the typed anthropic submodule (P1b.1).
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::Budget,
            thinking_wire: ThinkingWireShape::Anthropic,
        },
    ),
    (
        "claude-opus-4-8",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 0,
            // Tool-use wired via the typed anthropic submodule (P1b.1).
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::Adaptive,
            thinking_wire: ThinkingWireShape::Anthropic,
        },
    ),
    (
        "glm-5.2:cloud",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 0,
            tools: true,
            // Ollama's `keep_alive` holds the KV cache warm (implicit prompt cache).
            // Cache-read tokens are approximated via prefix overlap in the
            // ollama provider's `parse_line`, not reported natively by the API.
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    // deepseek-v4-pro: corrected via api-docs.deepseek.com (was 128_000/0/false).
    // DeepSeek's own docs confirm a 1M context window and 384K max output, plus
    // a Context Caching feature (cached read at $0.0028 vs $0.14 uncached).
    (
        "deepseek-v4-pro",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 384_000,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::DeepSeek,
        },
    ),
    // --- OpenCode Go models (12 new entries; deepseek-v4-pro corrected above) ---
    // glm-5.2: reconciled with existing glm-5.2:cloud (same model, 1M window).
    (
        "glm-5.2",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 131_072,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::DeepSeek,
        },
    ),
    // glm-5-turbo: GLM-5 family fast variant, ZAI Coding Plan model.
    (
        "glm-5-turbo",
        ModelInfo {
            context_window: 262_144,
            max_output: 131_072,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::DeepSeek,
        },
    ),
    // glm-4.7: Sonnet-level model, ZAI Coding Plan model.
    (
        "glm-4.7",
        ModelInfo {
            context_window: 200_000,
            max_output: 131_072,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::DeepSeek,
        },
    ),
    // glm-5.1: inferred from glm-5.2:cloud sibling (same GLM-5.x family, 1M window).
    (
        "glm-5.1",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    // Kimi: confirmed via platform.kimi.ai (262,144-token window).
    (
        "kimi-k2.7-code",
        ModelInfo {
            context_window: 262_144,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "kimi-k2.6",
        ModelInfo {
            context_window: 262_144,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    // deepseek-v4-flash: confirmed via api-docs.deepseek.com (1M window, 384K max output).
    (
        "deepseek-v4-flash",
        ModelInfo {
            context_window: 1_000_000,
            max_output: 384_000,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::DeepSeek,
        },
    ),
    // MiMo: unconfirmed — approx from public claims; override via ZOID_CONTEXT_CEILING.
    (
        "mimo-v2.5",
        ModelInfo {
            context_window: 128_000,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "mimo-v2.5-pro",
        ModelInfo {
            context_window: 128_000,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    // Anthropic-shape Go models: tools=false on day one (existing AnthropicProvider
    // is text-only P1b; a zoid-implementation limitation, not a model limitation —
    // flips to true when P1b.1 Anthropic tool_use/tool_result mapping lands).
    // prompt_cache=true per Go's advertised cached-read pricing for all 13 models.
    // Windows unconfirmed — approx from public claims; override via ZOID_CONTEXT_CEILING.
    (
        "minimax-m3",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: false,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "minimax-m2.7",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: false,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "minimax-m2.5",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: false,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "qwen3.7-max",
        ModelInfo {
            context_window: 256_000,
            max_output: 0,
            tools: false,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    (
        "qwen3.7-plus",
        ModelInfo {
            context_window: 256_000,
            max_output: 0,
            tools: false,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        },
    ),
    // OpenAI o-series: used for testing OpenAI reasoning_effort wire shape.
    (
        "o3",
        ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: true,
            prompt_cache: false,
            thinking: ThinkingSupport::ToggleWithEffort,
            thinking_wire: ThinkingWireShape::OpenAI,
        },
    ),
    // --- OpenCode Zen models (39 NEW entries; 13 overlap with Go & keep
    // their existing researched MODEL_CAPS entries — do NOT duplicate) ---
    // Anthropic Messages models (NEW, not in Go): 200k context, tools=true
    // (the Anthropic leaf supports tool-use via P1b.1), prompt_cache=true.
    (
        "claude-sonnet-4-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-fable-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-opus-4-7",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-opus-4-6",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-opus-4-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-sonnet-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "claude-haiku-4-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "qwen3.6-plus",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "qwen3.5-plus",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    // OpenAI Responses models (all 17 gpt-* are NEW): 200k, tools=true,
    // thinking=ToggleWithEffort, thinking_wire=OpenAI.
    (
        "gpt-5.5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.5-pro",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.4",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.4-pro",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.4-mini",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.4-nano",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.3-codex",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.3-codex-spark",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.2",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.2-codex",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.1",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.1-codex-max",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.1-codex",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5.1-codex-mini",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5-codex",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    (
        "gpt-5-nano",
        ModelInfo { context_window: 200_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::ToggleWithEffort, thinking_wire: ThinkingWireShape::OpenAI },
    ),
    // OpenAI Chat Completions models (NEW only): 128k context, tools=true.
    (
        "grok-4.5",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "grok-build-0.1",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "kimi-k2.5",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "deepseek-v4-flash-free",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "glm-5",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "big-pickle",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "hy3-free",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "mimo-v2.5-free",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "north-mini-code-free",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "nemotron-3-ultra-free",
        ModelInfo { context_window: 128_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::None, thinking_wire: ThinkingWireShape::None },
    ),
    // Google Gemini models (all 3 are NEW): 1M context, tools=true, thinking=Toggle.
    (
        "gemini-3-flash",
        ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::Toggle, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "gemini-3.1-pro",
        ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::Toggle, thinking_wire: ThinkingWireShape::None },
    ),
    (
        "gemini-3.5-flash",
        ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: false, thinking: ThinkingSupport::Toggle, thinking_wire: ThinkingWireShape::None },
    ),
];

/// Conservative fallback for models not in the registry. Under-estimating the
/// window makes ACM compact a little early (safe); over-estimating risks never
/// compacting and overflowing the real window.
const DEFAULT_MODEL_INFO: ModelInfo = ModelInfo {
    context_window: 32_000,
    max_output: 0,
    tools: true,
    prompt_cache: false,
    thinking: ThinkingSupport::None,
    thinking_wire: ThinkingWireShape::None,
};

/// Resolve a stored/legacy provider id to its canonical registry id.
/// Preserves today's behavior: bare `ollama` meant the cloud endpoint.
pub fn canonical_id(raw: &str) -> &str {
    match raw {
        "ollama" => "ollama-cloud",
        "anthropic" => "anthropic-api",
        other => other,
    }
}

/// The registry entry for a provider id (resolving legacy aliases).
pub fn entry(id: &str) -> Option<&'static ProviderEntry> {
    let id = canonical_id(id);
    PROVIDERS.iter().find(|e| e.id == id)
}

/// Known model ids for a provider (first = default). Empty for free-text-only
/// providers (local Ollama) or unknown ids. Resolves legacy aliases.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    entry(provider).map(|e| e.models).unwrap_or(&[])
}

/// The registry default base URL for an HTTP-transport provider, else `None`.
pub fn default_base_url(provider: &str) -> Option<&'static str> {
    match entry(provider).map(|e| e.transport) {
        Some(Transport::Http { default_base_url }) => Some(default_base_url),
        _ => None,
    }
}

/// Iterator over selectable (Available) entries — `[planned]` excluded.
pub fn selectable() -> impl Iterator<Item = &'static ProviderEntry> {
    PROVIDERS.iter().filter(|e| e.status == Status::Available)
}

/// Capabilities for `model`, looked up by exact id (case-insensitive) in the
/// `MODEL_CAPS` table. Unknown models get a conservative default (32k, no
/// prompt cache).
pub fn model_info(model: &str) -> ModelInfo {
    let m = model.to_ascii_lowercase();
    MODEL_CAPS
        .iter()
        .find(|(id, _)| id.to_ascii_lowercase() == m)
        .map(|(_, info)| *info)
        .unwrap_or(DEFAULT_MODEL_INFO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_info_exact_lookup() {
        assert_eq!(model_info("claude-sonnet-4-6").context_window, 1_000_000);
        assert!(model_info("claude-sonnet-4-6").prompt_cache);
        assert_eq!(model_info("claude-opus-4-8").context_window, 1_000_000);
        assert_eq!(model_info("glm-5.2:cloud").context_window, 1_000_000);
        assert!(
            model_info("glm-5.2:cloud").prompt_cache,
            "glm-5.2:cloud now reports prompt_cache=true (Ollama keep_alive implicit cache, approximated via prefix overlap)"
        );
        assert_eq!(model_info("deepseek-v4-pro").context_window, 1_000_000);
        assert_eq!(model_info("deepseek-v4-pro").max_output, 384_000);
        assert!(model_info("deepseek-v4-pro").prompt_cache);
    }

    #[test]
    fn model_info_case_insensitive() {
        assert_eq!(model_info("CLAUDE-SONNET-4-6").context_window, 1_000_000);
        assert_eq!(model_info("DEEPSEEK-V4-PRO").context_window, 1_000_000);
        assert_eq!(model_info("GlM-5.2:ClOuD").context_window, 1_000_000);
    }

    #[test]
    fn model_info_unknown_falls_back_to_conservative_default() {
        let info = model_info("some-tiny-local:8b");
        assert_eq!(info.context_window, 32_000);
        assert!(!info.prompt_cache);
        assert!(info.tools);
    }

    #[test]
    fn canonical_id_maps_legacy_aliases() {
        assert_eq!(canonical_id("ollama"), "ollama-cloud");
        assert_eq!(canonical_id("anthropic"), "anthropic-api");
        assert_eq!(canonical_id("ollama-local"), "ollama-local"); // pass-through
        assert_eq!(canonical_id("unknown"), "unknown");
    }

    #[test]
    fn models_for_by_id_and_alias() {
        assert_eq!(models_for("ollama"), &["glm-5.2:cloud"]); // alias → cloud
        assert_eq!(models_for("ollama-cloud"), &["glm-5.2:cloud"]);
        assert!(models_for("ollama-local").is_empty()); // local tags are free-text
        assert!(models_for("anthropic-api").contains(&"claude-sonnet-4-6"));
        assert!(models_for("nonexistent").is_empty());
    }

    #[test]
    fn entry_anthropic_api_is_http() {
        let e = entry("anthropic-api").unwrap();
        assert_eq!(e.id, "anthropic-api");
        assert_eq!(e.family, "anthropic");
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://api.anthropic.com"
            }
        );
        assert_eq!(e.status, Status::Available);
    }

    #[test]
    fn default_base_url_anthropic_api_only() {
        assert_eq!(
            default_base_url("anthropic-api"),
            Some("https://api.anthropic.com")
        );
        // removed rows resolve to None (entry() returns None)
        assert!(entry("anthropic-cli").is_none());
        assert!(entry("anthropic-sdk").is_none());
        assert!(default_base_url("anthropic-cli").is_none());
        assert!(default_base_url("anthropic-sdk").is_none());
    }

    #[test]
    fn selectable_has_six_providers() {
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert_eq!(ids.len(), 6);
        assert!(ids.contains(&"ollama-local"));
        assert!(ids.contains(&"ollama-cloud"));
        assert!(ids.contains(&"opencode-go"));
        assert!(ids.contains(&"opencode-zen"));
        assert!(ids.contains(&"anthropic-api"));
        assert!(ids.contains(&"zai-coding-plan"));
    }

    #[test]
    fn zai_coding_plan_registry_entry_exists_and_is_selectable() {
        let e = entry("zai-coding-plan").expect("zai-coding-plan entry must exist");
        assert_eq!(e.id, "zai-coding-plan");
        assert_eq!(e.family, "zai");
        assert_eq!(e.status, Status::Available);
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://api.z.ai/api/coding/paas/v4"
            }
        );
        assert_eq!(e.models, &["glm-5.2", "glm-5-turbo", "glm-4.7"]);
        assert_eq!(e.models.len(), 3);
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"zai-coding-plan"));
    }

    #[test]
    fn claude_models_now_support_tools() {
        assert!(model_info("claude-sonnet-4-6").tools);
        assert!(model_info("claude-opus-4-8").tools);
        assert!(model_info("claude-sonnet-4-6").prompt_cache);
        assert!(model_info("claude-opus-4-8").prompt_cache);
    }
}

#[cfg(test)]
mod opencode_zen_tests {
    use super::*;

    #[test]
    fn opencode_zen_registry_entry_exists_and_is_selectable() {
        let e = entry("opencode-zen").expect("opencode-zen entry must exist");
        assert_eq!(e.id, "opencode-zen");
        assert_eq!(e.family, "opencode-zen");
        assert_eq!(e.display, "opencode · zen");
        assert_eq!(e.status, Status::Available);
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://opencode.ai/zen"
            }
        );
        assert!(!e.models.is_empty(), "must list at least one model");
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"opencode-zen"));
    }

    #[test]
    fn canonical_id_opencode_zen_is_passthrough() {
        assert_eq!(canonical_id("opencode-zen"), "opencode-zen");
    }

    #[test]
    fn default_base_url_opencode_zen() {
        assert_eq!(
            default_base_url("opencode-zen"),
            Some("https://opencode.ai/zen")
        );
    }

    #[test]
    fn opencode_zen_model_caps_present() {
        for id in entry("opencode-zen").unwrap().models {
            let info = model_info(id);
            // conservative but non-default: ensure each model has an
            // explicit entry (not the 32k DEFAULT_MODEL_INFO floor).
            assert!(
                info.context_window >= 128_000,
                "{id} should have an explicit caps entry, got {info:?}"
            );
        }
    }

    #[test]
    fn opencode_zen_caps_match_table() {
        // Lock ALL 39 NEW model caps (13 overlap with Go — their existing caps
        // are authoritative, not duplicated here). Mirrors the Go provider's
        // opencode_go_model_caps_match_reconciled_table which locks all 13.
        let cases: &[(&str, u64, u64, bool, bool)] = &[
            // (id, context_window, max_output, tools, prompt_cache)
            // --- Anthropic Messages (NEW — 11 models; 4-6/opus-4-8 overlap) ---
            ("claude-sonnet-4-5", 200_000, 0, true, true),
            ("claude-fable-5", 200_000, 0, true, true),
            ("claude-opus-4-7", 200_000, 0, true, true),
            ("claude-opus-4-6", 200_000, 0, true, true),
            ("claude-opus-4-5", 200_000, 0, true, true),
            ("claude-sonnet-5", 200_000, 0, true, true),
            ("claude-haiku-4-5", 200_000, 0, true, true),
            ("qwen3.6-plus", 200_000, 0, true, true),
            ("qwen3.5-plus", 200_000, 0, true, true),
            // --- OpenAI Responses (all 17 gpt-* are NEW) ---
            ("gpt-5.5", 200_000, 0, true, false),
            ("gpt-5.5-pro", 200_000, 0, true, false),
            ("gpt-5.4", 200_000, 0, true, false),
            ("gpt-5.4-pro", 200_000, 0, true, false),
            ("gpt-5.4-mini", 200_000, 0, true, false),
            ("gpt-5.4-nano", 200_000, 0, true, false),
            ("gpt-5.3-codex", 200_000, 0, true, false),
            ("gpt-5.3-codex-spark", 200_000, 0, true, false),
            ("gpt-5.2", 200_000, 0, true, false),
            ("gpt-5.2-codex", 200_000, 0, true, false),
            ("gpt-5.1", 200_000, 0, true, false),
            ("gpt-5.1-codex-max", 200_000, 0, true, false),
            ("gpt-5.1-codex", 200_000, 0, true, false),
            ("gpt-5.1-codex-mini", 200_000, 0, true, false),
            ("gpt-5", 200_000, 0, true, false),
            ("gpt-5-codex", 200_000, 0, true, false),
            ("gpt-5-nano", 200_000, 0, true, false),
            // --- OpenAI Chat Completions (NEW — 10 models; glm/deepseek/kimi/minimax overlap) ---
            ("grok-4.5", 128_000, 0, true, false),
            ("grok-build-0.1", 128_000, 0, true, false),
            ("kimi-k2.5", 128_000, 0, true, false),
            ("deepseek-v4-flash-free", 128_000, 0, true, false),
            ("glm-5", 128_000, 0, true, false),
            ("big-pickle", 128_000, 0, true, false),
            ("hy3-free", 128_000, 0, true, false),
            ("mimo-v2.5-free", 128_000, 0, true, false),
            ("north-mini-code-free", 128_000, 0, true, false),
            ("nemotron-3-ultra-free", 128_000, 0, true, false),
            // --- Google Gemini (all 3 are NEW) ---
            ("gemini-3-flash", 1_000_000, 0, true, false),
            ("gemini-3.1-pro", 1_000_000, 0, true, false),
            ("gemini-3.5-flash", 1_000_000, 0, true, false),
        ];
        for (id, ctx, max_out, tools, pc) in cases {
            let info = model_info(id);
            assert_eq!(info.context_window, *ctx, "ctx mismatch for {id}");
            assert_eq!(info.max_output, *max_out, "max_output mismatch for {id}");
            assert_eq!(info.tools, *tools, "tools mismatch for {id}");
            assert_eq!(info.prompt_cache, *pc, "prompt_cache mismatch for {id}");
        }
    }

    #[test]
    fn opencode_go_entry_unchanged() {
        // Regression: the Go entry must NOT have been modified by the Zen slice.
        let e = entry("opencode-go").unwrap();
        assert_eq!(e.display, "opencode · go");
        assert_eq!(e.family, "opencode-go");
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://opencode.ai/zen/go"
            }
        );
        assert_eq!(e.models.len(), 13);
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    #[test]
    fn claude_models_have_thinking_support() {
        let sonnet = model_info("claude-sonnet-4-6");
        assert_eq!(sonnet.thinking, ThinkingSupport::Budget);
        assert_eq!(sonnet.thinking_wire, ThinkingWireShape::Anthropic);

        let opus = model_info("claude-opus-4-8");
        assert_eq!(opus.thinking, ThinkingSupport::Adaptive);
        assert_eq!(opus.thinking_wire, ThinkingWireShape::Anthropic);
    }

    #[test]
    fn deepseek_models_have_thinking_support() {
        let pro = model_info("deepseek-v4-pro");
        assert_eq!(pro.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(pro.thinking_wire, ThinkingWireShape::DeepSeek);

        let flash = model_info("deepseek-v4-flash");
        assert_eq!(flash.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(flash.thinking_wire, ThinkingWireShape::DeepSeek);
    }

    #[test]
    fn glm_5_2_has_thinking_with_effort() {
        let glm = model_info("glm-5.2");
        assert_eq!(glm.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(glm.thinking_wire, ThinkingWireShape::DeepSeek);
        assert_eq!(glm.max_output, 131_072);
    }

    #[test]
    fn glm_5_2_capabilities_locked() {
        let info = model_info("glm-5.2");
        assert_eq!(info.context_window, 1_000_000);
        assert_eq!(info.max_output, 131_072);
        assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
    }

    #[test]
    fn glm_5_turbo_capabilities_locked() {
        let info = model_info("glm-5-turbo");
        assert_eq!(info.context_window, 262_144);
        assert_eq!(info.max_output, 131_072);
        assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
    }

    #[test]
    fn glm_4_7_capabilities_locked() {
        let info = model_info("glm-4.7");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output, 131_072);
        assert_eq!(info.thinking, ThinkingSupport::ToggleWithEffort);
        assert_eq!(info.thinking_wire, ThinkingWireShape::DeepSeek);
    }

    #[test]
    fn unknown_model_defaults_to_no_thinking() {
        let info = model_info("some-unknown-model");
        assert_eq!(info.thinking, ThinkingSupport::None);
        assert_eq!(info.thinking_wire, ThinkingWireShape::None);
    }
}

#[cfg(test)]
mod opencode_go_tests {
    use super::*;

    #[test]
    fn opencode_go_registry_entry_exists_and_is_selectable() {
        let e = entry("opencode-go").expect("opencode-go entry must exist");
        assert_eq!(e.id, "opencode-go");
        assert_eq!(e.family, "opencode-go");
        assert_eq!(e.status, Status::Available);
        assert_eq!(
            e.transport,
            Transport::Http {
                default_base_url: "https://opencode.ai/zen/go"
            }
        );
        assert_eq!(e.models.len(), 13);
        assert_eq!(e.models[0], "glm-5.2"); // default model
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"opencode-go"));
    }

    #[test]
    fn canonical_id_opencode_go_is_passthrough() {
        assert_eq!(canonical_id("opencode-go"), "opencode-go");
    }

    /// Table-driven caps assertion: every Go model id has the reconciled caps.
    #[test]
    fn opencode_go_model_caps_match_reconciled_table() {
        let cases: &[(&str, u64, u64, bool, bool)] = &[
            // (id, context_window, max_output, tools, prompt_cache)
            ("glm-5.2", 1_000_000, 131_072, true, true),
            ("glm-5.1", 1_000_000, 0, true, true),
            ("kimi-k2.7-code", 262_144, 0, true, true),
            ("kimi-k2.6", 262_144, 0, true, true),
            ("deepseek-v4-pro", 1_000_000, 384_000, true, true),
            ("deepseek-v4-flash", 1_000_000, 384_000, true, true),
            ("mimo-v2.5", 128_000, 0, true, true),
            ("mimo-v2.5-pro", 128_000, 0, true, true),
            ("minimax-m3", 200_000, 0, false, true),
            ("minimax-m2.7", 200_000, 0, false, true),
            ("minimax-m2.5", 200_000, 0, false, true),
            ("qwen3.7-max", 256_000, 0, false, true),
            ("qwen3.7-plus", 256_000, 0, false, true),
        ];
        for (id, ctx, max_out, tools, pc) in cases {
            let info = model_info(id);
            assert_eq!(info.context_window, *ctx, "ctx mismatch for {id}");
            assert_eq!(info.max_output, *max_out, "max_output mismatch for {id}");
            assert_eq!(info.tools, *tools, "tools mismatch for {id}");
            assert_eq!(info.prompt_cache, *pc, "prompt_cache mismatch for {id}");
        }
    }

    /// Regression lock for the deepseek-v4-pro correction.
    #[test]
    fn deepseek_v4_pro_correction_locked() {
        let info = model_info("deepseek-v4-pro");
        assert_eq!(info.context_window, 1_000_000, "was 128_000; do not revert");
        assert_eq!(info.max_output, 384_000);
        assert!(info.prompt_cache, "was false; do not revert");
    }
}
