//! Layered application configuration (core §7.1). Pure types + merge here;
//! file/env IO lives in the binary. Secrets are NOT part of Config (see
//! `secret.rs`) — never serialize an API key to a config file.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillsConfig {
    /// Extra directories to scan for `<skill>/SKILL.md` files (beyond the
    /// convention dirs the bin adds). Unioned across config layers.
    pub source_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub provider: String,
    pub base_url: Option<String>,
    pub model: String,
    pub economy: EconomyConfig,
    pub reduced_motion: bool,
    pub skills: SkillsConfig,
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
        Self {
            context_ceiling: None,
            auto_evict_cold: true,
            compact_threshold_pct: 0,
            token_ceiling: None,
        }
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
            skills: SkillsConfig::default(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    UserGlobal,
    Project,
    Local,
    Env,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    pub provider: Source,
    pub base_url: Source,
    pub model: Source,
    pub context_ceiling: Source,
    pub auto_evict_cold: Source,
    pub compact_threshold_pct: Source,
    pub token_ceiling: Source,
    pub reduced_motion: Source,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialEconomy {
    pub context_ceiling: Option<u64>,
    pub auto_evict_cold: Option<bool>,
    pub compact_threshold_pct: Option<u8>,
    pub token_ceiling: Option<u64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialSkills {
    pub source_dirs: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PartialConfig {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub reduced_motion: Option<bool>,
    pub economy: PartialEconomy,
    pub skills: PartialSkills,
}

/// Parse one TOML layer. Unknown keys are rejected so typos surface early.
pub fn parse_toml(s: &str) -> anyhow::Result<PartialConfig> {
    Ok(toml::from_str(s)?)
}

/// Merge layers in order; later layers override earlier. Records the winning
/// source per field. `layers` MUST start with `(Source::Default, _)` conceptually;
/// callers pass real layers and merge seeds from `Config::default()`.
pub fn merge(layers: &[(Source, PartialConfig)]) -> (Config, Provenance) {
    let mut cfg = Config::default();
    let mut prov = Provenance {
        provider: Source::Default,
        base_url: Source::Default,
        model: Source::Default,
        context_ceiling: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        token_ceiling: Source::Default,
        reduced_motion: Source::Default,
    };
    for (src, p) in layers {
        if let Some(v) = &p.provider {
            cfg.provider = v.clone();
            prov.provider = *src;
        }
        if let Some(v) = &p.base_url {
            cfg.base_url = Some(v.clone());
            prov.base_url = *src;
        }
        if let Some(v) = &p.model {
            cfg.model = v.clone();
            prov.model = *src;
        }
        if let Some(v) = p.reduced_motion {
            cfg.reduced_motion = v;
            prov.reduced_motion = *src;
        }
        if let Some(v) = p.economy.context_ceiling {
            cfg.economy.context_ceiling = Some(v);
            prov.context_ceiling = *src;
        }
        if let Some(v) = p.economy.auto_evict_cold {
            cfg.economy.auto_evict_cold = v;
            prov.auto_evict_cold = *src;
        }
        if let Some(v) = p.economy.compact_threshold_pct {
            cfg.economy.compact_threshold_pct = v;
            prov.compact_threshold_pct = *src;
        }
        if let Some(v) = p.economy.token_ceiling {
            cfg.economy.token_ceiling = Some(v);
            prov.token_ceiling = *src;
        }
        if let Some(dirs) = &p.skills.source_dirs {
            for d in dirs {
                if !cfg.skills.source_dirs.contains(d) {
                    cfg.skills.source_dirs.push(d.clone());
                }
            }
        }
    }
    (cfg, prov)
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn later_layers_override_and_record_source() {
        let user =
            parse_toml("model = \"a\"\nreduced_motion = true\n[economy]\nauto_evict_cold = false")
                .unwrap();
        let proj = parse_toml("model = \"b\"").unwrap();
        let (cfg, prov) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.model, "b");
        assert_eq!(prov.model, Source::Project); // project overrode user
        assert!(cfg.reduced_motion);
        assert_eq!(prov.reduced_motion, Source::UserGlobal);
        assert!(!cfg.economy.auto_evict_cold);
        assert_eq!(prov.auto_evict_cold, Source::UserGlobal);
        assert_eq!(prov.provider, Source::Default); // untouched
    }

    #[test]
    fn empty_layer_changes_nothing() {
        let (cfg, prov) = merge(&[(Source::UserGlobal, PartialConfig::default())]);
        assert_eq!(cfg, Config::default());
        assert_eq!(prov.model, Source::Default);
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(parse_toml("bogus = 1").is_err());
    }

    #[test]
    fn parses_skills_source_dirs() {
        let p = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        assert_eq!(
            p.skills.source_dirs,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn merge_unions_source_dirs_across_layers() {
        let user = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let proj = parse_toml("[skills]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(
            cfg.skills.source_dirs,
            vec!["a".to_string(), "b".to_string(), "c".to_string()] // "b" not duplicated
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TomlValue {
    Str(String),
    Int(i64),
    Bool(bool),
    Unset,
}

/// Set (or, for `Unset`, remove) a dotted key in a TOML document string,
/// preserving all other content. Only the top-level table and a single nested
/// table (e.g. `economy.*`) are supported — matching Config's shape.
pub fn set_in_toml(existing: &str, dotted_key: &str, value: TomlValue) -> anyhow::Result<String> {
    let mut doc: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        existing.parse()?
    };
    let to_val = |v: &TomlValue| -> Option<toml::Value> {
        match v {
            TomlValue::Str(s) => Some(toml::Value::String(s.clone())),
            TomlValue::Int(i) => Some(toml::Value::Integer(*i)),
            TomlValue::Bool(b) => Some(toml::Value::Boolean(*b)),
            TomlValue::Unset => None,
        }
    };
    match dotted_key.split_once('.') {
        None => match to_val(&value) {
            Some(v) => {
                doc.insert(dotted_key.to_string(), v);
            }
            None => {
                doc.remove(dotted_key);
            }
        },
        Some((table, key)) => {
            let entry = doc
                .entry(table.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(t) = entry {
                match to_val(&value) {
                    Some(v) => {
                        t.insert(key.to_string(), v);
                    }
                    None => {
                        t.remove(key);
                    }
                }
            }
        }
    }
    Ok(toml::to_string_pretty(&doc)?)
}

#[cfg(test)]
mod write_tests {
    use super::*;

    #[test]
    fn sets_top_level_and_nested_preserving_others() {
        let src = "model = \"old\"\n[economy]\nauto_evict_cold = true\n";
        let out = set_in_toml(src, "model", TomlValue::Str("new".into())).unwrap();
        let out = set_in_toml(&out, "economy.context_ceiling", TomlValue::Int(512000)).unwrap();
        let p = parse_toml(&out).unwrap();
        assert_eq!(p.model.as_deref(), Some("new"));
        assert_eq!(p.economy.context_ceiling, Some(512000));
        assert_eq!(p.economy.auto_evict_cold, Some(true)); // preserved
    }

    #[test]
    fn unset_removes_key() {
        let out = set_in_toml("model = \"x\"\n", "model", TomlValue::Unset).unwrap();
        assert!(parse_toml(&out).unwrap().model.is_none());
    }

    #[test]
    fn writes_into_empty_document() {
        let out = set_in_toml("", "reduced_motion", TomlValue::Bool(true)).unwrap();
        assert_eq!(parse_toml(&out).unwrap().reduced_motion, Some(true));
    }
}
