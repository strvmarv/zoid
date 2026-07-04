//! Pure view-model for the configuration screen: turns a resolved Config +
//! Provenance + secret statuses into rendered sections. No IO, no rendering.

use zoid_core::config::{Config, Provenance, Source};
use zoid_core::secret::SecretStatus;
use zoid_model::{self as model, Status, Transport};

/// How a config field is edited/displayed (text, uint, bool, picker, or write-only secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Uint,
    Bool,
    /// Opens the col-3 contextual picker (provider / model).
    Pick,
    Secret,
}

/// One row in the col-3 picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub selectable: bool,
    pub is_current: bool,
}

/// The provider picker options (all registry entries; `[planned]` shown but
/// not selectable), each annotated with its transport endpoint/command.
pub fn provider_options(current_id: &str) -> Vec<PickOption> {
    let cur = model::canonical_id(current_id);
    model::PROVIDERS
        .iter()
        .map(|e| {
            let (kind, endpoint) = match e.transport {
                Transport::Http { default_base_url } => ("http", default_base_url.to_string()),
                Transport::Cli { default_command } => ("cli", default_command.to_string()),
                Transport::Sdk => ("sdk", "—".to_string()),
            };
            let planned = e.status == Status::Planned;
            let mut detail = format!("{kind}  {endpoint}");
            if planned {
                detail.push_str("  planned");
            }
            PickOption {
                id: e.id.to_string(),
                label: e.display.to_string(),
                detail,
                selectable: !planned,
                is_current: e.id == cur,
            }
        })
        .collect()
}

/// The model picker options for a provider (registry convenience list),
/// sorted alphabetically by name for easy scanning.
pub fn model_options(provider_id: &str, current_model: &str) -> Vec<PickOption> {
    let mut models: Vec<&str> = model::models_for(provider_id).to_vec();
    models.sort();
    models
        .iter()
        .map(|m| PickOption {
            id: (*m).to_string(),
            label: (*m).to_string(),
            detail: String::new(),
            selectable: true,
            is_current: *m == current_model,
        })
        .collect()
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

    let active = model::entry(&cfg.provider);
    let connection_row = match active.map(|e| e.transport) {
        Some(Transport::Cli { .. }) => FieldRow {
            label: "command",
            value: cfg.base_url.clone().unwrap_or_default(), // reuses base_url slot until CLI impl adds `command`
            kind: FieldKind::Text,
            source: prov.base_url,
            env_shadowed: prov.base_url == Source::Env,
        },
        // Http (and Sdk, which simply shows an empty base_url) → base_url row.
        _ => FieldRow {
            label: "base_url",
            value: cfg.base_url.clone().unwrap_or_default(),
            kind: FieldKind::Text,
            source: prov.base_url,
            env_shadowed: prov.base_url == Source::Env,
        },
    };
    let provider_model = Section {
        title: "Provider & Model".into(),
        rows: vec![
            FieldRow {
                label: "provider",
                value: cfg.provider.clone(),
                kind: FieldKind::Pick,
                source: prov.provider,
                env_shadowed: prov.provider == Source::Env,
            },
            FieldRow {
                label: "model",
                value: cfg.model.clone(),
                kind: FieldKind::Pick,
                source: prov.model,
                env_shadowed: prov.model == Source::Env,
            },
            connection_row,
        ],
    };
    let economy = Section {
        title: "Economy".into(),
        rows: vec![
            FieldRow {
                label: "context target",
                value: opt(&cfg.economy.context_target),
                kind: FieldKind::Uint,
                source: prov.context_target,
                env_shadowed: prov.context_target == Source::Env,
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
                label: "band headroom %",
                value: cfg.economy.band_headroom_pct.to_string(),
                kind: FieldKind::Uint,
                source: prov.band_headroom_pct,
                env_shadowed: prov.band_headroom_pct == Source::Env,
            },
            FieldRow {
                label: "recent turns",
                value: cfg.economy.recent_n.to_string(),
                kind: FieldKind::Uint,
                source: prov.recent_n,
                env_shadowed: prov.recent_n == Source::Env,
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
            context_target: Source::Default,
            auto_evict_cold: Source::Env,
            compact_threshold_pct: Source::Default,
            band_headroom_pct: Source::Default,
            recent_n: Source::Default,
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

    #[test]
    fn provider_options_annotate_endpoints_and_mark_planned() {
        let opts = provider_options("ollama-cloud");
        let cloud = opts.iter().find(|o| o.id == "ollama-cloud").unwrap();
        assert!(cloud.is_current);
        assert!(cloud.selectable);
        assert!(cloud.detail.contains("https://ollama.com"));

        let cli = opts.iter().find(|o| o.id == "anthropic-cli").unwrap();
        assert!(!cli.selectable); // planned
        assert!(cli.detail.contains("claude")); // command shown as its endpoint
        assert!(cli.label.contains("planned") || cli.detail.contains("planned"));
    }

    #[test]
    fn model_options_list_registry_models() {
        let opts = model_options("anthropic-api", "claude-opus-4-8");
        assert!(opts
            .iter()
            .any(|o| o.id == "claude-sonnet-4-6" && o.selectable));
        let cur = opts.iter().find(|o| o.id == "claude-opus-4-8").unwrap();
        assert!(cur.is_current);
    }

    #[test]
    fn provider_and_model_rows_are_pick_kind() {
        let cfg = Config::default();
        let prov = Provenance {
            provider: Source::Default,
            base_url: Source::Default,
            model: Source::Default,
            context_target: Source::Default,
            auto_evict_cold: Source::Default,
            compact_threshold_pct: Source::Default,
            band_headroom_pct: Source::Default,
            recent_n: Source::Default,
            reduced_motion: Source::Default,
        };
        let sections = build_sections(&cfg, &prov, &[]);
        let pm = &sections[0];
        assert_eq!(pm.rows[0].label, "provider");
        assert!(matches!(pm.rows[0].kind, FieldKind::Pick));
        assert_eq!(pm.rows[1].label, "model");
        assert!(matches!(pm.rows[1].kind, FieldKind::Pick));
        // Active provider is HTTP → connection row is base_url.
        assert_eq!(pm.rows[2].label, "base_url");
    }
}
