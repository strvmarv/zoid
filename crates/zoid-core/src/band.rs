//! Pure derivation of the eviction band from a model's capacity and the user's
//! context target (spec §3.6a). `capacity` is total context = input + output, so
//! the band reserves output room and can never exceed what the model can carry.

/// Floor on reserved output room when a model exposes no `max_output`.
pub const OUTPUT_RESERVE_FLOOR: u64 = 8_192;

/// Kept-clear margin below hard `capacity` for the pre-flight gate's hard floor.
pub const CAPACITY_SAFETY_MARGIN: u64 = 8_192;

/// The asymmetric operating band. `high_water == effective_target` (evict when
/// the estimate reaches it), `low_water` is where an eviction wave stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub high_water: u64,
    pub low_water: u64,
    pub effective_target: u64,
}

/// Derive the band for the active model. `context_target` is the user's soft
/// setpoint; it is clamped so it can never exceed `capacity - output_reserve`.
pub fn derive_band(
    capacity: u64,
    context_target: u64,
    max_output: Option<u64>,
    headroom_pct: u8,
) -> Band {
    let output_reserve = max_output.unwrap_or_else(|| OUTPUT_RESERVE_FLOOR.max(capacity / 10));
    let usable = capacity.saturating_sub(output_reserve);
    let effective_target = context_target.min(usable);
    let headroom = effective_target.saturating_mul(headroom_pct as u64) / 100;
    let low_water = effective_target.saturating_sub(headroom);
    Band { high_water: effective_target, low_water, effective_target }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_1m_model_384k_target() {
        // 1M capacity, 384k target, 20% headroom, default output reserve (100k).
        let b = derive_band(1_000_000, 384_000, None, 20);
        assert_eq!(b.effective_target, 384_000); // target < usable (900k)
        assert_eq!(b.high_water, 384_000);
        assert_eq!(b.low_water, 384_000 - 76_800); // 20% headroom
    }

    #[test]
    fn small_model_collapses_target_to_usable() {
        // 32k capacity, 384k target: effective target clamps to usable. The output
        // reserve is `max(OUTPUT_RESERVE_FLOOR, cap/10)` — for a 32k model the 8_192
        // floor binds (cap/10 = 3_200 would leave less than the 4_096-token max_tokens
        // response room), so usable = 32_000 - 8_192.
        let b = derive_band(32_000, 384_000, None, 20);
        assert_eq!(b.effective_target, 32_000 - OUTPUT_RESERVE_FLOOR);
        assert!(b.high_water <= 32_000);
        assert!(b.low_water < b.high_water);
    }

    #[test]
    fn explicit_max_output_is_respected() {
        let b = derive_band(200_000, 384_000, Some(16_000), 10);
        assert_eq!(b.effective_target, 200_000 - 16_000); // clamped to usable
        assert_eq!(b.low_water, b.effective_target - b.effective_target / 10);
    }

    #[test]
    fn tiny_capacity_never_underflows() {
        let b = derive_band(1_000, 384_000, None, 20);
        assert!(b.low_water <= b.high_water);
        assert!(b.high_water <= 1_000);
    }
}
