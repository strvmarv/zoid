//! serde-deserializable mirror types for the TOML registry, plus `TryFrom`
//! conversions into the dependency-free `zoid_model` types.

use serde::Deserialize;
use zoid_model::{
    ModelEntry, ModelInfo, ModelPatch, ProviderEntry, ProviderPatch, Registry, RegistryPatch,
    Source, Status, ThinkingSupport, ThinkingWireShape, Transport, WireShape,
};

#[derive(Debug, Deserialize)]
pub struct RawRegistry {
    #[serde(default)]
    pub provider: Vec<RawProvider>,
}

#[derive(Debug, Deserialize)]
pub struct RawProvider {
    pub id: String,
    pub display: String,
    pub family: String,
    pub transport: RawTransport,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub key_url: Option<String>,
    #[serde(default)]
    pub key_env: Option<String>,
    #[serde(default)]
    pub model: Vec<RawModel>,
}

fn default_status() -> String {
    "available".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RawTransport {
    Http { default_base_url: String },
    Cli { default_command: String },
    Sdk,
}

#[derive(Debug, Deserialize)]
pub struct RawModel {
    pub id: String,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default = "default_wire_shape")]
    pub wire_shape: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default = "default_ctx")]
    pub context_window: u64,
    #[serde(default)]
    pub max_output: u64,
    #[serde(default = "default_true")]
    pub tools: bool,
    #[serde(default)]
    pub prompt_cache: bool,
    #[serde(default = "default_thinking")]
    pub thinking: String,
    #[serde(default = "default_thinking_wire")]
    pub thinking_wire: String,
    // local-only provisioning fields
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub download_source: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub modelfile: Option<String>,
    #[serde(default)]
    pub num_ctx: Option<u32>,
    #[serde(default)]
    pub vram_curve: Option<String>,
}

fn default_wire_shape() -> String {
    "openai-chat".to_string()
}
fn default_source() -> String {
    "static".to_string()
}
fn default_ctx() -> u64 {
    32_000
}
fn default_true() -> bool {
    true
}
fn default_thinking() -> String {
    "none".to_string()
}
fn default_thinking_wire() -> String {
    "none".to_string()
}

fn parse_wire_shape(s: &str) -> anyhow::Result<WireShape> {
    Ok(match s {
        "openai-chat" => WireShape::OpenAIChat,
        "anthropic-messages" => WireShape::AnthropicMessages,
        "openai-responses" => WireShape::OpenAIResponses,
        "google-gemini" => WireShape::GoogleGemini,
        "ollama" => WireShape::Ollama,
        other => anyhow::bail!("unknown wire_shape: {other}"),
    })
}

fn parse_source(s: &str) -> anyhow::Result<Source> {
    Ok(match s {
        "static" => Source::Static,
        "wire" => Source::Wire,
        "user" => Source::User,
        other => anyhow::bail!("unknown source: {other}"),
    })
}

fn parse_thinking(s: &str) -> anyhow::Result<ThinkingSupport> {
    Ok(match s {
        "none" => ThinkingSupport::None,
        "toggle" => ThinkingSupport::Toggle,
        "toggle-with-effort" => ThinkingSupport::ToggleWithEffort,
        "budget" => ThinkingSupport::Budget,
        "adaptive" => ThinkingSupport::Adaptive,
        other => anyhow::bail!("unknown thinking: {other}"),
    })
}

fn parse_thinking_wire(s: &str) -> anyhow::Result<ThinkingWireShape> {
    Ok(match s {
        "none" => ThinkingWireShape::None,
        "anthropic" => ThinkingWireShape::Anthropic,
        "deepseek" => ThinkingWireShape::DeepSeek,
        "openai" => ThinkingWireShape::OpenAI,
        "ollama" => ThinkingWireShape::Ollama,
        other => anyhow::bail!("unknown thinking_wire: {other}"),
    })
}

fn parse_status(s: &str) -> anyhow::Result<Status> {
    Ok(match s {
        "available" => Status::Available,
        "planned" => Status::Planned,
        other => anyhow::bail!("unknown status: {other}"),
    })
}

impl TryFrom<RawRegistry> for Registry {
    type Error = anyhow::Error;
    fn try_from(raw: RawRegistry) -> anyhow::Result<Registry> {
        let mut providers = Vec::with_capacity(raw.provider.len());
        let mut seen_providers = std::collections::HashSet::new();
        for rp in raw.provider {
            if !seen_providers.insert(rp.id.clone()) {
                anyhow::bail!("duplicate provider id: {}", rp.id);
            }
            let transport = match rp.transport {
                RawTransport::Http { default_base_url } => Transport::Http { default_base_url },
                RawTransport::Cli { default_command } => Transport::Cli { default_command },
                RawTransport::Sdk => Transport::Sdk,
            };
            let mut models = Vec::with_capacity(rp.model.len());
            let mut seen = std::collections::HashSet::new();
            for rm in rp.model {
                let key = rm.id.to_ascii_lowercase();
                if !seen.insert(key) {
                    anyhow::bail!("duplicate model id in provider {}: {}", rp.id, rm.id);
                }
                models.push(ModelEntry {
                    id: rm.id,
                    display: rm.display,
                    wire_shape: parse_wire_shape(&rm.wire_shape)?,
                    source: parse_source(&rm.source)?,
                    default: rm.default,
                    hidden: rm.hidden,
                    info: ModelInfo {
                        context_window: rm.context_window,
                        max_output: rm.max_output,
                        tools: rm.tools,
                        prompt_cache: rm.prompt_cache,
                        thinking: parse_thinking(&rm.thinking)?,
                        thinking_wire: parse_thinking_wire(&rm.thinking_wire)?,
                    },
                    runtime: rm.runtime,
                    download_source: rm.download_source,
                    quant: rm.quant,
                    modelfile: rm.modelfile,
                    num_ctx: rm.num_ctx,
                    vram_curve: rm.vram_curve,
                });
            }
            providers.push(ProviderEntry {
                id: rp.id,
                display: rp.display,
                family: rp.family,
                transport,
                status: parse_status(&rp.status)?,
                key_url: rp.key_url.filter(|s| !s.is_empty()),
                key_env: rp.key_env.filter(|s| !s.is_empty()),
                models,
            });
        }
        Ok(Registry { providers })
    }
}

/// A user-file model row: every field is `Option` so a partial override
/// preserves the shipped values for anything the user didn't set.
#[derive(Debug, Deserialize)]
pub struct RawModelPatch {
    pub id: String,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub wire_shape: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub default: Option<bool>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output: Option<u64>,
    #[serde(default)]
    pub tools: Option<bool>,
    #[serde(default)]
    pub prompt_cache: Option<bool>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub thinking_wire: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub download_source: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub modelfile: Option<String>,
    #[serde(default)]
    pub num_ctx: Option<u32>,
    #[serde(default)]
    pub vram_curve: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RawProviderPatch {
    pub id: String,
    #[serde(default)]
    pub model: Vec<RawModelPatch>,
}

#[derive(Debug, Deserialize)]
pub struct RawRegistryPatch {
    #[serde(default)]
    pub provider: Vec<RawProviderPatch>,
}

impl TryFrom<RawRegistryPatch> for RegistryPatch {
    type Error = anyhow::Error;
    fn try_from(raw: RawRegistryPatch) -> anyhow::Result<RegistryPatch> {
        let mut providers = Vec::with_capacity(raw.provider.len());
        let mut seen_providers = std::collections::HashSet::new();
        for rp in raw.provider {
            if !seen_providers.insert(rp.id.clone()) {
                anyhow::bail!("duplicate provider id: {}", rp.id);
            }
            let mut models = Vec::with_capacity(rp.model.len());
            let mut seen = std::collections::HashSet::new();
            for rm in rp.model {
                let key = rm.id.to_ascii_lowercase();
                if !seen.insert(key) {
                    anyhow::bail!("duplicate model id in provider {}: {}", rp.id, rm.id);
                }
                models.push(ModelPatch {
                    id: rm.id,
                    display: rm.display,
                    wire_shape: rm.wire_shape.as_deref().map(parse_wire_shape).transpose()?,
                    source: rm.source.as_deref().map(parse_source).transpose()?,
                    default: rm.default,
                    hidden: rm.hidden,
                    context_window: rm.context_window,
                    max_output: rm.max_output,
                    tools: rm.tools,
                    prompt_cache: rm.prompt_cache,
                    thinking: rm.thinking.as_deref().map(parse_thinking).transpose()?,
                    thinking_wire: rm.thinking_wire.as_deref().map(parse_thinking_wire).transpose()?,
                    runtime: rm.runtime,
                    download_source: rm.download_source,
                    quant: rm.quant,
                    modelfile: rm.modelfile,
                    num_ctx: rm.num_ctx,
                    vram_curve: rm.vram_curve,
                });
            }
            providers.push(ProviderPatch { id: rp.id, models });
        }
        Ok(RegistryPatch { providers })
    }
}