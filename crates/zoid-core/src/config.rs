//! Layered application configuration (core §7.1). Pure types + merge here;
//! file/env IO lives in the binary. Secrets are NOT part of Config (see
//! `secret.rs`) — never serialize an API key to a config file.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: String,
    pub economy: EconomyConfig,
    pub reduced_motion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomyConfig {
    /// None → defer to the model registry's context_ceiling().
    pub context_ceiling: Option<u64>,
    pub auto_evict_cold: bool,
    /// 0 disables compaction; else percent of the ceiling (1–100).
    pub compact_threshold_pct: u8,
    pub token_ceiling: Option<u64>,
}

impl Default for EconomyConfig {
    fn default() -> Self {
        Self { context_ceiling: None, auto_evict_cold: true, compact_threshold_pct: 0, token_ceiling: None }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            base_url: None,
            model: String::new(), // empty → binary falls back to provider default_model()
            economy: EconomyConfig::default(),
            reduced_motion: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.provider, "ollama");
        assert!(c.economy.auto_evict_cold);
        assert_eq!(c.economy.compact_threshold_pct, 0);
        assert!(c.economy.context_ceiling.is_none());
    }
}
