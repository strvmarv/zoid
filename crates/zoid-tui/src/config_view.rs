//! Pure view-model for the configuration screen: turns a resolved Config +
//! Provenance + secret statuses into rendered sections. No IO, no rendering.

use zoid_core::config::{Config, Provenance, Source};
use zoid_core::secret::SecretStatus;

/// How a config field is edited/displayed (text, uint, bool, provider cycle, or write-only secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Uint,
    Bool,
    Cycle(&'static [&'static str]),
    Secret,
}

/// One rendered config row: label, current value, edit kind, provenance source, and whether an env var shadows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRow {
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
    pub source: Source,
    pub env_shadowed: bool,
}

/// A titled group of config rows shown in the left-nav / right-detail panes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub rows: Vec<FieldRow>,
}

/// Build the four config sections (Provider & Model, Economy, Interface, Secrets) from a
/// resolved Config + Provenance + secret statuses. Pure; no IO.
pub fn build_sections(
    cfg: &Config,
    prov: &Provenance,
    key_status: &[(&'static str, SecretStatus)],
) -> Vec<Section> {
    let onoff = |b: bool| {
        if b {
            "on".to_string()
        } else {
            "off".to_string()
        }
    };
    let opt = |o: &Option<u64>| o.map(|n| n.to_string()).unwrap_or_else(|| "(none)".into());

    let provider_model = Section {
        title: "Provider & Model".into(),
        rows: vec![
            FieldRow {
                label: "provider",
                value: cfg.provider.clone(),
                kind: FieldKind::Cycle(zoid_provider::model::KNOWN_PROVIDERS),
                source: prov.provider,
                env_shadowed: prov.provider == Source::Env,
            },
            // model is free-text (Ollama runs arbitrary tags) with a
            // registry-backed cycle layered on in routing (Task 12): a cycle key
            // steps through models_for(cfg.provider); typing overrides freely.
            FieldRow {
                label: "model",
                value: cfg.model.clone(),
                kind: FieldKind::Text,
                source: prov.model,
                env_shadowed: prov.model == Source::Env,
            },
            FieldRow {
                label: "base_url",
                value: cfg.base_url.clone().unwrap_or_default(),
                kind: FieldKind::Text,
                source: prov.base_url,
                env_shadowed: prov.base_url == Source::Env,
            },
        ],
    };
    let economy = Section {
        title: "Economy".into(),
        rows: vec![
            FieldRow {
                label: "context ceiling",
                value: opt(&cfg.economy.context_ceiling),
                kind: FieldKind::Uint,
                source: prov.context_ceiling,
                env_shadowed: prov.context_ceiling == Source::Env,
            },
            FieldRow {
                label: "auto-evict cold",
                value: onoff(cfg.economy.auto_evict_cold),
                kind: FieldKind::Bool,
                source: prov.auto_evict_cold,
                env_shadowed: prov.auto_evict_cold == Source::Env,
            },
            FieldRow {
                label: "compact at %",
                value: cfg.economy.compact_threshold_pct.to_string(),
                kind: FieldKind::Uint,
                source: prov.compact_threshold_pct,
                env_shadowed: prov.compact_threshold_pct == Source::Env,
            },
            FieldRow {
                label: "token ceiling",
                value: opt(&cfg.economy.token_ceiling),
                kind: FieldKind::Uint,
                source: prov.token_ceiling,
                env_shadowed: prov.token_ceiling == Source::Env,
            },
        ],
    };
    let interface = Section {
        title: "Interface".into(),
        rows: vec![FieldRow {
            label: "reduced motion",
            value: onoff(cfg.reduced_motion),
            kind: FieldKind::Bool,
            source: prov.reduced_motion,
            env_shadowed: prov.reduced_motion == Source::Env,
        }],
    };
    let secrets = Section {
        title: "Secrets".into(),
        rows: key_status
            .iter()
            .map(|(name, st)| {
                let (value, shadowed) = match st {
                    SecretStatus::Set { from_env: true } => ("set".to_string(), true),
                    SecretStatus::Set { from_env: false } => ("set".to_string(), false),
                    SecretStatus::NotSet => ("not set".to_string(), false),
                };
                // `source` is inert for secret rows: nothing reads it, only `env_shadowed`
                // drives the [env] marker.
                FieldRow {
                    label: name,
                    value,
                    kind: FieldKind::Secret,
                    source: if shadowed {
                        Source::Env
                    } else {
                        Source::Default
                    },
                    env_shadowed: shadowed,
                }
            })
            .collect(),
    };
    vec![provider_model, economy, interface, secrets]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_four_sections_with_env_shadow() {
        let cfg = Config::default();
        // Inline provenance: all Default except `model` and `auto_evict_cold` shadowed
        // by env. `auto_evict_cold` was previously hardcoded to `env_shadowed: false` in
        // build_sections; this proves it now reflects provenance uniformly.
        let prov = Provenance {
            provider: Source::Default,
            base_url: Source::Default,
            model: Source::Env,
            context_ceiling: Source::Default,
            auto_evict_cold: Source::Env,
            compact_threshold_pct: Source::Default,
            token_ceiling: Source::Default,
            reduced_motion: Source::Default,
        };
        let ks = [
            ("OLLAMA_API_KEY", SecretStatus::Set { from_env: true }),
            ("ANTHROPIC_API_KEY", SecretStatus::NotSet),
        ];
        let sections = build_sections(&cfg, &prov, &ks);
        assert_eq!(sections.len(), 4);
        let model_row = &sections[0].rows[1];
        assert_eq!(model_row.label, "model");
        assert!(model_row.env_shadowed);
        let economy = sections.iter().find(|s| s.title == "Economy").unwrap();
        let auto_evict_row = economy
            .rows
            .iter()
            .find(|r| r.label == "auto-evict cold")
            .unwrap();
        assert!(auto_evict_row.env_shadowed);
        let sec = sections.iter().find(|s| s.title == "Secrets").unwrap();
        assert!(sec.rows[0].env_shadowed); // OLLAMA set from env
        assert_eq!(sec.rows[1].value, "not set");
    }
}
