//! Pure eviction controller (spec §3.1). This file grows in Slice 1 (planner,
//! scorer, breadcrumb); Slice 0 lands only the policy the turn config carries.

use crate::band::{derive_band, Band};

/// The live turn's eviction parameters. `enabled: false` is a total bypass
/// (byte-identical to pre-ACM behavior) used by the zero-arg test constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvictionPolicy {
    pub enabled: bool,
    pub capacity: u64,
    pub context_target: u64,
    pub band_headroom_pct: u8,
    pub recent_n: usize,
    pub max_output: Option<u64>,
}

impl EvictionPolicy {
    pub fn disabled() -> Self {
        Self { enabled: false, capacity: 0, context_target: 0, band_headroom_pct: 0, recent_n: 0, max_output: None }
    }
    /// The band for this policy (spec §3.6a).
    pub fn band(&self) -> Band {
        derive_band(self.capacity, self.context_target, self.max_output, self.band_headroom_pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_policy_has_zero_band() {
        let b = EvictionPolicy::disabled().band();
        assert_eq!(b.high_water, 0);
    }
    #[test]
    fn enabled_policy_band_matches_derivation() {
        let p = EvictionPolicy { enabled: true, capacity: 1_000_000, context_target: 384_000, band_headroom_pct: 20, recent_n: 4, max_output: None };
        assert_eq!(p.band().high_water, 384_000);
    }
}
