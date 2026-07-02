//! Basic, caps-only model registry (spec 2026-07-01-model-registry.md): one
//! source of truth for known providers/models and per-model capabilities.
//! No cost/pricing (economy is tokens-only). Wire-derived caps (Ollama
//! /api/show) are a future refinement.

/// Stable, model-agnostic capabilities of a model. No cost fields by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output: u64, // 0 = "use provider default"
    pub tools: bool,
}

/// The providers the config screen can cycle. First entry is the default.
pub const KNOWN_PROVIDERS: &[&str] = &["ollama", "anthropic"];

/// Known model ids for a provider (first = that provider's default). Ollama can
/// run arbitrary tags, so this is a convenience list, not a closed set — the
/// config screen offers free-text entry alongside it.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    match provider {
        "ollama" => &["glm-5.2:cloud"],
        "anthropic" => &["claude-sonnet-4-6", "claude-opus-4-8"],
        _ => &[],
    }
}

/// Capabilities for `model`, matched by family (case-insensitive), else DEFAULT.
pub fn model_info(model: &str) -> ModelInfo {
    let m = model.to_ascii_lowercase();
    // Claude is a known 200k family; everything else (incl. GLM, whose exact
    // window is a registry TODO) takes the 256k conservative default.
    let context_window = if m.contains("claude") {
        200_000
    } else {
        256_000
    };
    ModelInfo {
        context_window,
        max_output: 0,
        tools: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_info_caps_by_family_else_default() {
        assert_eq!(model_info("claude-sonnet-4-6").context_window, 200_000);
        assert_eq!(model_info("CLAUDE-opus").context_window, 200_000);
        assert_eq!(model_info("glm-5.2:cloud").context_window, 256_000);
        assert_eq!(model_info("llama3.1:70b").context_window, 256_000);
        assert!(model_info("anything").tools);
    }

    #[test]
    fn known_providers_and_models() {
        assert_eq!(KNOWN_PROVIDERS, &["ollama", "anthropic"]);
        assert_eq!(models_for("ollama"), &["glm-5.2:cloud"]);
        assert!(models_for("anthropic").contains(&"claude-sonnet-4-6"));
        assert!(models_for("unknown").is_empty());
    }
}
