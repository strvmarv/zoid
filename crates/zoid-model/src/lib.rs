//! Basic, caps-only model registry (spec 2026-07-01-model-registry.md): one
//! source of truth for known providers/models and per-model capabilities.
//! No cost/pricing (economy is tokens-only). Wire-derived caps (Ollama
//! /api/show) are a future refinement.
//!
//! This lives in its own leaf crate (no dependencies) so both `zoid-provider`
//! and `zoid-tui` can share the catalog without the TUI reaching into the
//! provider implementation crate, and without coupling `zoid-provider` to
//! `zoid-core`. `zoid-provider` re-exports it as `zoid_provider::model`.
//!
//! NOTE: As of the registry redesign (Task 1), the old hand-synced Rust-const
//! provider/model registry (`PROVIDERS`, `ZEN_MODEL_IDS`, `MODEL_CAPS` and the
//! free fns `entry`/`models_for`/`default_base_url`/`selectable`/`model_info`)
//! has been deleted. The types here are now owned (`String`/`Vec<String>`)
//! so a future runtime-loaded TOML registry can populate them. The
//! `Registry`/`ProviderEntry`/`ModelEntry`/patch types below are the new shape;
//! lookup methods are added in Task 2. Downstream consumers are rewritten in
//! Tasks 7–9 and 11 — the workspace is intentionally broken in the interim.

pub mod local_seed;

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

/// Wire protocol a (provider, model) pair routes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireShape {
    OpenAIChat,
    AnthropicMessages,
    OpenAIResponses,
    GoogleGemini,
    Ollama,
}

/// Provenance of a model row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Static,
    Wire,
    User,
}

/// Whether an entry is implemented (selectable) or a visible-but-inert seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned,
}

/// One (provider, model) row: caps + wire shape + provenance + optional
/// local-provisioning fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub display: Option<String>,
    pub wire_shape: WireShape,
    pub source: Source,
    pub default: bool,
    pub hidden: bool,
    pub info: ModelInfo,
    pub runtime: Option<String>,
    pub download_source: Option<String>,
    pub quant: Option<String>,
    pub modelfile: Option<String>,
    pub num_ctx: Option<u32>,
    pub vram_curve: Option<String>,
}

/// How a provider entry is reached. Http/Cli carry their default connection
/// value; Sdk has none (ambient auth). This is the growth seam for new
/// transports (spec 2026-07-03-settings-redesign).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Http { default_base_url: String },
    Cli { default_command: String },
    Sdk,
}

/// One provider flavor. `id` is a stable hyphenated `family-variant` key;
/// code reads these fields, never substring-parses `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: String,
    pub display: String,
    pub family: String,
    pub transport: Transport,
    pub status: Status,
    /// URL the onboarding wizard's API-key step links to for acquiring a key.
    /// `None` for keyless providers (ollama-local).
    pub key_url: Option<String>,
    /// API key environment variable name (`KEY_ENV`). `None` for keyless
    /// providers (ollama-local).
    pub key_env: Option<String>,
    pub models: Vec<ModelEntry>,
}

/// The merged registry: providers + their models, with lookup methods.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub providers: Vec<ProviderEntry>,
}

impl Registry {
    /// The registry entry for a provider id (resolving legacy aliases via the
    /// free `canonical_id` fn — there is NO `Registry::canonical_id` associated
    /// fn; the free fn is the single source of truth).
    pub fn entry(&self, id: &str) -> Option<&ProviderEntry> {
        let id = canonical_id(id);
        self.providers.iter().find(|e| e.id == id)
    }

    /// The model entries for a provider (empty for unknown ids).
    pub fn models_for(&self, provider: &str) -> &[ModelEntry] {
        self.entry(provider).map(|e| e.models.as_slice()).unwrap_or(&[])
    }

    /// The default base URL for an HTTP-transport provider, else `None`.
    pub fn default_base_url(&self, provider: &str) -> Option<&str> {
        match self.entry(provider).map(|e| &e.transport) {
            Some(Transport::Http { default_base_url }) => Some(default_base_url.as_str()),
            _ => None,
        }
    }

    /// Iterator over selectable (Available) entries.
    pub fn selectable(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.providers.iter().filter(|e| e.status == Status::Available)
    }

    /// The default model id for a provider: the `default = true` row, else the
    /// first row. `None` when the provider has no models.
    pub fn default_model(&self, provider: &str) -> Option<&str> {
        let models = self.models_for(provider);
        models
            .iter()
            .find(|m| m.default)
            .or_else(|| models.first())
            .map(|m| m.id.as_str())
    }

    /// The wire shape for a (provider, model) pair. `None` when unknown.
    pub fn wire_shape(&self, provider: &str, model: &str) -> Option<WireShape> {
        let m = model.to_ascii_lowercase();
        self.models_for(provider)
            .iter()
            .find(|e| e.id.to_ascii_lowercase() == m)
            .map(|e| e.wire_shape)
    }

    /// Capabilities for a (provider, model) pair, looked up case-insensitively.
    /// Unknown models get a conservative default (32k, no prompt cache).
    pub fn model_info(&self, provider: &str, model: &str) -> ModelInfo {
        let m = model.to_ascii_lowercase();
        self.models_for(provider)
            .iter()
            .find(|e| e.id.to_ascii_lowercase() == m)
            .map(|e| e.info)
            .unwrap_or(DEFAULT_MODEL_INFO)
    }
}

/// A partial override of a model row (from the user file). Every field is
/// `Option`; `None` means "keep the shipped value". This is what makes
/// field-level merge possible — a user who writes only `hidden = true` must
/// not clobber the shipped caps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPatch {
    pub id: String,
    pub display: Option<String>,
    pub wire_shape: Option<WireShape>,
    pub source: Option<Source>,
    pub default: Option<bool>,
    pub hidden: Option<bool>,
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    pub tools: Option<bool>,
    pub prompt_cache: Option<bool>,
    pub thinking: Option<ThinkingSupport>,
    pub thinking_wire: Option<ThinkingWireShape>,
    pub runtime: Option<String>,
    pub download_source: Option<String>,
    pub quant: Option<String>,
    pub modelfile: Option<String>,
    pub num_ctx: Option<u32>,
    pub vram_curve: Option<String>,
}

/// A partial override of a provider (from the user file): its id + model patches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderPatch {
    pub id: String,
    pub models: Vec<ModelPatch>,
}

/// The user-file patch: providers + model patches, merged over the shipped registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryPatch {
    pub providers: Vec<ProviderPatch>,
}

/// Conservative fallback for models not in the registry. Under-estimating the
/// window makes ACM compact a little early (safe); over-estimating risks never
/// compacting and overflowing the real window.
pub const DEFAULT_MODEL_INFO: ModelInfo = ModelInfo {
    context_window: 32_000,
    max_output: 0,
    tools: true,
    prompt_cache: false,
    thinking: ThinkingSupport::None,
    thinking_wire: ThinkingWireShape::None,
};

/// Resolve a stored/legacy provider id to its canonical registry id.
/// Preserves today's behavior: bare `ollama` meant the cloud endpoint.
///
/// This is the single source of truth for alias resolution — there is no
/// `Registry::canonical_id` associated fn; `Registry::entry` calls this free
/// fn.
pub fn canonical_id(raw: &str) -> &str {
    match raw {
        "ollama" => "ollama-cloud",
        "anthropic" => "anthropic-api",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_types_are_owned_and_cloneable() {
        // ModelInfo stays Copy (no string fields); ProviderEntry/Transport become owned.
        let info = ModelInfo {
            context_window: 200_000,
            max_output: 0,
            tools: true,
            prompt_cache: true,
            thinking: ThinkingSupport::None,
            thinking_wire: ThinkingWireShape::None,
        };
        let _clone = info.clone();

        // ProviderEntry owns Strings.
        let entry = ProviderEntry {
            id: "opencode-zen".to_string(),
            display: "opencode · zen".to_string(),
            family: "opencode-zen".to_string(),
            transport: Transport::Http {
                default_base_url: "https://opencode.ai/zen".to_string(),
            },
            status: Status::Available,
            key_url: Some("https://opencode.ai".to_string()),
            key_env: Some("OPENCODE_GO_API_KEY".to_string()),
            models: vec![],
        };
        assert_eq!(entry.id, "opencode-zen");
        assert_eq!(entry.transport, Transport::Http {
            default_base_url: "https://opencode.ai/zen".to_string()
        });
    }

    #[test]
    fn canonical_id_maps_legacy_aliases() {
        assert_eq!(canonical_id("ollama"), "ollama-cloud");
        assert_eq!(canonical_id("anthropic"), "anthropic-api");
        assert_eq!(canonical_id("ollama-local"), "ollama-local"); // pass-through
        assert_eq!(canonical_id("unknown"), "unknown");
    }

    #[test]
    fn default_model_info_is_conservative() {
        let info = DEFAULT_MODEL_INFO;
        assert_eq!(info.context_window, 32_000);
        assert!(!info.prompt_cache);
        assert!(info.tools);
        assert_eq!(info.thinking, ThinkingSupport::None);
        assert_eq!(info.thinking_wire, ThinkingWireShape::None);
    }

    #[test]
    fn registry_default_is_empty() {
        let reg = Registry::default();
        assert!(reg.providers.is_empty());
    }

    #[test]
    fn registry_lookup_methods() {
        let reg = Registry {
            providers: vec![ProviderEntry {
                id: "anthropic-api".to_string(),
                display: "anthropic · api key".to_string(),
                family: "anthropic".to_string(),
                transport: Transport::Http { default_base_url: "https://api.anthropic.com".to_string() },
                status: Status::Available,
                key_url: Some("https://console.anthropic.com".to_string()),
                key_env: Some("ANTHROPIC_API_KEY".to_string()),
                models: vec![
                    ModelEntry {
                        id: "claude-sonnet-4-6".to_string(),
                        display: None,
                        wire_shape: WireShape::AnthropicMessages,
                        source: Source::Static,
                        default: true,
                        hidden: false,
                        info: ModelInfo { context_window: 1_000_000, max_output: 0, tools: true, prompt_cache: true, thinking: ThinkingSupport::Budget, thinking_wire: ThinkingWireShape::Anthropic },
                        runtime: None, download_source: None, quant: None, modelfile: None, num_ctx: None, vram_curve: None,
                    },
                ],
            }],
        };

        assert!(reg.entry("anthropic-api").is_some());
        assert!(reg.entry("ANTHROPIC-API").is_none()); // provider ids are exact
        assert_eq!(reg.models_for("anthropic-api").len(), 1);
        assert_eq!(reg.default_base_url("anthropic-api"), Some("https://api.anthropic.com"));
        assert_eq!(reg.default_model("anthropic-api"), Some("claude-sonnet-4-6"));
        assert_eq!(reg.wire_shape("anthropic-api", "claude-sonnet-4-6"), Some(WireShape::AnthropicMessages));
        // model id lookup is case-insensitive
        assert_eq!(reg.model_info("anthropic-api", "CLAUDE-SONNET-4-6").context_window, 1_000_000);
        // unknown model → conservative default
        assert_eq!(reg.model_info("anthropic-api", "nope").context_window, 32_000);
        assert_eq!(reg.selectable().count(), 1);
    }
}