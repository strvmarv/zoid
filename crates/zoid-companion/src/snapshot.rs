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

#[derive(Clone, Serialize, Debug)]
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

/// Manual `PartialEq` that excludes `updated_ms`. `updated_ms` is a render-time
/// stamp refreshed on every render-loop tick regardless of whether any real
/// state changed; if it were included in equality, `CompanionHub::publish_snapshot`'s
/// dedupe (and `SseReader::absorb`'s change check) would never fire — every
/// frame would compare unequal and wake SSE clients even when nothing but the
/// clock advanced. Excluding it lets dedupe fire on frames where only the
/// timestamp changed.
impl PartialEq for DashboardSnapshot {
    fn eq(&self, other: &Self) -> bool {
        // Destructure exhaustively so a future field addition is a COMPILE error
        // here rather than a silently-dropped comparison (which would let a real
        // dashboard update be deduped away). `updated_ms` is the one deliberate
        // exclusion; every other field must be weighed.
        let Self {
            session_name,
            model,
            provider,
            cwd,
            ctx_used,
            ctx_ceiling,
            session_tokens,
            cached_tokens,
            cache_supported,
            tasks_len,
            busy,
            tiers,
            churn,
            updated_ms: _,
        } = self;
        *session_name == other.session_name
            && *model == other.model
            && *provider == other.provider
            && *cwd == other.cwd
            && *ctx_used == other.ctx_used
            && *ctx_ceiling == other.ctx_ceiling
            && *session_tokens == other.session_tokens
            && *cached_tokens == other.cached_tokens
            && *cache_supported == other.cache_supported
            && *tasks_len == other.tasks_len
            && *busy == other.busy
            && *tiers == other.tiers
            && *churn == other.churn
    }
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

    fn base_snap() -> DashboardSnapshot {
        DashboardSnapshot {
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
            tiers: vec![],
            churn: vec![10, 20, 30],
            updated_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn updated_ms_excluded_from_equality() {
        let a = base_snap();
        let mut b = base_snap();
        b.updated_ms = a.updated_ms + 1000; // only the timestamp differs
        assert_eq!(a, b, "updated_ms alone must not break equality (dedup relies on this)");

        let mut c = base_snap();
        c.busy = !c.busy; // a real content change
        assert_ne!(a, c, "a real field change must still break equality");
    }
}
