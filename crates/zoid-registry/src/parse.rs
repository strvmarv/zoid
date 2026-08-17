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

    const USER: &str = r#"
[[provider]]
id = "anthropic-api"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  source = "wire"
"#;

    #[test]
    fn parse_user_rejects_static_source() {
        let bad = USER.replace("source = \"wire\"", "source = \"static\"");
        assert!(parse_user(&bad).is_err());
    }

    #[test]
    fn parse_user_omitted_source_is_none() {
        let raw = r#"
[[provider]]
id = "anthropic-api"

  [[provider.model]]
  id = "claude-sonnet-4-6"
"#;
        let patch = parse_user(raw).unwrap();
        assert_eq!(patch.providers.len(), 1);
        let m = &patch.providers[0].models[0];
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert!(m.source.is_none());
    }

    #[test]
    fn parse_user_accepts_wire_and_user_sources() {
        let wire = r#"
[[provider]]
id = "anthropic-api"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  source = "wire"
"#;
        let patch = parse_user(wire).unwrap();
        assert_eq!(patch.providers[0].models[0].source, Some(zoid_model::Source::Wire));

        let user = r#"
[[provider]]
id = "anthropic-api"

  [[provider.model]]
  id = "claude-sonnet-4-6"
  source = "user"
"#;
        let patch = parse_user(user).unwrap();
        assert_eq!(patch.providers[0].models[0].source, Some(zoid_model::Source::User));
    }

    #[test]
    fn shipped_models_toml_parses() {
        let text = include_str!("../../zoid-model/models.toml");
        let reg = parse_shipped(text).unwrap();
        // 7 selectable providers (gemini-api landed).
        assert_eq!(reg.selectable().count(), 7);
        assert!(reg.entry("opencode-zen").unwrap().models.len() >= 52);
        assert!(reg.entry("opencode-go").unwrap().models.len() == 13);
    }

    #[test]
    fn shipped_registry_invariants() {
        let reg = parse_shipped(include_str!("../../zoid-model/models.toml")).unwrap();
        // seven selectable providers (gemini-api landed).
        let ids: Vec<&str> = reg.selectable().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), 7);
        for id in [
            "ollama-local",
            "ollama-cloud",
            "opencode-go",
            "opencode-zen",
            "anthropic-api",
            "zai-coding-plan",
            "gemini-api",
        ] {
            assert!(ids.contains(&id));
        }
        // key_url invariant: ollama-local None, all others Some
        for e in reg.selectable() {
            if e.id == "ollama-local" {
                assert!(e.key_url.is_none());
            } else {
                assert!(e.key_url.is_some(), "{} must have key_url", e.id);
            }
        }
        // opencode-go has 13 models
        assert_eq!(reg.entry("opencode-go").unwrap().models.len(), 13);
        // every opencode-zen model has explicit caps >= 128k
        for m in &reg.entry("opencode-zen").unwrap().models {
            assert!(m.info.context_window >= 128_000, "{} needs explicit caps", m.id);
        }
        // claude-sonnet-4-6 is split: anthropic-api 1M, opencode-zen 200K
        assert_eq!(
            reg.model_info("anthropic-api", "claude-sonnet-4-6").context_window,
            1_000_000
        );
        assert_eq!(
            reg.model_info("opencode-zen", "claude-sonnet-4-6").context_window,
            200_000
        );
    }
}