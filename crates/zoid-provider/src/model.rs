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
    // Explicit per-family windows. Unknown models take a CONSERVATIVE default:
    // under-estimating the window makes ACM compact a little early (safe);
    // over-estimating risks never compacting and overflowing the real window.
    let context_window = if m.contains("claude") {
        200_000
    } else if m.contains("glm") {
        256_000
    } else {
        32_000 // conservative default for unknown / small local models
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
    fn model_info_windows_are_explicit_per_model() {
        // Known models get their real window.
        assert_eq!(model_info("claude-sonnet-4-6").context_window, 200_000);
        assert_eq!(model_info("claude-opus-4-8").context_window, 200_000);
        assert_eq!(model_info("glm-5.2:cloud").context_window, 256_000);
        // Case-insensitive family match still works.
        assert_eq!(model_info("CLAUDE-sonnet-4-6").context_window, 200_000);
        // Unknown models take the CONSERVATIVE (small) default, never an
        // optimistic large one — an over-high window makes ACM under-compact.
        assert_eq!(model_info("some-tiny-local:8b").context_window, 32_000);
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
