//! serde-deserializable mirror types for the TOML registry, plus `TryFrom`
//! conversions into the dependency-free `zoid_model` types.

use serde::{Deserialize, Serialize};
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

// ---------------------------------------------------------------------------
// Serialize mirror: a `RawRegistryPatch` shape that can be *emitted* as TOML.
// The deserialization mirror above (`RawModelPatch`/`RawProviderPatch`/
// `RawRegistryPatch`) has every field `Option` so a partial override preserves
// shipped values on read. The writer needs the inverse: a struct that omits
// `None` fields on serialize so the round-tripped file is clean and readable.
// These `Raw*Ser` types are the serialization-only mirror used by
// `refresh::write_user_file`.
// ---------------------------------------------------------------------------

fn wire_shape_str(w: WireShape) -> &'static str {
    match w {
        WireShape::OpenAIChat => "openai-chat",
        WireShape::AnthropicMessages => "anthropic-messages",
        WireShape::OpenAIResponses => "openai-responses",
        WireShape::GoogleGemini => "google-gemini",
        WireShape::Ollama => "ollama",
    }
}

fn source_str(s: Source) -> &'static str {
    match s {
        Source::Static => "static",
        Source::Wire => "wire",
        Source::User => "user",
    }
}

fn thinking_str(t: ThinkingSupport) -> &'static str {
    match t {
        ThinkingSupport::None => "none",
        ThinkingSupport::Toggle => "toggle",
        ThinkingSupport::ToggleWithEffort => "toggle-with-effort",
        ThinkingSupport::Budget => "budget",
        ThinkingSupport::Adaptive => "adaptive",
    }
}

fn thinking_wire_str(t: ThinkingWireShape) -> &'static str {
    match t {
        ThinkingWireShape::None => "none",
        ThinkingWireShape::Anthropic => "anthropic",
        ThinkingWireShape::DeepSeek => "deepseek",
        ThinkingWireShape::OpenAI => "openai",
        ThinkingWireShape::Ollama => "ollama",
    }
}

/// A model row in the user file, shaped for TOML serialization. `Option`
/// fields are skipped when `None` (via `#[serde(skip_serializing_if)]`) so the
/// emitted file only carries the fields that were actually set — round-tripping
/// a partial user edit back through `parse_user` preserves the same override
/// semantics.
#[derive(Debug, Serialize)]
pub struct RawModelSer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_wire: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modelfile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram_curve: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RawProviderSer {
    pub id: String,
    pub model: Vec<RawModelSer>,
}

#[derive(Debug, Serialize)]
pub struct RawRegistrySer {
    pub provider: Vec<RawProviderSer>,
}

impl RawModelSer {
    /// Build a `wire`-source row from a `ModelInfo` caps block. A wire row
    /// always carries its full caps (the reconcile fetched them), so every
    /// caps field is emitted; local-provisioning fields are left `None`.
    pub fn wire_row(id: &str, wire_shape: WireShape, info: &ModelInfo) -> Self {
        Self {
            id: id.to_string(),
            display: None,
            wire_shape: Some(wire_shape_str(wire_shape).to_string()),
            source: Some(source_str(Source::Wire).to_string()),
            default: None,
            hidden: None,
            context_window: Some(info.context_window),
            max_output: Some(info.max_output),
            tools: Some(info.tools),
            prompt_cache: Some(info.prompt_cache),
            thinking: Some(thinking_str(info.thinking).to_string()),
            thinking_wire: Some(thinking_wire_str(info.thinking_wire).to_string()),
            runtime: None,
            download_source: None,
            quant: None,
            modelfile: None,
            num_ctx: None,
            vram_curve: None,
        }
    }

    /// Re-serialize an existing parsed user patch row verbatim (preserves a
    /// user's manual `hidden`/`default`/`display`/local-provisioning edits).
    pub fn from_patch(p: &ModelPatch) -> Self {
        Self {
            id: p.id.clone(),
            display: p.display.clone(),
            wire_shape: p.wire_shape.map(wire_shape_str).map(str::to_string),
            source: p.source.map(source_str).map(str::to_string),
            default: p.default,
            hidden: p.hidden,
            context_window: p.context_window,
            max_output: p.max_output,
            tools: p.tools,
            prompt_cache: p.prompt_cache,
            thinking: p.thinking.map(thinking_str).map(str::to_string),
            thinking_wire: p.thinking_wire.map(thinking_wire_str).map(str::to_string),
            runtime: p.runtime.clone(),
            download_source: p.download_source.clone(),
            quant: p.quant.clone(),
            modelfile: p.modelfile.clone(),
            num_ctx: p.num_ctx,
            vram_curve: p.vram_curve.clone(),
        }
    }
}