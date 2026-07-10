//! The `.zoid-plugin.json` sidecar (schema 1): a superset of the mode
//! provenance that also records the ordered effects applied at install, so a
//! future uninstall can revert them (prev values captured for SetConfig).

use serde::{Deserialize, Serialize};
use zoid_core::wizard::ProvenanceEntry;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginProvenance {
    pub schema: u32,
    pub plugin: PluginStamp,
    pub source: PluginProvSource,
    pub files: Vec<ProvenanceEntry>,
    pub effects_applied: Vec<AppliedEffect>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStamp {
    pub id: String,
    pub manifest_ref: String,
    pub installed_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginProvSource {
    pub repo: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub subtree: String,
    /// "bundled" | "repo" | "url" — where the manifest came from.
    pub origin: String,
}

/// An effect as actually applied, with enough info to revert it. `SetConfig`
/// captures the prior value so uninstall restores the exact prior state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppliedEffect {
    Activate,
    OnboardingHint { text: String },
    SetConfig {
        key: String,
        prev: serde_json::Value,
        new: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::wizard::ProvenanceEntry;

    #[test]
    fn provenance_round_trips_and_has_no_host_paths() {
        let p = PluginProvenance {
            schema: 1,
            plugin: PluginStamp {
                id: "superpowers".into(),
                manifest_ref: "d884ae0".into(),
                installed_at: "2026-07-09T00:00:00Z".into(),
            },
            source: PluginProvSource {
                repo: "obra/superpowers".into(),
                ref_: "d884ae0".into(),
                subtree: "skills".into(),
                origin: "bundled".into(),
            },
            files: vec![ProvenanceEntry {
                canonical_path: "brainstorming/SKILL.md".into(),
                upstream_path: "skills/brainstorming/SKILL.md".into(),
                upstream_sha: "sha".into(),
                upstream_ref: "d884ae0".into(),
                upstream_snapshot: "snap".into(),
            }],
            effects_applied: vec![
                AppliedEffect::Activate,
                AppliedEffect::OnboardingHint { text: "hi".into() },
            ],
        };
        let json = serde_json::to_string_pretty(&p).unwrap();
        assert!(!json.contains("/home/"));
        let back: PluginProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
