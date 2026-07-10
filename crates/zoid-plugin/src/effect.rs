//! Install-time effects a plugin manifest may declare, and their risk tier.

use serde::{Deserialize, Serialize};

/// One install-time effect a plugin manifest may declare in `[[install]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Make the freshly-installed mode active.
    Activate,
    /// Emit an onboarding/status line after install.
    OnboardingHint { text: String },
    /// Write a config.toml key. Applying this is deferred to a follow-up plan;
    /// v1 rejects it at plan validation (it classifies as needing confirmation).
    SetConfig { key: String, value: toml::Value },
}

/// Whether an effect may apply silently (`Safe`) or needs explicit confirmation
/// (`Dangerous`). Classification lives with the effect so new effects declare
/// their own tier rather than every call-site re-deciding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    Safe,
    Dangerous,
}

impl Effect {
    pub fn risk(&self) -> RiskTier {
        match self {
            Effect::Activate | Effect::OnboardingHint { .. } => RiskTier::Safe,
            Effect::SetConfig { key, .. } => classify_config_key(key),
        }
    }
}

/// Fail-closed config-key classifier: only an allowlist of known-safe keys is
/// `Safe`; everything else (provider, base_url, approval, secrets-adjacent) is
/// `Dangerous`.
pub fn classify_config_key(key: &str) -> RiskTier {
    const SAFE_KEYS: &[&str] = &["skills.source_dirs", "modes.source_dirs"];
    if SAFE_KEYS.contains(&key) {
        RiskTier::Safe
    } else {
        RiskTier::Dangerous
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_and_hint_are_safe() {
        assert_eq!(Effect::Activate.risk(), RiskTier::Safe);
        assert_eq!(
            Effect::OnboardingHint { text: "hi".into() }.risk(),
            RiskTier::Safe
        );
    }

    #[test]
    fn known_config_keys_are_safe_everything_else_dangerous() {
        assert_eq!(classify_config_key("skills.source_dirs"), RiskTier::Safe);
        assert_eq!(classify_config_key("modes.source_dirs"), RiskTier::Safe);
        // Fail-closed: anything not on the allowlist is Dangerous.
        assert_eq!(classify_config_key("provider"), RiskTier::Dangerous);
        assert_eq!(classify_config_key("base_url"), RiskTier::Dangerous);
        assert_eq!(classify_config_key("approval.mode"), RiskTier::Dangerous);
    }

    #[test]
    fn set_config_risk_follows_key_classification() {
        let safe = Effect::SetConfig {
            key: "skills.source_dirs".into(),
            value: toml::Value::String("x".into()),
        };
        let dangerous = Effect::SetConfig {
            key: "provider".into(),
            value: toml::Value::String("x".into()),
        };
        assert_eq!(safe.risk(), RiskTier::Safe);
        assert_eq!(dangerous.risk(), RiskTier::Dangerous);
    }
}
