//! TOML → Registry.

use anyhow::Result;
use zoid_model::{Registry, RegistryPatch};

use crate::raw::{RawRegistry, RawRegistryPatch};

/// Parse the shipped registry. `source` defaults to `static`; `wire`/`user`
/// sources are rejected here (they belong in the user file).
pub fn parse_shipped(text: &str) -> Result<Registry> {
    let raw: RawRegistry = toml::from_str(text)?;
    let reg = Registry::try_from(raw)?;
    for p in &reg.providers {
        for m in &p.models {
            anyhow::ensure!(
                m.source == zoid_model::Source::Static,
                "shipped registry must only contain source = \"static\" (found {} in {})",
                m.id,
                p.id
            );
        }
    }
    Ok(reg)
}

/// Parse the user registry into a patch. `source` must be `wire` or `user`
/// (never `static`); omitted `source` defaults to `user`.
pub fn parse_user(text: &str) -> Result<RegistryPatch> {
    let raw: RawRegistryPatch = toml::from_str(text)?;
    let patch = RegistryPatch::try_from(raw)?;
    for p in &patch.providers {
        for m in &p.models {
            if let Some(s) = m.source {
                anyhow::ensure!(
                    s != zoid_model::Source::Static,
                    "user registry must not contain source = \"static\" (found {} in {})",
                    m.id,
                    p.id
                );
            }
        }
    }
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = r#"
[[provider]]
id = "anthropic-api"
display = "anthropic · api key"
family = "anthropic"
transport = { kind = "http", default_base_url = "https://api.anthropic.com" }
status = "available"
key_url = "https://console.anthropic.com"
key_env = "ANTHROPIC_API_KEY"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  wire_shape = "anthropic-messages"
  source = "static"
  default = true
  context_window = 1000000
  max_output = 0
  tools = true
  prompt_cache = true
  thinking = "budget"
  thinking_wire = "anthropic"
"#;

    #[test]
    fn parse_shipped_reads_provider_and_model() {
        let reg = parse_shipped(SHIPPED).unwrap();
        assert_eq!(reg.providers.len(), 1);
        let p = &reg.providers[0];
        assert_eq!(p.id, "anthropic-api");
        assert_eq!(p.key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(p.models.len(), 1);
        let m = &p.models[0];
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert_eq!(m.wire_shape, zoid_model::WireShape::AnthropicMessages);
        assert_eq!(m.source, zoid_model::Source::Static);
        assert!(m.default);
        assert_eq!(m.info.context_window, 1_000_000);
        assert_eq!(m.info.thinking, zoid_model::ThinkingSupport::Budget);
        assert_eq!(m.info.thinking_wire, zoid_model::ThinkingWireShape::Anthropic);
    }

    #[test]
    fn parse_rejects_unknown_enum_string() {
        let bad = SHIPPED.replace("thinking = \"budget\"", "thinking = \"bogus\"");
        assert!(parse_shipped(&bad).is_err());
    }

    #[test]
    fn parse_rejects_duplicate_model_id() {
        let dup = format!(
            "{SHIPPED}\n  [[provider.model]]\n  id = \"claude-sonnet-4-6\"\n  wire_shape = \"anthropic-messages\"\n  source = \"static\"\n"
        );
        assert!(parse_shipped(&dup).is_err());
    }
}