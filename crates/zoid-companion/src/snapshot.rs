//! The serializable projection the browser dashboard renders. Plain serde,
//! deliberately free of any `zoid-core` types so this crate stays a leaf.

use serde::Serialize;

#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct TierRow {
    pub label: String,
    pub tokens: u64,
    /// Heat rank: 2 = hot, 1 = warm, 0 = cold. Mirrors `cold` for convenience.
    pub heat: u8,
    pub cold: bool,
    pub pinned: bool,
}

#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct DashboardSnapshot {
    pub session_name: String,
    pub model: String,
    pub provider: String,
    pub cwd: String,
    pub ctx_used: u64,
    pub ctx_ceiling: u64,
    pub session_tokens: u64,
    pub cached_tokens: u64,
    pub cache_supported: bool,
    pub tasks_len: usize,
    pub busy: bool,
    pub tiers: Vec<TierRow>,
    /// Per-turn token series; the browser draws the SVG sparkline from it.
    pub churn: Vec<u64>,
    pub updated_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serializes_churn_as_json_array() {
        let snap = DashboardSnapshot {
            session_name: "demo".into(),
            model: "glm-5.2:cloud".into(),
            provider: "ollama".into(),
            cwd: "/home/x/zoid".into(),
            ctx_used: 312_000,
            ctx_ceiling: 384_000,
            session_tokens: 100,
            cached_tokens: 20,
            cache_supported: true,
            tasks_len: 3,
            busy: false,
            tiers: vec![TierRow {
                label: "system".into(),
                tokens: 1200,
                heat: 2,
                cold: false,
                pinned: true,
            }],
            churn: vec![10, 20, 30],
            updated_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"churn\":[10,20,30]"), "got: {json}");
        assert!(json.contains("\"heat\":2"), "got: {json}");
    }
}
