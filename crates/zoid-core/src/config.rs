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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkingConfig {
    pub enabled: bool,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModesConfig {
    /// Extra directories to scan for `<mode>/mode.md` folders (beyond the two
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
    pub modes: ModesConfig,
    pub companion: CompanionConfig,
    pub thinking: ThinkingConfig,
    pub approval: ApprovalConfig,
    pub embed: EmbedConfig,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalConfig {
    pub yolo: bool,
    pub shell_danger: Vec<String>,
    pub shell_allow: Vec<String>,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            yolo: false,
            shell_danger: vec![],
            shell_allow: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionConfig {
    /// TCP port for the companion server; 0 = OS-assigned ephemeral.
    pub port: u16,
    /// Auto-open the browser when the companion is enabled.
    pub open: bool,
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self {
            port: 0,
            open: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomyConfig {
    /// The soft setpoint the controller manages toward (tokens). Defaults to
    /// 300_000; None → the bin falls back to min(capacity, 300_000). The bin
    /// clamps the resolved target to model capacity for small-window models.
    /// Renamed from `context_ceiling`.
    pub context_target: Option<u64>,
    pub auto_evict_cold: bool,
    /// 0 disables compaction; else percent of the target (1–100). Default 80.
    pub compact_threshold_pct: u8,
    /// Eviction band headroom, percent of effective target (default 20).
    pub band_headroom_pct: u8,
    /// Most-recent turns never evictable (default 4).
    pub recent_n: usize,
    /// Re-assert the system prompt at the live edge every N estimated-appended
    /// tokens of novel content. 0 disables. Default 100_000. Units: estimate_tokens (chars/3).
    pub reassert_interval_tokens: u64,
}

impl Default for EconomyConfig {
    fn default() -> Self {
        Self {
            context_target: Some(300_000),
            auto_evict_cold: true,
            compact_threshold_pct: 80,
            band_headroom_pct: 20,
            recent_n: 4,
            reassert_interval_tokens: 100_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedConfig {
    /// Master switch (default true when compiled in with feature `local-embed`).
    pub enabled: bool,
    /// Ring-buffer capacity = the RAM knob (≈73 MB @ 50k, ≈220 MB @ 150k).
    pub max_vectors: usize,
    /// Fetch model weights on first use; false = use only if already cached.
    pub auto_download: bool,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_vectors: 50_000,
            auto_download: true,
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
            modes: ModesConfig::default(),
            companion: CompanionConfig::default(),
            thinking: ThinkingConfig::default(),
            approval: ApprovalConfig::default(),
            embed: EmbedConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiConfig {
    /// Master switch for the ephemeral edit/write diff snippets.
    pub edit_diff: bool,
    /// How many most-recent edits show an inline snippet (0 = counts only).
    pub edit_diff_inline: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            edit_diff: true,
            edit_diff_inline: 5,
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
        assert_eq!(c.economy.compact_threshold_pct, 80);
        assert_eq!(c.economy.context_target, Some(300_000));
        assert_eq!(c.economy.band_headroom_pct, 20);
        assert_eq!(c.economy.recent_n, 4);
    }

    #[test]
    fn companion_section_parses_and_merges() {
        let (p, _warn) = parse_toml("[companion]\nport = 9123\nopen = false").unwrap();
        assert_eq!(p.companion.port, Some(9123));
        assert_eq!(p.companion.open, Some(false));
        let (cfg, _prov) = merge(&[(Source::UserGlobal, p)]);
        assert_eq!(cfg.companion.port, 9123);
        assert!(!cfg.companion.open);
        // default when absent
        let (dflt, _) = merge(&[]);
        assert_eq!(dflt.companion.port, 0);
        assert!(dflt.companion.open);
    }

    #[test]
    fn embed_defaults_and_parse() {
        let c = Config::default();
        assert!(c.embed.enabled);
        assert_eq!(c.embed.max_vectors, 50_000);
        assert!(c.embed.auto_download);

        let (p, _warn) = parse_toml("[embed]\nenabled = false\nmax_vectors = 1000").unwrap();
        assert_eq!(p.embed.enabled, Some(false));
        assert_eq!(p.embed.max_vectors, Some(1000));
        let (cfg, _prov) = merge(&[(Source::UserGlobal, p)]);
        assert!(!cfg.embed.enabled);
        assert_eq!(cfg.embed.max_vectors, 1000);
        assert!(cfg.embed.auto_download); // default preserved when absent
    }

    #[test]
    fn ui_config_defaults_edit_diff_on_and_k_five() {
        let c = UiConfig::default();
        assert!(c.edit_diff, "edit diffs ship enabled");
        assert_eq!(c.edit_diff_inline, 5);
    }

    #[test]
    fn merge_applies_ui_overrides() {
        let (p, _) = parse_toml("[ui]\nedit_diff = false\nedit_diff_inline = 2").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert!(!cfg.ui.edit_diff);
        assert_eq!(cfg.ui.edit_diff_inline, 2);
    }

    #[test]
    fn ui_defaults_when_section_absent() {
        let (p, _) = parse_toml("[economy]\nrecent_n = 3").unwrap();
        let (cfg, _) = merge(&[(Source::Project, p)]);
        assert!(cfg.ui.edit_diff, "absent [ui] → default on");
        assert_eq!(cfg.ui.edit_diff_inline, 5);
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
    pub context_target: Source,
    pub auto_evict_cold: Source,
    pub compact_threshold_pct: Source,
    pub band_headroom_pct: Source,
    pub recent_n: Source,
    pub reassert_interval_tokens: Source,
    pub reduced_motion: Source,
    pub thinking_enabled: Source,
    pub thinking_effort: Source,
    pub approval: Source,
    pub ui_edit_diff: Source,
    pub ui_edit_diff_inline: Source,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEconomy {
    pub context_target: Option<u64>,
    pub auto_evict_cold: Option<bool>,
    pub compact_threshold_pct: Option<u8>,
    pub band_headroom_pct: Option<u8>,
    pub recent_n: Option<usize>,
    pub reassert_interval_tokens: Option<u64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialSkills {
    pub source_dirs: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialModes {
    pub source_dirs: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PartialThinking {
    pub enabled: Option<bool>,
    pub effort: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialCompanion {
    pub port: Option<u16>,
    pub open: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialEmbed {
    pub enabled: Option<bool>,
    pub max_vectors: Option<usize>,
    pub auto_download: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialApproval {
    pub yolo: Option<bool>,
    pub shell_danger: Option<Vec<String>>,
    pub shell_allow: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialUi {
    pub edit_diff: Option<bool>,
    pub edit_diff_inline: Option<u32>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct PartialConfig {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub reduced_motion: Option<bool>,
    pub economy: PartialEconomy,
    pub skills: PartialSkills,
    pub modes: PartialModes,
    pub companion: PartialCompanion,
    pub thinking: PartialThinking,
    pub approval: PartialApproval,
    pub embed: PartialEmbed,
    pub ui: PartialUi,
}

/// Parse one TOML layer. Known keys deserialize normally; unknown keys are NOT
/// rejected — their dotted paths are collected and returned so callers can warn
/// (preserving typo-surfacing without discarding the whole layer). A genuine
/// syntax error, or a wrong-typed *known* key, is still an `Err`.
pub fn parse_toml(s: &str) -> anyhow::Result<(PartialConfig, Vec<String>)> {
    let de = toml::Deserializer::new(s);
    let mut unknown: Vec<String> = Vec::new();
    let cfg: PartialConfig = serde_ignored::deserialize(de, |path| unknown.push(path.to_string()))?;
    Ok((cfg, unknown))
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
        context_target: Source::Default,
        auto_evict_cold: Source::Default,
        compact_threshold_pct: Source::Default,
        band_headroom_pct: Source::Default,
        recent_n: Source::Default,
        reassert_interval_tokens: Source::Default,
        reduced_motion: Source::Default,
        thinking_enabled: Source::Default,
        thinking_effort: Source::Default,
        approval: Source::Default,
        ui_edit_diff: Source::Default,
        ui_edit_diff_inline: Source::Default,
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
        if let Some(v) = p.economy.context_target {
            cfg.economy.context_target = Some(v);
            prov.context_target = *src;
        }
        if let Some(v) = p.economy.auto_evict_cold {
            cfg.economy.auto_evict_cold = v;
            prov.auto_evict_cold = *src;
        }
        if let Some(v) = p.economy.compact_threshold_pct {
            cfg.economy.compact_threshold_pct = v;
            prov.compact_threshold_pct = *src;
        }
        if let Some(v) = p.economy.band_headroom_pct {
            cfg.economy.band_headroom_pct = v;
            prov.band_headroom_pct = *src;
        }
        if let Some(v) = p.economy.recent_n {
            cfg.economy.recent_n = v;
            prov.recent_n = *src;
        }
        if let Some(v) = p.economy.reassert_interval_tokens {
            cfg.economy.reassert_interval_tokens = v;
            prov.reassert_interval_tokens = *src;
        }
        if let Some(v) = p.ui.edit_diff {
            cfg.ui.edit_diff = v;
            prov.ui_edit_diff = *src;
        }
        if let Some(v) = p.ui.edit_diff_inline {
            cfg.ui.edit_diff_inline = v;
            prov.ui_edit_diff_inline = *src;
        }
        if let Some(dirs) = &p.skills.source_dirs {
            for d in dirs {
                if !cfg.skills.source_dirs.contains(d) {
                    cfg.skills.source_dirs.push(d.clone());
                }
            }
        }
        if let Some(dirs) = &p.modes.source_dirs {
            for d in dirs {
                if !cfg.modes.source_dirs.contains(d) {
                    cfg.modes.source_dirs.push(d.clone());
                }
            }
        }
        if let Some(v) = p.companion.port {
            cfg.companion.port = v;
        }
        if let Some(v) = p.companion.open {
            cfg.companion.open = v;
        }
        if let Some(v) = p.thinking.enabled {
            cfg.thinking.enabled = v;
            prov.thinking_enabled = *src;
        }
        if let Some(v) = &p.thinking.effort {
            cfg.thinking.effort = Some(v.clone());
            prov.thinking_effort = *src;
        }
        if let Some(v) = p.approval.yolo {
            cfg.approval.yolo = v;
            prov.approval = *src;
        }
        if let Some(dirs) = &p.approval.shell_danger {
            for d in dirs {
                if !cfg.approval.shell_danger.contains(d) {
                    cfg.approval.shell_danger.push(d.clone());
                }
            }
        }
        if let Some(dirs) = &p.approval.shell_allow {
            for d in dirs {
                if !cfg.approval.shell_allow.contains(d) {
                    cfg.approval.shell_allow.push(d.clone());
                }
            }
        }
        if let Some(v) = p.embed.enabled {
            cfg.embed.enabled = v;
        }
        if let Some(v) = p.embed.max_vectors {
            cfg.embed.max_vectors = v;
        }
        if let Some(v) = p.embed.auto_download {
            cfg.embed.auto_download = v;
        }
    }
    (cfg, prov)
}

#[cfg(test)]
mod thinking_config_tests {
    use super::*;

    #[test]
    fn thinking_section_parses_and_merges() {
        let (p, _) = parse_toml("[thinking]\nenabled = true\neffort = \"high\"").unwrap();
        assert!(p.thinking.enabled.unwrap());
        assert_eq!(p.thinking.effort.as_deref(), Some("high"));
        let (cfg, prov) = merge(&[(Source::UserGlobal, p)]);
        assert!(cfg.thinking.enabled);
        assert_eq!(cfg.thinking.effort, Some("high".to_string()));
        assert_eq!(prov.thinking_enabled, Source::UserGlobal);
        assert_eq!(prov.thinking_effort, Source::UserGlobal);
    }

    #[test]
    fn thinking_defaults_to_disabled() {
        let (cfg, prov) = merge(&[]);
        assert!(!cfg.thinking.enabled);
        assert!(cfg.thinking.effort.is_none());
        assert_eq!(prov.thinking_enabled, Source::Default);
        assert_eq!(prov.thinking_effort, Source::Default);
    }

    #[test]
    fn thinking_enabled_without_effort_is_auto() {
        let (p, _) = parse_toml("[thinking]\nenabled = true").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, p)]);
        assert!(cfg.thinking.enabled);
        assert!(cfg.thinking.effort.is_none());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn later_layers_override_and_record_source() {
        let (user, _) =
            parse_toml("model = \"a\"\nreduced_motion = true\n[economy]\nauto_evict_cold = false")
                .unwrap();
        let (proj, _) = parse_toml("model = \"b\"").unwrap();
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
    fn unknown_key_is_warned_not_rejected() {
        let (pc, warn) = parse_toml("model = \"a\"\nbogus = 1").unwrap();
        assert_eq!(pc.model.as_deref(), Some("a")); // valid key still loads
        assert_eq!(warn, vec!["bogus".to_string()]);
    }

    #[test]
    fn unknown_economy_key_loads_siblings_and_warns_dotted() {
        let (pc, warn) =
            parse_toml("[economy]\ncompact_threshold_pct = 70\ncontext_ceiling = 512000").unwrap();
        assert_eq!(pc.economy.compact_threshold_pct, Some(70)); // sibling loads
        assert_eq!(pc.economy.context_target, None); // renamed key NOT applied
        assert_eq!(warn, vec!["economy.context_ceiling".to_string()]);
    }

    #[test]
    fn regression_stale_ceiling_does_not_drop_model_or_provider() {
        let toml =
            "model = \"glm-5.2\"\nprovider = \"ollama-cloud\"\n[economy]\ncontext_ceiling = 512000";
        let (pc, warn) = parse_toml(toml).unwrap();
        assert_eq!(pc.model.as_deref(), Some("glm-5.2"));
        assert_eq!(pc.provider.as_deref(), Some("ollama-cloud"));
        assert_eq!(warn, vec!["economy.context_ceiling".to_string()]);
    }

    #[test]
    fn malformed_toml_is_still_err() {
        assert!(parse_toml("this is = = not toml").is_err());
    }

    #[test]
    fn wrong_typed_known_key_is_still_err() {
        // recent_n expects an integer; a string is a hard error, not an unknown key.
        assert!(parse_toml("[economy]\nrecent_n = \"four\"").is_err());
    }

    #[test]
    fn no_unknown_keys_yields_empty_warnings() {
        let (_pc, warn) = parse_toml("model = \"a\"").unwrap();
        assert!(warn.is_empty());
    }

    #[test]
    fn parses_skills_source_dirs() {
        let (p, _) = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        assert_eq!(
            p.skills.source_dirs,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn merge_unions_source_dirs_across_layers() {
        let (user, _) = parse_toml("[skills]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[skills]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(
            cfg.skills.source_dirs,
            vec!["a".to_string(), "b".to_string(), "c".to_string()] // "b" not duplicated
        );
    }

    #[test]
    fn parses_modes_source_dirs() {
        let (p, _) = parse_toml("[modes]\nsource_dirs = [\"m1\", \"m2\"]").unwrap();
        assert_eq!(
            p.modes.source_dirs,
            Some(vec!["m1".to_string(), "m2".to_string()])
        );
    }

    #[test]
    fn merge_unions_modes_source_dirs() {
        let (user, _) = parse_toml("[modes]\nsource_dirs = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[modes]\nsource_dirs = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(
            cfg.modes.source_dirs,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
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
        let out = set_in_toml(&out, "economy.context_target", TomlValue::Int(512000)).unwrap();
        let (p, _) = parse_toml(&out).unwrap();
        assert_eq!(p.model.as_deref(), Some("new"));
        assert_eq!(p.economy.context_target, Some(512000));
        assert_eq!(p.economy.auto_evict_cold, Some(true)); // preserved
    }

    #[test]
    fn unset_removes_key() {
        let out = set_in_toml("model = \"x\"\n", "model", TomlValue::Unset).unwrap();
        assert!(parse_toml(&out).unwrap().0.model.is_none());
    }

    #[test]
    fn writes_into_empty_document() {
        let out = set_in_toml("", "reduced_motion", TomlValue::Bool(true)).unwrap();
        assert_eq!(parse_toml(&out).unwrap().0.reduced_motion, Some(true));
    }
}

#[cfg(test)]
mod approval_config_tests {
    use super::*;

    #[test]
    fn approval_section_parses_and_merges() {
        let (p, _) = parse_toml(
            "[approval]\nyolo = true\nshell_danger = [\"make deploy\"]\nshell_allow = [\"git push --force-with-lease\"]"
        ).unwrap();
        assert_eq!(p.approval.yolo, Some(true));
        assert_eq!(p.approval.shell_danger, Some(vec!["make deploy".to_string()]));
        assert_eq!(p.approval.shell_allow, Some(vec!["git push --force-with-lease".to_string()]));
        let (cfg, _) = merge(&[(Source::UserGlobal, p)]);
        assert!(cfg.approval.yolo);
        assert_eq!(cfg.approval.shell_danger, vec!["make deploy".to_string()]);
        assert_eq!(cfg.approval.shell_allow, vec!["git push --force-with-lease".to_string()]);
    }

    #[test]
    fn approval_defaults_to_safe() {
        let (cfg, _) = merge(&[]);
        assert!(!cfg.approval.yolo);
        assert!(cfg.approval.shell_danger.is_empty());
        assert!(cfg.approval.shell_allow.is_empty());
    }

    #[test]
    fn approval_shell_danger_unions_across_layers() {
        let (user, _) = parse_toml("[approval]\nshell_danger = [\"a\", \"b\"]").unwrap();
        let (proj, _) = parse_toml("[approval]\nshell_danger = [\"b\", \"c\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.approval.shell_danger, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn approval_shell_allow_unions_across_layers() {
        let (user, _) = parse_toml("[approval]\nshell_allow = [\"x\"]").unwrap();
        let (proj, _) = parse_toml("[approval]\nshell_allow = [\"y\"]").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert_eq!(cfg.approval.shell_allow, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn approval_yolo_last_write_wins() {
        let (user, _) = parse_toml("[approval]\nyolo = true").unwrap();
        let (proj, _) = parse_toml("[approval]\nyolo = false").unwrap();
        let (cfg, _) = merge(&[(Source::UserGlobal, user), (Source::Project, proj)]);
        assert!(!cfg.approval.yolo, "project layer overrides user-global");
    }
}

#[cfg(test)]
mod reassert_interval_tests {
    use super::*;

    #[test]
    fn economy_default_reassert_interval_is_100k() {
        assert_eq!(EconomyConfig::default().reassert_interval_tokens, 100_000);
    }

    #[test]
    fn parse_reassert_interval_into_partial() {
        let (pc, _unknown) = parse_toml("[economy]\nreassert_interval_tokens = 250000").unwrap();
        assert_eq!(pc.economy.reassert_interval_tokens, Some(250_000));
    }
}
